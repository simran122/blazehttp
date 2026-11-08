import { describe, expect, it } from 'vitest';

import { feedFailure, gateFailures } from './gate.ts';
import type { CheckReports, GateParser } from './gate.ts';
import type { CheckReport, HeadReport } from './protocol.ts';

const HEAD = Buffer.from('HTTP/1.1 200 OK\r\na: 1\r\n\r\n', 'latin1');
const WORKLOAD = { name: 'w', heads: [HEAD] };

const head = (overrides: Partial<HeadReport> = {}): HeadReport => ({
  status: 200,
  headLen: HEAD.length,
  length: null,
  keepAlive: false,
  fields: [{ name: 'a', value: '1' }],
  ...overrides,
});
const report = (...heads: HeadReport[]): CheckReport => ({ heads, allocsPerHead: 0 });
const parser = (id: string): GateParser => ({ id, orderPreserved: true, consumed: [] });
const reports = (entries: Record<string, CheckReport>): CheckReports =>
  new Map(Object.entries(entries).map(([id, r]) => [id, new Map([['w', r]])]));

describe('gateFailures', () => {
  it('passes when the reference agrees with itself and the input', () => {
    expect(gateFailures([parser('ours')], [WORKLOAD], reports({ ours: report(head()) }), 'ours')).toEqual([]);
  });

  it('checks the reference against the input bytes, not only against itself', () => {
    const wrong = reports({ ours: report(head({ headLen: 3 })) });
    expect(gateFailures([parser('ours')], [WORKLOAD], wrong, 'ours')).toEqual([
      `ours w: head 0 length 3, expected ${String(HEAD.length)}`,
    ]);
  });

  it('reports a parser that returns more heads than the input holds', () => {
    const extra = reports({ ours: report(head()), other: report(head(), head()) });
    expect(gateFailures([parser('ours'), parser('other')], [WORKLOAD], extra, 'ours')).toEqual([
      'other w: 2 heads, expected 1',
    ]);
  });

  it('reports an over-long field list against the reference', () => {
    const longer = head({ fields: [...head().fields, { name: 'b', value: '2' }] });
    const failures = gateFailures(
      [parser('ours'), parser('other')],
      [WORKLOAD],
      reports({ ours: report(head()), other: report(longer) }),
      'ours',
    );
    expect(failures).toEqual(['other w: head 0: 2 fields, expected 1']);
  });

  it('reports missing reports', () => {
    expect(gateFailures([parser('ours')], [WORKLOAD], new Map(), 'ours')).toEqual(['ours w: no report']);
    expect(gateFailures([parser('other')], [WORKLOAD], reports({ other: report(head()) }), 'ours')).toEqual([
      'other w: no reference report',
    ]);
  });
});

describe('feedFailure', () => {
  it('accepts a feed that consumed the whole head and reports one that did not', () => {
    expect(feedFailure('p', WORKLOAD, { ns: 1, mode: 'incremental', headLen: HEAD.length })).toBeUndefined();
    expect(feedFailure('p', WORKLOAD, { ns: 1, mode: 'restart', headLen: 5 })).toBe(
      `p w: fed head length 5, expected ${String(HEAD.length)}`,
    );
  });
});
