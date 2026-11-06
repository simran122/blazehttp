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

/** Headings at depth ≥ 2 that own fenced blocks become tests, as in mdgator (which normalises CR and CRLF to LF). */
const parseMarkdown = (md: string): MdTest[] => {
  const tests: MdTest[] = [];
  let current: MdTest | undefined;
  let pendingMeta: unknown;
  let fence: { readonly info: string; readonly line: number; readonly body: string[] } | undefined;
  for (const [i, line] of md.split(/\r\n?|\n/).entries()) {
    if (fence !== undefined) {
      if (!FENCE_CLOSE.test(line)) {
        fence.body.push(`${line}\n`);
        continue;
      }
      current?.blocks.set(fence.info, [...(current.blocks.get(fence.info) ?? []), fence.body.join('')]);
      if (pendingMeta !== undefined) current?.meta.set(fence.info, pendingMeta);
      pendingMeta = undefined;
      fence = undefined;
      continue;
    }
    const heading = SUB_HEADING.exec(line)?.[1];
    if (heading !== undefined || TOP_HEADING.test(line)) {
      current = heading === undefined ? undefined : { name: heading, blocks: new Map(), meta: new Map() };
      if (current !== undefined) tests.push(current);
      pendingMeta = undefined;
      continue;
    }
    const meta = META.exec(line)?.[1];
    if (meta !== undefined) pendingMeta = JSON.parse(meta);
    const info = FENCE_OPEN.exec(line)?.[1];
    if (info !== undefined) fence = { info, line: i + 1, body: [] };
  }
  if (fence !== undefined) throw new Error(`unclosed fence at line ${String(fence.line)}`);
  return tests.filter((t) => t.blocks.size > 0);
};

// ---- llhttp test log ----

type LogEvent =
  | { readonly kind: 'begin'; readonly off: number }
  | { readonly kind: 'span'; readonly off: number; readonly len: number; readonly span: string; readonly text: string }
  | { readonly kind: 'valueComplete' }
  | { readonly kind: 'headers'; readonly status: number; readonly version: string }
  | { readonly kind: 'complete'; readonly off: number }
  | { readonly kind: 'error'; readonly code: number }
  | { readonly kind: 'other' };

const LOG_LINE = /^off=(\d+) (.*)$/;
const SPAN = /^len=(\d+) span\[([\w.]+)\]=("(?:.*)"|lf|cr)$/;
const HEADERS = /^headers complete status=(\d+) v=(\d+)\/(\d+) /;
const ERROR = /^error code=(\d+) reason=/;
const SPAN_CHAR: Readonly<Record<string, string>> = { lf: '\n', cr: '\r' };
const FIXED_EVENTS: Readonly<Record<string, (off: number) => LogEvent>> = {
  'message begin': (off) => ({ kind: 'begin', off }),
  'message complete': (off) => ({ kind: 'complete', off }),
  'header_value complete': () => ({ kind: 'valueComplete' }),
};

const parseLogLine = (line: string, fail: (why: string) => never): LogEvent => {
  const match = LOG_LINE.exec(line) ?? fail('an unparsed log line');
  const off = Number(match[1]);
  const rest = String(match[2]);
  const span = SPAN.exec(rest);
  if (span !== null) {
    const value = String(span[3]);
    return {
      kind: 'span',
      off,
      len: Number(span[1]),
      span: String(span[2]),
      text: SPAN_CHAR[value] ?? value.slice(1, -1),
    };
  }
  const headers = HEADERS.exec(rest);
  if (headers !== null) {
    return { kind: 'headers', status: Number(headers[1]), version: `${String(headers[2])}.${String(headers[3])}` };
  }
  const error = ERROR.exec(rest);
  if (error !== null) return { kind: 'error', code: Number(error[1]) };
  return FIXED_EVENTS[rest]?.(off) ?? { kind: 'other' };
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

/** The response the client delivers once the server closes the connection after `input`. */
const finalOutcome = (msg: Message, method: string, fail: (why: string) => never): Outcome => {
  const { headers, protocol, body } = msg;
  const status = msg.status ?? fail('no final response head');
  if (protocol !== 'HTTP') fail(`protocol ${protocol}`);
  if (msg.version !== '1.0' && msg.version !== '1.1') fail(`version ${String(msg.version)}`);
  if (status > 599) fail(`status ${String(status)}`);
  if (headers.some(([, v]) => /[\r\n]/.test(v))) fail('line break in a header value');
  const values = (name: string): string[] => headers.filter(([n]) => n.toLowerCase() === name).map(([, v]) => v);
  const te = values('transfer-encoding');
  const cl = values('content-length');
  const tokens = values('connection').flatMap((v) => v.split(',').map((t) => t.trim().toLowerCase()));
  const persistent = !tokens.includes('close') && (msg.version === '1.1' || tokens.includes('keep-alive'));
  const noBody = method === 'HEAD' || status === 204 || status === 304;
  // §5.2 Bodies: a 204/304 (not HEAD) announcing a body is accepted without one, but its connection is not reused.
  const unsentBodyStatus = method !== 'HEAD' && (status === 204 || status === 304);
  const ok = (bodyText: string, keepAlive: boolean): Outcome => ({
    native: { ok: { status, headers, body: bodyText, keepAlive } },
  });
  if (te.length > 0) {
    if (te.join() !== 'chunked' || cl.length > 0 || msg.version !== '1.1') fail(`Transfer-Encoding "${te.join(', ')}"`);
    if (noBody) return ok('', persistent && !unsentBodyStatus);
    // removal: P1a (#7)
    return { skip: 'chunked body: chunked decoding and transfer-encoding.md arrive in P1a (#7)' };
  }
  if (cl.length > 0) {
    const [length = ''] = cl;
    if (cl.length > 1 || !/^\d+$/.test(length) || BigInt(length) > U64_MAX) fail(`Content-Length "${cl.join(', ')}"`);
    if (noBody) return ok('', persistent && !(unsentBodyStatus && BigInt(length) > 0n));
    return Buffer.byteLength(body) < Number(length)
      ? { native: { error: 'PROTOCOL/incomplete-body' } }
      : ok(body, persistent);
  }
  return noBody ? ok('', persistent) : ok(body, false);
};

const outcomeOf = (
  events: readonly LogEvent[],
  method: string,
  errorNames: ReadonlyMap<number, string>,
  fail: (why: string) => never,
): Outcome => {
  let msg: Message | undefined;
  let nextBegin = 0;
  const current = (): Message => msg ?? fail('a log event outside a message');
  const spanTarget: Readonly<Record<string, keyof Pick<Message, 'field' | 'value' | 'protocol' | 'body'>>> = {
    header_field: 'field',
    header_value: 'value',
    protocol: 'protocol',
    body: 'body',
  };
  for (const event of events) {
    if (event.kind === 'begin') {
      if (event.off !== nextBegin) fail('bytes before the status line');
      msg = { headers: [], protocol: '', body: '', field: '', value: '' };
    } else if (event.kind === 'span') {
      const target = spanTarget[event.span];
      if (target !== undefined) current()[target] += event.text;
    } else if (event.kind === 'valueComplete') {
      const m = current();
      m.headers.push([m.field, m.value.replace(OWS, '')]);
      m.field = '';
      m.value = '';
    } else if (event.kind === 'headers') {
      current().status = event.status;
      current().version = event.version;
    } else if (event.kind === 'complete') {
      const m = current();
      if (Number(m.status) >= 200) return finalOutcome(m, method, fail);
      // §5.2 Interim heads: framing fields are rejected and `Connection: close` is sticky; not modelled here.
      // A `101` is not interim for us: llhttp's upgrade pause that follows maps to an error.
      if (m.status !== SWITCHING_PROTOCOLS && m.headers.some(([n]) => INTERIM_DECIDED_FIELDS.has(n.toLowerCase()))) {
        fail('framing or Connection in a 1xx head');
      }
      nextBegin = event.off;
      msg = undefined;
    } else if (event.kind === 'error') {
      const name = errorNames.get(event.code) ?? fail('an unknown llhttp error code');
      return { native: nativeError(name) };
    }
  }
  return finalOutcome(msg ?? fail('input that ends after an interim response'), method, fail);
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

const importCase = (
  test: MdTest,
  name: string,
  ctx: ImportContext,
): { readonly case: VectorCase; readonly overridden: boolean } | SkippedCase => {
  const fail = (why: string): never => {
    throw new Error(`${name}: cannot model ${why}; decide the case with an override in ${OVERRIDES_FILE}`);
  };
  const [http, ...moreHttp] = test.blocks.get('http') ?? [];
  const [log, ...moreLogs] = test.blocks.get('log') ?? [];
  if (http === undefined || log === undefined || moreHttp.length > 0 || moreLogs.length > 0) {
    throw new Error(`${name}: expected one http and one log block`);
  }
  if (!test.meta.has('http')) throw new Error(`${name}: missing meta comment before the http block`);
  const meta = asRecord(test.meta.get('http'), `${name}: meta`);
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
