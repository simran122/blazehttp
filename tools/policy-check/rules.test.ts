import { expect, test } from 'vitest';
import { checkFile, type Context, workspaceLintSettings } from './rules.ts';

const WORKSPACE_TOML = `[workspace.lints.rust]
deprecated = "deny"

[workspace.lints.clippy]
# comment lines and spacing do not matter
pedantic = { level = "deny", priority = -1 }
allow_attributes =  "deny"
`;
const WORKSPACE = workspaceLintSettings('Cargo.toml', WORKSPACE_TOML);
if ('rule' in WORKSPACE) throw new Error(WORKSPACE.message);
const CTX: Context = { packageVersion: '0.0.0', workspaceLints: WORKSPACE.settings };

const v = (path: string, src: string) => checkFile(path, src, CTX).map((x) => x.rule);
/** Rules for a crate manifest that already inherits the workspace lints, so tests see only the rule under test. */
const crate = (path: string, toml: string) => v(path, `[lints]\nworkspace = true\n${toml}`);
test('flags allow/expect of forbidden lints in rust', () => {
  for (const a of ['#[allow(dead_code)]', '#![allow(deprecated)]', '#[allow(unused_imports)]', '#[expect(dead_code)]'])
    expect(v('crates/x/src/a.rs', `${a}\nfn f() {}`)).toContain('rust-allow-forbidden');
});
test('rust lint exceptions are expect with a reason, never allow in any attribute form', () => {
  const rust = (src: string) => checkFile('crates/x/src/a.rs', src, CTX);
  for (const a of [
    '#[allow(clippy::too_many_lines)]',
    '#![allow(clippy::pedantic, reason = "x")]',
    '#[cfg_attr(test, allow(clippy::unwrap_used))]',
    '#![ cfg_attr ( fuzzing , allow ( clippy::pedantic ) ) ]',
  ])
    expect(rust(`fn a() {}\n${a}\nfn f() {}`).map((x) => [x.rule, x.line])).toEqual([['rust-allow-attribute', 2]]);
  expect(rust('#[cfg_attr(\n    test,\n    allow(clippy::x, reason = "y")\n)]\nfn f() {}').map((x) => x.line)).toEqual([
    1,
  ]);
  expect(
    rust('#[expect(clippy::too_many_lines, reason = "x")]\n#[derive(Debug)]\n#[doc = "allowance"]\nfn f() {}'),
  ).toEqual([]);
});
test('TODO needs an issue link', () => {
  expect(v('a.ts', '// TODO: later')).toEqual(['todo-needs-issue']);
  expect(v('a.ts', '// TODO(#12): later')).toEqual([]);
});
test('removal note needs condition and issue; overdue versions fail', () => {
  expect(v('a.rs', '// removal: someday')).toEqual(['removal-note']);
  expect(v('a.rs', '// removal: P1a (#7)')).toEqual([]);
  expect(checkFile('a.rs', '// removal: 0.0.0 (#7)', { ...CTX, packageVersion: '0.1.0' }).map((x) => x.rule)).toEqual([
    'removal-note',
  ]);
});
test('commented-out rust code is rejected, doc comments are not', () => {
  expect(v('a.rs', '// let x = 1;')).toEqual(['rust-commented-code']);
  expect(v('a.rs', '/// let x = 1;\n//! use foo;')).toEqual([]);
});
test('install scripts are rejected', () => {
  expect(v('packages/blazehttp/package.json', JSON.stringify({ scripts: { postinstall: 'x' } }))).toEqual([
    'no-install-scripts',
  ]);
});
test('engine crates may not depend on HTTP engines', () => {
  expect(crate('crates/engine-core/Cargo.toml', '[dependencies]\nhyper = "1"')).toEqual(['no-http-engine-dep']);
  expect(crate('crates/engine-bench/Cargo.toml', '[dependencies]\nhyper = "1"')).toEqual([]);
});
test('dotted keys and package renames are matched', () => {
  const engine = (toml: string) => crate('crates/engine-core/Cargo.toml', toml);
  expect(engine('[dependencies]\nhyper.workspace = true')).toEqual(['no-http-engine-dep']);
  expect(engine('[dependencies]\nreqwest.version = "0.12"')).toEqual(['no-http-engine-dep']);
  expect(engine('[dependencies]\nx = { package = "hyper" }')).toEqual(['no-http-engine-dep']);
  expect(engine('[dependencies.x]\npackage = "ureq"\nversion = "3"')).toEqual(['no-http-engine-dep']);
  expect(engine('[dependencies.x]\nversion = "3"')).toEqual([]);
});
test('reports file, line and message', () => {
  expect(checkFile('a.ts', 'x\n// TODO: y', CTX)).toEqual([
    { file: 'a.ts', line: 2, rule: 'todo-needs-issue', message: expect.any(String) as string },
  ]);
});
test('non code/config files are ignored', () => {
  expect(v('docs/a.md', '// TODO: later')).toEqual([]);
});
test('invalid package.json and clean install scripts', () => {
  expect(v('package.json', '{')).toEqual([]);
  expect(v('package.json', JSON.stringify({ scripts: { build: 'x' } }))).toEqual([]);
  expect(v('package.json', '[]')).toEqual([]);
});
test('dependency table headers and inline tables are matched', () => {
  expect(crate('crates/node-binding/Cargo.toml', '[dependencies.reqwest]\nversion = "0.12"')).toEqual([
    'no-http-engine-dep',
  ]);
  expect(crate('crates/node-binding/Cargo.toml', '[dependencies]\ntokio = "1"\nh2 = { version = "0.4" }')).toEqual([
    'no-http-engine-dep',
  ]);
  expect(crate('crates/node-binding/Cargo.toml', '[package]\nhyper = "1"')).toEqual([]);
});
test('dependency table state follows every header', () => {
  const engine = (toml: string) => crate('crates/engine-core/Cargo.toml', toml);
  expect(engine('[dependencies]\ntokio = "1"\n[features]\nhyper = []')).toEqual([]);
  expect(engine('[dependencies.x]\nversion = "1"\n[dependencies]\nhyper = "1"')).toEqual(['no-http-engine-dep']);
});
/** Rules raised by `src` in a TS file, with the line of each. */
const ts = (src: string, path = 'a.ts') => checkFile(path, src, CTX).map((x) => `${x.rule}@${String(x.line)}`);
test('eslint disables are next-line only, with named rules and a reason', () => {
  expect(ts('x;\n// eslint-disable-next-line complexity -- generated table\nx;')).toEqual([]);
  expect(
    ts('  // eslint-disable-next-line @typescript-eslint/max-params, complexity -- callback shape', 'a.js'),
  ).toEqual([]);
  for (const bad of [
    '/* eslint-disable */',
    '/* eslint-disable complexity */',
    '/* eslint-enable */',
    '// eslint-disable complexity -- whole file',
    '// eslint-disable-line complexity -- same line',
    '// eslint-disable-next-line complexity',
    '// eslint-disable-next-line -- no rule',
    '// eslint-disable-next-line complexity --',
    '/* eslint-disable-next-line complexity -- block form */',
  ])
    expect(ts(`x;\n${bad}`), bad).toEqual(['eslint-directive@2']);
  expect(v('a.toml', '# eslint-disable')).toEqual([]);
});
test('eslint directives are found per comment, not per line', () => {
  expect(ts('x;\n/*\n eslint-disable no-debugger, complexity\n*/\nx;')).toEqual(['eslint-directive@2']);
  expect(ts('/* eslint no-debugger: off */')).toEqual(['eslint-directive@1']);
  expect(ts('/* eslint max-lines-per-function: ["error", 1000] */')).toEqual(['eslint-directive@1']);
  expect(ts('/* global window */\n/* globals a, b */')).toEqual(['eslint-directive@1', 'eslint-directive@2']);
  expect(ts('/* eslint-disable complexity */ // eslint-disable-next-line no-debugger -- r')).toEqual([
    'eslint-directive@1',
  ]);
  expect(ts('// eslint-disable-line x -- a // eslint-disable-next-line y -- b')).toEqual(['eslint-directive@1']);
  expect(ts('const re = /\\/\\/ eslint-disable/;\nconst t = `/* eslint-disable */${String(re)}`;')).toEqual([]);
});
test('only comments in ESLint directive grammar count: prose that starts with the same words passes', () => {
  for (const prose of [
    '// exported for tests',
    '// global state lives here',
    '// globals are frozen',
    '// eslint.config.js reads this',
    '// eslint-plugin-sonarjs is pinned',
    '/* eslint.config.js reads this */',
    '/* globalThis is used */',
    '/* exporter of things */',
  ])
    expect(ts(prose), prose).toEqual([]);
  for (const directive of [
    '// eslint-disable',
    '// eslint-enable complexity',
    '// eslint-disable-line',
    '/* eslint-env node */',
    '/* exported foo */',
    '/* global */',
    '/* eslint */',
  ])
    expect(ts(directive), directive).toEqual(['eslint-directive@1']);
});
test('every script extension is checked, JSX comments included and JSX text excluded', () => {
  for (const path of ['a.cjs', 'a.cts', 'a.jsx', 'a.tsx']) {
    expect(ts('/* eslint-disable */', path), path).toEqual(['eslint-directive@1']);
    expect(v(path, '// TODO: later'), path).toEqual(['todo-needs-issue']);
  }
  expect(ts('const j = <div>{/* eslint-disable z */}</div>;', 'a.tsx')).toEqual(['eslint-directive@1']);
  expect(ts('const j = <p>/* eslint-disable */ is text</p>;', 'a.jsx')).toEqual([]);
});
test('an unclosed rust attribute counts to the end of the file', () => {
  expect(v('a.rs', '#[allow(clippy::x')).toEqual(['rust-allow-attribute']);
});
test('rust attributes are read from code only: comments, strings and char literals are skipped', () => {
  for (const src of [
    '/// never write #[allow(x)]\nfn f() {}',
    '//! crate docs: #![allow(x)]\nfn f() {}',
    'fn f() { let s = "#[allow(x)]"; }',
    'fn f() { let s = r#"#[allow(x)] "quoted" "#; let b = b"#[allow(y)]"; }',
    'fn f() { let s = "escaped \\" #[allow(x)]"; }',
    '// use #[allow(x)]\nfn f() {}',
    '/* outer /* nested */ #[allow(x)] */\nfn f() {}',
    '#[doc = "see allow(x)"]\nfn f() {}',
    '#[deprecated(note = "allow(x) was removed")]\nfn f() {}',
    "fn f() { let c = '#'; let d = '\\''; let e = '['; }\nfn g() {}",
    "fn f<'a>(x: &'a str) -> &'a str { x }\n#[expect(clippy::x, reason = \"r\")]\nfn g() {}",
  ])
    expect(v('a.rs', src), src).not.toContain('rust-allow-attribute');
  for (const src of [
    '# ! [allow(clippy::doc_markdown)]\nfn f() {}',
    '#\n[allow(clippy::x)]\nfn f() {}',
    '#![cfg_attr(fuzzing, doc = "]", allow(clippy::x))]\nfn f() {}',
    "fn f<'a>(x: &'a str) {}\n#[allow(clippy::x)]\nfn g() {}",
    "fn f() { let c = '\"'; }\n#[allow(clippy::x)]\nfn g() {}",
  ])
    expect(v('a.rs', src), src).toContain('rust-allow-attribute');
});
test('blanked comments and literals keep line numbers; astral char literals and a trailing quote are read', () => {
  const lines = (src: string) => checkFile('a.rs', src, CTX).map((x) => `${x.rule}@${String(x.line)}`);
  expect(lines('/* a\n#[allow(x)]\n*/\n#[allow(y)]\nfn f() {}')).toEqual(['rust-allow-attribute@4']);
  expect(lines("fn f() { let c = '😀'; }\n#[allow(x)]\nfn g() {}")).toEqual(['rust-allow-attribute@2']);
  expect(lines("#[allow(x)]\nfn f<'")).toEqual(['rust-allow-attribute@1']);
});
test('unclosed rust comments and literals run to the end of the file', () => {
  for (const src of [
    '/* open #[allow(x)]',
    'fn f() { "open #[allow(x)]',
    "fn f() { '\\",
    'fn f() { r#"open #[allow(x)]',
  ])
    expect(v('a.rs', src), src).not.toContain('rust-allow-attribute');
});
test('every crate inherits the workspace lints', () => {
  expect(v('crates/x/Cargo.toml', '[package]\nname = "x"\n\n[lints]\nworkspace = true\n')).toEqual([]);
  for (const toml of ['[package]\nname = "x"\n', '[lints]\nworkspace = false\n', '[lints.clippy]\npedantic = "deny"\n'])
    expect(v('crates/x/Cargo.toml', toml), toml).toEqual(['crate-lints']);
  expect(v('Cargo.toml', '[package]\n')).toEqual([]);
});
test('crate lint inheritance is read in every TOML form', () => {
  for (const toml of [
    'lints.workspace = true\n[package]\nname = "x"\n',
    'lints = { workspace = true }\n[package]\nname = "x"\n',
    '[package]\nname = "x"\n[lints] # inherit\nworkspace=true # yes\n',
    '[package]\r\nname = "x # not a comment"\r\n[ lints ]\r\nworkspace = true\r\n',
  ])
    expect(v('crates/x/Cargo.toml', toml), toml).toEqual([]);
  for (const toml of [
    '[package]\ndescription = """\n[lints]\nworkspace = true\n"""\n',
    "[package]\ndescription = '''\n[lints]\nworkspace = true\n'''\n",
    '[package]\nlints.workspace = true\n',
    '[lints]\n# workspace = true\n',
  ])
    expect(v('crates/x/Cargo.toml', toml), toml).toEqual(['crate-lints']);
});
test('a fuzz crate repeats every workspace lint in its own tables', () => {
  const fuzz = (toml: string) => checkFile('crates/x/fuzz/Cargo.toml', toml, CTX).map((x) => x.message);
  const full =
    '[lints.rust]\ndeprecated = "deny"\n\n[lints.clippy]\npedantic = { level = "deny", priority = -1 }\nallow_attributes = "deny"\n';
  expect(fuzz(full)).toEqual([]);
  expect(fuzz(full.replace('allow_attributes = "deny"\n', ''))).toEqual([
    'standalone crate lacks workspace lint [lints.clippy] allow_attributes = "deny"',
  ]);
  expect(
    fuzz(
      '[lints.clippy]\ndeprecated = "deny"\npedantic = { level = "deny", priority = -1 }\nallow_attributes = "deny"\n',
    ),
  ).toEqual([
    'standalone crate lacks workspace lint [lints.rust] deprecated = "deny"',
    'standalone crate sets [lints.clippy] deprecated = "deny", which the workspace does not',
  ]);
});
test('a fuzz crate lint table equals the workspace: no extra or loosened lints', () => {
  const fuzz = (toml: string) => checkFile('crates/x/fuzz/Cargo.toml', toml, CTX).map((x) => x.message);
  const full =
    '[lints.rust]\ndeprecated = "deny"\n\n[lints.clippy]\npedantic = {level="deny",priority=-1}\nallow_attributes = "deny"\n';
  expect(fuzz(full)).toEqual([]);
  expect(fuzz(`${full}too_many_lines = "allow"\n`)).toEqual([
    'standalone crate sets [lints.clippy] too_many_lines = "allow", which the workspace does not',
  ]);
  expect(fuzz(full.replace('allow_attributes = "deny"', 'allow_attributes = "warn"'))).toEqual([
    'standalone crate sets [lints.clippy] allow_attributes = "warn"; the workspace sets "deny"',
  ]);
  expect(fuzz(`${full}# see """\ntoo_many_lines = "allow"\n# """\n`)).toEqual([
    'standalone crate sets [lints.clippy] too_many_lines = "allow", which the workspace does not',
  ]);
  expect(fuzz(`lints.workspace = true\n${full}`)).toEqual([
    'standalone crate sets [lints] workspace = true, which the workspace does not',
  ]);
  expect(
    fuzz(
      "['lints'.rust]\n'deprecated' = 'deny'\n[lints.'clippy']\npedantic = { level = 'deny', priority = -1 }\nallow_attributes = 'deny'\n",
    ),
  ).toEqual([]);
});
test('manifests are read by a real TOML parser: quotes in comments and literal keys behave as in Cargo', () => {
  expect(v('crates/x/Cargo.toml', '[package]\nname = "x" # quoting: """\n[lints]\nworkspace = true\n')).toEqual([]);
  expect(v('crates/x/Cargo.toml', "['lints']\n'workspace' = true\n")).toEqual([]);
  const broken = checkFile('crates/x/Cargo.toml', '[package]\nname = "x\n', CTX);
  expect(broken.map((x) => [x.rule, x.line])).toEqual([['crate-lints', 2]]);
  expect(broken[0]?.message).toMatch(/^cannot parse TOML: /);
});
test('a benchmark harness crate is a standalone crate too, at any depth under bench/', () => {
  const lacks = (path: string) => checkFile(path, '[package]\nname = "b"\n', CTX).map((x) => x.message);
  expect(lacks('bench/parsers/rust/Cargo.toml')).toContain(
    'standalone crate lacks workspace lint [lints.rust] deprecated = "deny"',
  );
  expect(lacks('bench/x/Cargo.toml')).toHaveLength(lacks('crates/x/fuzz/Cargo.toml').length);
  expect(lacks('bench/Cargo.toml')).toEqual([]);
});

test('a fuzz crate with no lint tables lacks every workspace lint', () => {
  expect(checkFile('crates/x/fuzz/Cargo.toml', '[package]\nname = "f"\n', CTX).map((x) => x.message)).toEqual([
    'standalone crate lacks workspace lint [lints.rust] deprecated = "deny"',
    'standalone crate lacks workspace lint [lints.clippy] pedantic.level = "deny"',
    'standalone crate lacks workspace lint [lints.clippy] pedantic.priority = -1',
    'standalone crate lacks workspace lint [lints.clippy] allow_attributes = "deny"',
  ]);
});
test('integers beyond 2^53 parse; they are compared as exact text', () => {
  const big = '[package.metadata.x]\nid = 9007199254740993\n';
  expect(v('crates/x/Cargo.toml', `${big}[lints]\nworkspace = true\n`)).toEqual([]);
  const root = workspaceLintSettings(
    'Cargo.toml',
    '[workspace.lints.clippy]\nx = { level = "deny", priority = 9007199254740993 }\n',
  );
  expect('settings' in root && [...root.settings]).toEqual([
    ['clippy.x.level', '"deny"'],
    ['clippy.x.priority', '"9007199254740993"'],
  ]);
});
test('a root manifest that does not parse is a violation naming the file', () => {
  expect(workspaceLintSettings('Cargo.toml', '[workspace]\nmembers = [\n')).toEqual({
    file: 'Cargo.toml',
    line: 3,
    rule: 'crate-lints',
    message: expect.stringMatching(/^cannot parse TOML: /) as string,
  });
});
