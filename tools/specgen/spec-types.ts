import { asArray, asBoolean, asNumber, asObject, asRecord, asString, asStrings, fail } from '../json-guards.ts';

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

const OPTIONAL_ERROR_KEYS = ['details', 'fields', 'wire', 'enumType'];
const MAX_WIRE_CODES = 256;
const CODE_NAME = /^[A-Z][A-Z_]*$/;
const LIMIT_NAME = /^[a-z][A-Za-z0-9]*$/;

/** Root of every JSON path in errors, e.g. `spec.json $.errors[0].code: expected a string`. */
const SPEC = 'spec.json $';

const parseField = (v: unknown, path: string): FieldType => (v === 'u32' || v === 'string' ? v : asStrings(v, path));

const parseError = (raw: unknown, path: string): ErrorSpec => {
  const e = asObject(raw, path, ['code'], OPTIONAL_ERROR_KEYS);
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
  const root = asRecord(raw, SPEC);
  const h1 = asRecord(asRecord(root.limits, `${SPEC}.limits`).h1, `${SPEC}.limits.h1`);
  return {
    errors: asArray(root.errors, `${SPEC}.errors`).map((e, i) => parseError(e, `${SPEC}.errors[${String(i)}]`)),
    limits: { h1: Object.fromEntries(Object.entries(h1).map(([k, v]) => [k, asNumber(v, `${SPEC}.limits.h1.${k}`)])) },
    methods: asStrings(root.methods, `${SPEC}.methods`),
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

type FieldEntry = readonly [string, FieldType];

/** Field list of a code: not empty, and every enum field a valid detail list. */
const checkFields = (where: string, fields: readonly FieldEntry[]): void => {
  if (fields.length === 0) fail(`${where}.fields`, 'empty object');
  for (const [name, type] of fields) if (Array.isArray(type)) checkDetailList(`${where}.fields.${name}`, type, 256);
};

/** Wire words carry at most one u32 and one enum, and never a string. */
const checkWireFields = (where: string, fields: readonly FieldEntry[]): void => {
  const strings = fields.filter(([, t]) => t === 'string').map(([name]) => name);
  if (strings.length > 0)
    fail(where, `string field "${strings.join(', ')}" needs wire: false (words carry only u32 and enums)`);
  const enums = fields.filter(([, t]) => typeof t !== 'string');
  if (enums.length > 1 || fields.length - enums.length > 1)
    fail(where, 'a wire code takes at most one u32 and one enum field');
};

const checkError = (e: ErrorSpec, index: number): void => {
  const where = `${SPEC}.errors[${String(index)}] (${e.code})`;
  if (!CODE_NAME.test(e.code)) fail(where, 'code must be UPPER_SNAKE_CASE');
  if ((e.details === undefined) === (e.fields === undefined)) fail(where, 'exactly one of details or fields');
  if (e.details !== undefined) {
    checkDetailList(`${where}.details`, e.details, 256);
    return;
  }
  const fields = Object.entries(e.fields ?? {});
  checkFields(where, fields);
  if (e.wire !== false) checkWireFields(where, fields);
};

/** Semantic validation: ids must fit the wire format and stay unambiguous. */
export function validateSpec(spec: Readonly<Spec>): void {
  if (spec.errors.length > MAX_WIRE_CODES) fail(`${SPEC}.errors`, `more than ${String(MAX_WIRE_CODES)} codes`);
  const dup = duplicates(spec.errors.map((e) => e.code));
  if (dup !== undefined) fail(`${SPEC}.errors`, `duplicate code "${dup}"`);
  spec.errors.forEach(checkError);
  checkDetailList(`${SPEC}.methods`, spec.methods, 256);
  if (spec.methods.includes('CONNECT'))
    fail(`${SPEC}.methods`, 'CONNECT must never be representable (design section 12)');
  for (const [name, value] of Object.entries(spec.limits.h1)) {
    if (!LIMIT_NAME.test(name) || !Number.isSafeInteger(value) || value < 0) {
      fail(`${SPEC}.limits.h1.${name}`, 'limits are camelCase names with non-negative integer values');
    }
  }
}
