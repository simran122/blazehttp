// Parses what a harness prints (bench/parsers/README.md "Harness protocol") and compares heads across parsers
// (the differential correctness gate that runs before any timing).

interface Field {
  readonly name: string;
  readonly value: string;
}

export interface HeadReport {
  readonly status: number;
  readonly headLen: number;
  readonly contentLength: number | null;
  readonly fields: readonly Field[];
}

export interface CheckReport {
  readonly heads: readonly HeadReport[];
  readonly allocsPerHead: number;
}

export type FeedMode = 'incremental' | 'line-restart' | 'restart' | 'restart-skip';

export interface FeedReport {
  readonly ns: number;
  readonly mode: FeedMode;
  readonly headLen: number;
}

const FEED_MODES: ReadonlySet<string> = new Set<FeedMode>(['incremental', 'line-restart', 'restart', 'restart-skip']);

function integer(text: string | undefined, what: string): number {
  if (text === undefined || !/^\d+$/.test(text)) throw new Error(`bad ${what}: ${String(text)}`);
  return Number(text);
}

function decimal(text: string | undefined, what: string): number {
  const n = Number(text);
  if (text === undefined || text === '' || !Number.isFinite(n) || n < 0) {
    throw new Error(`bad ${what}: ${String(text)}`);
  }
  return n;
}

function hex(text: string | undefined): string {
  if (text === undefined || !/^(?:[0-9a-f]{2})*$/.test(text)) throw new Error(`bad hex: ${String(text)}`);
  return Buffer.from(text, 'hex').toString('latin1');
}

const lines = (stdout: string): string[][] =>
  stdout
    .split('\n')
    .filter((line) => line !== '')
    .map((line) => line.split(' '));

/** `head …` + `field …` lines per head, then one `allocs` line. */
export function parseCheck(stdout: string): CheckReport {
  const heads: { status: number; headLen: number; contentLength: number | null; count: number; fields: Field[] }[] = [];
  let allocsPerHead: number | undefined;
  for (const [tag, ...rest] of lines(stdout)) {
    if (allocsPerHead !== undefined) throw new Error('output after the allocs line');
    if (tag === 'head') {
      const [status, headLen, length, count] = rest;
      heads.push({
        status: integer(status, 'status'),
        headLen: integer(headLen, 'head length'),
        contentLength: length === '-' ? null : integer(length, 'content length'),
        count: integer(count, 'field count'),
        fields: [],
      });
    } else if (tag === 'field') {
      const head = heads.at(-1);
      if (head === undefined) throw new Error('field before head');
      head.fields.push({ name: hex(rest[0]), value: hex(rest[1]) });
    } else if (tag === 'allocs') {
      allocsPerHead = decimal(rest[0], 'allocs');
    } else {
      throw new Error(`unexpected line: ${String(tag)}`);
    }
  }
  if (allocsPerHead === undefined) throw new Error('missing allocs line');
  for (const head of heads) {
    if (head.fields.length !== head.count) throw new Error('field count does not match the field lines');
  }
  return {
    heads: heads.map(({ status, headLen, contentLength, fields }) => ({ status, headLen, contentLength, fields })),
    allocsPerHead,
  };
}

function single(stdout: string, tag: string): string[] {
  const all = lines(stdout);
  const [first] = all;
  if (all.length !== 1 || first?.[0] !== tag) throw new Error(`expected one ${tag} line, got: ${stdout}`);
  return first.slice(1);
}

/** `ns <ns per head> <sink>`. */
export function parseTime(stdout: string): number {
  return decimal(single(stdout, 'ns')[0], 'ns per head');
}

/** `feed <total ns> <mode> <head length>`. */
export function parseFeed(stdout: string): FeedReport {
  const [ns, mode, headLen] = single(stdout, 'feed');
  if (mode === undefined || !FEED_MODES.has(mode)) throw new Error(`bad feed mode: ${String(mode)}`);
  return { ns: integer(ns, 'feed ns'), mode: mode as FeedMode, headLen: integer(headLen, 'head length') };
}

/** Names lower-cased and fields stably sorted by name: the form a parser that keeps a header map can produce. */
function unordered(fields: readonly Field[]): Field[] {
  return fields
    .map((f) => ({ name: f.name.toLowerCase(), value: f.value }))
    .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
}

/**
 * Every way `actual` differs from `expected` (empty when they agree). Status, head length, Content-Length and the
 * field count must match exactly; names and values must match byte for byte, in order when `orderPreserved`,
 * otherwise as a case-insensitive name multiset (values keep their order per name).
 */
export function differences(
  expected: readonly HeadReport[],
  actual: readonly HeadReport[],
  orderPreserved: boolean,
): string[] {
  if (expected.length !== actual.length) return [`${String(actual.length)} heads, expected ${String(expected.length)}`];
  return expected.flatMap((want, i) => {
    const got = actual[i] as HeadReport; // same length as `expected`
    const at = `head ${String(i)}`;
    const found: string[] = [];
    for (const key of ['status', 'headLen', 'contentLength'] as const) {
      if (want[key] !== got[key]) found.push(`${at}: ${key} ${String(got[key])}, expected ${String(want[key])}`);
    }
    if (want.fields.length !== got.fields.length) {
      return [...found, `${at}: ${String(got.fields.length)} fields, expected ${String(want.fields.length)}`];
    }
    const [a, b] = orderPreserved ? [want.fields, got.fields] : [unordered(want.fields), unordered(got.fields)];
    a.forEach((field, j) => {
      const other = b[j] as Field; // same field count
      if (other.name !== field.name || other.value !== field.value) {
        found.push(`${at} field ${String(j)}: ${JSON.stringify(other)}, expected ${JSON.stringify(field)}`);
      }
    });
    return found;
  });
}
