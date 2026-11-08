/**
 * The subset of TOML the policy rules read from Cargo manifests: tables, dotted keys, inline tables and comments,
 * flattened to `table.key` → value. Multi-line strings are skipped, never read as keys. Values are kept as normalised
 * text (whitespace collapsed); arrays of tables are read like plain tables.
 */

const TABLE_HEADER = /^\[\[?\s*([^\]]+?)\s*\]\]?$/;
const MULTILINE_QUOTE = /"""|'''/g;

/** `line` without its `#` comment; a `#` inside a quoted string is kept. */
const withoutComment = (line: string): string => {
  let quote: string | undefined;
  for (let i = 0; i < line.length; i += 1) {
    const c = line[i];
    if (quote === undefined && c === '#') return line.slice(0, i);
    if (quote === undefined && (c === '"' || c === "'")) quote = c;
    else if (quote === '"' && c === '\\') i += 1;
    else if (c === quote) quote = undefined;
  }
  return line;
};

/** The parts of an inline-table body split at commas outside nested braces, brackets and quotes. */
const splitTopLevel = (body: string): string[] => {
  const parts: string[] = [];
  let depth = 0;
  let quote: string | undefined;
  let start = 0;
  for (let i = 0; i < body.length; i += 1) {
    const c = body[i] ?? '';
    if (quote !== undefined) quote = c === quote ? undefined : quote;
    else if (c === '"' || c === "'") quote = c;
    else depth += Number('{['.includes(c)) - Number('}]'.includes(c));
    if (quote === undefined && depth === 0 && c === ',') {
      parts.push(body.slice(start, i));
      start = i + 1;
    }
  }
  return [...parts, body.slice(start)];
};

/** `a . "b"` → `a.b`. */
const normaliseKey = (key: string): string =>
  key
    .split('.')
    .map((part) => part.trim().replace(/^"(.*)"$/, '$1'))
    .join('.');

/** Adds `key = value`, flattening an inline table into one dotted key per leaf. */
const addEntry = (entries: Map<string, string>, key: string, value: string): void => {
  const text = value.trim();
  if (!(text.startsWith('{') && text.endsWith('}'))) {
    entries.set(key, text.replace(/\s+/g, ' '));
    return;
  }
  for (const pair of splitTopLevel(text.slice(1, -1))) {
    const eq = pair.indexOf('=');
    if (eq > 0) addEntry(entries, `${key}.${normaliseKey(pair.slice(0, eq))}`, pair.slice(eq + 1));
  }
};

/** Reads TOML line by line, tracking the current table and any open multi-line string. */
class TomlReader {
  readonly entries = new Map<string, string>();
  private table = '';
  private inMultiline = false;

  readLine(raw: string): void {
    const opensOrCloses = (raw.match(MULTILINE_QUOTE) ?? []).length % 2 === 1;
    if (this.inMultiline || opensOrCloses) {
      this.inMultiline = this.inMultiline !== opensOrCloses;
      return;
    }
    const line = withoutComment(raw).trim();
    const header = TABLE_HEADER.exec(line)?.[1];
    if (header !== undefined) {
      this.table = normaliseKey(header);
      return;
    }
    const eq = line.indexOf('=');
    if (eq <= 0) return;
    const key = normaliseKey(line.slice(0, eq));
    addEntry(this.entries, this.table === '' ? key : `${this.table}.${key}`, line.slice(eq + 1));
  }
}

/** Every key of a TOML document as `table.key` → normalised value, inline tables flattened. */
export const tomlEntries = (toml: string): ReadonlyMap<string, string> => {
  const reader = new TomlReader();
  for (const line of toml.split(/\r?\n/)) reader.readLine(line);
  return reader.entries;
};
