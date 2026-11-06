/** A field is a number, a string, or an enum given as its ordered values. */
export type FieldType = 'u32' | 'string' | readonly string[];

export interface ErrorSpec {
  readonly code: string;
  /** Ordered detail values; the index is the wire detail id. Exclusive with `fields`. */
  readonly details?: readonly string[];
  /** Named fields; at most one `u32` (wire word 2) and one enum (wire detail id) on wire codes. */
  readonly fields?: Readonly<Record<string, FieldType>>;
  /** `false` keeps the code out of the Rust `ErrorCode` and off the wire. Its array index stays reserved. */
  readonly wire?: boolean;
  /** Rust name of the enum field's type; defaults to the variant name plus the field name. */
  readonly enumType?: string;
}

export interface Spec {
  /** Append-only: the array index is the wire code id. */
  readonly errors: readonly ErrorSpec[];
  readonly limits: { readonly h1: Readonly<Record<string, number>> };
  /** Append-only: the array index is the wire method id. */
  readonly methods: readonly string[];
}

const ERROR_KEYS = new Set(['code', 'details', 'fields', 'wire', 'enumType']);
const MAX_WIRE_CODES = 256;
const CODE_NAME = /^[A-Z][A-Z_]*$/;
const LIMIT_NAME = /^[a-z][A-Za-z0-9]*$/;

const fail = (path: string, problem: string): never => {
  throw new Error(`spec.json ${path}: ${problem}`);
};
const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const asRecord = (v: unknown, path: string): Record<string, unknown> =>
  isRecord(v) ? v : fail(path, 'expected an object');
const asArray = (v: unknown, path: string): unknown[] =>
  Array.isArray(v) ? (v as unknown[]) : fail(path, 'expected an array');
const asString = (v: unknown, path: string): string => (typeof v === 'string' ? v : fail(path, 'expected a string'));
const asStrings = (v: unknown, path: string): string[] =>
  asArray(v, path).map((x, i) => asString(x, `${path}[${String(i)}]`));
const asNumber = (v: unknown, path: string): number => (typeof v === 'number' ? v : fail(path, 'expected a number'));
const asBoolean = (v: unknown, path: string): boolean =>
  typeof v === 'boolean' ? v : fail(path, 'expected a boolean');

const parseField = (v: unknown, path: string): FieldType => (v === 'u32' || v === 'string' ? v : asStrings(v, path));

const parseError = (raw: unknown, path: string): ErrorSpec => {
  const e = asRecord(raw, path);
  for (const key of Object.keys(e)) if (!ERROR_KEYS.has(key)) fail(`${path}.${key}`, 'unknown key');
  const { details, fields, wire, enumType } = e;
  return {
    code: asString(e.code, `${path}.code`),
    ...(details === undefined ? {} : { details: asStrings(details, `${path}.details`) }),
    ...(fields === undefined
      ? {}
      : {
          fields: Object.fromEntries(
            Object.entries(asRecord(fields, `${path}.fields`)).map(([k, v]) => [
              k,
              parseField(v, `${path}.fields.${k}`),
            ]),
          ),
        }),
    ...(wire === undefined ? {} : { wire: asBoolean(wire, `${path}.wire`) }),
    ...(enumType === undefined ? {} : { enumType: asString(enumType, `${path}.enumType`) }),
  };
};

/** Structural validation of the parsed JSON. Errors name the JSON path. */
export function parseSpec(raw: unknown): Spec {
  const root = asRecord(raw, '$');
  const h1 = asRecord(asRecord(root.limits, '$.limits').h1, '$.limits.h1');
  return {
    errors: asArray(root.errors, '$.errors').map((e, i) => parseError(e, `$.errors[${String(i)}]`)),
    limits: { h1: Object.fromEntries(Object.entries(h1).map(([k, v]) => [k, asNumber(v, `$.limits.h1.${k}`)])) },
    methods: asStrings(root.methods, '$.methods'),
  };
}

const capitalize = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);

/** Rust identifier for a value: `slow-body` -> `SlowBody`, `dns/notfound` -> `DnsNotfound`, `idleBody` -> `IdleBody`. */
export const pascal = (value: string): string => value.split(/[-/_]/).map(capitalize).join('');

/** First value whose Rust identifier repeats an earlier one. */
const duplicates = (values: readonly string[], key: (v: string) => string = (v) => v): string | undefined =>
  values.find((v, i) => values.map(key).indexOf(key(v)) !== i);

const checkDetailList = (where: string, values: readonly string[], idBits: number): void => {
  if (values.length === 0) fail(where, 'empty list');
  if (values.length > idBits) fail(where, `more than ${String(idBits)} values`);
  const dup = duplicates(values, pascal);
  if (dup !== undefined) fail(where, `duplicate value "${dup}"`);
};

const checkError = (e: ErrorSpec, index: number): void => {
  const where = `$.errors[${String(index)}] (${e.code})`;
  if (!CODE_NAME.test(e.code)) fail(where, 'code must be UPPER_SNAKE_CASE');
  if ((e.details === undefined) === (e.fields === undefined)) fail(where, 'exactly one of details or fields');
  if (e.details !== undefined) {
    checkDetailList(`${where}.details`, e.details, 256);
    return;
  }
  const fields = Object.entries(e.fields ?? {});
  if (fields.length === 0) fail(`${where}.fields`, 'empty object');
  for (const [name, type] of fields) if (Array.isArray(type)) checkDetailList(`${where}.fields.${name}`, type, 256);
  if (e.wire === false) return;
  const strings = fields.filter(([, t]) => t === 'string').map(([name]) => name);
  if (strings.length > 0)
    fail(where, `string field "${strings.join(', ')}" needs wire: false (words carry only u32 and enums)`);
  const enums = fields.filter(([, t]) => typeof t !== 'string');
  if (enums.length > 1 || fields.length - enums.length > 1)
    fail(where, 'a wire code takes at most one u32 and one enum field');
};

/** Semantic validation: ids must fit the wire format and stay unambiguous. */
export function validateSpec(spec: Readonly<Spec>): void {
  if (spec.errors.length > MAX_WIRE_CODES) fail('$.errors', `more than ${String(MAX_WIRE_CODES)} codes`);
  const dup = duplicates(spec.errors.map((e) => e.code));
  if (dup !== undefined) fail('$.errors', `duplicate code "${dup}"`);
  spec.errors.forEach(checkError);
  checkDetailList('$.methods', spec.methods, 256);
  if (spec.methods.includes('CONNECT')) fail('$.methods', 'CONNECT must never be representable (design section 12)');
  for (const [name, value] of Object.entries(spec.limits.h1)) {
    if (!LIMIT_NAME.test(name) || !Number.isSafeInteger(value) || value < 0) {
      fail(`$.limits.h1.${name}`, 'limits are camelCase names with non-negative integer values');
    }
  }
}
