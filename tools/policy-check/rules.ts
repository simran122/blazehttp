export type RuleId =
  | 'rust-allow-forbidden'
  | 'rust-allow-attribute'
  | 'todo-needs-issue'
  | 'removal-note'
  | 'rust-commented-code'
  | 'no-install-scripts'
  | 'no-http-engine-dep'
  | 'eslint-disable-reason';

export type Violation = { file: string; line: number; rule: RuleId; message: string };
export type Context = { packageVersion: string };
type Rule = (path: string, content: string, ctx: Context) => Violation[];

/** Rules apply only to code and config files, never to prose docs. */
const CHECKED_FILE = /\.(rs|ts|mts|js|mjs|toml|yml|yaml|json)$/;

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

const SCRIPT_FILE = /\.(ts|mts|js|mjs)$/;
/** Any ESLint disable or enable directive, in a line or block comment. */
const ESLINT_DIRECTIVE = /(?:\/\/|\/\*)\s*eslint-(?:disable|enable)/;
/** The one accepted form (design §13.1 rule 9): next line only, named rules, and a reason after `--`. */
const ESLINT_NEXT_LINE = /\/\/\s*eslint-disable-next-line\s+[@a-z][\w@/-]*(?:\s*,\s*[@a-z][\w@/-]*)*\s+--\s+\S/;

const eslintDisableReason: Rule = (path, content) =>
  SCRIPT_FILE.test(path)
    ? scanLines(path, content, 'eslint-disable-reason', (line) =>
        ESLINT_DIRECTIVE.test(line) && !ESLINT_NEXT_LINE.test(line)
          ? 'only eslint-disable-next-line <rule> -- <reason> is allowed (design §13.1 rule 9)'
          : undefined,
      )
    : [];

const RULES: Record<RuleId, Rule> = {
  'rust-allow-forbidden': rustAllowForbidden,
  'rust-allow-attribute': rustAllowAttribute,
  'todo-needs-issue': todoNeedsIssue,
  'removal-note': removalNote,
  'rust-commented-code': rustCommentedCode,
  'no-install-scripts': noInstallScripts,
  'no-http-engine-dep': noHttpEngineDep,
  'eslint-disable-reason': eslintDisableReason,
};

export const checkFile = (path: string, content: string, ctx: Context): Violation[] =>
  CHECKED_FILE.test(path) ? Object.values(RULES).flatMap((rule) => rule(path, content, ctx)) : [];
