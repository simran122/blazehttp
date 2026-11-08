import { readdirSync, readFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { ENGINE_ERRORS, METHODS } from '../../packages/blazehttp/src/generated/spec.ts';
import { asArray, asObject, asRecord, asString, fail } from '../json-guards.ts';

/** Repository root, so the tools work from any working directory. */
export const REPO_ROOT = join(import.meta.dirname, '..', '..');
/** Vector files live under this repo-relative directory, registered in its `index.json` (docs/notes/vectors.md). */
export const VECTORS_DIR = 'tests/vectors';
const INDEX_FILE = 'index.json';

export interface VectorSource {
  readonly project: string;
  readonly url: string;
  /** Upstream release tag the cases were imported from; `commit` is the commit it points to. */
  readonly tag?: string;
  readonly commit: string;
  readonly licence: string;
  readonly copyright: string;
}

interface OkExpect {
  readonly status: number;
  /** Final-response fields in wire order: name as sent, value without surrounding OWS. */
  readonly headers: readonly (readonly [string, string])[];
  readonly body: string;
  readonly keepAlive: boolean;
}

/** `error` is the engine's display form `CODE/detail`, e.g. `PROTOCOL/malformed-response`. */
export type NativeExpect = { readonly ok: OkExpect } | { readonly error: string };

/**
 * How the case's byte strings (`input`, expected header names and values, body) become bytes: `utf8` encodes the
 * string as UTF-8; `latin1` maps each code point U+0000–U+00FF to one byte (used only when a byte ≥ 0x80 occurs).
 */
type InputEncoding = 'utf8' | 'latin1';

export interface VectorCase {
  readonly name: string;
  /** Every byte the server sends; the server then closes the connection. */
  readonly input: string;
  readonly inputEncoding: InputEncoding;
  readonly request: { readonly method: string };
  readonly expect: {
    readonly native: NativeExpect;
    /** Optional while only the native engine runs the vectors; required once undici runs them. */
    // removal: P4 (#5)
    readonly undici?: NativeExpect;
  };
  /** What reference implementations did with the input, keyed by project (informational). */
  readonly reference?: Readonly<Record<string, string>>;
  /** Provenance of a case re-derived from an upstream test: `<file>:<test> @ <tag>` (informational). */
  readonly derivedFrom?: string;
}

/** `reuse` as the writer returns it: `Reuse::Allowed` or `Reuse::CloseAfterExchange`. */
const REUSE_VALUES = ['allowed', 'closeAfterExchange'] as const;
type Reuse = (typeof REUSE_VALUES)[number];

/** The writer's whole output (head and body) and its reuse verdict, or its error in display form `CODE/detail`. */
type RequestExpect = { readonly bytes: string; readonly reuse: Reuse } | { readonly error: string };

/** One request the writer serialises (`tests/vectors/h1-request/`, docs/notes/request-writer.md §9.3). */
interface RequestVectorCase {
  readonly name: string;
  /** How every byte string of the case (request fields and expected bytes) becomes bytes, as for responses. */
  readonly inputEncoding: InputEncoding;
  readonly request: {
    readonly method: string;
    readonly target: string;
    readonly templateHost: string;
    readonly templateFields: string;
    /** The field count JS serialised into `templateFields`, not the number of CRLFs present. */
    readonly templateFieldLines: number;
    readonly headers: string;
    /** The field count the JS layer passes with `headers`, not the number of CRLFs present. */
    readonly headerLines: number;
    readonly body: string;
    /** The connection's authority; defaults to `templateHost`. */
    readonly connAuthority?: string;
  };
  readonly expect: RequestExpect;
  readonly derivedFrom?: string;
}

export interface VectorFile<C = VectorCase> {
  readonly source: VectorSource;
  readonly cases: readonly C[];
}

export type RequestVectorFile = VectorFile<RequestVectorCase>;

/** Vector file (relative path) → suites that consume it. */
export type VectorIndex = Readonly<Record<string, readonly string[]>>;

/** Every error string the engine can produce, as `ErrorCode`'s `Display` prints it (generated, wire codes only). */
const ENGINE_ERROR_SET: ReadonlySet<string> = new Set(ENGINE_ERRORS);
const METHOD_SET: ReadonlySet<string> = new Set(METHODS);
const REUSE_SET: ReadonlySet<string> = new Set(REUSE_VALUES);
/** `RequestParts::header_lines` and `HeaderTemplate::new`'s `field_lines` are `u16`s. */
const MAX_LINE_COUNT = 65_535;
/** The request's line counts, in schema order. */
const REQUEST_COUNTS = ['templateFieldLines', 'headerLines'] as const;
/** The request's byte strings, in schema order. */
const REQUEST_TEXTS = ['target', 'templateHost', 'templateFields', 'headers', 'body'] as const;
const MIN_STATUS = 100;
const MAX_STATUS = 599;
/** A code point that is not one latin1 byte. */
const NOT_LATIN1 = /[\u{100}-\u{10ffff}]/u;
const isLatin1 = (text: string): boolean => !NOT_LATIN1.test(text);
/** A UTF-16 surrogate without its pair: not a code point, so no bytes in any encoding. */
const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;
/** A latin1 byte that UTF-8 would encode differently: the reason to choose `latin1`. */
const HIGH_BYTE = /[\x80-\xff]/u;

const parseSource = (value: unknown, at: string): VectorSource => {
  const s = asObject(value, at, ['project', 'url', 'commit', 'licence', 'copyright'], ['tag']);
  return {
    project: asString(s['project'], `${at}.project`),
    url: asString(s['url'], `${at}.url`),
    ...(s['tag'] === undefined ? {} : { tag: asString(s['tag'], `${at}.tag`) }),
    commit: asString(s['commit'], `${at}.commit`),
    licence: asString(s['licence'], `${at}.licence`),
    copyright: asString(s['copyright'], `${at}.copyright`),
  };
};

const parseOk = (value: unknown, at: string): OkExpect => {
  const ok = asObject(value, at, ['status', 'headers', 'body', 'keepAlive']);
  const status = ok['status'];
  if (typeof status !== 'number' || !Number.isInteger(status) || status < MIN_STATUS || status > MAX_STATUS) {
    fail(`${at}.status`, `expected an integer in ${String(MIN_STATUS)}..=${String(MAX_STATUS)}`);
  }
  if (typeof ok['keepAlive'] !== 'boolean') fail(`${at}.keepAlive`, 'expected a boolean');
  const headers = asArray(ok['headers'], `${at}.headers`).map((pair, i): readonly [string, string] => {
    const where = `${at}.headers[${String(i)}]`;
    const [name, value, ...rest] = asArray(pair, where);
    if (rest.length > 0 || value === undefined) fail(where, 'expected [name, value]');
    return [asString(name, where), asString(value, where)];
  });
  return {
    status: status as number,
    headers,
    body: asString(ok['body'], `${at}.body`),
    keepAlive: ok['keepAlive'] as boolean,
  };
};

/** A generated method name. */
const parseMethod = (value: unknown, at: string): string => {
  const method = asString(value, at);
  return METHOD_SET.has(method) ? method : fail(at, `${method} is not a generated Method`);
};

/** An error the engine can produce, in display form. */
const parseEngineError = (value: unknown, at: string): string => {
  const error = asString(value, at);
  return ENGINE_ERROR_SET.has(error) ? error : fail(at, `${error} is not an engine error`);
};

/** The optional informational `derivedFrom`. */
const parseDerivedFrom = (value: unknown, at: string): { readonly derivedFrom?: string } =>
  value === undefined ? {} : { derivedFrom: asString(value, at) };

export const parseNativeExpect = (value: unknown, at: string): NativeExpect => {
  const keys = typeof value === 'object' && value !== null ? Object.keys(value) : [];
  if (keys.length !== 1) fail(at, 'expected exactly one of ok, error');
  if (keys[0] === 'ok') return { ok: parseOk(asObject(value, at, ['ok'])['ok'], `${at}.ok`) };
  return { error: parseEngineError(asObject(value, at, ['error'])['error'], `${at}.error`) };
};

/**
 * `texts` are the case's byte strings as `[path, text]`. Every one must be well-formed UTF-16 (a lone surrogate has no
 * UTF-8 or latin1 bytes, so the Rust loader would reject it); `latin1` needs every one latin1 and one byte ≥ 0x80.
 */
const parseEncoding = (value: unknown, texts: readonly (readonly [string, string])[], at: string): InputEncoding => {
  const broken = texts.find(([, text]) => LONE_SURROGATE.test(text));
  if (broken !== undefined) fail(broken[0], 'a lone surrogate');
  if (value === 'utf8') return value;
  if (value !== 'latin1') return fail(`${at}.inputEncoding`, 'expected utf8 or latin1');
  const wide = texts.find(([, text]) => !isLatin1(text));
  if (wide !== undefined) fail(wide[0], 'a code point above U+00FF in a latin1 case');
  if (!texts.some(([, text]) => HIGH_BYTE.test(text))) {
    fail(`${at}.inputEncoding`, 'latin1 without a byte >= 0x80; use utf8');
  }
  return value;
};

const parseCase = (value: unknown, at: string): VectorCase => {
  const c = asObject(value, at, ['name', 'input', 'inputEncoding', 'request', 'expect'], ['reference', 'derivedFrom']);
  const input = asString(c['input'], `${at}.input`);
  const inputEncoding = parseEncoding(c['inputEncoding'], [[`${at}.input`, input]], at);
  const request = asObject(c['request'], `${at}.request`, ['method']);
  const method = parseMethod(request['method'], `${at}.request.method`);
  const expectJson = asObject(c['expect'], `${at}.expect`, ['native'], ['undici']);
  const reference =
    c['reference'] === undefined
      ? undefined
      : Object.fromEntries(
          Object.entries(asRecord(c['reference'], `${at}.reference`)).map(([k, v]) => [
            k,
            asString(v, `${at}.reference.${k}`),
          ]),
        );
  const native = parseNativeExpect(expectJson['native'], `${at}.expect.native`);
  if (inputEncoding === 'latin1' && 'ok' in native) {
    const { headers, body } = native.ok;
    if (![body, ...headers.flat()].every(isLatin1)) {
      fail(`${at}.expect.native.ok`, 'a code point above U+00FF in a latin1 case');
    }
  }
  return {
    name: asString(c['name'], `${at}.name`),
    input,
    inputEncoding,
    request: { method },
    expect: {
      native,
      ...(expectJson['undici'] === undefined
        ? {}
        : { undici: parseNativeExpect(expectJson['undici'], `${at}.expect.undici`) }),
    },
    ...(reference === undefined ? {} : { reference }),
    ...parseDerivedFrom(c['derivedFrom'], `${at}.derivedFrom`),
  };
};

const parseRequestExpect = (value: unknown, at: string): RequestExpect => {
  if (typeof value === 'object' && value !== null && 'error' in value) {
    return { error: parseEngineError(asObject(value, at, ['error'])['error'], `${at}.error`) };
  }
  const written = asObject(value, at, ['bytes', 'reuse']);
  const reuse = asString(written['reuse'], `${at}.reuse`);
  if (!REUSE_SET.has(reuse)) fail(`${at}.reuse`, 'expected allowed or closeAfterExchange');
  return { bytes: asString(written['bytes'], `${at}.bytes`), reuse: reuse as Reuse };
};

/** A line count: an integer in `0..=65535` (a `u16` in Rust; serde rejects `1.5`, `-1` and `65536`). */
const parseLineCount = (value: unknown, at: string): number => {
  const ok = typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= MAX_LINE_COUNT;
  return ok ? value : fail(at, `expected an integer in 0..=${String(MAX_LINE_COUNT)}`);
};

const parseRequest = (value: unknown, at: string): RequestVectorCase['request'] => {
  const r = asObject(value, at, ['method', ...REQUEST_TEXTS, ...REQUEST_COUNTS], ['connAuthority']);
  const method = parseMethod(r['method'], `${at}.method`);
  const [templateFieldLines, headerLines] = REQUEST_COUNTS.map((k) => parseLineCount(r[k], `${at}.${k}`));
  const [target, templateHost, templateFields, headers, body] = REQUEST_TEXTS.map((k) => asString(r[k], `${at}.${k}`));
  return {
    method,
    target: String(target),
    templateHost: String(templateHost),
    templateFields: String(templateFields),
    templateFieldLines: Number(templateFieldLines),
    headers: String(headers),
    headerLines: Number(headerLines),
    body: String(body),
    ...(r['connAuthority'] === undefined ? {} : { connAuthority: asString(r['connAuthority'], `${at}.connAuthority`) }),
  };
};

const parseRequestCase = (value: unknown, at: string): RequestVectorCase => {
  const c = asObject(value, at, ['name', 'inputEncoding', 'request', 'expect'], ['derivedFrom']);
  const request = parseRequest(c['request'], `${at}.request`);
  const expect = parseRequestExpect(c['expect'], `${at}.expect`);
  const texts: [string, string][] = [
    ...REQUEST_TEXTS.map((k): [string, string] => [`${at}.request.${k}`, request[k]]),
    ...(request.connAuthority === undefined
      ? []
      : [[`${at}.request.connAuthority`, request.connAuthority] as [string, string]]),
    ...('bytes' in expect ? [[`${at}.expect.bytes`, expect.bytes] as [string, string]] : []),
  ];
  return {
    name: asString(c['name'], `${at}.name`),
    inputEncoding: parseEncoding(c['inputEncoding'], texts, at),
    request,
    expect,
    ...parseDerivedFrom(c['derivedFrom'], `${at}.derivedFrom`),
  };
};

/** A vector file whose cases are each parsed by `parseOne`, with unique names; `label` prefixes error messages. */
const parseFile = <C extends { readonly name: string }>(
  data: unknown,
  label: string,
  parseOne: (value: unknown, at: string) => C,
): VectorFile<C> => {
  const file = asObject(data, label, ['source', 'cases']);
  const cases = asArray(file['cases'], `${label}.cases`).map((c, i) => parseOne(c, `${label}.cases[${String(i)}]`));
  if (cases.length === 0) fail(label, 'expected at least one case');
  const names = new Set<string>();
  for (const { name } of cases) {
    if (names.has(name)) fail(label, `duplicate case name ${name}`);
    names.add(name);
  }
  return { source: parseSource(file['source'], `${label}.source`), cases };
};

/** Validates parsed JSON as a response vector file (`h1-response/`); `label` prefixes error messages. */
export function parseVectorFile(data: unknown, label: string): VectorFile {
  return parseFile(data, label, parseCase);
}

/** Validates parsed JSON as a request vector file (`h1-request/`); `label` prefixes error messages. */
export function parseRequestVectorFile(data: unknown, label: string): RequestVectorFile {
  return parseFile(data, label, parseRequestCase);
}

/**
 * The case schema of a vector file, chosen by the name of the directory that holds it (docs/notes/vectors.md: every
 * vector file sits exactly one level below `tests/vectors`, in a directory named after its case kind).
 */
const FILE_PARSERS: Readonly<Record<string, (data: unknown, label: string) => VectorFile | RequestVectorFile>> = {
  'h1-response': parseVectorFile,
  'h1-request': parseRequestVectorFile,
};

/** Loads a response vector file (`h1-response/`), e.g. for the llhttp importer. */
export function loadResponseVectorFile(path: string): VectorFile {
  return parseVectorFile(JSON.parse(readFileSync(path, 'utf8')), path);
}

/** Loads a vector file and validates it with the schema of its directory (`h1-response/` or `h1-request/`). */
export function loadVectorFile(path: string): VectorFile | RequestVectorFile {
  const dir = basename(dirname(path));
  const parse = Object.hasOwn(FILE_PARSERS, dir) ? FILE_PARSERS[dir] : undefined;
  if (parse === undefined) return fail(path, `no case schema for directory ${dir}`);
  return parse(JSON.parse(readFileSync(path, 'utf8')), path);
}

export function parseIndex(data: unknown, label: string): VectorIndex {
  return Object.fromEntries(
    Object.entries(asRecord(data, label)).map(([file, suites]) => [
      file,
      asArray(suites, `${label}.${file}`).map((s) => asString(s, `${label}.${file}`)),
    ]),
  );
}

export function readIndex(root: string): VectorIndex {
  const path = join(root, INDEX_FILE);
  return parseIndex(JSON.parse(readFileSync(path, 'utf8')), path);
}

/** Vector files under `root` as `/`-separated relative paths, sorted; `index.json` excluded. */
export function listVectorFiles(root: string): string[] {
  return readdirSync(root, { recursive: true, encoding: 'utf8' })
    .map((p) => p.replaceAll('\\', '/'))
    .filter((p) => p.endsWith('.json') && p !== INDEX_FILE)
    .sort();
}
