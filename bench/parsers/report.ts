// Result records (one JSONL line each) and the tables the runner prints.
import type { FeedMode } from './protocol.ts';
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
  /** Per-round `ours / theirs`; below 1 means our parser was faster. */
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

const fixed = (n: number, digits: number): string => n.toFixed(digits);
const range = (s: Spread, digits: number): string => `${fixed(s.min, digits)}–${fixed(s.max, digits)}`;

export function markdownTable(header: readonly string[], rows: readonly (readonly string[])[]): string {
  const line = (cells: readonly string[]): string => `| ${cells.join(' | ')} |`;
  return [line(header), line(header.map(() => '---')), ...rows.map(line)].join('\n');
}

export function timedTable(results: readonly TimedResult[]): string {
  return markdownTable(
    ['workload', 'parser', 'ns/head median', 'ns min–max', 'ours/theirs median', 'ours/theirs min–max', 'allocs/head'],
    results.map((r) => [
      r.workload,
      `${r.parser} (${r.language})`,
      fixed(r.nsPerHead.median, 1),
      range(r.nsPerHead, 1),
      fixed(r.oursOverTheirs.median, 3),
      range(r.oursOverTheirs, 3),
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
