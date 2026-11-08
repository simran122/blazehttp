import ts from 'typescript';

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
/** Lint settings by tool (`rust`, `clippy`): whitespace-normalised `key = value` lines. */
export type LintTables = ReadonlyMap<string, ReadonlySet<string>>;
/** `workspaceLints` are the root manifest's `[workspace.lints.*]` tables. */
export type Context = { packageVersion: string; workspaceLints: LintTables };
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

const ATTRIBUTE_START = /#!?\s*\[/g;
const ALLOW_CALL = /\ballow\s*\(/;

/** Index just past the `]` that closes the attribute whose `[` is at `open` (or the end of `content`). */
const attributeEnd = (content: string, open: number): number => {
  let depth = 0;
  for (let i = open; i < content.length; i += 1) {
    depth += Number(content[i] === '[') - Number(content[i] === ']');
    if (depth === 0) return i + 1;
  }
  return content.length;
};

/** Every outer (`#[…]`) and inner (`#![…]`) attribute with its first line, brackets balanced across lines. */
const rustAttributes = (content: string): { readonly line: number; readonly text: string }[] => {
  let line = 1;
  let counted = 0;
  return [...content.matchAll(ATTRIBUTE_START)].map((match) => {
    line += content.slice(counted, match.index).split('\n').length - 1;
    counted = match.index;
    const open = match.index + match[0].length - 1;
    return { line, text: content.slice(match.index, attributeEnd(content, open)) };
  });
};

/**
 * Lint exceptions are `#[expect(lint, reason = "…")]` only (design §13.1 rule 9). Clippy's `allow_attributes` sees
 * outer attributes only, so this rule rejects `allow(` in every attribute form: outer, inner and inside `cfg_attr`.
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
/** Any comment ESLint reads as a directive or inline config: `eslint…`, `global`, `globals`, `exported`. */
const ESLINT_DIRECTIVE = /^(?:eslint|globals?\b|exported\b)/;
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
  return !ESLINT_DIRECTIVE.test(text) || (line && ESLINT_NEXT_LINE.test(text));
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

const TOML_HEADER = /^\[([^\]]+)\]\s*$/;
const TOML_KEY_LINE = /^[\w-]+\s*=/;
const CRATE_MANIFEST = /^crates\/[^/]+\/Cargo\.toml$/;
const FUZZ_MANIFEST = /^crates\/[^/]+\/fuzz\/Cargo\.toml$/;

/** Key lines of each TOML table, whitespace-normalised, e.g. `lints.clippy` → `{'pedantic = …'}`. */
const tomlTables = (toml: string): Map<string, Set<string>> => {
  const tables = new Map<string, Set<string>>();
  let current = new Set<string>();
  for (const raw of toml.split('\n')) {
    const line = raw.trim().replace(/\s+/g, ' ');
    const header = TOML_HEADER.exec(line)?.[1];
    if (header !== undefined) {
      current = tables.get(header) ?? new Set<string>();
      tables.set(header, current);
    } else if (TOML_KEY_LINE.test(line)) current.add(line);
  }
  return tables;
};

/** Lint settings by tool (`rust`, `clippy`) from the `[<prefix>.<tool>]` tables of a manifest. */
export const lintTables = (toml: string, prefix: 'workspace.lints' | 'lints'): LintTables =>
  new Map(
    [...tomlTables(toml)]
      .filter(([name]) => name.startsWith(`${prefix}.`))
      .map(([name, keys]) => [name.slice(prefix.length + 1), keys]),
  );

/**
 * Every crate inherits the workspace lints (`[lints] workspace = true`). A fuzz crate is its own workspace, so its
 * `[lints.<tool>]` tables must repeat every workspace lint setting.
 */
const crateLints: Rule = (path, content, ctx) => {
  if (CRATE_MANIFEST.test(path)) {
    const inherits = tomlTables(content).get('lints')?.has('workspace = true') === true;
    return inherits ? [] : [violation(path, 1, 'crate-lints', 'crate must set [lints] workspace = true')];
  }
  if (!FUZZ_MANIFEST.test(path)) return [];
  const own = lintTables(content, 'lints');
  return [...ctx.workspaceLints].flatMap(([tool, settings]) =>
    [...settings]
      .filter((setting) => own.get(tool)?.has(setting) !== true)
      .map((setting) =>
        violation(path, 1, 'crate-lints', `fuzz crate lacks workspace lint [lints.${tool}] ${setting}`),
      ),
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
