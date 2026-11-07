// The heads every harness parses (bench/parsers/README.md "Workloads"). Generated from the shared fixtures with the
// same rotation rule as crates/engine-core/examples/h1_parse_bench.rs, so the numbers are comparable with it.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const HEAD_END = '\r\n\r\n';
const VARIANTS = 64;

/** `[needle, bytes after the needle's start that vary]`: date seconds, content length, ids and etag hex digits. */
const VARYING: readonly (readonly [string, number])[] = [
  ['12:00:', 8],
  ['content-length: ', 19],
  ['x-request-id: ', 16],
  ['traceparent: 00-', 18],
  ['etag: W/"34f-', 15],
];

export interface Workload {
  readonly name: string;
  /** `timed`: parsed whole, many times; `feed`: one head fed one byte per call. */
  readonly kind: 'timed' | 'feed';
  readonly heads: readonly Buffer[];
}

const isHexDigit = (byte: number): boolean => /^[0-9a-fA-F]$/.test(String.fromCharCode(byte));

/** Throws unless `head` is one complete head: it ends with the first CRLF CRLF it contains. */
export function assertHead(head: Buffer): void {
  const end = head.indexOf(HEAD_END, 0, 'latin1');
  if (end < 0 || end + HEAD_END.length !== head.length) {
    throw new Error('a head must end at its first CRLF CRLF');
  }
}

/** `count` heads of the same length as `base`, differing in the digits named by `VARYING`. */
export function variants(base: Buffer, count = VARIANTS): Buffer[] {
  return Array.from({ length: count }, (_, v) => {
    const head = Buffer.from(base);
    for (const [needle, end] of VARYING) {
      const at = head.indexOf(needle, 0, 'latin1');
      if (at < 0) continue;
      const digits = head.subarray(at + needle.length, at + end);
      digits.forEach((byte, i) => {
        if (!isHexDigit(byte)) return;
        const n = (v * 7 + i * 3) % 10;
        // Keep the first content-length digit non-zero (three-digit values).
        digits[i] = i === 0 && needle === 'content-length: ' ? 0x31 + (n % 9) : 0x30 + n;
      });
    }
    return head;
  });
}

/** 64 heads cycling through the bases (variant `v` of base `v % bases.length`), so shapes and digits both vary. */
export function rotated(bases: readonly Buffer[]): Buffer[] {
  const perBase = bases.map((base) => variants(base));
  return Array.from({ length: VARIANTS }, (_, v) => {
    const head = perBase[v % perBase.length]?.[v];
    if (head === undefined) throw new Error('no fixtures');
    return head;
  });
}

/** The 1-byte feed inputs (a)–(d) of docs/notes/response-parser.md §8.1. */
export function feedInputs(): Workload[] {
  const status = 'HTTP/1.1 200 OK\r\n';
  const fields = Array.from({ length: 99 }, (_, i) => `x-fill-${String(i).padStart(4, '0')}: ${'a'.repeat(585)}\r\n`);
  const reason = 'r'.repeat(8192 - 'HTTP/1.1 200 '.length - 2);
  const inputs: [string, string][] = [
    ['feed-a-60k-value', `${status}x-v: ${'v'.repeat(60_000)}\r\ncontent-length: 0\r\n\r\n`],
    ['feed-b-99x600', `${status}${fields.join('')}\r\n`],
    ['feed-c-60k-name', `${status}${'x'.repeat(60_000)}: 1\r\n\r\n`],
    ['feed-d-8k-status', `HTTP/1.1 200 ${reason}\r\n\r\n`],
  ];
  return inputs.map(([name, text]) => ({ name, kind: 'feed', heads: [Buffer.from(text, 'latin1')] }));
}

/** Every workload, read from the shared fixture directory (`tests/fixtures/h1-heads`). */
export function workloads(fixtureDir: string): Workload[] {
  const read = (name: string): Buffer => readFileSync(join(fixtureDir, name));
  const [h3, h20, synthetic] = [read('1k.http'), read('1k-h20.http'), read('synthetic-h20.http')];
  const all: Workload[] = [
    { name: '1k-3-headers', kind: 'timed', heads: [h3] },
    { name: '1k-20-headers', kind: 'timed', heads: [h20] },
    { name: 'rotated-64', kind: 'timed', heads: rotated([h3, h20, synthetic]) },
    ...feedInputs(),
  ];
  for (const workload of all) workload.heads.forEach(assertHead);
  return all;
}
