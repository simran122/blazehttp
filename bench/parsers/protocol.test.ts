import { describe, expect, it } from 'vitest';

import { differences, parseCheck, parseFeed, parseTime } from './protocol.ts';
import type { HeadReport } from './protocol.ts';

const hex = (s: string): string => Buffer.from(s, 'latin1').toString('hex');
const field = (name: string, value: string): string => `field ${hex(name)} ${hex(value)}`;

const CHECK = [
  'head 200 109 847 2',
  field('content-type', 'application/json'),
  field('content-length', '847'),
  'head 204 40 - 0',
  'allocs 0.5',
  '',
].join('\n');

const head = (fields: [string, string][], overrides: Partial<HeadReport> = {}): HeadReport => ({
  status: 200,
  headLen: 109,
  contentLength: 847,
  fields: fields.map(([name, value]) => ({ name, value })),
  ...overrides,
});

describe('parseCheck', () => {
  it('reads heads, fields and allocations', () => {
    expect(parseCheck(CHECK)).toEqual({
      heads: [
        head([
          ['content-type', 'application/json'],
          ['content-length', '847'],
        ]),
        head([], { status: 204, headLen: 40, contentLength: null }),
      ],
      allocsPerHead: 0.5,
    });
  });

  it('keeps arbitrary bytes in names and values', () => {
    const report = parseCheck(`head 200 10 - 1\n${field('x', '"\t\xff')}\nallocs 0\n`);
    expect(report.heads[0]?.fields).toEqual([{ name: 'x', value: '"\t\xff' }]);
  });

  it.each([
    ['a field count that does not match', 'head 200 10 - 2\nfield 78 79\nallocs 0\n'],
    ['a missing allocs line', 'head 200 10 - 0\n'],
    ['output after allocs', 'allocs 0\nhead 200 10 - 0\n'],
    ['a field before any head', 'field 78 79\nallocs 0\n'],
    ['an unknown line', 'heads 1\nallocs 0\n'],
    ['bad hex', 'head 200 10 - 1\nfield 7 79\nallocs 0\n'],
    ['a missing value', 'head 200 10 - 1\nfield 78\nallocs 0\n'],
    ['a non-numeric status', 'head OK 10 - 0\nallocs 0\n'],
    ['negative allocations', 'allocs -1\n'],
    ['empty allocations', 'allocs\n'],
  ])('rejects %s', (_, stdout) => {
    expect(() => parseCheck(stdout)).toThrow();
  });
});

describe('parseTime and parseFeed', () => {
  it('read one result line', () => {
    expect(parseTime('ns 45.25 123\n')).toBe(45.25);
    expect(parseFeed('feed 180000 incremental 60041\n')).toEqual({ ns: 180000, mode: 'incremental', headLen: 60041 });
  });

  it.each([
    ['time', () => parseTime('ns 1\nns 2\n')],
    ['time', () => parseTime('feed 1 incremental 2\n')],
    ['time', () => parseTime('ns NaN 1\n')],
    ['feed', () => parseFeed('feed 1 sometimes 2\n')],
    ['feed', () => parseFeed('feed 1.5 restart 2\n')],
  ])('reject malformed %s output', (_, parse) => {
    expect(parse).toThrow();
  });
});

describe('differences (the differential gate)', () => {
  const reference = [
    head([
      ['content-type', 'a'],
      ['x-b', '1'],
      ['x-b', '2'],
    ]),
  ];

  it('accepts identical heads', () => {
    expect(differences(reference, reference, true)).toEqual([]);
  });

  it('accepts a canonicalised, reordered map only when order is not preserved', () => {
    const map = [
      head([
        ['X-B', '1'],
        ['X-B', '2'],
        ['Content-Type', 'a'],
      ]),
    ];
    expect(differences(reference, map, false)).toEqual([]);
    expect(differences(reference, map, true)).toHaveLength(3);
  });

  it('reports value order within a name even for maps', () => {
    const swapped = [
      head([
        ['content-type', 'a'],
        ['x-b', '2'],
        ['x-b', '1'],
      ]),
    ];
    expect(differences(reference, swapped, false)).toHaveLength(2);
  });

  it('reports scalar, count and length mismatches', () => {
    expect(
      differences(reference, [head(reference[0]?.fields.map((f) => [f.name, f.value]) ?? [], { status: 201 })], true),
    ).toEqual(['head 0: status 201, expected 200']);
    expect(differences(reference, [head([['content-type', 'a']], { headLen: 1, contentLength: null })], true)).toEqual([
      'head 0: headLen 1, expected 109',
      'head 0: contentLength null, expected 847',
      'head 0: 1 fields, expected 3',
    ]);
    expect(differences(reference, [], true)).toEqual(['0 heads, expected 1']);
  });

  it('reports a value that differs by one byte', () => {
    const changed = [
      head([
        ['content-type', 'a '],
        ['x-b', '1'],
        ['x-b', '2'],
      ]),
    ];
    expect(differences(reference, changed, true)).toEqual([
      'head 0 field 0: {"name":"content-type","value":"a "}, expected {"name":"content-type","value":"a"}',
    ]);
  });
});
