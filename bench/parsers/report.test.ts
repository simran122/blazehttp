import { describe, expect, it } from 'vitest';

import { feedTable, markdownTable, timedTable } from './report.ts';
import { ratios, spread } from './stats.ts';

describe('stats', () => {
  it('takes the median, min and max', () => {
    expect(spread([3, 1, 2])).toEqual({ median: 2, min: 1, max: 3 });
    expect(spread([4, 1, 3, 2])).toEqual({ median: 2.5, min: 1, max: 4 });
    expect(() => spread([])).toThrow('no values');
  });

  it('divides ours by theirs per round', () => {
    expect(ratios([40, 50], [80, 50])).toEqual([0.5, 1]);
    expect(() => ratios([1], [1, 2])).toThrow('rounds differ');
  });
});

describe('tables', () => {
  it('renders markdown', () => {
    expect(markdownTable(['a', 'b'], [['1', '2']])).toBe('| a | b |\n| --- | --- |\n| 1 | 2 |');
  });

  it('formats timed and feed results', () => {
    const timed = timedTable([
      {
        kind: 'timed',
        parser: 'httparse',
        language: 'Rust',
        workload: '1k-3-headers',
        heads: 1,
        bytes: 109,
        rounds: 7,
        nsPerHead: { median: 52.04, min: 51.5, max: 53 },
        oursOverTheirs: { median: 0.8412, min: 0.8, max: 0.9 },
        allocsPerHead: 0,
        work: 'slices',
      },
    ]);
    expect(timed.split('\n')[2]).toBe(
      '| 1k-3-headers | httparse (Rust) | 52.0 | 51.5–53.0 | 0.841 | 0.800–0.900 | 0.0 |',
    );
    const feed = feedTable([
      {
        kind: 'feed',
        parser: 'llhttp',
        language: 'C',
        workload: 'feed-a-60k-value',
        bytes: 60_043,
        rounds: 5,
        mode: 'incremental',
        ms: { median: 0.21, min: 0.2, max: 0.25 },
      },
    ]);
    expect(feed.split('\n')[2]).toBe('| feed-a-60k-value | 60043 | llhttp (C) | incremental | 0.210 | 0.200–0.250 |');
  });
});
