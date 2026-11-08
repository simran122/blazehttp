import { describe, expect, it } from 'vitest';

import { MAX_RUN_MS, integerOption } from './options.ts';

describe('integerOption', () => {
  const runMs = (text: string): number => integerOption(text, '--run-ms', 1, MAX_RUN_MS);

  it('accepts the bounds of a bounded option', () => {
    expect(MAX_RUN_MS).toBe(60_000);
    expect(runMs('1')).toBe(1);
    expect(runMs('60000')).toBe(60_000);
  });

  it.each(['0', '60001', '1e3', '', '-1', ' 5', '1.5', '99999999999999999999'])(
    'rejects %j for a bounded option, naming the range',
    (text) => {
      expect(() => runMs(text)).toThrow(/^--run-ms must be an integer from 1 to 60000$/);
    },
  );

  it('names only the minimum for an option with no upper bound', () => {
    expect(() => integerOption('3', '--rounds', 5)).toThrow(/^--rounds must be an integer ≥ 5$/);
    expect(integerOption('5', '--rounds', 5)).toBe(5);
    expect(integerOption('0', '--cpuset', 0)).toBe(0);
  });

  it('rejects an integer beyond what a number holds exactly, even with no upper bound', () => {
    expect(() => integerOption('9007199254740993', '--feed-rounds', 1)).toThrow(
      /^--feed-rounds must be an integer ≥ 1$/,
    );
    expect(integerOption('9007199254740991', '--feed-rounds', 1)).toBe(Number.MAX_SAFE_INTEGER);
  });
});
