import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it, onTestFinished } from 'vitest';
import {
  loadResponseVectorFile,
  loadVectorFile,
  parseIndex,
  parseRequestVectorFile,
  parseVectorFile,
  type RequestVectorFile,
  type VectorFile,
} from './schema.ts';

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

/** Invalid vector files: what is wrong, how to build the file, and the error it must raise. */
const INVALID_FILES: [string, () => unknown, RegExp][] = [
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
      mutate((f) => ((f.cases[0] as { expect: { native: { ok: { status: number } } } }).expect.native.ok.status = 1.5)),
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
];

/** Writes `data` to `<tmp>/<dir>/v.json` (removed after the test) and returns the path. */
const writeVectorFile = (dir: string, data: unknown): string => {
  const root = mkdtempSync(join(tmpdir(), 'vectors-'));
  onTestFinished(() => {
    rmSync(root, { recursive: true, force: true });
  });
  mkdirSync(join(root, dir), { recursive: true });
  const path = join(root, dir, 'v.json');
  writeFileSync(path, JSON.stringify(data));
  return path;
};

describe('loadVectorFile', () => {
  it('loads a valid file', () => {
    expect(loadVectorFile(writeVectorFile('h1-response', VALID))).toEqual(VALID);
  });

  it.each(INVALID_FILES)('rejects %s', (_, make, message) => {
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

const VALID_REQUEST: RequestVectorFile = {
  source: VALID.source,
  cases: [
    {
      name: 'written',
      inputEncoding: 'utf8',
      request: {
        method: 'POST',
        target: '/x',
        templateHost: 'h',
        templateFields: '',
        templateFieldLines: 0,
        headers: 'a: b\r\n',
        headerLines: 1,
        body: 'hi',
      },
      expect: { bytes: 'POST /x HTTP/1.1\r\nhost: h\r\na: b\r\ncontent-length: 2\r\n\r\nhi', reuse: 'allowed' },
      derivedFrom: 'test/x.js:y @ v1',
    },
    {
      name: 'error',
      inputEncoding: 'latin1',
      request: {
        method: 'GET',
        target: '/',
        templateHost: 'h',
        templateFields: '',
        templateFieldLines: 0,
        headers: '',
        headerLines: 0,
        body: '',
        connAuthority: 'h\u00e9',
      },
      expect: { error: 'ENGINE/internal' },
    },
  ],
};

type MutableRequestCase = Record<string, unknown> & {
  request: Record<string, unknown>;
  expect: Record<string, unknown>;
};

const mutateRequest = (edit: (cases: MutableRequestCase[]) => void): unknown => {
  const copy = structuredClone(VALID_REQUEST) as unknown as { cases: MutableRequestCase[] };
  edit(copy.cases);
  return copy;
};

/** Invalid request vector files: what is wrong, how to build the file, and the error it must raise. */
const INVALID_REQUEST_FILES: [string, () => unknown, RegExp][] = [
  ['a missing request field', () => mutateRequest(([c]) => delete c?.request['headers']), /request\.headers: missing/],
  ['an unknown method', () => mutateRequest(([c]) => c && (c.request['method'] = 'TRACE')), /TRACE is not a generated/],
  [
    'a header line count outside u16',
    () => mutateRequest(([c]) => c && (c.request['headerLines'] = 65_536)),
    /headerLines: expected an integer in 0\.\.=65535/,
  ],
  [
    'a fractional line count',
    () => mutateRequest(([c]) => c && (c.request['headerLines'] = 0.5)),
    /cases\[0\]\.request\.headerLines: expected an integer in 0\.\.=65535/,
  ],
  ['an unknown reuse', () => mutateRequest(([c]) => c && (c.expect['reuse'] = 'never')), /expected allowed or/],
  ['bytes without reuse', () => mutateRequest(([c]) => c && delete c.expect['reuse']), /expect\.reuse: missing/],
  [
    'an error the engine cannot produce',
    () => mutateRequest(([, c]) => c && (c.expect = { error: 'PARSE/json' })),
    /PARSE\/json is not an engine error/,
  ],
  [
    'a code point above U+00FF in a latin1 case',
    () => mutateRequest(([, c]) => c && (c.request['connAuthority'] = '\u0100\u00e9')),
    /cases\[1\]\.request\.connAuthority: a code point above U\+00FF/,
  ],
  [
    'latin1 without a high byte',
    () => mutateRequest(([, c]) => c && delete c.request['connAuthority']),
    /latin1 without a byte >= 0x80; use utf8/,
  ],
  ['an unknown key', () => mutateRequest(([c]) => c && (c['input'] = '')), /cases\[0\]\.input: unknown key/],
  ['a duplicate name', () => mutateRequest(([, c]) => c && (c['name'] = 'written')), /duplicate case name written/],
  [
    'a negative line count',
    () => mutateRequest(([c]) => c && (c.request['headerLines'] = -1)),
    /headerLines: expected an integer/,
  ],
  [
    'a template line count outside u16',
    () => mutateRequest(([c]) => c && (c.request['templateFieldLines'] = 65_536)),
    /templateFieldLines: expected an integer in 0\.\.=65535/,
  ],
  [
    'a non-numeric template line count',
    () => mutateRequest(([c]) => c && (c.request['templateFieldLines'] = '1')),
    /templateFieldLines: expected an integer/,
  ],
  [
    'a missing template line count',
    () => mutateRequest(([c]) => delete c?.request['templateFieldLines']),
    /request\.templateFieldLines: missing/,
  ],
  [
    'a lone surrogate',
    () => mutateRequest(([c]) => c && (c.request['target'] = '/\ud800')),
    /cases\[0\]\.request\.target: a lone surrogate/,
  ],
  [
    'a lone low surrogate in the expected bytes',
    () => mutateRequest(([c]) => c && (c.expect['bytes'] = 'x\udc00')),
    /cases\[0\]\.expect\.bytes: a lone surrogate/,
  ],
  [
    'a non-string connAuthority',
    () => mutateRequest(([c]) => c && (c.request['connAuthority'] = 1)),
    /cases\[0\]\.request\.connAuthority: expected a string/,
  ],
  [
    'a non-string derivedFrom',
    () => mutateRequest(([c]) => c && (c['derivedFrom'] = 1)),
    /cases\[0\]\.derivedFrom: expected a string/,
  ],
  [
    'bytes missing from a written expectation',
    () => mutateRequest(([c]) => c && delete c.expect['bytes']),
    /cases\[0\]\.expect\.bytes: missing/,
  ],
  [
    'an expectation that is not an object',
    () => mutateRequest(([c]) => c && Object.assign(c, { expect: 'allowed' })),
    /cases\[0\]\.expect: expected an object/,
  ],
  [
    'an unknown inputEncoding',
    () => mutateRequest(([c]) => c && (c['inputEncoding'] = 'ascii')),
    /cases\[0\]\.inputEncoding: expected utf8 or latin1/,
  ],
];

describe('request vector files', () => {
  it('loads a valid file by its directory', () => {
    expect(loadVectorFile(writeVectorFile('h1-request', VALID_REQUEST))).toEqual(VALID_REQUEST);
  });

  it('refuses a directory without a case schema', () => {
    expect(() => loadVectorFile(writeVectorFile('other', VALID_REQUEST))).toThrow(/no case schema for directory other/);
  });

  it('reads the case kind from the directory one level below the vectors root only', () => {
    // A nested file is named by its own directory, so `h1-request/x/v.json` has no schema (docs/notes/vectors.md).
    expect(() => loadVectorFile(writeVectorFile(join('h1-request', 'x'), VALID_REQUEST))).toThrow(
      /no case schema for directory x/,
    );
  });

  it('loads a response file through the typed loader', () => {
    expect(loadResponseVectorFile(writeVectorFile('h1-response', VALID))).toEqual(VALID);
  });

  it.each(INVALID_REQUEST_FILES)('rejects %s', (_, make, message) => {
    expect(() => parseRequestVectorFile(make(), 'f')).toThrow(message);
  });

  it('accepts latin1 carried only by the expected bytes', () => {
    const file = mutateRequest(([c]) => {
      if (c === undefined) return;
      c['inputEncoding'] = 'latin1';
      c.expect['bytes'] = '\u00ff';
    });
    expect(parseRequestVectorFile(file, 'f').cases[0]?.inputEncoding).toBe('latin1');
  });
});
