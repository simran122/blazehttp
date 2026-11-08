import { describe, expect, it } from 'vitest';

import { feedTable, markdownTable, sampleKey, timedResults, timedTable } from './report.ts';
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

describe('timedResults', () => {
  const REFERENCE = { ours: 'ours', oursWithFields: 'ours+f' };
  const parsers = [
    { id: 'ours', language: 'Rust', work: 'a' },
    { id: 'ours+f', language: 'Rust', work: 'b' },
    { id: 'theirs', language: 'C', work: 'c' },
  ];
  const workloads = [{ name: 'w', heads: [Buffer.from('ab'), Buffer.from('cde')] }];
  const samples = new Map([
    [sampleKey('ours', 'w'), [10, 20, 30]],
    [sampleKey('ours+f', 'w'), [20, 40, 60]],
    [sampleKey('theirs', 'w'), [40, 40, 40]],
  ]);

  it('builds one record per parser and workload with both ratio series', () => {
    const results = timedResults(parsers, workloads, { samples, allocs: () => 0 }, REFERENCE);
    expect(results).toHaveLength(3);
    expect(results[2]).toEqual({
      kind: 'timed',
      parser: 'theirs',
      language: 'C',
      workload: 'w',
      heads: 2,
      bytes: 5,
      rounds: 3,
      nsPerHead: { median: 40, min: 40, max: 40 },
      fieldsOverTheirs: { median: 1, min: 0.5, max: 1.5 },
      oursOverTheirs: { median: 0.5, min: 0.25, max: 0.75 },
      allocsPerHead: 0,
      work: 'c',
    });
    // Our own rows compare with themselves at exactly 1.
    expect(results[0]?.oursOverTheirs).toEqual({ median: 1, min: 1, max: 1 });
  });

  it('refuses missing samples', () => {
    expect(() => timedResults(parsers, workloads, { samples: new Map(), allocs: () => 0 }, REFERENCE)).toThrow(
      'no samples for ours/w',
    );
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
        fieldsOverTheirs: { median: 1.1, min: 1, max: 1.2 },
        oursOverTheirs: { median: 0.8412, min: 0.8, max: 0.9 },
        allocsPerHead: 0,
        work: 'slices',
      },
    ]);
    expect(timed.split('\n')[2]).toBe(
      '| 1k-3-headers | httparse (Rust) | 52.0 | 51.5–53.0 | 1.100 | 1.000–1.200 | 0.841 | 0.0 |',
    );
    const feed = feedTable([
      {
        kind: 'feed',
        parser: 'llhttp',
        language: 'C',
        workload: 'feed-a-60k-value',
        bytes: 60_045,
        rounds: 5,
        mode: 'incremental',
        ms: { median: 0.21, min: 0.2, max: 0.25 },
      },
    ]);
    expect(feed.split('\n')[2]).toBe('| feed-a-60k-value | 60045 | llhttp (C) | incremental | 0.210 | 0.200–0.250 |');
  });
});
