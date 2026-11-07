import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
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

export interface VectorFile {
  readonly source: VectorSource;
  readonly cases: readonly VectorCase[];
}

/** Vector file (relative path) → suites that consume it. */
export type VectorIndex = Readonly<Record<string, readonly string[]>>;

/** Every error string the engine can produce, as `ErrorCode`'s `Display` prints it (generated, wire codes only). */
const ENGINE_ERROR_SET: ReadonlySet<string> = new Set(ENGINE_ERRORS);
const METHOD_SET: ReadonlySet<string> = new Set(METHODS);
const MIN_STATUS = 100;
const MAX_STATUS = 599;
/** A code point that is not one latin1 byte. */
const NOT_LATIN1 = /[\u{100}-\u{10ffff}]/u;
const isLatin1 = (text: string): boolean => !NOT_LATIN1.test(text);
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

export const parseNativeExpect = (value: unknown, at: string): NativeExpect => {
  const keys = typeof value === 'object' && value !== null ? Object.keys(value) : [];
  if (keys.length !== 1) fail(at, 'expected exactly one of ok, error');
  if (keys[0] === 'ok') return { ok: parseOk(asObject(value, at, ['ok'])['ok'], `${at}.ok`) };
  const error = asString(asObject(value, at, ['error'])['error'], `${at}.error`);
  return ENGINE_ERROR_SET.has(error) ? { error } : fail(`${at}.error`, `${error} is not an engine error`);
};

const parseEncoding = (value: unknown, input: string, at: string): InputEncoding => {
  if (value === 'utf8') return value;
  if (value !== 'latin1') return fail(`${at}.inputEncoding`, 'expected utf8 or latin1');
  if (!isLatin1(input)) fail(`${at}.input`, 'a code point above U+00FF in a latin1 case');
  if (!HIGH_BYTE.test(input)) fail(`${at}.inputEncoding`, 'latin1 without a byte >= 0x80; use utf8');
  return value;
};

const parseCase = (value: unknown, at: string): VectorCase => {
  const c = asObject(value, at, ['name', 'input', 'inputEncoding', 'request', 'expect'], ['reference', 'derivedFrom']);
  const input = asString(c['input'], `${at}.input`);
  const inputEncoding = parseEncoding(c['inputEncoding'], input, at);
  const request = asObject(c['request'], `${at}.request`, ['method']);
  const method = asString(request['method'], `${at}.request.method`);
  if (!METHOD_SET.has(method)) fail(`${at}.request.method`, `${method} is not a generated Method`);
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
  const derivedFrom = c['derivedFrom'] === undefined ? undefined : asString(c['derivedFrom'], `${at}.derivedFrom`);
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
    ...(derivedFrom === undefined ? {} : { derivedFrom }),
  };
};

/** Validates parsed JSON as a vector file; `label` prefixes error messages. */
export function parseVectorFile(data: unknown, label: string): VectorFile {
  const file = asObject(data, label, ['source', 'cases']);
  const cases = asArray(file['cases'], `${label}.cases`).map((c, i) => parseCase(c, `${label}.cases[${String(i)}]`));
  if (cases.length === 0) fail(label, 'expected at least one case');
  const names = new Set<string>();
  for (const { name } of cases) {
    if (names.has(name)) fail(label, `duplicate case name ${name}`);
    names.add(name);
  }
  return { source: parseSource(file['source'], `${label}.source`), cases };
}

export function loadVectorFile(path: string): VectorFile {
  return parseVectorFile(JSON.parse(readFileSync(path, 'utf8')), path);
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
