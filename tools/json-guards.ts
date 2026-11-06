/** Validators for parsed JSON shared by the tools. Every error reads `<path>: <problem>`. */

export type JsonRecord = Readonly<Record<string, unknown>>;

export const fail = (path: string, problem: string): never => {
  throw new Error(`${path}: ${problem}`);
};

export const asRecord = (value: unknown, path: string): JsonRecord =>
  typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as JsonRecord)
    : fail(path, 'expected an object');

/** An object with exactly the `required` keys plus any of the `optional` ones. */
export const asObject = (
  value: unknown,
  path: string,
  required: readonly string[],
  optional: readonly string[] = [],
): JsonRecord => {
  const json = asRecord(value, path);
  const unknownKey = Object.keys(json).find((k) => !required.includes(k) && !optional.includes(k));
  if (unknownKey !== undefined) fail(`${path}.${unknownKey}`, 'unknown key');
  const missing = required.find((k) => !(k in json));
  if (missing !== undefined) fail(`${path}.${missing}`, 'missing');
  return json;
};

export const asArray = (value: unknown, path: string): readonly unknown[] =>
  Array.isArray(value) ? (value as readonly unknown[]) : fail(path, 'expected an array');

export const asString = (value: unknown, path: string): string =>
  typeof value === 'string' ? value : fail(path, 'expected a string');

export const asStrings = (value: unknown, path: string): string[] =>
  asArray(value, path).map((x, i) => asString(x, `${path}[${String(i)}]`));

export const asNumber = (value: unknown, path: string): number =>
  typeof value === 'number' ? value : fail(path, 'expected a number');

export const asBoolean = (value: unknown, path: string): boolean =>
  typeof value === 'boolean' ? value : fail(path, 'expected a boolean');
