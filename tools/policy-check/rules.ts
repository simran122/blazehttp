export type RuleId =
  | 'rust-allow-forbidden'
  | 'todo-needs-issue'
  | 'removal-note'
  | 'rust-commented-code'
  | 'no-install-scripts'
  | 'no-http-engine-dep';

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

const noHttpEngineDep: Rule = (path, content) => {
  if (!ENGINE_CRATE_MANIFEST.test(path)) return [];
  let inDependencies = false; // inside a `[dependencies]`-style table
  let inDependencyTable = false; // inside a `[dependencies.<name>]` table
  return scanLines(path, content, 'no-http-engine-dep', (line) => {
    const header = DEPENDENCY_HEADER.exec(line.trim());
    if (header !== null || line.trim().startsWith('[')) {
      inDependencies = header !== null && header[1] === undefined;
      inDependencyTable = header?.[1] !== undefined;
      return engineMessage(header?.[1]);
    }
    if (inDependencyTable) return engineMessage(PACKAGE_RENAME.exec(line)?.[1]);
    if (!inDependencies) return undefined;
    return engineMessage(DEPENDENCY_KEY.exec(line)?.[1]) ?? engineMessage(PACKAGE_RENAME.exec(line)?.[1]);
  });
};

const RULES: Record<RuleId, Rule> = {
  'rust-allow-forbidden': rustAllowForbidden,
  'todo-needs-issue': todoNeedsIssue,
  'removal-note': removalNote,
  'rust-commented-code': rustCommentedCode,
  'no-install-scripts': noInstallScripts,
  'no-http-engine-dep': noHttpEngineDep,
};

export const checkFile = (path: string, content: string, ctx: Context): Violation[] =>
  CHECKED_FILE.test(path) ? Object.values(RULES).flatMap((rule) => rule(path, content, ctx)) : [];
