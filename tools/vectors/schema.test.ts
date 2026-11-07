import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it, onTestFinished } from 'vitest';
import { loadVectorFile, parseIndex, parseVectorFile, type VectorFile } from './schema.ts';

const VALID: VectorFile = {
  source: {
    project: 'p',
    url: 'https://x.test/a',
    tag: 'v1',
    commit: 'c',
    licence: 'MIT',
    copyright: 'Copyright (c) X',
  },
  cases: [
    {
      name: 'ok',
      input: 'HTTP/1.1 200 OK\r\n\r\n',
      inputEncoding: 'utf8',
      request: { method: 'GET' },
      expect: { native: { ok: { status: 200, headers: [['a', 'b']], body: '', keepAlive: false } } },
      reference: { llhttp: 'log' },
    },
    {
      name: 'error',
      input: 'x',
      inputEncoding: 'utf8',
      request: { method: 'HEAD' },
      expect: { native: { error: 'PROTOCOL/malformed-response' }, undici: { error: 'TOO_MANY_REDIRECTS' } },
    },
    {
      name: 'latin1',
      input: 'HTTP/1.1 200 OK\r\nX: \u00a0\r\n\r\n',
      inputEncoding: 'latin1',
      request: { method: 'GET' },
      expect: { native: { ok: { status: 200, headers: [['X', '\u00a0']], body: '', keepAlive: false } } },
      derivedFrom: 'src/lib.rs:test_x @ v1',
    },
  ],
};

const mutate = (edit: (file: { cases: Record<string, unknown>[] } & Record<string, unknown>) => void): unknown => {
  const copy = structuredClone(VALID) as unknown as { cases: Record<string, unknown>[] } & Record<string, unknown>;
  edit(copy);
  return copy;
};

describe('loadVectorFile', () => {
  it('loads a valid file', () => {
    const dir = mkdtempSync(join(tmpdir(), 'vectors-'));
    onTestFinished(() => {
      rmSync(dir, { recursive: true, force: true });
    });
    const path = join(dir, 'v.json');
    writeFileSync(path, JSON.stringify(VALID));
    expect(loadVectorFile(path)).toEqual(VALID);
  });

  it.each([
    ['not an object', () => 1, /f: expected an object/],
    ['unknown key', () => mutate((f) => (f['extra'] = 1)), /f\.extra: unknown key/],
    ['missing source field', () => mutate((f) => delete (f['source'] as Record<string, unknown>)['url']), /url/],
    ['no cases', () => mutate((f) => (f.cases = [])), /at least one case/],
    ['cases not an array', () => mutate((f) => Object.assign(f, { cases: {} })), /cases: expected an array/],
    [
      'bad encoding',
      () => mutate((f) => (f.cases[0] = { ...f.cases[0], inputEncoding: 'base64' })),
      /expected utf8 or latin1/,
    ],
    [
      'a code point above U+00FF in a latin1 case',
      () => mutate((f) => (f.cases[2] = { ...f.cases[2], input: '\u00a0\u0100' })),
      /cases\[2\]\.input: a code point above U\+00FF/,
    ],
    [
      'a latin1 expectation above U+00FF',
      () =>
        mutate(
          (f) => ((f.cases[2] as { expect: { native: { ok: { body: string } } } }).expect.native.ok.body = '\u0100'),
        ),
      /cases\[2\]\.expect\.native\.ok: a code point above U\+00FF/,
    ],
    [
      'latin1 without a high byte',
      () => mutate((f) => (f.cases[2] = { ...f.cases[2], input: 'HTTP/1.1 200 OK\r\n\r\n' })),
      /latin1 without a byte >= 0x80; use utf8/,
    ],
    [
      'bad derivedFrom',
      () => mutate((f) => (f.cases[2] = { ...f.cases[2], derivedFrom: 1 })),
      /derivedFrom: expected a string/,
    ],
    [
      'an error the engine cannot produce',
      () => mutate((f) => (f.cases[1] = { ...f.cases[1], expect: { native: { error: 'HTTP_STATUS' } } })),
      /HTTP_STATUS is not an engine error/,
    ],
    [
      'a status outside 100..=599',
      () =>
        mutate(
          (f) => ((f.cases[0] as { expect: { native: { ok: { status: number } } } }).expect.native.ok.status = 70000),
        ),
      /status: expected an integer in 100\.\.=599/,
    ],
    [
      'an unknown method',
      () => mutate((f) => (f.cases[0] = { ...f.cases[0], request: { method: 'CONNECT' } })),
      /CONNECT is not a generated Method/,
    ],
    [
      'unknown error',
      () => mutate((f) => (f.cases[1] = { ...f.cases[1], expect: { native: { error: 'X/y' } } })),
      /X\/y/,
    ],
    [
      'both ok and error',
      () => mutate((f) => (f.cases[1] = { ...f.cases[1], expect: { native: { ok: 1, error: 'x' } } })),
      /exactly one/,
    ],
    [
      'bad status',
      () =>
        mutate(
          (f) => ((f.cases[0] as { expect: { native: { ok: { status: number } } } }).expect.native.ok.status = 1.5),
        ),
      /status: expected an integer/,
    ],
    [
      'bad header pair',
      () =>
        mutate(
          (f) =>
            ((f.cases[0] as { expect: { native: { ok: { headers: unknown } } } }).expect.native.ok.headers = [['a']]),
        ),
      /name, value/,
    ],
    [
      'bad keepAlive',
      () =>
        mutate(
          (f) =>
            ((f.cases[0] as { expect: { native: { ok: { keepAlive: unknown } } } }).expect.native.ok.keepAlive = 'yes'),
        ),
      /keepAlive: expected a boolean/,
    ],
    [
      'bad reference',
      () => mutate((f) => (f.cases[0] = { ...f.cases[0], reference: { llhttp: 1 } })),
      /llhttp: expected a string/,
    ],
    ['duplicate name', () => mutate((f) => (f.cases[1] = { ...f.cases[1], name: 'ok' })), /duplicate case name ok/],
  ])('rejects %s', (_, make, message) => {
    expect(() => parseVectorFile(make(), 'f')).toThrow(message);
  });
});

describe('parseIndex', () => {
  it('maps files to suites and rejects other shapes', () => {
    expect(parseIndex({ 'a.json': ['s'] }, 'i')).toEqual({ 'a.json': ['s'] });
    expect(() => parseIndex({ 'a.json': 's' }, 'i')).toThrow(/a.json: expected an array/);
    expect(() => parseIndex({ 'a.json': [1] }, 'i')).toThrow(/expected a string/);
  });
});
