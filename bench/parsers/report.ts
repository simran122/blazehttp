// Result records (one JSONL line each), their assembly from the raw samples, and the tables the runner prints.
import type { FeedMode } from './protocol.ts';
import { ratios, spread } from './stats.ts';
import type { Spread } from './stats.ts';

export interface TimedResult {
  readonly kind: 'timed';
  readonly parser: string;
  readonly language: string;
  readonly workload: string;
  readonly heads: number;
  readonly bytes: number;
  readonly rounds: number;
  readonly nsPerHead: Spread;
  /** Per-round `blazehttp+fields / theirs` (the headline comparison); below 1 means ours was faster. */
  readonly fieldsOverTheirs: Spread;
  /** Per-round `blazehttp / theirs` (our parser without the per-field split). */
  readonly oursOverTheirs: Spread;
  readonly allocsPerHead: number;
  readonly work: string;
}

export interface FeedResult {
  readonly kind: 'feed';
  readonly parser: string;
  readonly language: string;
  readonly workload: string;
  readonly bytes: number;
  readonly rounds: number;
  readonly mode: FeedMode;
  readonly ms: Spread;
}

export interface ResultParser {
  readonly id: string;
  readonly language: string;
  readonly work: string;
}

export interface ResultWorkload {
  readonly name: string;
  readonly heads: readonly Buffer[];
}

export const sampleKey = (parser: string, workload: string): string => `${parser}/${workload}`;

/**
 * One result per parser × workload from the per-round samples (`sampleKey` → ns per head, one per round), with
 * per-round ratios against `ours` and `oursWithFields`.
 */
export function timedResults(
  parsers: readonly ResultParser[],
  workloads: readonly ResultWorkload[],
  samples: ReadonlyMap<string, readonly number[]>,
  allocs: (parser: string, workload: string) => number,
  reference: { readonly ours: string; readonly oursWithFields: string },
): TimedResult[] {
  const series = (parser: string, workload: string): readonly number[] => {
    const values = samples.get(sampleKey(parser, workload));
    if (values === undefined) throw new Error(`no samples for ${sampleKey(parser, workload)}`);
    return values;
  };
  return workloads.flatMap((w) =>
    parsers.map((parser): TimedResult => {
      const ns = series(parser.id, w.name);
      return {
        kind: 'timed',
        parser: parser.id,
        language: parser.language,
        workload: w.name,
        heads: w.heads.length,
        bytes: w.heads.reduce((n, head) => n + head.length, 0),
        rounds: ns.length,
        nsPerHead: spread(ns),
        fieldsOverTheirs: spread(ratios(series(reference.oursWithFields, w.name), ns)),
        oursOverTheirs: spread(ratios(series(reference.ours, w.name), ns)),
        allocsPerHead: allocs(parser.id, w.name),
        work: parser.work,
      };
    }),
  );
}

const fixed = (n: number, digits: number): string => n.toFixed(digits);
const range = (s: Spread, digits: number): string => `${fixed(s.min, digits)}–${fixed(s.max, digits)}`;

export function markdownTable(header: readonly string[], rows: readonly (readonly string[])[]): string {
  const line = (cells: readonly string[]): string => `| ${cells.join(' | ')} |`;
  return [line(header), line(header.map(() => '---')), ...rows.map(line)].join('\n');
}

export function timedTable(results: readonly TimedResult[]): string {
  return markdownTable(
    [
      'workload',
      'parser',
      'ns/head median',
      'ns min–max',
      'ours+fields/theirs median',
      'min–max',
      'ours/theirs median',
      'allocs/head',
    ],
    results.map((r) => [
      r.workload,
      `${r.parser} (${r.language})`,
      fixed(r.nsPerHead.median, 1),
      range(r.nsPerHead, 1),
      fixed(r.fieldsOverTheirs.median, 3),
      range(r.fieldsOverTheirs, 3),
      fixed(r.oursOverTheirs.median, 3),
      fixed(r.allocsPerHead, 1),
    ]),
  );
}

export function feedTable(results: readonly FeedResult[]): string {
  return markdownTable(
    ['input', 'bytes', 'parser', 'partial input', 'ms median', 'ms min–max'],
    results.map((r) => [
      r.workload,
      String(r.bytes),
      `${r.parser} (${r.language})`,
      r.mode,
      fixed(r.ms.median, 3),
      range(r.ms, 3),
    ]),
  );
}
