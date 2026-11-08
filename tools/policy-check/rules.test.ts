import { expect, test } from 'vitest';
import { checkFile } from './rules.ts';

const v = (path: string, src: string) => checkFile(path, src, { packageVersion: '0.0.0' }).map((x) => x.rule);
test('flags allow/expect of forbidden lints in rust', () => {
  for (const a of ['#[allow(dead_code)]', '#![allow(deprecated)]', '#[allow(unused_imports)]', '#[expect(dead_code)]'])
    expect(v('crates/x/src/a.rs', `${a}\nfn f() {}`)).toContain('rust-allow-forbidden');
});
test('rust lint exceptions are expect with a reason, never allow in any attribute form', () => {
  const rust = (src: string) => checkFile('crates/x/src/a.rs', src, { packageVersion: '0.0.0' });
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
  expect(checkFile('a.rs', '// removal: 0.0.0 (#7)', { packageVersion: '0.1.0' }).map((x) => x.rule)).toEqual([
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
  expect(v('crates/engine-core/Cargo.toml', '[dependencies]\nhyper = "1"')).toEqual(['no-http-engine-dep']);
  expect(v('crates/engine-bench/Cargo.toml', '[dependencies]\nhyper = "1"')).toEqual([]);
});
test('dotted keys and package renames are matched', () => {
  const engine = (toml: string) => v('crates/engine-core/Cargo.toml', toml);
  expect(engine('[dependencies]\nhyper.workspace = true')).toEqual(['no-http-engine-dep']);
  expect(engine('[dependencies]\nreqwest.version = "0.12"')).toEqual(['no-http-engine-dep']);
  expect(engine('[dependencies]\nx = { package = "hyper" }')).toEqual(['no-http-engine-dep']);
  expect(engine('[dependencies.x]\npackage = "ureq"\nversion = "3"')).toEqual(['no-http-engine-dep']);
  expect(engine('[dependencies.x]\nversion = "3"')).toEqual([]);
});
test('reports file, line and message', () => {
  expect(checkFile('a.ts', 'x\n// TODO: y', { packageVersion: '0.0.0' })).toEqual([
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
  expect(v('crates/node-binding/Cargo.toml', '[dependencies.reqwest]\nversion = "0.12"')).toEqual([
    'no-http-engine-dep',
  ]);
  expect(v('crates/node-binding/Cargo.toml', '[dependencies]\ntokio = "1"\nh2 = { version = "0.4" }')).toEqual([
    'no-http-engine-dep',
  ]);
  expect(v('crates/node-binding/Cargo.toml', '[package]\nhyper = "1"')).toEqual([]);
});
test('dependency table state follows every header', () => {
  const engine = (toml: string) => v('crates/engine-core/Cargo.toml', toml);
  expect(engine('[dependencies]\ntokio = "1"\n[features]\nhyper = []')).toEqual([]);
  expect(engine('[dependencies.x]\nversion = "1"\n[dependencies]\nhyper = "1"')).toEqual(['no-http-engine-dep']);
});
/** Rules raised by `src` in a TS file, with the line of each. */
const ts = (src: string, path = 'a.ts') =>
  checkFile(path, src, { packageVersion: '0.0.0' }).map((x) => `${x.rule}@${String(x.line)}`);
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
