import { describe, expect, it } from 'vitest';
import {
  diffCases,
  importLlhttp,
  type LlhttpOverride,
  nativeError,
  normaliseInput,
  parseErrorNames,
  mitCopyright,
  parseOverrides,
  tagCommit,
} from './import-llhttp.ts';
import { LICENCE_TEXTS } from './notices.ts';
import type { VectorSource } from './schema.ts';

const FENCE = '```';
const SOURCE: VectorSource = {
  project: 'llhttp',
  url: 'https://github.com/nodejs/llhttp/blob/abc/test/response/fixture.md',
  commit: 'abc',
  licence: 'MIT',
  copyright: 'Copyright © 2018 Fedor Indutny',
};
const ERROR_NAMES = parseErrorNames(
  'export const ERROR = {\n  OK: 0,\n  UNEXPECTED_CONTENT_LENGTH: 4,\n  INVALID_CONSTANT: 8,\n} as const;\n',
);
const MALFORMED = { error: 'PROTOCOL/malformed-response' } as const;
const RTSP_OVERRIDE: LlhttpOverride = {
  native: MALFORMED,
  rule: '§5.2 Status line: exactly HTTP/1.0 or HTTP/1.1',
};

/** One markdown test the way llhttp writes them: heading, meta comment, `http` block, `log` block. */
const mdCase = (name: string, meta: string, http: string, log: string): string =>
  `## ${name}\n\n<!-- meta=${meta} -->\n${FENCE}http\n${http}\n${FENCE}\n\n${FENCE}log\n${log}\n${FENCE}\n`;
const mdFile = (...cases: string[]): string => `Fixture\n=======\n\n${cases.join('\n')}`;
const RESPONSE = '{"type": "response"}';
const LENIENT = '{"type": "response-lenient-all"}';

const SIMPLE = mdCase(
  'Simple',
  RESPONSE,
  'HTTP/1.1 200 OK\nContent-Length: 5\n\nhello',
  [
    'off=0 message begin',
    'off=0 len=4 span[protocol]="HTTP"',
    'off=5 len=3 span[version]="1.1"',
    'off=17 len=14 span[header_field]="Content-Length"',
    'off=32 header_field complete',
    'off=33 len=1 span[header_value]="5"',
    'off=36 header_value complete',
    'off=38 headers complete status=200 v=1/1 flags=20 content_length=5',
    'off=38 len=5 span[body]="hello"',
    'off=43 message complete',
  ].join('\n'),
);
const DUPLICATE_CL = mdCase(
  'Duplicate',
  RESPONSE,
  'HTTP/1.1 200 OK\nContent-Length: 1\nContent-Length: 1\n\n',
  [
    'off=0 message begin',
    'off=17 len=14 span[header_field]="Content-Length"',
    'off=33 len=1 span[header_value]="1"',
    'off=36 header_value complete',
    'off=36 len=14 span[header_field]="Content-Length"',
    'off=52 error code=4 reason="Duplicate Content-Length"',
  ].join('\n'),
);
const RTSP = mdCase(
  'RTSP',
  RESPONSE,
  'RTSP/1.1 200 OK\n\n',
  [
    'off=0 message begin',
    'off=0 len=4 span[protocol]="RTSP"',
    'off=19 headers complete status=200 v=1/1 flags=0 content_length=0',
  ].join('\n'),
);
const FIXTURE = mdFile(SIMPLE, DUPLICATE_CL, RTSP);

const run = (md: string, overrides: ReadonlyMap<string, LlhttpOverride> = new Map()) =>
  importLlhttp([{ stem: 'fixture', md, source: SOURCE }], { errorNames: ERROR_NAMES, overrides });
const runOne = (md: string, overrides?: ReadonlyMap<string, LlhttpOverride>) => {
  const [result] = run(mdFile(md), overrides);
  if (result === undefined) throw new Error('no result');
  return result;
};
const withRtsp = new Map([['fixture / RTSP', RTSP_OVERRIDE]]);

describe('importLlhttp', () => {
  it('converts http/log block pairs into cases', () => {
    const [result] = run(FIXTURE, withRtsp);
    expect(result?.file.source).toEqual(SOURCE);
    expect(result?.file.cases.map((c) => c.name)).toEqual([
      'fixture / Simple',
      'fixture / Duplicate',
      'fixture / RTSP',
    ]);
    const simple = result?.file.cases[0];
    expect(simple).toMatchObject({
      input: 'HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello',
      inputEncoding: 'utf8',
      request: { method: 'GET' },
      expect: {
        native: { ok: { status: 200, headers: [['Content-Length', '5']], body: 'hello', keepAlive: true } },
      },
    });
    expect(simple?.reference?.['llhttp']).toContain('off=43 message complete');
  });

  it('maps llhttp errors to native expectations', () => {
    expect(nativeError('HPE_INVALID_CONSTANT')).toEqual(MALFORMED);
    expect(nativeError('HPE_UNEXPECTED_CONTENT_LENGTH')).toEqual({ error: 'PROTOCOL/smuggling-risk' });
    expect(nativeError('HPE_INVALID_TRANSFER_ENCODING')).toEqual({ error: 'PROTOCOL/smuggling-risk' });
    const [result] = run(FIXTURE, withRtsp);
    expect(result?.file.cases[1]?.expect.native).toEqual({ error: 'PROTOCOL/smuggling-risk' });
  });

  it('maps an llhttp error after an interim response', () => {
    const upgrade = runOne(
      mdCase(
        '101',
        RESPONSE,
        'HTTP/1.1 101 Switching Protocols\n\nproto',
        [
          'off=0 message begin',
          'off=0 len=4 span[protocol]="HTTP"',
          'off=36 headers complete status=101 v=1/1 flags=0 content_length=0',
          'off=36 message complete',
          'off=36 error code=8 reason="Pause on CONNECT/Upgrade"',
        ].join('\n'),
      ),
    );
    expect(upgrade.file.cases[0]?.expect.native).toEqual(MALFORMED);
  });

  it('applies overrides with a cited rule', () => {
    const [result] = run(FIXTURE, withRtsp);
    expect(result?.file.cases[2]?.expect.native).toEqual(MALFORMED);
    expect(result?.overridden).toEqual(['fixture / RTSP']);
    expect(() => parseOverrides({ 'fixture / RTSP': { native: MALFORMED } })).toThrow(/rule/);
    expect(() => parseOverrides({ 'fixture / RTSP': { native: MALFORMED, rule: 'stricter' } })).toThrow(/§/);
    expect(parseOverrides({ 'fixture / RTSP': RTSP_OVERRIDE }).get('fixture / RTSP')).toEqual(RTSP_OVERRIDE);
  });

  it('lets an override decide a case whose log cannot be modelled', () => {
    const name = 'fixture / Odd';
    const odd = mdCase('Odd', RESPONSE, 'HTTP/1.1 200 OK\n\n', 'something else');
    expect(() => run(mdFile(odd))).toThrow(/fixture \/ Odd: cannot model an unparsed log line; decide the case/);
    const decided = runOne(odd, new Map([[name, RTSP_OVERRIDE]]));
    expect(decided.file.cases[0]?.expect.native).toEqual(MALFORMED);
    expect(decided.overridden).toEqual([name]);
  });

  it('reads CRLF markdown like LF markdown', () => {
    const lf = run(FIXTURE, withRtsp);
    expect(run(FIXTURE.replaceAll('\n', '\r\n'), withRtsp)).toEqual(lf);
  });

  it('refuses a case it cannot model without an override', () => {
    expect(() => run(FIXTURE)).toThrow(/fixture \/ RTSP.*protocol RTSP/);
  });

  it('rejects stale overrides', () => {
    const stale = new Map([...withRtsp, ['fixture / Gone', RTSP_OVERRIDE]]);
    expect(() => run(FIXTURE, stale)).toThrow(/unused override.*Gone/);
  });

  it('checks every log span against the normalised input', () => {
    const shifted = SIMPLE.replace('off=33 len=1', 'off=34 len=1');
    expect(() => run(mdFile(shifted))).toThrow(/span at off=34/);
  });

  it('skips lenient modes and chunked bodies', () => {
    const lenient = runOne(mdCase('Lenient', LENIENT, 'HTTP/1.1 200 OK\n\n', 'off=0 x'));
    expect(lenient.skipped.map((s) => s.name)).toEqual(['fixture / Lenient']);
    expect(lenient.skipped[0]?.reason).toContain('response-lenient-all');
    const inlineJs = runOne(mdCase('Inline', RESPONSE, 'HTTP/1.1 200 OK\nA: ${1 + 1}\n\n', 'off=0 x'));
    expect(inlineJs.skipped[0]?.reason).toMatch(/never evaluates/);
    const chunked = runOne(
      mdCase(
        'Chunked',
        RESPONSE,
        'HTTP/1.1 200 OK\nTransfer-Encoding: chunked\n\n0\n\n',
        [
          'off=0 message begin',
          'off=0 len=4 span[protocol]="HTTP"',
          'off=17 len=17 span[header_field]="Transfer-Encoding"',
          'off=36 len=7 span[header_value]="chunked"',
          'off=45 header_value complete',
          'off=47 headers complete status=200 v=1/1 flags=208 content_length=0',
        ].join('\n'),
      ),
    );
    expect(chunked.skipped[0]?.reason).toMatch(/chunked/);
    expect(chunked.file.cases).toEqual([]);
  });

  it('derives framing and keep-alive from the final response', () => {
    const head = runOne(
      mdCase(
        'Head',
        '{"type": "response", "skipBody": true}',
        'HTTP/1.0 200 OK\nContent-Length: 9\nConnection: keep-alive\n\n',
        [
          'off=0 message begin',
          'off=0 len=4 span[protocol]="HTTP"',
          'off=17 len=14 span[header_field]="Content-Length"',
          'off=33 len=1 span[header_value]="9"',
          'off=36 header_value complete',
          'off=36 len=10 span[header_field]="Connection"',
          'off=48 len=10 span[header_value]="keep-alive"',
          'off=60 header_value complete',
          'off=62 headers complete status=200 v=1/0 flags=21 content_length=9',
          'off=62 message complete',
          'off=62 error code=8 reason="trailing data is ignored"',
        ].join('\n'),
      ),
    );
    expect(head.file.cases[0]).toMatchObject({
      request: { method: 'HEAD' },
      expect: { native: { ok: { status: 200, body: '', keepAlive: true } } },
    });

    const interimThenEof = runOne(
      mdCase(
        'Interim',
        RESPONSE,
        'HTTP/1.1 100 Continue\n\nHTTP/1.1 200 OK\nConnection: close\n\nbody',
        [
          'off=0 message begin',
          'off=0 len=4 span[protocol]="HTTP"',
          'off=25 headers complete status=100 v=1/1 flags=0 content_length=0',
          'off=25 message complete',
          'off=25 message begin',
          'off=25 len=4 span[protocol]="HTTP"',
          'off=42 len=10 span[header_field]="Connection"',
          'off=54 len=5 span[header_value]="close"',
          'off=61 header_value complete',
          'off=63 headers complete status=200 v=1/1 flags=2 content_length=0',
          'off=63 len=4 span[body]="body"',
        ].join('\n'),
      ),
    );
    expect(interimThenEof.file.cases[0]?.expect.native).toEqual({
      ok: { status: 200, headers: [['Connection', 'close']], body: 'body', keepAlive: false },
    });

    const short = runOne(
      mdCase(
        'Short',
        RESPONSE,
        'HTTP/1.1 204 No Content\nContent-Length: 0 \n\n',
        [
          'off=0 message begin',
          'off=0 len=4 span[protocol]="HTTP"',
          'off=25 len=14 span[header_field]="Content-Length"',
          'off=41 len=2 span[header_value]="0 "',
          'off=45 header_value complete',
          'off=47 headers complete status=204 v=1/1 flags=20 content_length=0',
          'off=47 message complete',
        ].join('\n'),
      ),
    );
    expect(short.file.cases[0]?.expect.native).toMatchObject({ ok: { headers: [['Content-Length', '0']] } });

    const bodyless = (status: string, headerLines: string, logLines: string[]) =>
      runOne(
        mdCase(
          status,
          RESPONSE,
          `HTTP/1.1 ${status}\n${headerLines}\n`,
          ['off=0 message begin', ...logLines].join('\n'),
        ),
      ).file.cases[0]?.expect.native;
    expect(
      bodyless('204 No Content', '', [
        'off=0 len=4 span[protocol]="HTTP"',
        'off=27 headers complete status=204 v=1/1 flags=0 content_length=0',
        'off=27 message complete',
      ]),
    ).toEqual({ ok: { status: 204, headers: [], body: '', keepAlive: true } });
    expect(
      bodyless('304 Not Modified', 'Transfer-Encoding: chunked\n', [
        'off=0 len=4 span[protocol]="HTTP"',
        'off=27 len=17 span[header_field]="Transfer-Encoding"',
        'off=46 len=7 span[header_value]="chunked"',
        'off=55 header_value complete',
        'off=57 headers complete status=304 v=1/1 flags=208 content_length=0',
        'off=57 message complete',
      ]),
    ).toEqual({ ok: { status: 304, headers: [['Transfer-Encoding', 'chunked']], body: '', keepAlive: false } });
    expect(
      bodyless('304 Not Modified', 'Content-Length: 10\n', [
        'off=0 len=4 span[protocol]="HTTP"',
        'off=27 len=14 span[header_field]="Content-Length"',
        'off=43 len=2 span[header_value]="10"',
        'off=47 header_value complete',
        'off=49 headers complete status=304 v=1/1 flags=20 content_length=10',
        'off=49 message complete',
      ]),
    ).toEqual({ ok: { status: 304, headers: [['Content-Length', '10']], body: '', keepAlive: false } });
    expect(short.file.cases[0]?.expect.native).toMatchObject({ ok: { keepAlive: true } });
  });

  /** A one-field interim head (`status`), the way llhttp logs it, followed by a final head or llhttp's upgrade pause. */
  const interimWithField = (status: string, name: string, value: string, after: string[]): string => {
    const statusLine = `HTTP/1.1 ${status}\r\n`;
    const nameOff = statusLine.length;
    const valueOff = nameOff + name.length + 2;
    const end = valueOff + value.length + 2;
    const code = status.slice(0, 3);
    return mdCase(
      `${code} ${name}`,
      RESPONSE,
      `HTTP/1.1 ${status}\n${name}: ${value}\n\n`,
      [
        'off=0 message begin',
        'off=0 len=4 span[protocol]="HTTP"',
        `off=${String(nameOff)} len=${String(name.length)} span[header_field]="${name}"`,
        `off=${String(valueOff)} len=${String(value.length)} span[header_value]="${value}"`,
        `off=${String(end - 2)} header_value complete`,
        `off=${String(end)} headers complete status=${code} v=1/1 flags=0 content_length=0`,
        `off=${String(end)} message complete`,
        ...after.map((line) => line.replace('END', String(end))),
      ].join('\n'),
    );
  };

  it.each([
    ['Content-Length', '0'],
    ['Transfer-Encoding', 'chunked'],
    ['Connection', 'close'],
  ])('refuses an interim head with %s (§5.2 Interim heads)', (name, value) => {
    expect(() => runOne(interimWithField('103 Early Hints', name, value, []))).toThrow(
      /cannot model framing or Connection in a 1xx head/,
    );
  });

  it('does not refuse a 101 with Connection: upgrade (not interim; the upgrade pause decides)', () => {
    const upgrade = runOne(
      interimWithField('101 Switching Protocols', 'Connection', 'upgrade', [
        'off=END error code=8 reason="Pause on CONNECT/Upgrade"',
      ]),
    );
    expect(upgrade.file.cases[0]?.expect.native).toEqual(MALFORMED);
  });

  it.each([
    ['HTTP/1.0 without keep-alive', '1.0', [['Content-Length', '2']]],
    [
      'a close token in a list',
      '1.1',
      [
        ['Connection', 'close, keep-alive'],
        ['Content-Length', '2'],
      ],
    ],
  ] as const)('keeps no connection alive for %s', (_, version, fields) => {
    const lines = fields.map(([n, v]) => `${n}: ${v}\r\n`);
    const log = ['off=0 message begin', 'off=0 len=4 span[protocol]="HTTP"'];
    let off = 17;
    for (const [i, [name, value]] of fields.entries()) {
      log.push(`off=${String(off)} len=${String(name.length)} span[header_field]="${name}"`);
      log.push(`off=${String(off + name.length + 2)} len=${String(value.length)} span[header_value]="${value}"`);
      off += String(lines[i]).length;
      log.push(`off=${String(off)} header_value complete`);
    }
    const head = off + 2;
    log.push(`off=${String(head)} headers complete status=200 v=${version.replace('.', '/')} flags=20`);
    log.push(`off=${String(head)} len=2 span[body]="ok"`, `off=${String(head + 2)} message complete`);
    const http = `HTTP/${version} 200 OK\n${fields.map(([n, v]) => `${n}: ${v}\n`).join('')}\nok`;
    expect(runOne(mdCase('Close', RESPONSE, http, log.join('\n'))).file.cases[0]?.expect.native).toEqual({
      ok: { status: 200, headers: fields, body: 'ok', keepAlive: false },
    });
  });

  it('expects incomplete-body when EOF cuts a Content-Length body', () => {
    const cut = runOne(
      mdCase(
        'Cut',
        RESPONSE,
        'HTTP/1.1 200 OK\nContent-Length: 9\n\nab',
        [
          'off=0 message begin',
          'off=0 len=4 span[protocol]="HTTP"',
          'off=17 len=14 span[header_field]="Content-Length"',
          'off=33 len=1 span[header_value]="9"',
          'off=36 header_value complete',
          'off=38 headers complete status=200 v=1/1 flags=20 content_length=9',
          'off=38 len=2 span[body]="ab"',
        ].join('\n'),
      ),
    );
    expect(cut.file.cases[0]?.expect.native).toEqual({ error: 'PROTOCOL/incomplete-body' });
  });

  it.each([
    ['bytes before the status line', '\\r\\nHTTP/1.1 200 OK\n\n', 'off=2 message begin'],
    ['no final response head', 'HTTP/1.1 200 OK', 'off=0 message begin'],
    [
      'version 0.9',
      'HTTP/0.9 200 OK\n\n',
      'off=0 message begin\noff=0 len=4 span[protocol]="HTTP"\noff=19 headers complete status=200 v=0/9 flags=0',
    ],
    [
      'status 600',
      'HTTP/1.1 600 X\n\n',
      'off=0 message begin\noff=0 len=4 span[protocol]="HTTP"\noff=18 headers complete status=600 v=1/1 flags=0',
    ],
    [
      'Content-Length "x"',
      'HTTP/1.1 200 OK\nContent-Length: x\n\n',
      'off=0 message begin\noff=0 len=4 span[protocol]="HTTP"\noff=17 len=14 span[header_field]="Content-Length"\noff=33 len=1 span[header_value]="x"\noff=36 header_value complete\noff=38 headers complete status=200 v=1/1 flags=0',
    ],
    [
      'Transfer-Encoding "gzip"',
      'HTTP/1.1 200 OK\nTransfer-Encoding: gzip\n\n',
      'off=0 message begin\noff=0 len=4 span[protocol]="HTTP"\noff=17 len=17 span[header_field]="Transfer-Encoding"\noff=36 len=4 span[header_value]="gzip"\noff=42 header_value complete\noff=44 headers complete status=200 v=1/1 flags=200',
    ],
    [
      'line break in a header value',
      'HTTP/1.1 200 OK\nA: b\n c\n\n',
      'off=0 message begin\noff=0 len=4 span[protocol]="HTTP"\noff=17 len=1 span[header_field]="A"\noff=20 len=1 span[header_value]="b"\noff=21 len=1 span[header_value]=cr\noff=29 header_value complete\noff=31 headers complete status=200 v=1/1 flags=0',
    ],
    [
      'an unknown llhttp error code',
      'HTTP/1.1 200 OK\n\n',
      'off=0 message begin\noff=0 len=4 span[protocol]="HTTP"\noff=0 error code=99 reason="?"',
    ],
    ['an unparsed log line', 'HTTP/1.1 200 OK\n\n', 'something else'],
    ['a log event outside a message', 'HTTP/1.1 200 OK\n\n', 'off=0 len=4 span[protocol]="HTTP"'],
    [
      'input that ends after an interim response',
      'HTTP/1.1 100 Continue\n\n',
      'off=0 message begin\noff=0 len=4 span[protocol]="HTTP"\noff=25 headers complete status=100 v=1/1 flags=0\noff=25 message complete',
    ],
    [
      'Content-Length "1, 2"',
      'HTTP/1.1 304 X\nContent-Length: 1\nContent-Length: 2\n\n',
      'off=0 message begin\noff=0 len=4 span[protocol]="HTTP"\noff=16 len=14 span[header_field]="Content-Length"\noff=32 len=1 span[header_value]="1"\noff=35 header_value complete\noff=35 len=14 span[header_field]="Content-Length"\noff=51 len=1 span[header_value]="2"\noff=54 header_value complete\noff=56 headers complete status=304 v=1/1 flags=20',
    ],
    [
      'Content-Length "18446744073709551616"',
      'HTTP/1.1 200 OK\nContent-Length: 18446744073709551616\n\n',
      'off=0 message begin\noff=0 len=4 span[protocol]="HTTP"\noff=17 len=14 span[header_field]="Content-Length"\noff=33 len=20 span[header_value]="18446744073709551616"\noff=55 header_value complete\noff=57 headers complete status=200 v=1/1 flags=20',
    ],
    [
      'Transfer-Encoding "chunked"',
      'HTTP/1.1 200 OK\nTransfer-Encoding: chunked\nContent-Length: 1\n\n',
      'off=0 message begin\noff=0 len=4 span[protocol]="HTTP"\noff=17 len=17 span[header_field]="Transfer-Encoding"\noff=36 len=7 span[header_value]="chunked"\noff=45 header_value complete\noff=45 len=14 span[header_field]="Content-Length"\noff=61 len=1 span[header_value]="1"\noff=64 header_value complete\noff=66 headers complete status=200 v=1/1 flags=228',
    ],
    [
      'Transfer-Encoding "chunked"',
      'HTTP/1.0 200 OK\nTransfer-Encoding: chunked\n\n',
      'off=0 message begin\noff=0 len=4 span[protocol]="HTTP"\noff=17 len=17 span[header_field]="Transfer-Encoding"\noff=36 len=7 span[header_value]="chunked"\noff=45 header_value complete\noff=47 headers complete status=200 v=1/0 flags=208',
    ],
  ])('refuses to model %s', (reason, http, log) => {
    expect(() => run(mdFile(mdCase('Odd', RESPONSE, http, log)))).toThrow(new RegExp(`fixture / Odd: .*${reason}`));
  });

  it('rejects malformed markdown tests', () => {
    expect(() => run(mdFile(mdCase('Same', LENIENT, 'x', 'y'), mdCase('Same', LENIENT, 'x', 'y')))).toThrow(
      /duplicate case/,
    );
    expect(() => run(mdFile(`## No meta\n\n${FENCE}http\nx\n${FENCE}\n\n${FENCE}log\ny\n${FENCE}\n`))).toThrow(/meta/);
    expect(() => run(mdFile(`## No log\n\n<!-- meta=${RESPONSE} -->\n${FENCE}http\nx\n${FENCE}\n`))).toThrow(
      /http and one log/,
    );
    expect(() => run(mdFile(`## Open\n\n${FENCE}http\nx\n`))).toThrow(/unclosed/);
    expect(() => run(mdFile(mdCase('Null meta', 'null', 'x', 'y')))).toThrow(
      /fixture \/ Null meta: meta: expected an object/,
    );
    expect(() => run(mdFile(`## Only log\n\n${FENCE}log\ny\n${FENCE}\n`))).toThrow(/http and one log/);
    expect(run(`${FENCE}http\nx\n${FENCE}\n# Title\n`)[0]?.file.cases).toEqual([]);
  });
});

describe('normaliseInput', () => {
  it('reproduces the llhttp md-test escapes in order', () => {
    expect(normaliseInput('A\\\nB\nC\\r\\n\\t\\f\\x41\\101\n')).toBe('AB\r\nC\r\n\t\fAA');
    expect(normaliseInput('a\r\nb\rc\n')).toBe('a\r\nb\r\nc');
  });
});

describe('parseErrorNames', () => {
  it('reads the ERROR table and rejects a missing one', () => {
    expect(ERROR_NAMES.get(4)).toBe('HPE_UNEXPECTED_CONTENT_LENGTH');
    expect(() => parseErrorNames('const X = {};')).toThrow(/ERROR/);
  });
});

describe('parseOverrides', () => {
  it('validates the override file shape', () => {
    expect(() => parseOverrides([])).toThrow(/object/);
    expect(() => parseOverrides({ x: null })).toThrow(/overrides.x: expected an object/);
    expect(() => parseOverrides({ x: { ...RTSP_OVERRIDE, extra: 1 } })).toThrow(/overrides.x.extra: unknown key/);
    expect(() => parseOverrides({ x: { native: { error: 'NOPE/x' }, rule: '§5.2 x' } })).toThrow(/NOPE/);
  });
});

describe('mitCopyright', () => {
  it('returns the copyright line of an exact MIT licence', () => {
    const mit = `MIT License\n\nCopyright © 2018 Someone\n\n${String(LICENCE_TEXTS['MIT'])}\n`;
    expect(mitCopyright(mit)).toBe('Copyright © 2018 Someone');
    expect(() => mitCopyright(mit.replace('Permission', 'No permission'))).toThrow(/not the MIT licence/);
    expect(() => mitCopyright('Apache License')).toThrow(/not the MIT licence/);
  });
});

describe('tagCommit', () => {
  const tagSha = 'a'.repeat(40);
  const commitSha = 'b'.repeat(40);
  it('prefers the peeled commit of an annotated tag', () => {
    expect(tagCommit(`${tagSha}\trefs/tags/v1\n${commitSha}\trefs/tags/v1^{}\n`, 'v1')).toBe(commitSha);
    expect(tagCommit(`${tagSha}\trefs/tags/v1\n`, 'v1')).toBe(tagSha);
    expect(() => tagCommit('', 'v1')).toThrow(/no commit for tag v1/);
  });
});

describe('diffCases', () => {
  it('reports added, removed and changed case names', () => {
    const [result] = run(FIXTURE, withRtsp);
    if (result === undefined) throw new Error('no result');
    const { file } = result;
    const [simple, duplicate, rtsp] = file.cases;
    if (simple === undefined || duplicate === undefined || rtsp === undefined) throw new Error('missing case');
    const previous = { ...file, cases: [{ ...simple, input: 'x' }, duplicate, { ...rtsp, name: 'gone' }] };
    expect(diffCases(previous, file)).toEqual({
      added: ['fixture / RTSP'],
      removed: ['gone'],
      changed: ['fixture / Simple'],
    });
    expect(diffCases(undefined, file).added).toHaveLength(3);
  });
});
