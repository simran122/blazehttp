// Parses what a harness prints (bench/parsers/README.md "Harness protocol") and compares heads across parsers
// (the differential correctness gate that runs before any timing).

interface Field {
  readonly name: string;
  readonly value: string;
}

/** How the body is delimited: a Content-Length, chunked, or neither (`null`: read to EOF). */
type Length = number | 'chunked' | null;

/** One head as a harness reports it: the facts come from the timed `parse` path, the fields from its field view. */
export interface HeadReport {
  readonly status: number;
  readonly headLen: number;
  readonly length: Length;
  readonly keepAlive: boolean;
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

/** How the gate compares one parser's fields with the reference. */
export interface FieldView {
  /** False when the parser keeps fields in a map (names canonicalised, order lost): compared as a multiset. */
  readonly orderPreserved: boolean;
  /** Lower-case names the parser consumes into its facts and drops from its field view (checked via the facts). */
  readonly consumed: readonly string[];
}

const FEED_MODES: ReadonlySet<string> = new Set<FeedMode>(['incremental', 'line-restart', 'restart', 'restart-skip']);

/** A non-negative decimal integer that a JavaScript number holds exactly. */
function integer(text: string | undefined, what: string): number {
  const n = Number(text);
  if (text === undefined || !/^\d+$/.test(text) || !Number.isSafeInteger(n)) {
    throw new Error(`bad ${what}: ${String(text)}`);
  }
  return n;
}

/** A plain decimal (`12`, `0.5`; no sign, exponent or hex), above zero when `positive`. */
function decimal(text: string | undefined, what: string, positive: boolean): number {
  const n = Number(text);
  if (text === undefined || !/^\d+(?:\.\d+)?$/.test(text) || (positive && n === 0)) {
    throw new Error(`bad ${what}: ${String(text)}`);
  }
  return n;
}

function hex(text: string | undefined): string {
  if (text === undefined || !/^(?:[0-9a-f]{2})*$/.test(text)) throw new Error(`bad hex: ${String(text)}`);
  return Buffer.from(text, 'hex').toString('latin1');
}

const FLAGS: Readonly<Record<string, boolean>> = { '0': false, '1': true };

function flag(text: string | undefined, what: string): boolean {
  const value = FLAGS[text ?? ''];
  if (value === undefined) throw new Error(`bad ${what}: ${String(text)}`);
  return value;
}

const LENGTH_WORDS: Readonly<Record<string, Length>> = { '-': null, chunked: 'chunked' };

function length(text: string | undefined): Length {
  const word = LENGTH_WORDS[text ?? ''];
  return word === undefined ? integer(text, 'length') : word;
}

const lines = (stdout: string): string[][] =>
  stdout
    .split('\n')
    .filter((line) => line !== '')
    .map((line) => line.split(' '));

/** `head <status> <head len> <length> <keep-alive> <field count>` + `field …` lines per head, then `allocs`. */
export function parseCheck(stdout: string): CheckReport {
  const heads: (Omit<HeadReport, 'fields'> & { count: number; fields: Field[] })[] = [];
  let allocsPerHead: number | undefined;
  for (const [tag, ...rest] of lines(stdout)) {
    if (allocsPerHead !== undefined) throw new Error('output after the allocs line');
    if (tag === 'head') {
      const [status, headLen, len, keepAlive, count] = rest;
      heads.push({
        status: integer(status, 'status'),
        headLen: integer(headLen, 'head length'),
        length: length(len),
        keepAlive: flag(keepAlive, 'keep-alive'),
        count: integer(count, 'field count'),
        fields: [],
      });
    } else if (tag === 'field') {
      const head = heads.at(-1);
      if (head === undefined) throw new Error('field before head');
      head.fields.push({ name: hex(rest[0]), value: hex(rest[1]) });
    } else if (tag === 'allocs') {
      allocsPerHead = decimal(rest[0], 'allocs', false);
    } else {
      throw new Error(`unexpected line: ${String(tag)}`);
    }
  }
  if (allocsPerHead === undefined) throw new Error('missing allocs line');
  return {
    heads: heads.map(({ count, ...head }) => {
      if (head.fields.length !== count) throw new Error('field count does not match the field lines');
      return head;
    }),
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
  return decimal(single(stdout, 'ns')[0], 'ns per head', true);
}

/** `feed <total ns> <mode> <head length>`. */
export function parseFeed(stdout: string): FeedReport {
  const [ns, mode, headLen] = single(stdout, 'feed');
  if (mode === undefined || !FEED_MODES.has(mode)) throw new Error(`bad feed mode: ${String(mode)}`);
  return { ns: integer(ns, 'feed ns'), mode: mode as FeedMode, headLen: integer(headLen, 'head length') };
}

/** The fields a parser with `view` can be compared on, in the order it can be compared in. */
function comparable(fields: readonly Field[], view: FieldView): Field[] {
  const kept = fields.filter((f) => !view.consumed.includes(f.name.toLowerCase()));
  if (view.orderPreserved) return kept;
  // Names lower-cased and stably sorted by name: the form a parser that keeps a header map can produce.
  return kept
    .map((f) => ({ name: f.name.toLowerCase(), value: f.value }))
    .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
}

/**
 * Every way `actual` differs from `expected` (empty when they agree). Status, head length and length must match
 * exactly; keep-alive too unless the body is read to EOF (parsers differ on whether that alone ends reuse). Names and
 * values must match byte for byte, as `view` allows.
 */
export function differences(expected: readonly HeadReport[], actual: readonly HeadReport[], view: FieldView): string[] {
  if (expected.length !== actual.length) return [`${String(actual.length)} heads, expected ${String(expected.length)}`];
  return expected.flatMap((want, i) => {
    const got = actual[i] as HeadReport; // same length as `expected`
    const at = `head ${String(i)}`;
    const keys =
      want.length === null
        ? (['status', 'headLen', 'length'] as const)
        : (['status', 'headLen', 'length', 'keepAlive'] as const);
    const found = keys
      .filter((key) => want[key] !== got[key])
      .map((key) => `${at}: ${key} ${String(got[key])}, expected ${String(want[key])}`);
    const [a, b] = [comparable(want.fields, view), comparable(got.fields, view)];
    if (a.length !== b.length) return [...found, `${at}: ${String(b.length)} fields, expected ${String(a.length)}`];
    a.forEach((field, j) => {
      const other = b[j] as Field; // same field count
      if (other.name !== field.name || other.value !== field.value) {
        found.push(`${at} field ${String(j)}: ${JSON.stringify(other)}, expected ${JSON.stringify(field)}`);
      }
    });
    return found;
  });
}
