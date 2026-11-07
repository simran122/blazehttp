import { describe, expect, it } from 'vitest';

import { differences, parseCheck, parseFeed, parseTime } from './protocol.ts';
import type { FieldView, HeadReport } from './protocol.ts';

const hex = (s: string): string => Buffer.from(s, 'latin1').toString('hex');
const field = (name: string, value: string): string => `field ${hex(name)} ${hex(value)}`;

const CHECK = [
  'head 200 109 847 1 2',
  field('content-type', 'application/json'),
  field('content-length', '847'),
  'head 204 40 - 0 0',
  'head 200 60 chunked 0 0',
  'allocs 0.5',
  '',
].join('\n');

const head = (fields: [string, string][], overrides: Partial<HeadReport> = {}): HeadReport => ({
  status: 200,
  headLen: 109,
  length: 847,
  keepAlive: true,
  fields: fields.map(([name, value]) => ({ name, value })),
  ...overrides,
});

const ORDERED: FieldView = { orderPreserved: true, consumed: [] };
const MAP: FieldView = { orderPreserved: false, consumed: [] };

describe('parseCheck', () => {
  it('reads heads, facts, fields and allocations', () => {
    expect(parseCheck(CHECK)).toEqual({
      heads: [
        head([
          ['content-type', 'application/json'],
          ['content-length', '847'],
        ]),
        head([], { status: 204, headLen: 40, length: null, keepAlive: false }),
        head([], { headLen: 60, length: 'chunked', keepAlive: false }),
      ],
      allocsPerHead: 0.5,
    });
  });

  it('keeps arbitrary bytes in names and values', () => {
    const report = parseCheck(`head 200 10 - 0 1\n${field('x', '"\t\xff')}\nallocs 0\n`);
    expect(report.heads[0]?.fields).toEqual([{ name: 'x', value: '"\t\xff' }]);
  });

  it.each([
    ['a field count that does not match', 'head 200 10 - 0 2\nfield 78 79\nallocs 0\n'],
    ['a missing allocs line', 'head 200 10 - 0 0\n'],
    ['output after allocs', 'allocs 0\nhead 200 10 - 0 0\n'],
    ['a field before any head', 'field 78 79\nallocs 0\n'],
    ['an unknown line', 'heads 1\nallocs 0\n'],
    ['bad hex', 'head 200 10 - 0 1\nfield 7 79\nallocs 0\n'],
    ['a missing value', 'head 200 10 - 0 1\nfield 78\nallocs 0\n'],
    ['a non-numeric status', 'head OK 10 - 0 0\nallocs 0\n'],
    ['a bad keep-alive flag', 'head 200 10 - yes 0\nallocs 0\n'],
    ['a bad length word', 'head 200 10 close 0 0\nallocs 0\n'],
    ['a length beyond 2^53', 'head 200 10 9007199254740993 1 0\nallocs 0\n'],
    ['negative allocations', 'allocs -1\n'],
    ['hex allocations', 'allocs 0x10\n'],
    ['exponent allocations', 'allocs 1e3\n'],
    ['empty allocations', 'allocs\n'],
  ])('rejects %s', (_, stdout) => {
    expect(() => parseCheck(stdout)).toThrow();
  });
});

describe('parseTime and parseFeed', () => {
  it('read one result line', () => {
    expect(parseTime('ns 45.25 123\n')).toBe(45.25);
    expect(parseFeed('feed 180000 incremental 60041\n')).toEqual({ ns: 180000, mode: 'incremental', headLen: 60041 });
    expect(parseFeed('feed 1 line-restart 2\n').mode).toBe('line-restart');
  });

  it.each([
    ['two lines', () => parseTime('ns 1\nns 2\n')],
    ['the wrong tag', () => parseTime('feed 1 incremental 2\n')],
    ['NaN', () => parseTime('ns NaN 1\n')],
    ['zero ns', () => parseTime('ns 0 1\n')],
    ['an exponent', () => parseTime('ns 1e3 1\n')],
    ['an unknown mode', () => parseFeed('feed 1 sometimes 2\n')],
    ['fractional feed ns', () => parseFeed('feed 1.5 restart 2\n')],
  ])('reject %s', (_, parse) => {
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
    expect(differences(reference, reference, ORDERED)).toEqual([]);
  });

  it('accepts a canonicalised, reordered map only when order is not preserved', () => {
    const map = [
      head([
        ['X-B', '1'],
        ['X-B', '2'],
        ['Content-Type', 'a'],
      ]),
    ];
    expect(differences(reference, map, MAP)).toEqual([]);
    expect(differences(reference, map, ORDERED)).toHaveLength(3);
  });

  it('reports value order within a name even for maps', () => {
    const swapped = [
      head([
        ['content-type', 'a'],
        ['x-b', '2'],
        ['x-b', '1'],
      ]),
    ];
    expect(differences(reference, swapped, MAP)).toHaveLength(2);
  });

  it('ignores consumed fields on both sides', () => {
    const withConsumed = [
      head([
        ['Connection', 'close'],
        ['content-type', 'a'],
        ['x-b', '1'],
        ['x-b', '2'],
      ]),
    ];
    expect(differences(withConsumed, reference, { orderPreserved: false, consumed: ['connection'] })).toEqual([]);
    expect(differences(withConsumed, reference, MAP)).toEqual(['head 0: 3 fields, expected 4']);
  });

  it('reports fact, count and length mismatches', () => {
    const same = reference[0]?.fields.map((f): [string, string] => [f.name, f.value]) ?? [];
    expect(differences(reference, [head(same, { status: 201, keepAlive: false })], ORDERED)).toEqual([
      'head 0: status 201, expected 200',
      'head 0: keepAlive false, expected true',
    ]);
    expect(differences(reference, [head([['content-type', 'a']], { headLen: 1, length: 'chunked' })], ORDERED)).toEqual(
      ['head 0: headLen 1, expected 109', 'head 0: length chunked, expected 847', 'head 0: 1 fields, expected 3'],
    );
    expect(differences(reference, [], ORDERED)).toEqual(['0 heads, expected 1']);
  });

  it('does not compare keep-alive when the body is read to EOF', () => {
    const eof = [head([], { length: null, keepAlive: false })];
    expect(differences(eof, [head([], { length: null, keepAlive: true })], ORDERED)).toEqual([]);
  });

  it('reports a value that differs by one byte', () => {
    const changed = [
      head([
        ['content-type', 'a '],
        ['x-b', '1'],
        ['x-b', '2'],
      ]),
    ];
    expect(differences(reference, changed, ORDERED)).toEqual([
      'head 0 field 0: {"name":"content-type","value":"a "}, expected {"name":"content-type","value":"a"}',
    ]);
  });
});
