// Statistics for the parser bench: median and range over rounds, and per-round ratios against our parser
// (the same statistic as crates/engine-core/examples/h1_parse_bench.rs: the median of per-run ratios).

export interface Spread {
  readonly median: number;
  readonly min: number;
  readonly max: number;
}

export function spread(values: readonly number[]): Spread {
  if (values.length === 0) throw new Error('no values');
  const sorted = [...values].sort((a, b) => a - b);
  // The middle element, or the two middle elements of an even count.
  const middle = sorted.slice((sorted.length - 1) >> 1, (sorted.length >> 1) + 1);
  const median = middle.reduce((sum, n) => sum + n, 0) / middle.length;
  return { median, min: Math.min(...values), max: Math.max(...values) };
}

/** `ours[i] / theirs[i]` per round: below 1 means our parser was faster in that round. */
export function ratios(ours: readonly number[], theirs: readonly number[]): number[] {
  if (ours.length !== theirs.length) throw new Error('rounds differ');
  return theirs.map((n, i) => Number(ours[i]) / n);
}
