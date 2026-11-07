import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { asObject, asRecord } from '../json-guards.ts';
import { LICENCE_TEXTS } from './notices.ts';
import {
  loadVectorFile,
  type NativeExpect,
  parseNativeExpect,
  parseVectorFile,
  REPO_ROOT,
  type VectorCase,
  type VectorFile,
  VECTORS_DIR,
  type VectorSource,
} from './schema.ts';

/** Upstream cases come from the latest published release; tag and commit are recorded per file (docs/notes/vectors.md). */
const LLHTTP_REPO = 'https://github.com/nodejs/llhttp';
const LLHTTP_LATEST_RELEASE = 'https://api.github.com/repos/nodejs/llhttp/releases/latest';
/** llhttp publishes each release as `release/vX` (generated C sources) and `vX` (source tree with `test/`). */
const BUILD_TAG_PREFIX = /^release\//;
/** P0 response files; `transfer-encoding.md` waits for chunked decoding. */
// removal: P1a (#7)
const STEMS = ['sample', 'content-length', 'connection', 'invalid'] as const;
const OUT_DIR = join(REPO_ROOT, VECTORS_DIR, 'h1-response');
/** Repo-relative, as named in messages; read through `REPO_ROOT`. */
const OVERRIDES_FILE = 'tools/vectors/overrides/llhttp.json';
/** Network calls give up after this long. */
const NETWORK_TIMEOUT_MS = 30_000;

const MALFORMED = 'PROTOCOL/malformed-response';
const SMUGGLING = 'PROTOCOL/smuggling-risk';
/** llhttp errors with a more specific engine error than `PROTOCOL/malformed-response`. */
const ERROR_BY_LLHTTP: Readonly<Record<string, string>> = {
  HPE_UNEXPECTED_CONTENT_LENGTH: SMUGGLING,
  HPE_INVALID_TRANSFER_ENCODING: SMUGGLING,
};
const U64_MAX = 0xffff_ffff_ffff_ffffn;
const CITED_RULE = /^§\d+(?:\.\d+)* \S/;

export interface LlhttpOverride {
  readonly native: NativeExpect;
  /** The design rule that makes blazehttp differ from llhttp, e.g. `§5.2 Status line: …`. */
  readonly rule: string;
}

export interface LlhttpInput {
  readonly stem: string;
  readonly md: string;
  readonly source: VectorSource;
}

export interface ImportContext {
  /** llhttp numeric error code → `HPE_*` name. */
  readonly errorNames: ReadonlyMap<number, string>;
  /** Case name → expectation that replaces the derived one. */
  readonly overrides: ReadonlyMap<string, LlhttpOverride>;
}

export interface SkippedCase {
  readonly name: string;
  readonly reason: string;
}

/** Commit a tag points to, from `git ls-remote <repo> refs/tags/<tag> refs/tags/<tag>^{}` output. */
export function tagCommit(lsRemote: string, tag: string): string {
  const refs = new Map(
    lsRemote
      .split('\n')
      .map((line) => line.split('\t'))
      .map(([sha, ref]) => [String(ref), String(sha)]),
  );
  const sha = refs.get(`refs/tags/${tag}^{}`) ?? refs.get(`refs/tags/${tag}`);
  if (sha === undefined || !/^[0-9a-f]{40}$/.test(sha)) throw new Error(`no commit for tag ${tag}`);
  return sha;
}

export interface CaseDiff {
  readonly added: readonly string[];
  readonly removed: readonly string[];
  readonly changed: readonly string[];
}

/** Case names added, removed or changed (any field) from `previous` to `next`. */
export function diffCases(previous: VectorFile | undefined, next: VectorFile): CaseDiff {
  const before = new Map((previous?.cases ?? []).map((c) => [c.name, JSON.stringify(c)]));
  const after = new Map(next.cases.map((c) => [c.name, JSON.stringify(c)]));
  return {
    added: [...after.keys()].filter((n) => !before.has(n)),
    removed: [...before.keys()].filter((n) => !after.has(n)),
    changed: [...after].filter(([n, c]) => before.has(n) && before.get(n) !== c).map(([n]) => n),
  };
}

export interface ImportResult {
  readonly stem: string;
  readonly file: VectorFile;
  readonly skipped: readonly SkippedCase[];
  readonly overridden: readonly string[];
}

type Header = [string, string];

/** Engine error for an llhttp error (`HPE_*` name). */
export function nativeError(llhttpName: string): NativeExpect {
  return { error: ERROR_BY_LLHTTP[llhttpName] ?? MALFORMED };
}

/** Copyright line of an upstream MIT licence file, after checking its text is exactly the MIT licence. */
export function mitCopyright(licence: string): string {
  const match = /^MIT License\n\n(Copyright [^\n]+)\n\n([\s\S]*?)\n*$/.exec(licence);
  if (match === null || match[2] !== LICENCE_TEXTS['MIT']) throw new Error('upstream licence is not the MIT licence');
  return String(match[1]);
}

/** Input normalisation of llhttp `test/md-test.ts`, step for step (POSIX path; `${…}` is never evaluated). */
export function normaliseInput(raw: string): string {
  return raw
    .replace(/\n$/, '')
    .replace(/\\(\r\n|\r|\n)/g, '')
    .replace(/\r\n|\r|\n/g, '\r\n')
    .replace(/\\r/g, '\r')
    .replace(/\\n/g, '\n')
    .replace(/\\t/g, '\t')
    .replace(/\\f/g, '\f')
    .replace(/\\x([0-9a-fA-F]+)/g, (_, hex: string) => String.fromCharCode(parseInt(hex, 16)))
    .replace(/\\([0-7]{1,3})/g, (_, digits: string) => String.fromCharCode(parseInt(digits, 8)));
}

/** `HPE_*` names by code, read from llhttp `src/llhttp/constants.ts`. */
export function parseErrorNames(constantsTs: string): ReadonlyMap<number, string> {
  const table = /export const ERROR = \{([^}]*)\}/.exec(constantsTs)?.[1];
  if (table === undefined) throw new Error('no ERROR table in llhttp constants');
  return new Map(
    [...table.matchAll(/^\s*([A-Z0-9_]+): (\d+),/gm)].map(([, name, code]) => [Number(code), `HPE_${String(name)}`]),
  );
}

export function parseOverrides(data: unknown): ReadonlyMap<string, LlhttpOverride> {
  return new Map(
    Object.entries(asRecord(data, 'overrides')).map(([name, entry]) => {
      const at = `overrides.${name}`;
      const { native, rule } = asObject(entry, at, ['native', 'rule']);
      if (typeof rule !== 'string' || !CITED_RULE.test(rule)) {
        throw new Error(`${at}.rule: must cite a design section, e.g. "§5.2 …"`);
      }
      return [name, { native: parseNativeExpect(native, `${at}.native`), rule }];
    }),
  );
}

// ---- Markdown (the subset of mdgator that llhttp's test files use) ----

interface MdTest {
  readonly name: string;
  readonly blocks: Map<string, string[]>;
  /** Metadata comment attached to the next fenced block, keyed by block language. */
  readonly meta: Map<string, unknown>;
}

const SUB_HEADING = /^#{2,6}\s+(.*?)\s*#*\s*$/;
/** An ATX `#` heading or a setext `===` underline: a depth-1 heading, which owns no tests. */
const TOP_HEADING = /^#\s|^=+\s*$/;
const META = /<!--\s+meta=(.*)\s+-->/;
const FENCE_OPEN = /^```(\S*)\s*$/;
const FENCE_CLOSE = /^```\s*$/;

interface Fence {
  readonly info: string;
  readonly line: number;
  readonly body: string[];
}

/** Reads llhttp markdown line by line; mdgator semantics, with CR and CRLF already split like LF. */
class MarkdownReader {
  private readonly tests: MdTest[] = [];
  /** The test owning the next block; `undefined` under a depth-1 heading or before any heading. */
  private current: MdTest | undefined;
  /** Meta comment waiting for the next fenced block. */
  private pendingMeta: unknown;
  private fence: Fence | undefined;

  readLine(line: string, lineNumber: number): void {
    if (this.fence === undefined) this.readOuterLine(line, lineNumber);
    else this.readFenceLine(this.fence, line);
  }

  /** Tests that own at least one block; fails on a fence left open. */
  finish(): MdTest[] {
    if (this.fence !== undefined) throw new Error(`unclosed fence at line ${String(this.fence.line)}`);
    return this.tests.filter((t) => t.blocks.size > 0);
  }

  /** Body text, or the closing fence that files the block (and its meta) under the current test. */
  private readFenceLine(fence: Fence, line: string): void {
    if (!FENCE_CLOSE.test(line)) {
      fence.body.push(`${line}\n`);
      return;
    }
    const { current } = this;
    current?.blocks.set(fence.info, [...(current.blocks.get(fence.info) ?? []), fence.body.join('')]);
    if (this.pendingMeta !== undefined) current?.meta.set(fence.info, this.pendingMeta);
    this.pendingMeta = undefined;
    this.fence = undefined;
  }

  /** A heading opens (depth ≥ 2) or closes (depth 1) a test; a meta comment or fence opening is noted. */
  private readOuterLine(line: string, lineNumber: number): void {
    const heading = SUB_HEADING.exec(line)?.[1];
    if (heading !== undefined || TOP_HEADING.test(line)) {
      this.current = heading === undefined ? undefined : { name: heading, blocks: new Map(), meta: new Map() };
      if (this.current !== undefined) this.tests.push(this.current);
      this.pendingMeta = undefined;
      return;
    }
    const meta = META.exec(line)?.[1];
    if (meta !== undefined) this.pendingMeta = JSON.parse(meta);
    const info = FENCE_OPEN.exec(line)?.[1];
    if (info !== undefined) this.fence = { info, line: lineNumber, body: [] };
  }
}

/** Headings at depth ≥ 2 that own fenced blocks become tests, as in mdgator (which normalises CR and CRLF to LF). */
const parseMarkdown = (md: string): MdTest[] => {
  const reader = new MarkdownReader();
  for (const [i, line] of md.split(/\r\n?|\n/).entries()) reader.readLine(line, i + 1);
  return reader.finish();
};

// ---- llhttp test log ----

/** Fields of each log event, by kind. */
interface LogEventFields {
  readonly begin: { readonly off: number };
  readonly span: { readonly off: number; readonly len: number; readonly span: string; readonly text: string };
  readonly valueComplete: unknown;
  readonly headers: { readonly status: number; readonly version: string };
  readonly complete: { readonly off: number };
  readonly error: { readonly code: number };
  readonly other: unknown;
}

type LogEventKind = keyof LogEventFields;

/** One event of kind `K` (the union of all kinds by default); the map form lets a handler table be indexed by kind. */
type LogEvent<K extends LogEventKind = LogEventKind> = {
  readonly [P in K]: { readonly kind: P } & LogEventFields[P];
}[K];

const LOG_LINE = /^off=(\d+) (.*)$/;
const SPAN = /^len=(\d+) span\[([\w.]+)\]=("(?:.*)"|lf|cr)$/;
const HEADERS = /^headers complete status=(\d+) v=(\d+)\/(\d+) /;
const ERROR = /^error code=(\d+) reason=/;
const SPAN_CHAR: ReadonlyMap<string, string> = new Map([
  ['lf', '\n'],
  ['cr', '\r'],
]);
const FIXED_EVENTS: ReadonlyMap<string, (off: number) => LogEvent> = new Map<string, (off: number) => LogEvent>([
  ['message begin', (off) => ({ kind: 'begin', off })],
  ['message complete', (off) => ({ kind: 'complete', off })],
  ['header_value complete', () => ({ kind: 'valueComplete' })],
]);
/** llhttp callbacks the model does not need. Any other event fails, so a new callback is never silently ignored. */
const IGNORED_EVENTS: ReadonlySet<string> = new Set([
  'protocol complete',
  'version complete',
  'status complete',
  'header_field complete',
  'chunk complete',
  'reset',
  'skip body',
]);
const CHUNK_HEADER = /^chunk header len=\d+$/;

const spanEvent = (off: number, rest: string): LogEvent<'span'> | undefined => {
  const span = SPAN.exec(rest);
  if (span === null) return undefined;
  const value = String(span[3]);
  return {
    kind: 'span',
    off,
    len: Number(span[1]),
    span: String(span[2]),
    text: SPAN_CHAR.get(value) ?? value.slice(1, -1),
  };
};

/** A `headers complete` or `error` event, which carry values. */
const valueEvent = (rest: string): LogEvent<'headers' | 'error'> | undefined => {
  const headers = HEADERS.exec(rest);
  if (headers !== null) {
    return { kind: 'headers', status: Number(headers[1]), version: `${String(headers[2])}.${String(headers[3])}` };
  }
  const error = ERROR.exec(rest);
  return error === null ? undefined : { kind: 'error', code: Number(error[1]) };
};

/** A fixed-text event: one the model uses, one it ignores, or a failure naming the unknown event. */
const namedEvent = (off: number, rest: string, fail: (why: string) => never): LogEvent => {
  const fixed = FIXED_EVENTS.get(rest);
  if (fixed !== undefined) return fixed(off);
  if (IGNORED_EVENTS.has(rest) || CHUNK_HEADER.test(rest)) return { kind: 'other' };
  return fail(`an unknown log event "${rest}"`);
};

const parseLogLine = (line: string, fail: (why: string) => never): LogEvent => {
  const match = LOG_LINE.exec(line) ?? fail('an unparsed log line');
  const off = Number(match[1]);
  const rest = String(match[2]);
  return spanEvent(off, rest) ?? valueEvent(rest) ?? namedEvent(off, rest, fail);
};

/** Log lines with md-test's expected-output escapes applied (`\t`, `\f`). */
const logLines = (log: string): string[] => log.replace(/\\t/g, '\t').replace(/\\f/g, '\f').split('\n').slice(0, -1);

// ---- Expectation from the llhttp events (vector semantics: docs/notes/vectors.md) ----

type Outcome = { readonly native: NativeExpect } | { readonly skip: string };

interface Message {
  readonly headers: Header[];
  protocol: string;
  body: string;
  status?: number;
  version?: string;
  field: string;
  value: string;
}

const OWS = /^[ \t]+|[ \t]+$/g;
/** Fields whose presence in an interim head changes the native outcome (§5.2 Interim heads); an override decides. */
const INTERIM_DECIDED_FIELDS: ReadonlySet<string> = new Set(['content-length', 'transfer-encoding', 'connection']);
const SWITCHING_PROTOCOLS = 101;

/** A final response head the importer can model, with what its framing rules need. */
interface FinalResponse {
  readonly status: number;
  readonly version: string;
  readonly headers: Header[];
  readonly body: string;
  /** The connection may be reused (§5.2 keep-alive rules). */
  readonly persistent: boolean;
  /** HEAD, 204 or 304: no body follows the head. */
  readonly noBody: boolean;
  /** §5.2 Bodies: a 204/304 (not HEAD) announcing a body is accepted without one, but its connection is not reused. */
  readonly unsentBodyStatus: boolean;
}

const headerValues = (headers: readonly Header[], name: string): string[] =>
  headers.filter(([n]) => n.toLowerCase() === name).map(([, v]) => v);

/** HTTP/1.1 persists unless `Connection: close`; HTTP/1.0 only with `Connection: keep-alive`. */
const persists = (headers: readonly Header[], version: string): boolean => {
  const tokens = headerValues(headers, 'connection').flatMap((v) => v.split(',').map((t) => t.trim().toLowerCase()));
  return !tokens.includes('close') && (version === '1.1' || tokens.includes('keep-alive'));
};

/** Status and version of a final head inside the modelled grammar; anything else fails. */
const modelledHead = (msg: Message, fail: (why: string) => never): { status: number; version: string } => {
  const status = msg.status ?? fail('no final response head');
  const { version } = msg;
  if (msg.protocol !== 'HTTP') fail(`protocol ${msg.protocol}`);
  if (version !== '1.0' && version !== '1.1') fail(`version ${String(version)}`);
  if (status > 599) fail(`status ${String(status)}`);
  if (msg.headers.some(([, v]) => /[\r\n]/.test(v))) fail('line break in a header value');
  return { status, version };
};

const finalResponse = (msg: Message, method: string, fail: (why: string) => never): FinalResponse => {
  const { status, version } = modelledHead(msg, fail);
  const bodylessStatus = status === 204 || status === 304;
  return {
    status,
    version,
    headers: msg.headers,
    body: msg.body,
    persistent: persists(msg.headers, version),
    noBody: method === 'HEAD' || bodylessStatus,
    unsentBodyStatus: method !== 'HEAD' && bodylessStatus,
  };
};

const delivered = (r: FinalResponse, body: string, keepAlive: boolean): Outcome => ({
  native: { ok: { status: r.status, headers: r.headers, body, keepAlive } },
});

const chunkedOutcome = (
  r: FinalResponse,
  te: readonly string[],
  hasContentLength: boolean,
  fail: (why: string) => never,
): Outcome => {
  if (te.join() !== 'chunked' || hasContentLength || r.version !== '1.1') fail(`Transfer-Encoding "${te.join(', ')}"`);
  if (r.noBody) return delivered(r, '', r.persistent && !r.unsentBodyStatus);
  // removal: P1a (#7)
  return { skip: 'chunked body: chunked decoding and transfer-encoding.md arrive in P1a (#7)' };
};

const contentLengthOutcome = (r: FinalResponse, cl: readonly string[], fail: (why: string) => never): Outcome => {
  const [length = ''] = cl;
  if (cl.length > 1 || !/^\d+$/.test(length) || BigInt(length) > U64_MAX) fail(`Content-Length "${cl.join(', ')}"`);
  if (r.noBody) return delivered(r, '', r.persistent && !(r.unsentBodyStatus && BigInt(length) > 0n));
  return Buffer.byteLength(r.body) < Number(length)
    ? { native: { error: 'PROTOCOL/incomplete-body' } }
    : delivered(r, r.body, r.persistent);
};

/** The response the client delivers once the server closes the connection after `input`. */
const finalOutcome = (msg: Message, method: string, fail: (why: string) => never): Outcome => {
  const r = finalResponse(msg, method, fail);
  const te = headerValues(r.headers, 'transfer-encoding');
  const cl = headerValues(r.headers, 'content-length');
  if (te.length > 0) return chunkedOutcome(r, te, cl.length > 0, fail);
  if (cl.length > 0) return contentLengthOutcome(r, cl, fail);
  return r.noBody ? delivered(r, '', r.persistent) : delivered(r, r.body, false);
};

type SpanTarget = keyof Pick<Message, 'field' | 'value' | 'protocol' | 'body'>;
const SPAN_TARGET: ReadonlyMap<string, SpanTarget> = new Map<string, SpanTarget>([
  ['header_field', 'field'],
  ['header_value', 'value'],
  ['protocol', 'protocol'],
  ['body', 'body'],
]);

type EventHandlers = {
  readonly [K in LogEventKind]: (replay: LogReplay, event: LogEvent<K>) => Outcome | undefined;
};

/** Replays llhttp log events: interim messages are checked and dropped; the final message or an error decides. */
class LogReplay {
  /** Handler per event kind; each returns the outcome once an event decides it, else `undefined`. */
  private static readonly HANDLERS: EventHandlers = {
    begin: (replay, event) => replay.begin(event),
    span: (replay, event) => replay.span(event),
    valueComplete: (replay) => replay.valueComplete(),
    headers: (replay, event) => replay.headers(event),
    complete: (replay, event) => replay.complete(event),
    error: (replay, event) => replay.error(event),
    other: () => undefined,
  };

  private msg: Message | undefined;
  private nextBegin = 0;
  private readonly method: string;
  private readonly errorNames: ReadonlyMap<number, string>;
  private readonly fail: (why: string) => never;

  constructor(method: string, errorNames: ReadonlyMap<number, string>, fail: (why: string) => never) {
    this.method = method;
    this.errorNames = errorNames;
    this.fail = fail;
  }

  /** The outcome once an event decides it, else `undefined`. */
  apply<K extends LogEventKind>(event: LogEvent<K>): Outcome | undefined {
    return LogReplay.HANDLERS[event.kind](this, event);
  }

  /** The log ended: the open message is the final response. */
  end(): Outcome {
    return finalOutcome(this.msg ?? this.fail('input that ends after an interim response'), this.method, this.fail);
  }

  private begin(event: LogEvent<'begin'>): Outcome | undefined {
    if (event.off !== this.nextBegin) this.fail('bytes before the status line');
    this.msg = { headers: [], protocol: '', body: '', field: '', value: '' };
    return undefined;
  }

  private span(event: LogEvent<'span'>): Outcome | undefined {
    const target = SPAN_TARGET.get(event.span);
    if (target !== undefined) this.current()[target] += event.text;
    return undefined;
  }

  private valueComplete(): Outcome | undefined {
    const m = this.current();
    m.headers.push([m.field, m.value.replace(OWS, '')]);
    m.field = '';
    m.value = '';
    return undefined;
  }

  private headers(event: LogEvent<'headers'>): Outcome | undefined {
    this.current().status = event.status;
    this.current().version = event.version;
    return undefined;
  }

  private complete(event: LogEvent<'complete'>): Outcome | undefined {
    const m = this.current();
    if (Number(m.status) >= 200) return finalOutcome(m, this.method, this.fail);
    // §5.2 Interim heads: framing fields are rejected and `Connection: close` is sticky; not modelled here.
    // A `101` is not interim for us: llhttp's upgrade pause that follows maps to an error.
    if (m.status !== SWITCHING_PROTOCOLS && m.headers.some(([n]) => INTERIM_DECIDED_FIELDS.has(n.toLowerCase()))) {
      this.fail('framing or Connection in a 1xx head');
    }
    this.nextBegin = event.off;
    this.msg = undefined;
    return undefined;
  }

  private error(event: LogEvent<'error'>): Outcome {
    const name = this.errorNames.get(event.code) ?? this.fail('an unknown llhttp error code');
    return { native: nativeError(name) };
  }

  private current(): Message {
    return this.msg ?? this.fail('a log event outside a message');
  }
}

const outcomeOf = (
  events: readonly LogEvent[],
  method: string,
  errorNames: ReadonlyMap<number, string>,
  fail: (why: string) => never,
): Outcome => {
  const replay = new LogReplay(method, errorNames, fail);
  for (const event of events) {
    const outcome = replay.apply(event);
    if (outcome !== undefined) return outcome;
  }
  return replay.end();
};

/** Every span in the log must equal the normalised input at its offset: proof the normalisation matches. */
const checkSpans = (events: readonly LogEvent[], input: Buffer, fail: (why: string) => never): void => {
  for (const e of events) {
    if (e.kind !== 'span') continue;
    const text = Buffer.from(e.text);
    if (text.length !== e.len || !input.subarray(e.off, e.off + e.len).equals(text)) {
      fail(`log span at off=${String(e.off)} that differs from the normalised input`);
    }
  }
};

const INLINE_JS = /\$\{(.+?)\}/;

/** The single `http` block and single `log` block a test must have. */
const testBlocks = (test: MdTest, name: string): { readonly http: string; readonly log: string } => {
  const [http, ...moreHttp] = test.blocks.get('http') ?? [];
  const [log, ...moreLogs] = test.blocks.get('log') ?? [];
  if (http === undefined || log === undefined || moreHttp.length > 0 || moreLogs.length > 0) {
    throw new Error(`${name}: expected one http and one log block`);
  }
  return { http, log };
};

/** The meta comment before the `http` block. */
const httpMeta = (test: MdTest, name: string): Record<string, unknown> => {
  if (!test.meta.has('http')) throw new Error(`${name}: missing meta comment before the http block`);
  return asRecord(test.meta.get('http'), `${name}: meta`);
};

const importCase = (
  test: MdTest,
  name: string,
  ctx: ImportContext,
): { readonly case: VectorCase; readonly overridden: boolean } | SkippedCase => {
  const fail = (why: string): never => {
    throw new Error(`${name}: cannot model ${why}; decide the case with an override in ${OVERRIDES_FILE}`);
  };
  const { http, log } = testBlocks(test, name);
  const meta = httpMeta(test, name);
  if (meta['type'] !== 'response')
    return { name, reason: `llhttp ${String(meta['type'])} mode; blazehttp has no lenient modes (§5.2)` };
  const input = normaliseInput(http);
  if (INLINE_JS.test(input)) return { name, reason: 'inline JavaScript in the input; the importer never evaluates it' };
  const method = meta['skipBody'] === true ? 'HEAD' : 'GET';
  // An override decides the case outright: llhttp's log is neither modelled nor span-checked for it.
  const override = ctx.overrides.get(name);
  const outcome =
    override ??
    ((): Outcome => {
      const events = logLines(log).map((line) => parseLogLine(line, fail));
      checkSpans(events, Buffer.from(input), fail);
      return outcomeOf(events, method, ctx.errorNames, fail);
    })();
  if ('skip' in outcome) return { name, reason: outcome.skip };
  return {
    overridden: override !== undefined,
    case: {
      name,
      input,
      inputEncoding: 'utf8',
      request: { method },
      expect: { native: outcome.native },
      reference: { llhttp: log.replace(/\n$/, '') },
    },
  };
};

/** Converts llhttp markdown test files into vector files; every override must be used. */
export function importLlhttp(files: readonly LlhttpInput[], ctx: ImportContext): ImportResult[] {
  const used = new Set<string>();
  const results = files.map(({ stem, md, source }) => {
    const cases: VectorCase[] = [];
    const skipped: SkippedCase[] = [];
    const names = new Set<string>();
    for (const test of parseMarkdown(md)) {
      const name = `${stem} / ${test.name}`;
      if (names.has(name)) throw new Error(`${name}: duplicate case name`);
      names.add(name);
      const result = importCase(test, name, ctx);
      if (!('case' in result)) {
        skipped.push(result);
        continue;
      }
      cases.push(result.case);
      if (result.overridden) used.add(name);
    }
    return { stem, file: { source, cases }, skipped, overridden: cases.map((c) => c.name).filter((n) => used.has(n)) };
  });
  const unused = [...ctx.overrides.keys()].filter((n) => !used.has(n));
  if (unused.length > 0) throw new Error(`unused override ${unused.join(', ')}`);
  return results;
}

/* v8 ignore start -- network CLI, run by hand (docs/notes/vectors.md) */
if (import.meta.main) {
  const refresh = process.argv.includes('--refresh');
  const paths = STEMS.map((stem) => join(OUT_DIR, `llhttp-${stem}.json`));
  const previous = paths.map((path) => (existsSync(path) ? loadVectorFile(path) : undefined));
  const recorded = new Set(
    previous.flatMap((file) => (file?.source.tag === undefined ? [] : [`${file.source.tag} ${file.source.commit}`])),
  );
  if (recorded.size > 1) throw new Error(`llhttp vector files record different releases: ${[...recorded].join(', ')}`);
  const [pinned] = recorded;
  const signal = (): AbortSignal => AbortSignal.timeout(NETWORK_TIMEOUT_MS);
  // `--refresh` moves to the latest published release; otherwise re-import the recorded tag (e.g. after an override change).
  const latestTag = async (): Promise<string> => {
    const response = await fetch(LLHTTP_LATEST_RELEASE, {
      headers: { accept: 'application/vnd.github+json' },
      signal: signal(),
    });
    if (!response.ok) throw new Error(`${LLHTTP_LATEST_RELEASE}: HTTP ${String(response.status)}`);
    const { tag_name: tagName } = (await response.json()) as { tag_name: unknown };
    return String(tagName).replace(BUILD_TAG_PREFIX, '');
  };
  const tag = refresh || pinned === undefined ? await latestTag() : String(pinned.split(' ')[0]);
  const lsRemote = execFileSync('git', ['ls-remote', `${LLHTTP_REPO}.git`, `refs/tags/${tag}`, `refs/tags/${tag}^{}`], {
    encoding: 'utf8',
    timeout: NETWORK_TIMEOUT_MS,
  });
  const commit = tagCommit(lsRemote, tag);
  if (!refresh && pinned !== undefined && pinned !== `${tag} ${commit}`) {
    throw new Error(`tag ${tag} now points to ${commit}, not the recorded ${pinned}; review it with --refresh`);
  }
  const base = `https://raw.githubusercontent.com/nodejs/llhttp/${commit}/`;
  const fetchText = async (path: string): Promise<string> => {
    const response = await fetch(base + path, { signal: signal() });
    if (!response.ok) throw new Error(`${base}${path}: HTTP ${String(response.status)}`);
    return response.text();
  };
  const copyright = mitCopyright(await fetchText('LICENSE'));
  const errorNames = parseErrorNames(await fetchText('src/llhttp/constants.ts'));
  const overrides = parseOverrides(JSON.parse(readFileSync(join(REPO_ROOT, OVERRIDES_FILE), 'utf8')));
  const inputs = await Promise.all(
    STEMS.map(async (stem) => ({
      stem,
      md: await fetchText(`test/response/${stem}.md`),
      source: {
        project: 'llhttp',
        url: `${LLHTTP_REPO}/blob/${commit}/test/response/${stem}.md`,
        tag,
        commit,
        licence: 'MIT',
        copyright,
      },
    })),
  );
  // Build and validate every file before writing any, so a failure never leaves a mixed import behind.
  const outputs = importLlhttp(inputs, { errorNames, overrides }).map((result, i) => {
    const path = String(paths[i]);
    return { ...result, path, text: `${JSON.stringify(parseVectorFile(result.file, path), null, 2)}\n` };
  });
  mkdirSync(OUT_DIR, { recursive: true });
  for (const [i, { path, text, file, skipped, overridden }] of outputs.entries()) {
    writeFileSync(path, text);
    console.log(
      `${path} (llhttp ${tag} ${commit}): ${String(file.cases.length)} cases, ${String(overridden.length)} overridden`,
    );
    const diff = diffCases(previous[i], file);
    for (const what of ['added', 'removed', 'changed'] as const) {
      for (const name of diff[what]) console.log(`  ${what} ${name}`);
    }
    for (const s of skipped) console.log(`  skipped ${s.name}: ${s.reason}`);
  }
}
/* v8 ignore stop */
