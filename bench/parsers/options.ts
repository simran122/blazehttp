// Command-line option values of the parser bench runner (run.ts).

/** The longest `--run-ms`: with the 100 ms warm-up and container start it stays well inside the invocation timeout. */
export const MAX_RUN_MS = 60_000;

/**
 * A decimal integer option (digits only: no sign, space, exponent or fraction) from `min` up to `max`, or with no
 * upper bound when `max` is absent. Values beyond what a number holds exactly are rejected either way.
 */
export function integerOption(text: string, what: string, min: number, max?: number): number {
  const n = Number(text);
  const valid = /^\d+$/.test(text) && Number.isSafeInteger(n) && n >= min && (max === undefined || n <= max);
  if (!valid) {
    const range = max === undefined ? `≥ ${String(min)}` : `from ${String(min)} to ${String(max)}`;
    throw new Error(`${what} must be an integer ${range}`);
  }
  return n;
}
