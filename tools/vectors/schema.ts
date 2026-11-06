import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { ERROR_WIRE } from '../../packages/blazehttp/src/generated/spec.ts';

/** Vector files live under this directory, registered in its `index.json` (format: docs/notes/vectors.md). */
export const VECTORS_ROOT = 'tests/vectors';
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

export interface VectorCase {
  readonly name: string;
  /** Every byte the server sends; the server then closes the connection. */
  readonly input: string;
  readonly inputEncoding: 'utf8';
  readonly request: { readonly method: string };
  readonly expect: {
    readonly native: NativeExpect;
    /** Optional while only the native engine runs the vectors; required once undici runs them. */
    // removal: P4 (#5)
    readonly undici?: NativeExpect;
  };
  /** What reference implementations did with the input, keyed by project (informational). */
  readonly reference?: Readonly<Record<string, string>>;
}

export interface VectorFile {
  readonly source: VectorSource;
  readonly cases: readonly VectorCase[];
}

/** Vector file (relative path) → suites that consume it. */
export type VectorIndex = Readonly<Record<string, readonly string[]>>;

/** Every error string the engine can produce, as `ErrorCode`'s `Display` prints it. */
const KNOWN_ERRORS: ReadonlySet<string> = new Set(
  ERROR_WIRE.flatMap((e) => (e.details === undefined ? [e.code] : e.details.map((d) => `${e.code}/${d}`))),
);

type Json = Readonly<Record<string, unknown>>;

const fail = (at: string, message: string): never => {
  throw new Error(`${at}: ${message}`);
};

export const record = (value: unknown, at: string): Json =>
  typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Json)
    : fail(at, 'expected an object');

export const object = (value: unknown, at: string, keys: readonly string[], optional: readonly string[] = []): Json => {
  const json = record(value, at);
  const unknownKey = Object.keys(json).find((k) => !keys.includes(k) && !optional.includes(k));
  if (unknownKey !== undefined) fail(at, `unknown key ${unknownKey}`);
  const missing = keys.find((k) => !(k in json));
  if (missing !== undefined) fail(at, `missing key ${missing}`);
  return json;
};

const array = (value: unknown, at: string): readonly unknown[] =>
  Array.isArray(value) ? (value as readonly unknown[]) : fail(at, 'expected an array');

const string = (value: unknown, at: string): string =>
  typeof value === 'string' ? value : fail(at, 'expected a string');

const parseSource = (value: unknown, at: string): VectorSource => {
  const s = object(value, at, ['project', 'url', 'commit', 'licence', 'copyright'], ['tag']);
  return {
    project: string(s['project'], `${at}.project`),
    url: string(s['url'], `${at}.url`),
    ...(s['tag'] === undefined ? {} : { tag: string(s['tag'], `${at}.tag`) }),
    commit: string(s['commit'], `${at}.commit`),
    licence: string(s['licence'], `${at}.licence`),
    copyright: string(s['copyright'], `${at}.copyright`),
  };
};

const parseOk = (value: unknown, at: string): OkExpect => {
  const ok = object(value, at, ['status', 'headers', 'body', 'keepAlive']);
  const status = ok['status'];
  if (typeof status !== 'number' || !Number.isInteger(status)) fail(`${at}.status`, 'expected an integer');
  if (typeof ok['keepAlive'] !== 'boolean') fail(`${at}.keepAlive`, 'expected a boolean');
  const headers = array(ok['headers'], `${at}.headers`).map((pair, i): readonly [string, string] => {
    const where = `${at}.headers[${String(i)}]`;
    const [name, value, ...rest] = array(pair, where);
    if (rest.length > 0 || value === undefined) fail(where, 'expected [name, value]');
    return [string(name, where), string(value, where)];
  });
  return {
    status: status as number,
    headers,
    body: string(ok['body'], `${at}.body`),
    keepAlive: ok['keepAlive'] as boolean,
  };
};

export const parseNativeExpect = (value: unknown, at: string): NativeExpect => {
  const keys = typeof value === 'object' && value !== null ? Object.keys(value) : [];
  if (keys.length !== 1) fail(at, 'expected exactly one of ok, error');
  if (keys[0] === 'ok') return { ok: parseOk(object(value, at, ['ok'])['ok'], `${at}.ok`) };
  const error = string(object(value, at, ['error'])['error'], `${at}.error`);
  return KNOWN_ERRORS.has(error) ? { error } : fail(`${at}.error`, `${error} is not an engine error`);
};

const parseCase = (value: unknown, at: string): VectorCase => {
  const c = object(value, at, ['name', 'input', 'inputEncoding', 'request', 'expect'], ['reference']);
  if (c['inputEncoding'] !== 'utf8') fail(`${at}.inputEncoding`, 'expected utf8');
  const request = object(c['request'], `${at}.request`, ['method']);
  const expectJson = object(c['expect'], `${at}.expect`, ['native'], ['undici']);
  const reference =
    c['reference'] === undefined
      ? undefined
      : Object.fromEntries(
          Object.entries(record(c['reference'], `${at}.reference`)).map(([k, v]) => [
            k,
            string(v, `${at}.reference.${k}`),
          ]),
        );
  return {
    name: string(c['name'], `${at}.name`),
    input: string(c['input'], `${at}.input`),
    inputEncoding: 'utf8',
    request: { method: string(request['method'], `${at}.request.method`) },
    expect: {
      native: parseNativeExpect(expectJson['native'], `${at}.expect.native`),
      ...(expectJson['undici'] === undefined
        ? {}
        : { undici: parseNativeExpect(expectJson['undici'], `${at}.expect.undici`) }),
    },
    ...(reference === undefined ? {} : { reference }),
  };
};

/** Validates parsed JSON as a vector file; `label` prefixes error messages. */
export function parseVectorFile(data: unknown, label: string): VectorFile {
  const file = object(data, label, ['source', 'cases']);
  const cases = array(file['cases'], `${label}.cases`).map((c, i) => parseCase(c, `${label}.cases[${String(i)}]`));
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
    Object.entries(record(data, label)).map(([file, suites]) => [
      file,
      array(suites, `${label}.${file}`).map((s) => string(s, `${label}.${file}`)),
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
