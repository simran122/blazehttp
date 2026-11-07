import { join } from 'node:path';

import { describe, expect, it } from 'vitest';

import { assertHead, feedInputs, rotated, variants, workloads } from './workloads.ts';

const FIXTURES = join(import.meta.dirname, '../../tests/fixtures/h1-heads');
const BASE = Buffer.from(
  'HTTP/1.1 200 OK\r\ncontent-length: 847\r\ndate: Fri, 06 Nov 2026 12:00:00 GMT\r\nx-request-id: 0b7c1f0e-5d2a\r\n\r\n',
  'latin1',
);

describe('variants', () => {
  const heads = variants(BASE);

  it('keeps the length and the shape, changing only the named digits', () => {
    expect(heads).toHaveLength(64);
    for (const head of heads) {
      expect(head.length).toBe(BASE.length);
      expect(head.toString('latin1')).toMatch(
        /^HTTP\/1\.1 200 OK\r\ncontent-length: [1-9]\d\d\r\ndate: Fri, 06 Nov 2026 12:00:\d\d GMT\r\nx-request-id: \d\d7c1f0e-5d2a\r\n\r\n$/,
      );
    }
    expect(new Set(heads.map((h) => h.toString('latin1'))).size).toBeGreaterThan(1);
  });

  it('matches the Rust bench rule for variant 1', () => {
    // content-length digits i = 0, 1, 2: (7 + 3i) % 10 = 7, 0, 3, the first mapped to '1' + 7 % 9.
    expect(heads[1]?.toString('latin1')).toContain('content-length: 803\r\n');
    expect(heads[1]?.toString('latin1')).toContain('12:00:70 GMT');
  });

  it('leaves heads without the needles unchanged', () => {
    const plain = Buffer.from('HTTP/1.1 204 No Content\r\n\r\n', 'latin1');
    expect(variants(plain, 2)).toEqual([plain, plain]);
  });
});

describe('rotated', () => {
  it('cycles through the bases', () => {
    const other = Buffer.from('HTTP/1.1 204 No Content\r\n\r\n', 'latin1');
    const heads = rotated([BASE, other]);
    expect(heads).toHaveLength(64);
    expect(heads[1]).toEqual(other);
    expect(heads[2]?.length).toBe(BASE.length);
    expect(() => rotated([])).toThrow('no fixtures');
  });
});

describe('feed inputs', () => {
  it('are the four inputs of the research note with their sizes', () => {
    const sizes = feedInputs().map((w) => [w.name, w.heads[0]?.length]);
    expect(sizes).toEqual([
      ['feed-a-60k-value', 60_045],
      ['feed-b-99x600', 59_419],
      ['feed-c-60k-name', 60_024],
      ['feed-d-8k-status', 8_194],
    ]);
  });
});

describe('workloads', () => {
  it('reads the shared fixtures and validates every head', () => {
    const all = workloads(FIXTURES);
    expect(all.map((w) => [w.name, w.kind, w.heads.length])).toEqual([
      ['1k-3-headers', 'timed', 1],
      ['1k-20-headers', 'timed', 1],
      ['1k-3-headers-x64', 'timed', 64],
      ['1k-20-headers-x64', 'timed', 64],
      ['mixed-64', 'timed', 64],
      ['feed-a-60k-value', 'feed', 1],
      ['feed-b-99x600', 'feed', 1],
      ['feed-c-60k-name', 'feed', 1],
      ['feed-d-8k-status', 'feed', 1],
      ['gate-framing', 'gate', 2],
    ]);
    expect(all[0]?.heads[0]?.length).toBe(109);
    expect(all[1]?.heads[0]?.length).toBe(779);
  });

  it('rejects a buffer that is not exactly one head', () => {
    expect(() => {
      assertHead(Buffer.from('HTTP/1.1 200 OK\r\n\r\nHTTP/1.1 200 OK\r\n\r\n'));
    }).toThrow();
    expect(() => {
      assertHead(Buffer.from('HTTP/1.1 200 OK\r\n'));
    }).toThrow();
  });
});
