import ts from 'typescript';
import { parse, TomlDate, TomlError, type TomlTable, type TomlValue } from 'smol-toml';

export type RuleId =
  | 'rust-allow-forbidden'
  | 'rust-allow-attribute'
  | 'todo-needs-issue'
  | 'removal-note'
  | 'rust-commented-code'
  | 'no-install-scripts'
  | 'no-http-engine-dep'
  | 'eslint-directive'
  | 'crate-lints';

export type Violation = { file: string; line: number; rule: RuleId; message: string };
/** Lint settings as `<tool>.<lint>[.<field>]` → JSON-encoded TOML value, e.g. `clippy.pedantic.level` → `"deny"`. */
export type LintSettings = ReadonlyMap<string, string>;
/** `workspaceLints` are the root manifest's `[workspace.lints.*]` settings. */
export type Context = { packageVersion: string; workspaceLints: LintSettings };
type Rule = (path: string, content: string, ctx: Context) => Violation[];

/** Rules apply only to code and config files, never to prose docs. */
const CHECKED_FILE = /\.(rs|[cm]?[jt]sx?|toml|yml|yaml|json)$/;

const FORBIDDEN_LINTS = /^(deprecated|dead_code|unused\w*)$/;
const HTTP_ENGINE_CRATES = new Set(['hyper', 'hyper-util', 'reqwest', 'ureq', 'isahc', 'h2']);
const INSTALL_SCRIPTS = ['preinstall', 'install', 'postinstall'];
const ENGINE_CRATE_MANIFEST = /^crates\/(engine-core|node-binding)\/(?:.*\/)?Cargo\.toml$/;

const violation = (file: string, line: number, rule: RuleId, message: string): Violation => ({
  file,
  line,
  rule,
  message,
});

/** Runs `inspect` on every line; it returns a message for a violating line. */
const scanLines = (path: string, content: string, rule: RuleId, inspect: (line: string) => string | undefined) =>
  content.split('\n').flatMap((text, index) => {
    const message = inspect(text);
    return message === undefined ? [] : [violation(path, index + 1, rule, message)];
  });

const compareVersions = (a: string, b: string): number => a.localeCompare(b, 'en', { numeric: true });

const rustAllowForbidden: Rule = (path, content) =>
  path.endsWith('.rs')
    ? scanLines(path, content, 'rust-allow-forbidden', (line) => {
        const match = /#!?\[\s*(?:allow|expect)\s*\(([^)]*)\)\s*\]/.exec(line);
        const names = (match?.[1] ?? '').split(',').map((n) => n.trim().replace(/^\w+::/, ''));
        return names.some((n) => FORBIDDEN_LINTS.test(n)) ? 'allow/expect of a forbidden lint' : undefined;
      })
    : [];

const ATTRIBUTE_START = /#\s*!?\s*\[/g;
const ALLOW_CALL = /\ballow\s*\(/;
/** A string literal's opening: optional `b`/`c` prefix, optional raw `r` with its `#`s, then `"`. */
const STRING_OPEN = /[bc]?(?:r(#*))?"/y;
const IDENT_CHAR = /\w/;

/** End of the block comment starting at `i` (slash-star to star-slash); Rust block comments nest. */
const blockCommentEnd = (src: string, i: number): number => {
  let depth = 0;
  let j = i;
  while (j < src.length) {
    const pair = src.slice(j, j + 2);
    const step = Number(pair === '/*') - Number(pair === '*/');
    depth += step;
    j += step === 0 ? 1 : 2;
    if (depth === 0) return j;
  }
  return src.length;
};

/** End of a `"…"` body starting at `from`, skipping `\` escapes. */
const quotedEnd = (src: string, from: number): number => {
  for (let j = from; j < src.length; j += 1) {
    if (src[j] === '\\') j += 1;
    else if (src[j] === '"') return j + 1;
  }
  return src.length;
};

/** End of a string literal (plain, byte, C or raw) starting at `i`, or `undefined` when none starts there. */
const stringEnd = (src: string, i: number): number | undefined => {
  if (src[i] !== '"' && IDENT_CHAR.test(src[i - 1] ?? '')) return undefined;
  STRING_OPEN.lastIndex = i;
  const open = STRING_OPEN.exec(src);
  if (open === null) return undefined;
  const bodyStart = i + open[0].length;
  const hashes = open[1];
  if (hashes === undefined) return quotedEnd(src, bodyStart);
  const close = src.indexOf(`"${hashes}`, bodyStart);
  return close === -1 ? src.length : close + 1 + hashes.length;
};

/** End of a char literal at `i` (`'x'`, `'\n'`, `'\u{…}'`), or `undefined` for a lifetime such as `'a`. */
const charLiteralEnd = (src: string, i: number): number | undefined => {
  if (src[i + 1] === '\\') {
    const close = src.indexOf("'", i + 3);
    return close === -1 ? src.length : close + 1;
  }
  const width = (src.codePointAt(i + 1) ?? 0) > 0xffff ? 2 : 1;
  return src[i + 1 + width] === "'" ? i + 2 + width : undefined;
};

/** End of the comment or literal that starts at `i`, or `undefined` when code starts there. */
const nonCodeEnd = (src: string, i: number): number | undefined => {
  if (src.startsWith('//', i)) {
    const newline = src.indexOf('\n', i);
    return newline === -1 ? src.length : newline;
  }
  if (src.startsWith('/*', i)) return blockCommentEnd(src, i);
  if (src[i] === "'") return charLiteralEnd(src, i);
  return stringEnd(src, i);
};

/** `src` with every comment and string or char literal blanked to spaces; newlines and offsets are kept. */
const rustCodeOnly = (src: string): string => {
  const out = src.split('');
  let i = 0;
  while (i < src.length) {
    const end = nonCodeEnd(src, i);
    if (end === undefined) {
      i += 1;
      continue;
    }
    for (let k = i; k < end; k += 1) if (out[k] !== '\n') out[k] = ' ';
    i = end;
  }
  return out.join('');
};

/** Index just past the `]` that closes the attribute whose `[` is at `open` (or the end of `content`). */
const attributeEnd = (content: string, open: number): number => {
  let depth = 0;
  for (let i = open; i < content.length; i += 1) {
    depth += Number(content[i] === '[') - Number(content[i] === ']');
    if (depth === 0) return i + 1;
  }
  return content.length;
};

/**
 * Every outer (`#[…]`) and inner (`#![…]`) attribute with its first line, read from code only (comments and literals
 * blanked), brackets balanced across lines.
 */
const rustAttributes = (content: string): { readonly line: number; readonly text: string }[] => {
  const code = rustCodeOnly(content);
  let line = 1;
  let counted = 0;
  return [...code.matchAll(ATTRIBUTE_START)].map((match) => {
    line += code.slice(counted, match.index).split('\n').length - 1;
    counted = match.index;
    const open = match.index + match[0].length - 1;
    return { line, text: code.slice(match.index, attributeEnd(code, open)) };
  });
};

/**
 * Lint exceptions are `#[expect(lint, reason = "…")]` only (design §13.1 rule 9). Clippy's `allow_attributes` sees
 * outer attributes only, so this rule rejects `allow(` in every attribute written in the source (outer, inner, inside
 * `cfg_attr`), read from code only. Attributes a macro builds from its arguments are left to review.
 */
const rustAllowAttribute: Rule = (path, content) =>
  path.endsWith('.rs')
    ? rustAttributes(content)
        .filter(({ text }) => ALLOW_CALL.test(text))
        .map(({ line }) =>
          violation(path, line, 'rust-allow-attribute', 'use #[expect(lint, reason = "…")], not allow'),
        )
    : [];

const todoNeedsIssue: Rule = (path, content) =>
  scanLines(path, content, 'todo-needs-issue', (line) =>
    /(?:\/\/|\/\*|^\s*\*|#)\s*(?:TODO|FIXME)(?!\(#\d+\))/.test(line)
      ? 'TODO/FIXME must link an issue, e.g. TODO(#123):'
      : undefined,
  );

const removalNote: Rule = (path, content, ctx) =>
  scanLines(path, content, 'removal-note', (line) => {
    const note = /(?:\/\/|#)\s*removal:\s*(.*)$/.exec(line)?.[1];
    if (note === undefined) return undefined;
    const parsed = /^(.+?)\s*\(#\d+\)\s*$/.exec(note);
    if (parsed === null) return 'removal note needs "<version|condition> (#issue)"';
    const version = /^\d+\.\d+\.\d+$/.test(parsed[1] ?? '') ? parsed[1] : undefined;
    return version !== undefined && compareVersions(ctx.packageVersion, version) >= 0
      ? `removal version ${version} is due (package is ${ctx.packageVersion})`
      : undefined;
  });

const COMMENTED_RUST =
  /^\s*\/\/(?![/!])\s*(?:(?:let|use|fn|pub|struct|enum|impl|mod|const|static|return)\s|.*[;{}]\s*$)/;
const rustCommentedCode: Rule = (path, content) =>
  path.endsWith('.rs')
    ? scanLines(path, content, 'rust-commented-code', (line) =>
        COMMENTED_RUST.test(line) ? 'commented-out code must be deleted' : undefined,
      )
    : [];

const noInstallScripts: Rule = (path, content) => {
  if (!/(^|\/)package\.json$/.test(path)) return [];
  let manifest: unknown;
  try {
    manifest = JSON.parse(content);
  } catch {
    return [];
  }
  const scripts = (manifest as { scripts?: Record<string, unknown> } | null)?.scripts ?? {};
  return INSTALL_SCRIPTS.filter((name) => name in scripts).map((name) => {
    const index = content.split('\n').findIndex((l) => l.includes(`"${name}"`));
    return violation(path, index + 1, 'no-install-scripts', `"${name}" script is forbidden`);
  });
};

const DEPENDENCY_HEADER = /^\[(?:target\.[^\]]+\.)?(?:dev-|build-)?dependencies(?:\.([\w-]+))?\]/;
const DEPENDENCY_KEY = /^\s*([\w-]+)(?:\.[\w-]+)*\s*=/;
const PACKAGE_RENAME = /\bpackage\s*=\s*"([\w-]+)"/;

const engineMessage = (crate: string | undefined): string | undefined =>
  crate !== undefined && HTTP_ENGINE_CRATES.has(crate) ? `HTTP engine crate ${crate}` : undefined;

/** The Cargo table a line sits in: a `[dependencies]`-style list, a `[dependencies.<name>]` table, or another table. */
type CargoTable = 'dependency-list' | 'dependency-table' | 'other';

const tableOf = (header: RegExpExecArray | null): CargoTable => {
  if (header === null) return 'other';
  return header[1] === undefined ? 'dependency-list' : 'dependency-table';
};

/** Engine crate named by a line inside a table: by key in a dependency list, or by a `package = "…"` rename. */
const dependencyLineMessage = (table: CargoTable, line: string): string | undefined => {
  if (table === 'other') return undefined;
  const renamed = engineMessage(PACKAGE_RENAME.exec(line)?.[1]);
  return table === 'dependency-table' ? renamed : (engineMessage(DEPENDENCY_KEY.exec(line)?.[1]) ?? renamed);
};

const noHttpEngineDep: Rule = (path, content) => {
  if (!ENGINE_CRATE_MANIFEST.test(path)) return [];
  let table: CargoTable = 'other';
  return scanLines(path, content, 'no-http-engine-dep', (line) => {
    const trimmed = line.trim();
    if (!trimmed.startsWith('[')) return dependencyLineMessage(table, line);
    const header = DEPENDENCY_HEADER.exec(trimmed);
    table = tableOf(header);
    return engineMessage(header?.[1]);
  });
};

/** JS and TS in every module flavour: `.js .mjs .cjs .jsx`, `.ts .mts .cts .tsx`. */
const SCRIPT_FILE = /\.[cm]?[jt]sx?$/;
/** TypeScript script kind by file extension; anything else parses as TypeScript. */
const SCRIPT_KIND: ReadonlyMap<string, ts.ScriptKind> = new Map([
  ['tsx', ts.ScriptKind.TSX],
  ['jsx', ts.ScriptKind.JSX],
  ['js', ts.ScriptKind.JS],
  ['mjs', ts.ScriptKind.JS],
  ['cjs', ts.ScriptKind.JS],
]);
/**
 * Line comments ESLint reads as directives (`eslint-disable-line`, `eslint-disable-next-line`), plus bare
 * `eslint-disable`/`eslint-enable`, which ESLint ignores in line comments but which read like directives.
 */
const LINE_DIRECTIVE = /^eslint-(?:disable|enable)(?:-line|-next-line)?(?:\s|$)/;
/**
 * Block comments ESLint reads as directives or inline config: `eslint`, `eslint-disable…`, `eslint-enable`,
 * `eslint-env`, `global`, `globals` and `exported`, each followed by a space or the end.
 */
const BLOCK_DIRECTIVE = /^(?:eslint(?:-disable(?:-line|-next-line)?|-enable|-env)?|globals?|exported)(?:\s|$)/;
/** The one accepted form (design §13.1 rule 9), as the whole of a line comment: named rules and a reason. */
const ESLINT_NEXT_LINE = /^eslint-disable-next-line [@a-z][\w@/-]*(?:, ?[@a-z][\w@/-]*)* -- \S.*$/;
const DIRECTIVE_MESSAGE = 'only a line comment "eslint-disable-next-line <rule> -- <reason>" may steer ESLint';

/**
 * Every comment of a script, found from the TypeScript parse tree: the leading and trailing trivia of each node, so
 * strings, template literals and regexes are never mistaken for comments. JSX text is not trivia and is skipped.
 */
const scriptComments = (
  path: string,
  content: string,
): { readonly source: ts.SourceFile; ranges: ts.CommentRange[] } => {
  const kind = SCRIPT_KIND.get(path.slice(path.lastIndexOf('.') + 1)) ?? ts.ScriptKind.TS;
  const source = ts.createSourceFile(path, content, ts.ScriptTarget.Latest, true, kind);
  const byStart = new Map<number, ts.CommentRange>();
  /** JSX text spans: the trivia scan after a `>` would read text such as `/* x *\/` there as a comment. */
  const jsxTexts: (readonly [number, number])[] = [];
  const visit = (node: ts.Node): void => {
    if (node.kind === ts.SyntaxKind.JsxText) {
      jsxTexts.push([node.pos, node.end]);
      return;
    }
    const leading = ts.getLeadingCommentRanges(content, node.pos) ?? [];
    const trailing = ts.getTrailingCommentRanges(content, node.end) ?? [];
    for (const range of [...leading, ...trailing]) byStart.set(range.pos, range);
    node.getChildren(source).forEach(visit);
  };
  visit(source);
  // ponytail: linear scan of JSX text spans per comment; files here have few of either.
  const inJsxText = (range: ts.CommentRange): boolean =>
    jsxTexts.some(([start, end]) => range.pos >= start && range.pos < end);
  return { source, ranges: [...byStart.values()].filter((range) => !inJsxText(range)) };
};

/** A comment ESLint would not read as a directive, or the one accepted `eslint-disable-next-line` form. */
const isAllowedComment = (content: string, range: ts.CommentRange): boolean => {
  const line = range.kind === ts.SyntaxKind.SingleLineCommentTrivia;
  const text = content.slice(range.pos + 2, line ? range.end : range.end - 2).trim();
  if (!line) return !BLOCK_DIRECTIVE.test(text);
  return !LINE_DIRECTIVE.test(text) || ESLINT_NEXT_LINE.test(text);
};

/**
 * ESLint may be steered only by `// eslint-disable-next-line <rule> -- <reason>`. This runs outside ESLint, so no
 * directive can switch it off, and checks each comment whole, so multi-line blocks and several comments on a line count.
 */
const eslintDirective: Rule = (path, content) => {
  if (!SCRIPT_FILE.test(path)) return [];
  const { source, ranges } = scriptComments(path, content);
  return ranges
    .filter((range) => !isAllowedComment(content, range))
    .map((range) => {
      const line = source.getLineAndCharacterOfPosition(range.pos).line + 1;
      return violation(path, line, 'eslint-directive', DIRECTIVE_MESSAGE);
    });
};

const CRATE_MANIFEST = /^crates\/[^/]+\/Cargo\.toml$/;
/** Crates that are their own workspace (outside the root one): fuzz targets and benchmark harnesses. */
const STANDALONE_MANIFEST = /^(?:crates\/[^/]+\/fuzz|bench\/(?:[^/]+\/)*[^/]+)\/Cargo\.toml$/;

const isTable = (value: TomlValue | undefined): value is TomlTable =>
  typeof value === 'object' && !Array.isArray(value) && !(value instanceof TomlDate);

/** `table[key]` when `table` is a TOML table that has its own `key`. */
const field = (table: TomlValue | undefined, key: string): TomlValue | undefined =>
  isTable(table) && Object.hasOwn(table, key) ? table[key] : undefined;

/** JSON replacer that writes a bigint (a TOML integer beyond 2^53) as its exact decimal text. */
const exactBigInt = (_key: string, value: unknown): unknown => (typeof value === 'bigint' ? value.toString() : value);

/** A `lints` table flattened to `<tool>.<lint>[.<field>]` → JSON value, e.g. `clippy.pedantic.level` → `"deny"`. */
const lintSettingsOf = (lints: TomlValue | undefined): LintSettings => {
  const settings = new Map<string, string>();
  const visit = (value: TomlValue, key: string): void => {
    if (!isTable(value)) {
      settings.set(key, JSON.stringify(value, exactBigInt));
      return;
    }
    for (const [name, inner] of Object.entries(value)) visit(inner, key === '' ? name : `${key}.${name}`);
  };
  if (lints !== undefined) visit(lints, '');
  return settings;
};

/** `clippy.pedantic.level` → `[lints.clippy] pedantic.level`; `workspace` → `[lints] workspace`. */
const lintName = (key: string): string => {
  const dot = key.indexOf('.');
  return dot === -1 ? `[lints] ${key}` : `[lints.${key.slice(0, dot)}] ${key.slice(dot + 1)}`;
};

/** How a standalone crate's lint settings differ from the workspace's: missing, changed or extra keys. */
const lintDifferences = (workspace: LintSettings, own: LintSettings): string[] => {
  const keys = new Set([...workspace.keys(), ...own.keys()]);
  return [...keys].flatMap((key) => {
    const expected = workspace.get(key);
    const actual = own.get(key);
    if (expected === actual) return [];
    if (actual === undefined) return [`standalone crate lacks workspace lint ${lintName(key)} = ${String(expected)}`];
    if (expected === undefined)
      return [`standalone crate sets ${lintName(key)} = ${actual}, which the workspace does not`];
    return [`standalone crate sets ${lintName(key)} = ${actual}; the workspace sets ${expected}`];
  });
};

/** The parsed manifest, or a violation naming the file when it is not valid TOML (fail closed). */
const parseManifest = (path: string, content: string): { readonly manifest: TomlTable } | Violation => {
  try {
    return { manifest: parse(content, { integersAsBigInt: 'asNeeded' }) };
  } catch (error) {
    if (!(error instanceof TomlError)) throw error;
    return violation(path, error.line, 'crate-lints', `cannot parse TOML: ${String(error.message.split('\n')[0])}`);
  }
};

/** The root manifest's `[workspace.lints]` settings, or a violation naming `path` when it is not valid TOML. */
export const workspaceLintSettings = (path: string, toml: string): { readonly settings: LintSettings } | Violation => {
  const read = parseManifest(path, toml);
  return 'rule' in read ? read : { settings: lintSettingsOf(field(field(read.manifest, 'workspace'), 'lints')) };
};

/**
 * Every crate inherits the workspace lints (`[lints] workspace = true`). A standalone crate (fuzz targets, bench
 * harnesses) is its own workspace, so its `[lints.<tool>]` tables must equal the workspace settings: none missing,
 * loosened or added.
 */
const crateLints: Rule = (path, content, ctx) => {
  const crate = CRATE_MANIFEST.test(path);
  if (!crate && !STANDALONE_MANIFEST.test(path)) return [];
  const read = parseManifest(path, content);
  if ('rule' in read) return [read];
  const lints = field(read.manifest, 'lints');
  if (crate) {
    const inherits = field(lints, 'workspace') === true;
    return inherits ? [] : [violation(path, 1, 'crate-lints', 'crate must set [lints] workspace = true')];
  }
  return lintDifferences(ctx.workspaceLints, lintSettingsOf(lints)).map((message) =>
    violation(path, 1, 'crate-lints', message),
  );
};

const RULES: Record<RuleId, Rule> = {
  'rust-allow-forbidden': rustAllowForbidden,
  'rust-allow-attribute': rustAllowAttribute,
  'todo-needs-issue': todoNeedsIssue,
  'removal-note': removalNote,
  'rust-commented-code': rustCommentedCode,
  'no-install-scripts': noInstallScripts,
  'no-http-engine-dep': noHttpEngineDep,
  'eslint-directive': eslintDirective,
  'crate-lints': crateLints,
};

export const checkFile = (path: string, content: string, ctx: Context): Violation[] =>
  CHECKED_FILE.test(path) ? Object.values(RULES).flatMap((rule) => rule(path, content, ctx)) : [];
