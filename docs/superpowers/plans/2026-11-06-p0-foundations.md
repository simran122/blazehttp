# blazehttp P0 Foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver design phase P0 (weeks 1–3): the monorepo with CI on 8 targets, packaging dry-run, `spec.json` codegen, the vector import pipeline, G0/G1/leaderboard harness skeletons, the Rust G0 competitors, the quick cross-language baseline, a minimal own HTTP/1.1 engine prototype driven from Rust and from Node, and the measurements that settle the open P0 items (design §22). P0 is accepted when the prototype engine is ≥ hyper `client::conn` in the G0 plaintext reference cell.

**Architecture:** A Cargo + pnpm monorepo. `crates/engine-core` holds the prototype engine: a strict scalar response parser, a request writer, and one shard with connection tasks on a `current_thread` runtime plus `LocalSet`. Completions leave through `trait CompletionSink`. Two crates implement that trait: `crates/node-binding` (napi-rs, TSFN wake, two swapped completion `Vec`s) and `crates/engine-bench` (a channel on the harness runtime, so G0 measures the same engine path Node uses). `crates/bench-server` is the fixed-response test server. `bench/harness` (TypeScript) runs every benchmark in Docker with CPU caps, netem, idle-host checks and bootstrap statistics. `packages/blazehttp` gets the native loader and the seed of `NativeSession`. Everything else in the design starts in P1a or later.

**Tech Stack:** Rust (edition 2024, stable), tokio, napi-rs 3 (`napi`, `napi-derive`, `napi-build`, `@napi-rs/cli`), TypeScript (strict), Node 26 for development (package floor ≥ 22.19), pnpm workspaces, Vitest, cargo-nextest, cargo-fuzz, cargo-deny, cargo-audit, cargo-shear, cargo-hack, cargo-llvm-cov, tsdown, knip, typescript-eslint, Prettier, publint, `@arethetypeswrong/cli`, Docker, GitHub Actions.

**Spec:** `docs/specs/2026-11-06-blazehttp-design.md` (v6). Executors read the design and this plan. Section references (§n) point into the design.

## Global Constraints

Every task must also meet these. Values are copied from the design.

- Rust **edition 2024**, stable toolchain only, pinned in `rust-toolchain.toml` to the stable release that is current when Task 1 runs (MSRV = that release, recorded in `docs/notes/toolchain.md`). Nightly is allowed only for Miri and `cargo-fuzz`, on a pinned nightly date (§6.3). The local machine has rustc 1.91.1, which is too old: run `rustup update stable` first.
- `engine-core` declares `#![forbid(unsafe_code)]`. `unsafe` is allowed only in `engine-simd` (P1a) and `node-binding`, and every block needs a `// SAFETY:` comment (G7).
- Lints come from the workspace (`[workspace.lints]`, and every crate sets `[lints] workspace = true`): `deprecated = "deny"`, clippy `pedantic` deny, `clippy::await_holding_refcell_ref` deny. CI runs with `RUSTFLAGS=-D warnings`. These attributes are forbidden anywhere and the policy check enforces it: `#[allow(deprecated)]`, `#[allow(dead_code)]`, `#[allow(unused…)]`, and the same names inside `expect(...)` (§14.1–14.2).
- Release profile: `lto = "fat"`, `codegen-units = 1`, `panic = "unwind"`. `node-binding` asserts `panic = "unwind"` at compile time (§11.3).
- Never use an HTTP engine as the engine. hyper, reqwest, ureq, isahc, h2 and undici are allowed only as **benchmark competitors** inside `engine-bench` and `bench/`. `engine-core` and `node-binding` may depend only on infrastructure crates: tokio, rustls/tokio-rustls, decoders, `url`/`idna`, `memchr`, `parking_lot`, `mimalloc`, napi-rs/napi-sys (§14).
- Nothing deprecated anywhere. That covers `@typescript-eslint/no-deprecated` as an error, tests run with `--throw-deprecation`, no Node `DEP*` APIs, and only documented APIs (`buf.write(str, off, 'latin1')`, not `latin1Write`). CI fails on a deprecated npm dependency. Actions stay on current majors (§14.1).
- No unused code. `knip`, `noUnusedLocals`, `noUnusedParameters`, `cargo-shear` and `cargo hack --each-feature` must all be clean. Every `TODO`/`FIXME` links an issue (`TODO(#123):`). Temporary code carries `removal: <version|condition> (#issue)`. Commented-out code is rejected (§14.2).
- TypeScript: `strict`, no `any`, lookup maps instead of `switch`, and only `Error` may be extended. Rust: exhaustive `enum` + `match`, no `Box<dyn …>` on the hot path (§13).
- No `postinstall`. `@blazehttp/<target>` packages are exact-pinned `optionalDependencies`. Build with `napi build --no-js` plus our own loader with a SHA-256 integrity check (§12).
- Licence is `MIT OR Apache-2.0`. Ported vectors keep their copyright notice in the vector file and in `THIRD_PARTY_NOTICES`. No third-party code goes into the npm packages (§6.1).
- Coverage: ≥ 95% for TS and ≥ 90% for Rust (G7).
- The 8 targets: `darwin-arm64`, `darwin-x64`, `linux-x64-gnu`, `linux-x64-musl`, `linux-arm64-gnu`, `linux-arm64-musl`, `win32-x64-msvc`, `win32-arm64-msvc`. CI runners: `macos-15`, `macos-15-intel`, `ubuntu-24.04`, `ubuntu-24.04-arm` (musl through `docker run` on Alpine), `windows-2025`, `windows-11-arm` (§14).
- Node floor is `>=22.19` (`engines`). Benchmarks run on Node 24 LTS and Node 26 (§16.1).
- Every queue, buffer and cache is bounded. When a bound is hit the request fails fast with a typed error and never blocks (G6).
- Benchmark host rules:
  - The arm64 reference host is the owner's Mac with Docker Desktop at 8 CPUs.
  - Gate runs happen **only inside the nightly window**, and the owner keeps other containers stopped then. **Never stop the owner's containers.** The harness aborts if a container it did not start is running.
  - The G0 server gets ≥ 4 CPUs and the client gets 1 CPU. A run is discarded when server CPU is ≥ 60% or the host is not idle (§16.1–16.2).
- Statistics: ratio of medians over **10 rounds**, with a bootstrap 95% CI. G0 engine-bound cells need the CI lower bound ≥ 1.0× (stretch 1.15×). The reference cell needs a median ≥ 63,790 req/s **and** a CI lower bound ≥ 63,790 × 0.97 (§16.4).
- Git (repo rule, which overrides the global one-step rule): Claude commits, pushes `p0/<slug>` branches and opens PRs into `milestone/p0`. A PR merges after CI is green and the agent review passes. The owner approves the `milestone/p0` → `main` PR. Commit messages are imperative and never mention Claude, AI or agents.
- Library APIs come from Context7 (`library-docs-context7`) or docs.rs for the exact version locked in `Cargo.lock`/the lockfile. Rust claims in design notes cite doc.rust-lang.org first (§6.3).

## Review Focus

These are the inputs and conditions most likely to hurt users that no single task's main tests exercise. Each one has a test in the task that owns the code.

1. **A response split across reads at any byte boundary**: 1-byte reads, CRLF split between reads, a head split from its body. The result must equal the single-read result. → Task 4 `every_vector_is_split_invariant`.
2. **The server closes an idle keep-alive connection, or resets mid-body.** The request must settle with `NETWORK/closed-before-response` or `PROTOCOL/incomplete-body`, never hang, and the next request must get a fresh connection. → Task 6 `server_closed_idle_connection_settles_with_network_error`, `eof_mid_body_settles_incomplete_body`.
3. **More requests submitted than the shard queue holds.** They must fail fast with `QUEUE_FULL` `{ scope: 'global', bound }`, never block the main thread, and never grow memory without bound. → Task 6 `queue_full_fails_fast`, Task 12 `queue_full_is_a_completion_not_a_throw`.
4. **The native binary is missing, built for another libc, tampered with, or the platform is unsupported.** Loading must return a typed result and must not crash. → Task 13 `tampered_binary_returns_integrity`, `missing_package_returns_binary_load`, `unsupported_platform`. Task 15 Alpine job `glibc_binary_on_musl_returns_binary_load`.
5. **Process lifetime.** A Node process with no pending requests must exit on its own, and one with pending requests must stay alive until they settle. → Task 12 `process_exits_when_idle`, `process_stays_alive_while_pending`.

## Process rules added by the owner during execution (2026-11-06)

These apply to every task from Task 4 on and override anything above that conflicts.

- **Research gate before engine code.** Tasks 4, 5, 6, 12 and 13, and every engine component in later phases, start with a research step before any code is written:
  1. A research agent writes `docs/notes/<component>.md`, covering:
     - 2–3 reference implementations at their **latest published release**: what makes them fast, and what we should not copy;
     - their bug trackers, CVEs and advisories;
     - the fastest cross-language techniques, each with its expected gain and how to measure it;
     - official docs at the locked versions (doc.rust-lang.org first, §6.3);
     - an **online search** of engineering blogs, security research (request smuggling and desync), benchmarks, talks and papers, and upstream issue threads.

     Every source is linked with its access date. Each idea is marked **verified** (checked against docs, source code or a measurement) or **unverified**; unverified ideas go only to `docs/perf/ideas.md`. The note compares at least two design options (time and space complexity, allocations, risks) and states the decision and how it will be measured.
  2. A reviewer checks the note, and gaps are fixed in the note.
  3. The implementer builds what the note decided, and the code reviewers check that the code follows it.
- **Review setup.**
  - The first review runs two reviewers in parallel:
    - the `code-review` agent, which follows `~/.claude/rules/code-review.md` and returns the coverage table;
    - the built-in `/code-review` at high effort on the task PR.
  - Tasks 4, 6, 12 and 13 also get a `security-audit` pass.
  - All findings, minor ones included, are merged into **one** fix round. After it, a verification pass checks every finding (FIXED / NOT FIXED) and applies the full checklist to the fix diff.
  - **Review until clean:** if the verification pass finds anything, of any severity, including problems the first review missed, the implementer fixes it and another verification pass runs. A task merges only after a pass reports zero findings. If a fifth pass still finds problems, the controller reports to the owner instead of continuing silently.
  - Implementers check their own diff against the same checklist before reporting.
- **Agents** run on Opus at high effort.
- **Upstream test vectors** come from the latest published, non-prerelease release **source** tag. The exact commit is recorded, and `--refresh` re-imports at the start of each phase.
- **Code structure:** every task follows design §13.1, "production-grade code structure", the owner rule: organise by component; one responsibility per file, split past about 500 lines; narrow public APIs; unit tests with their code; matching component names across tests, fuzz, bench, vectors and notes; fixed top level; one-way dependencies. Reviewers check it as part of the maintainability category.
- **One function, one job** (design §13.1 rule 9): each function does one thing that its name states, in at most 60 lines. CI enforces this through clippy `too_many_lines`/`too_many_arguments` (`clippy.toml`) and ESLint `max-lines-per-function`/`complexity`/`max-depth`/`max-params`. Any exception needs a reviewed `#[expect]` or `eslint-disable-next-line` with a reason.
- **OOP and design-pattern hard rules** (design §13.2): encapsulation, interfaces at seams, static dispatch on hot paths, composition over inheritance, SOLID, and GoF patterns only as listed in the §13 table. Reviewers check them under maintainability, and a violation is an Important finding.
- **Owner decisions applied to the design:**
  - the HTTP `QUERY` method is wire id 7, and is safe and idempotent (§5.1, §9.3, §21);
  - a status line with no reason phrase is accepted (§5.2);
  - `Content-Length` is `1*DIGIT` after OWS trimming (§5.2).

---

## Repository layout after P0

```
Cargo.toml  rust-toolchain.toml  rustfmt.toml  clippy.toml  deny.toml
package.json  pnpm-workspace.yaml  tsconfig.base.json  eslint.config.js  .prettierrc.json  knip.json  vitest.config.ts
LICENSE-MIT  LICENSE-APACHE  THIRD_PARTY_NOTICES  README.md
policy/spec.json                          shared data (§9.2), single source of truth
tools/specgen/                            spec.json → TS + Rust
tools/policy-check/                       repo rules (§14.1–14.2)
tools/vectors/                            vector schema, importers, notices generator
tools/pack/                               integrity hashes, packing, placeholders
tests/vectors/index.json                  vector file → consuming suites
tests/vectors/h1-response/*.json
crates/engine-core/                       prototype engine (parser, writer, shard, connection, sink trait)
crates/engine-core/fuzz/                  cargo-fuzz (own workspace)
crates/node-binding/                      napi bridge
crates/engine-bench/                      G0 harness + Rust competitors
crates/bench-server/                      fixed-response server (lib + bin)
bench/harness/                            Docker orchestration, stats, validity, reports (TS)
bench/g1/                                 Node-side G1 client runner
bench/xlang/{c,cpp,zig,go}/               cross-language baseline clients
bench/docker/                             Dockerfiles
bench/results/                            committed gate-run JSONL
packages/blazehttp/                       main npm package (loader, NativeSession seed, generated spec)
npm/<target>/                             8 platform packages
docs/notes/  docs/perf/ideas.md  docs/perf/journal.md  docs/reports/p0.md
.github/workflows/{ci.yml,release.yml,measure.yml}
```

`spike/` is deleted in Task 14, after `bench/` reproduces the §1 numbers.

## Schedule and parallel tracks

| Week | Engine track | Bench track | Platform track |
|---|---|---|---|
| 1 | T4 parser, T5 writer | T9 bench-server, T10 harness | T1 bootstrap, T2 specgen, T3 vectors |
| 2 | T6 shard + connection, T12 binding | T7, T8 engine-bench | T13 loader + session, T15 CI matrix |
| 3 | T17 wake latency, T18–T19 measurements | T11 xlang baseline, T14 G1 + spike retirement | T16 packaging, T20 gate + review |

Dependencies: T1 → T2 → {T3, T4, T9}. T4 → T5 → T6. T6 → {T7, T12}. T7 → T8. T9 → {T7, T10}. T12 → T13 → {T14, T15, T16}. {T10, T11, T14} → {T17, T18, T19} → T20. T11 must land before T14 (spike deletion).

**Schedule deviation:** §9.4 asks for the 2-hop wake measurement in week 1. Measuring it needs T6 + T12 + T13, so it lands at the start of week 2 (T17 starts as soon as T13 merges). This must be reported to the owner in the P0 report.

**Owner checkpoints:**
- Task 1: confirm the GitHub account and that the repo should be public before `gh repo create`.
- Task 16: the owner publishes the `0.0.0` placeholders and configures OIDC trusted publishers.
- Tasks 17–20: gate runs need the nightly window, which the owner has not chosen yet. Until then, runs use `--dev` and do not count as gate evidence.
- Task 20: x64 VM access, the decision on `io_uring` funding (if triggered), and approval of the milestone PR.

---

### Task 1: Repository bootstrap and policy checks

**Files:**
- Create: `.gitignore`, `LICENSE-MIT`, `LICENSE-APACHE`, `README.md` (one paragraph plus a "status: pre-release" line), `package.json` (private root; scripts `lint`, `typecheck`, `test`, `knip`, `policy`, `specgen`), `pnpm-workspace.yaml` (`packages/*`, `npm/*`, `bench/*`, `tools/*`), `tsconfig.base.json`, `eslint.config.js`, `.prettierrc.json`, `knip.json`, `vitest.config.ts`, `rust-toolchain.toml`, `rustfmt.toml`, `deny.toml`
- Create: `tools/policy-check/rules.ts`, `tools/policy-check/check.ts`, `tools/policy-check/npm-deprecated.ts`, `tools/policy-check/rules.test.ts`
- Create: `docs/perf/ideas.md`, `docs/perf/journal.md` (entry template: date, idea, hypothesis, method, numbers with CI, kept/dropped, commit), `docs/notes/toolchain.md`
- Create: `.github/workflows/ci.yml` (jobs `policy` and `ts` on `ubuntu-24.04`; Task 15 extends it)

**Interfaces:**
- Produces: `type Violation = { file: string; line: number; rule: RuleId; message: string }`
- Produces: `type RuleId = 'rust-allow-forbidden' | 'todo-needs-issue' | 'removal-note' | 'rust-commented-code' | 'no-install-scripts' | 'no-http-engine-dep'`
- Produces: `function checkFile(path: string, content: string, ctx: { packageVersion: string }): Violation[]` in `rules.ts`
- Produces: CLI `pnpm policy`, which walks git-tracked files and exits 1 on any violation

- [ ] **Step 1: Prepare the toolchain and the repository**

  1. Run `rustup update stable`.
  2. Pin the resulting version in `rust-toolchain.toml` (`channel = "<x.y.z>"`, components `rustfmt`, `clippy`, `llvm-tools-preview`) and record it in `docs/notes/toolchain.md` with a citation of doc.rust-lang.org/edition-guide.
  3. Run `git init -b main`.
  4. Make the first commit with only the existing files (`.claude/CLAUDE.md`, `docs/specs/`, `docs/superpowers/`, `spike/`). Message: `Add design spec, P0 plan and benchmark spike`.

- [ ] **Step 2: Owner checkpoint, then create the remote**

  1. Run `gh api user -q .login`. Show the login to the owner and ask: "Create public repo `<login>/blazehttp`?"
  2. Once the owner confirms, run `gh repo create <login>/blazehttp --public --source . --push`.
  3. Run `git checkout -b milestone/p0 && git push -u origin milestone/p0`.
  4. Protect `main`: no direct push, no force push, PR required (`gh api -X PUT repos/<login>/blazehttp/branches/main/protection`). Required checks are added in Task 15.
  5. Run `git checkout -b p0/bootstrap milestone/p0`.

- [ ] **Step 3: Write the failing rule tests** in `tools/policy-check/rules.test.ts`

```ts
const v = (path: string, src: string) => checkFile(path, src, { packageVersion: '0.0.0' }).map((x) => x.rule);
test('flags allow/expect of forbidden lints in rust', () => {
  for (const a of ['#[allow(dead_code)]', '#![allow(deprecated)]', '#[allow(unused_imports)]', '#[expect(dead_code)]'])
    expect(v('crates/x/src/a.rs', `${a}\nfn f() {}`)).toEqual(['rust-allow-forbidden']);
});
test('TODO needs an issue link', () => {
  expect(v('a.ts', '// TODO: later')).toEqual(['todo-needs-issue']);
  expect(v('a.ts', '// TODO(#12): later')).toEqual([]);
});
test('removal note needs condition and issue; overdue versions fail', () => {
  expect(v('a.rs', '// removal: someday')).toEqual(['removal-note']);
  expect(v('a.rs', '// removal: P1a (#7)')).toEqual([]);
  expect(checkFile('a.rs', '// removal: 0.0.0 (#7)', { packageVersion: '0.1.0' }).map((x) => x.rule)).toEqual(['removal-note']);
});
test('commented-out rust code is rejected, doc comments are not', () => {
  expect(v('a.rs', '// let x = 1;')).toEqual(['rust-commented-code']);
  expect(v('a.rs', '/// let x = 1;\n//! use foo;')).toEqual([]);
});
test('install scripts are rejected', () => {
  expect(v('packages/blazehttp/package.json', JSON.stringify({ scripts: { postinstall: 'x' } }))).toEqual(['no-install-scripts']);
});
test('engine crates may not depend on HTTP engines', () => {
  expect(v('crates/engine-core/Cargo.toml', '[dependencies]\nhyper = "1"')).toEqual(['no-http-engine-dep']);
  expect(v('crates/engine-bench/Cargo.toml', '[dependencies]\nhyper = "1"')).toEqual([]);
});
```

- [ ] **Step 4: Run** `pnpm vitest run tools/policy-check`. Expected: FAIL, because `checkFile` is not defined yet.

- [ ] **Step 5: Implement `checkFile` and the CLI**

  - Implement it as a `Record<RuleId, (path, content, ctx) => Violation[]>` lookup map with no `switch`.
  - Install scripts are `preinstall`, `install` and `postinstall`.
  - `no-http-engine-dep` applies to `crates/engine-core/**` and `crates/node-binding/**`. It matches the crate names `hyper`, `hyper-util`, `reqwest`, `ureq`, `isahc` and `h2`.
  - `npm-deprecated.ts` runs `pnpm list --prod --json --depth Infinity` and then `npm view <name>@<version> deprecated` for each package. Any non-empty output fails. It also fails a package with no release in 24 months unless the package is listed in `tools/policy-check/reviewed.json` with a reason.

- [ ] **Step 6: Configure the TS toolchain**

  - `tsconfig.base.json`: `strict`, `noUnusedLocals`, `noUnusedParameters`, `exactOptionalPropertyTypes`, `noUncheckedIndexedAccess`, `verbatimModuleSyntax`, `module`/`moduleResolution` `nodenext`.
  - `eslint.config.js`: typescript-eslint `strictTypeChecked`, `@typescript-eslint/no-deprecated: error`, `no-unused-vars` and `no-unused-expressions` as errors, `sonarjs/no-commented-code: error`, and `no-restricted-syntax` banning `SwitchStatement`.
  - `vitest.config.ts`: Vitest `projects`, `execArgv: ['--throw-deprecation']`, and v8 coverage thresholds of 95 for lines, branches, functions and statements.
  - `knip.json`: entries for each workspace, including test files and `bench/**`.
  - Check every config key against the current docs through Context7.

- [ ] **Step 7: Run** `pnpm vitest run tools/policy-check && pnpm lint && pnpm typecheck && pnpm knip && pnpm policy`. Expected: all pass.

- [ ] **Step 8: Add the `policy` and `ts` CI jobs** to `ci.yml`. Use `pnpm/action-setup` and `actions/setup-node` on current majors, pinned by commit SHA. Node 26. The jobs run `pnpm install --frozen-lockfile`, lint, typecheck, knip, policy, `npm-deprecated`, and vitest with coverage.

- [ ] **Step 9: Commit and open the PR**

```bash
git add -A && git commit -m "Bootstrap monorepo tooling and repository policy checks"
git push -u origin p0/bootstrap && gh pr create --base milestone/p0 --fill
```

---

### Task 2: `spec.json` and code generation

**Files:**
- Create: `policy/spec.json`, `tools/specgen/spec-types.ts`, `tools/specgen/gen.ts`, `tools/specgen/gen.test.ts`, `tools/specgen/fixtures/mini-spec.json`
- Create (generated, committed): `packages/blazehttp/src/generated/spec.ts`, `crates/engine-core/src/generated/spec.rs`
- Create: `Cargo.toml` (workspace: `members = ["crates/*"]`, `exclude = ["crates/engine-core/fuzz"]`, `resolver = "3"`, `[workspace.package] edition = "2024"`, `license = "MIT OR Apache-2.0"`, `publish = false`, `[workspace.lints]` per Global Constraints, `[profile.release]` per Global Constraints)
- Create: `crates/engine-core/Cargo.toml`, `crates/engine-core/src/lib.rs` (`#![forbid(unsafe_code)]`, `pub mod generated;`)
- Create: `packages/blazehttp/package.json` (`name: blazehttp`, `version: 0.0.0`, `type: module`, `engines.node: ">=22.19"`, `license`, `exports` filled in Task 16), `packages/blazehttp/tsconfig.json`, `packages/blazehttp/src/index.ts` (empty public surface: `export {}` until P2)
- Test: `tools/specgen/gen.test.ts`, `crates/engine-core/tests/spec_wire.rs`

**`policy/spec.json` contents in P0.** Include only the sections P0 code consumes. Later phases add the rest (§14.2).

- `errors`: an ordered array, where **array index = wire code id**, append-only. It holds all 15 codes from §8 with their details, verbatim:
  - Codes whose details are string lists: `{ "code": "TIMEOUT", "details": ["connect","headers","idleBody","idleSend","total","queue","stall","slow-body"] }`.
  - Codes with structured fields: `HTTP_STATUS {status}`, `TOO_MANY_REDIRECTS {count}`, `SSRF_BLOCKED {address, rule}`, `QUEUE_FULL {scope: ["client","origin","bytes","contexts","global"], bound}`, `INVALID_CONFIG {field, reason}`. They use `"fields": { name: "u32" | "string" | [enum…] }`.
  - Codes that never cross the wire carry `"wire": false`: `HTTP_STATUS`, `INVALID_CONFIG`, `SSRF_BLOCKED` (string fields; P1b extends the wire), and `PARSE`.
- `limits.h1`: `maxHeadBytes: 65536`, `maxStatusLineBytes: 8192`, `maxHeaderFields: 100`, `maxInterimResponses: 10`.
- `methods`: `["GET","HEAD","POST","PUT","DELETE","PATCH","OPTIONS"]`. Index = wire id. Generated as the TS `type Method` and the Rust `pub enum Method` with `from_wire`/`to_wire`. `CONNECT` is never representable (§12).
- `wire`: record layouts in u32 words, added in Task 12.

**Interfaces:**
- Produces (TS, generated):
  - `export type HttpErrorCode = 'TIMEOUT' | … (15)`
  - `export interface ErrorDetailMap { TIMEOUT: 'connect' | …; QUEUE_FULL: { scope: 'client' | … ; bound: number }; … }`
  - `export const ERROR_WIRE: readonly { code: HttpErrorCode; details: readonly string[] | undefined; fields: readonly string[] | undefined }[]`
  - `export const H1_LIMITS: { maxHeadBytes: 65536; … }` as const
- Produces (Rust, generated, in `engine_core::generated::spec`):
  - `pub enum ErrorCode { Timeout(TimeoutDetail), Aborted(AbortedDetail), Network(NetworkDetail), Tls(TlsDetail), Protocol(ProtocolDetail), TooLarge(TooLargeDetail), TooManyRedirects { count: u32 }, RedirectRejected(RedirectRejectedDetail), Proxy(ProxyDetail), QueueFull { scope: QueueScope, bound: u32 }, Engine(EngineDetail) }`. Only wire codes appear here.
  - Detail enums are `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`.
  - `impl ErrorCode { pub fn to_wire(self) -> (u32, u32); pub fn from_wire(code: u32, arg: u32) -> Option<Self> }`. Word 1 is `code_id | detail_or_enum_id << 8`, and word 2 is the numeric field (`bound`, `count`).
  - `pub const H1_LIMITS: H1Limits`
- CLI: `pnpm specgen` writes the files, and `pnpm specgen --check` exits 1 if the committed output differs. CI runs `--check`.

- [ ] **Step 1: Write the failing tests**
  - `gen.test.ts`:
    - `generates TS and Rust for the mini fixture`: inline snapshot of both outputs for a fixture with one detail code and one field code.
    - `rejects string fields on wire codes`: throws `/wire: false/`.
    - `rejects duplicate codes and empty detail lists`.
    - `check mode reports drift`: compares against a modified string and returns `{ ok: false, file }`.
  - `spec_wire.rs`:
    - `every_wire_code_round_trips`: iterates a generated `ErrorCode::ALL_SAMPLES: &[ErrorCode]` (one per variant/detail) and asserts `from_wire(to_wire(e)) == Some(e)`.
    - `queue_full_keeps_bound`.
- [ ] **Step 2: Run** `pnpm vitest run tools/specgen && cargo test -p engine-core --test spec_wire`. Expected: FAIL.
- [ ] **Step 3: Implement `generate(spec: Spec): { ts: string; rust: string }`** and the CLI in `gen.ts`.
  - Validate `spec.json` against `spec-types.ts` at load. Errors name the JSON path.
  - Write Rust identifiers in PascalCase, mapping `slow-body` to `SlowBody` and `dns/notfound` to `DnsNotfound`. Rust needs no `rustfmt` pass, because the generator emits formatted code. CI checks this with `cargo fmt --check`.
- [ ] **Step 4: Run** `pnpm specgen && pnpm vitest run tools/specgen && cargo test -p engine-core && cargo clippy --workspace -- -D warnings && pnpm specgen --check`. Expected: PASS.
- [ ] **Step 5: Commit** `Add shared policy spec with TypeScript and Rust code generation` and open a PR to `milestone/p0`.

---

### Task 3: Test-vector format, loaders, llhttp importer and notices

**Files:**
- Create: `tools/vectors/schema.ts`, `tools/vectors/import-llhttp.ts`, `tools/vectors/notices.ts`, `tools/vectors/check-index.ts`, `tools/vectors/*.test.ts`, `tools/vectors/overrides/llhttp.json`
- Create: `tests/vectors/index.json`, `tests/vectors/h1-response/llhttp-{sample,content-length,connection,invalid}.json`
- Create: `crates/engine-core/tests/support/vectors.rs` (Rust loader; `serde`/`serde_json` as dev-dependencies)
- Create: `THIRD_PARTY_NOTICES` (generated by `notices.ts`), `docs/notes/vectors.md`

**Vector file format** (documented in `docs/notes/vectors.md`):

```json
{
  "source": { "project": "llhttp", "url": "https://github.com/nodejs/llhttp/blob/<sha>/test/response/content-length.md",
              "commit": "<sha>", "licence": "MIT", "copyright": "<verbatim from upstream LICENSE>" },
  "cases": [{
    "name": "content-length / multiple", "input": "HTTP/1.1 200 OK\r\n…", "inputEncoding": "utf8",
    "request": { "method": "GET" },
    "expect": { "native": { "ok": { "status": 200, "headers": [["content-length", "5"]], "body": "hello", "keepAlive": true } } },
    "reference": { "llhttp": "error HPE_UNEXPECTED_CONTENT_LENGTH" }
  }]
}
```

- `expect.native` is either `{ ok: … }` or `{ error: "<CODE>/<detail>" }`.
- `expect.undici` is optional in P0 and **required from P4** (`// removal: P4 (#issue)` on the optionality in `schema.ts`).
- `tests/vectors/index.json` maps each file to the suites that consume it, e.g. `{ "h1-response/llhttp-sample.json": ["engine-core:h1_response"] }`.

**Interfaces:**
- Produces (TS): `loadVectorFile(path: string): VectorFile`, and `checkIndex(root: string): string[]`, which returns unreferenced or missing files.
- Produces (Rust test support): `pub fn load_suite(suite: &str) -> Vec<(String /*file*/, Case)>`, which reads `index.json` and panics on an unregistered file under the suite's directory. Also `pub struct Case { name, input: Vec<u8>, method: String, expect: Expect }` and `pub enum Expect { Ok { status: u16, headers: Vec<(String, String)>, body: Vec<u8>, keep_alive: bool }, Error(String) }`.

- [ ] **Step 1: Pin llhttp and read its harness.**
  1. Choose the latest llhttp release tag and record its commit SHA.
  2. Read its `test/md-test.ts` (or the current equivalent) at that SHA so the importer reproduces its input normalisation **exactly**: line-ending conversion and escape handling. Write the findings into `docs/notes/vectors.md` with links.
  3. Copy the copyright line verbatim from `LICENSE-MIT` at that SHA.
- [ ] **Step 2: Write the failing tests**
  - `import-llhttp.test.ts` covers a three-case inline markdown fixture:
    - `converts http/log block pairs into cases`
    - `maps llhttp errors to native expectations`: any `HPE_*` → `PROTOCOL/malformed-response`, except `HPE_UNEXPECTED_CONTENT_LENGTH` and `HPE_INVALID_TRANSFER_ENCODING` → `PROTOCOL/smuggling-risk`. (As built: the `HPE_HEADER_OVERFLOW` mapping was removed because llhttp never emits it, and llhttp reports duplicate CL as `HPE_UNEXPECTED_CONTENT_LENGTH`.)
    - `applies overrides with a cited rule`: an override entry without `rule: "§5.2 …"` throws.
  - `notices.test.ts`: `one notice block per distinct (project, licence, copyright)`.
  - `check-index.test.ts`: `reports unreferenced and missing files`.
- [ ] **Step 3: Run** `pnpm vitest run tools/vectors`. Expected: FAIL.
- [ ] **Step 4: Implement the schema, importer, notices and check-index.**
  - The importer downloads at the pinned SHA from `raw.githubusercontent.com` and writes the four P0 files. `transfer-encoding.md` waits for P1a, together with chunked decoding.
  - It also writes the llhttp log as `reference`.
  - It applies `overrides/llhttp.json`, where §5.2 makes us stricter than llhttp.
- [ ] **Step 5: Review every imported case where llhttp *accepts*** against the §5.2 rules. Add an override with the cited rule wherever ours must reject: bare LF, obs-fold, a 101 status, whitespace before the colon, and so on. List the overridden case names in `docs/notes/vectors.md`.
- [ ] **Step 6: Run** `pnpm vitest run tools/vectors && node tools/vectors/check-index.ts && node tools/vectors/notices.ts --check`. Expected: PASS. The vector files are consumed in Task 4, so `check-index` lists them under `engine-core:h1_response`.
- [ ] **Step 7: Commit** `Add test-vector format, llhttp importer and third-party notices` and open a PR.

---

### Task 4: `ResponseParser` (scalar, strict §5.2 head rules)

**Files:**
- Create: `crates/engine-core/src/h1/mod.rs`, `crates/engine-core/src/h1/parser.rs`
- Create: `tests/vectors/h1-response/blazehttp-rules.json` (source project `blazehttp`, licence `MIT OR Apache-2.0`), registered in `index.json`
- Create: `crates/engine-core/tests/h1_response.rs`
- Create: `crates/engine-core/fuzz/Cargo.toml`, `crates/engine-core/fuzz/fuzz_targets/h1_response.rs`
- Create: `docs/notes/response-parser.md`. Update `docs/perf/ideas.md` with the P0 survey.

**Interfaces:**
- Consumes: `ErrorCode`, `H1_LIMITS`, `Method` (generated, Task 2)
- Produces:

```rust
pub enum HttpVersion { Http10, Http11 }
pub enum Framing { ContentLength(u64), Chunked, CloseDelimited, Empty }
pub struct ResponseHead { pub status: u16, pub version: HttpVersion, pub head_len: usize,
                          pub headers: core::ops::Range<usize>, pub framing: Framing, pub keep_alive: bool }
pub enum ParseStatus { Complete(ResponseHead), Interim { consumed: usize }, Partial }
pub struct ResponseParser { /* resume offset, interim count, bytes used by interim heads */ }
impl ResponseParser {
    pub fn new(method: Method) -> Self;
    pub fn reset(&mut self, method: Method);
    /// `buf` always starts at the current message start and only grows between calls.
    /// Scanning resumes from where the last call stopped (no rescanning: O(head bytes) total).
    pub fn parse(&mut self, buf: &[u8]) -> Result<ParseStatus, ErrorCode>;
}
```

`headers` is the byte range of the header block. The engine extracts `Content-Length`, `Transfer-Encoding`, `Connection` and `Keep-Alive` during the scan and builds no map (§5.1). After `Interim { consumed }`, the caller drops `consumed` bytes and calls again. Interim heads count toward the 64 KiB limit and the limit of 10 interim responses.

**Error mapping (decided here, cited in the design note):**
- Head > 64 KiB or status line > 8 KiB → `TooLarge(Headers)`.
- Framing conflicts → `Protocol(SmugglingRisk)`: CL together with TE, duplicate CL (even when equal), more than one TE field, TE on HTTP/1.0, or a TE value other than exactly `chunked`.
- Every other grammar or count violation → `Protocol(MalformedResponse)`, including more than 100 fields and more than 10 interim responses.

- [ ] **Step 1: Research gate (see "Process rules"), the speed programme survey (§7).** The research agent and the note review complete before Step 2. Study picohttpparser, httparse, llhttp, hyper `proto/h1`, h11, Go `net/http` and Bun's client. For each, record in `docs/perf/ideas.md` what makes it fast and the bugs its tracker shows. Write `docs/notes/response-parser.md`: what we take, what we do differently, and links to exact files. Cite doc.rust-lang.org/std for any `std` API used.
- [ ] **Step 2: Write `blazehttp-rules.json`.** Add one rejecting case and one accepting neighbour for **every** §5.2 head rule:
  - The status line: `HTTP/1.0` and `HTTP/1.1` only, exactly one SP, 3 digits in 100–599, then CRLF (empty reason, accepted) or SP and a reason without CTL, `101` rejected, HTTP/0.9 rejected.
  - Bare LF and bare CR anywhere in the head.
  - Token names, whitespace before the colon, obs-fold.
  - CTL in values (HTAB allowed, DEL rejected).
  - 100 vs 101 fields; a head of 65,536 vs 65,537 bytes, including bytes from 1xx heads; 10 vs 11 interim responses.
  - CL grammar: sign, whitespace, list, duplicate, u64 overflow.
  - TE rules.
  - No body for HEAD, 1xx, 204 or 304.
  - HTTP/1.0 keep-alive only with `Connection: keep-alive`; `Connection: close`.
  - Close-delimited when neither CL nor TE is present on a status that has a body.
- [ ] **Step 3: Write the failing tests** in `tests/h1_response.rs`:

```rust
#[test] fn all_h1_response_vectors() { for (file, case) in load_suite("engine-core:h1_response") { assert_case(&file, &case, parse_whole(&case)); } }
#[test] fn every_vector_is_split_invariant() {           // Review Focus 1
    for (file, case) in load_suite("engine-core:h1_response") {
        let whole = parse_whole(&case);
        for split in 1..case.input.len() { assert_eq!(parse_fed_in_two(&case, split), whole, "{file}/{} split {split}", case.name); }
        assert_eq!(parse_byte_by_byte(&case), whole, "{file}/{} 1-byte reads", case.name);
    }
}
#[test] fn errors_never_contain_header_bytes() { /* Debug/Display of every error from the corpus contains no input substring ≥ 4 bytes */ }
```

  `parse_whole` and its two variants run the parser and then slice the body by `Framing::ContentLength`. This keeps CL body slicing inside the test helper, because the connection owns body reading.
- [ ] **Step 4: Run** `cargo nextest run -p engine-core --test h1_response`. Expected: FAIL.
- [ ] **Step 5: Implement the parser.**
  - Use a single forward scan with `memchr` for CR, a resume offset stored in the struct, and token and value tables as `const [bool; 256]`.
  - Do no allocation.
  - Parse `Content-Length` with checked u64 arithmetic.
- [ ] **Step 6: Run** the tests, then `cargo clippy -p engine-core --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 7: Add the fuzz target.** It takes `arbitrary` bytes plus a split point and must not panic. The split result must equal the whole result. Run `cargo +nightly-<pinned> fuzz run h1_response -- -max_total_time=300` locally. Expected: no crash. Record the pinned nightly date in `docs/notes/toolchain.md`.
- [ ] **Step 8: Commit** `Add strict HTTP/1.1 response head parser with rule vectors and fuzz target` and open a PR.

---

### Task 5: `RequestWriter` and header templates

**Files:**
- Create: `crates/engine-core/src/h1/writer.rs`, `crates/engine-core/tests/h1_writer.rs`, `docs/notes/request-writer.md`

**Interfaces:**
- Consumes: `Method`, `ErrorCode` (Tasks 2 and 4)
- Produces:

```rust
pub struct HeaderTemplate { /* pre-encoded "host: …\r\nname: value\r\n…" bytes */ }
impl HeaderTemplate { pub fn new(host: &str, defaults: &[(&str, &str)]) -> Result<Self, ErrorCode>; }
pub struct RequestParts<'a> { pub method: Method, pub target: &'a [u8], pub headers: &'a [u8] /* "name: value\r\n"* */, pub body: &'a [u8] }
pub fn write_request(out: &mut Vec<u8>, parts: &RequestParts<'_>, template: &HeaderTemplate) -> Result<(), ErrorCode>;
```

`write_request` clears `out` and writes the request line, the template, the validated per-request headers, `content-length` (only when there is a body, or the method is POST, PUT or PATCH), the blank line and the body, all into one buffer for one write. It reuses `out`'s capacity, so steady state does not allocate. Re-validation failures return `Engine(Internal)`: the user-facing `INVALID_CONFIG` comes from the JS layer in P2, and a failure here means that contract was bypassed (§12).

- [ ] **Step 0: Research gate** (see "Process rules"). The research agent writes `docs/notes/request-writer.md`, and it is reviewed before any code in this task.
- [ ] **Step 1: Write the failing tests:**
  - `get_uses_template_once_and_single_buffer`: assert the exact bytes `GET /data HTTP/1.1\r\nhost: 127.0.0.1:3999\r\n\r\n`.
  - `post_sets_content_length_from_body`
  - `rejects_forbidden_user_headers`: one case per header: `host`, `transfer-encoding`, `connection`, `upgrade`, `expect`, `te`, `keep-alive`, `proxy-connection`, `proxy-authorization`, `content-length`.
  - `rejects_target_with_space_ctl_or_non_ascii`
  - `rejects_header_value_ctl_except_htab`
  - `rejects_non_token_header_name`
  - `template_rejects_invalid_defaults`
  - `reuses_output_capacity`: assert the second write keeps the same `as_ptr()` and capacity.
- [ ] **Step 2: Run** `cargo nextest run -p engine-core --test h1_writer`. Expected: FAIL.
- [ ] **Step 3: Implement** with the shared token and value tables from `parser.rs`. Move them to `h1/grammar.rs` so the writer does not duplicate them.
- [ ] **Step 4: Run** the tests and clippy. Expected: PASS.
- [ ] **Step 5: Commit** `Add request writer with pre-encoded header templates` and open a PR.

---

### Task 6: Shard, connection tasks, buffer pool and `CompletionSink`

**Files:**
- Create: `crates/engine-core/src/completion.rs`, `crates/engine-core/src/shard.rs`, `crates/engine-core/src/h1/connection.rs`, `crates/engine-core/src/buf.rs`
- Create: `crates/engine-core/tests/shard_loopback.rs` (an in-test tokio server per scenario)
- Create: `docs/notes/shard-and-connection.md`

**Interfaces:**
- Consumes: the parser and writer (Tasks 4 and 5)
- Produces:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub struct RequestKey(pub u64);  // opaque; JS encodes slot+generation (+shard id from P1a)
pub struct Submit { pub key: RequestKey, pub method: Method, pub req: Vec<u8>,
                    pub target: Range<u32>, pub headers: Range<u32>, pub body: Range<u32> }   // ranges index `req`
pub enum ShardCommand { Submit(Submit) }                         // P1a adds Credit, Cancel, UploadChunk, Poison
pub struct ResponseData { pub status: u16, pub buf: Vec<u8>, pub head: Range<u32>, pub body: Range<u32> }
pub struct Completion { pub key: RequestKey, pub req: Vec<u8> /* returned to its owner */, pub outcome: Result<ResponseData, ErrorCode> }
pub trait CompletionSink: 'static {
    /// Push one terminal record; append any read buffers the consumer has returned for this shard thread into `reclaimed`.
    fn push(&self, c: Completion, reclaimed: &mut Vec<Vec<u8>>);
}
pub struct ShardConfig { pub addr: SocketAddr, pub host: String, pub max_connections: usize,
                         pub queue_capacity: usize, pub read_buf_size: usize /* default 65536 */ }
#[derive(Clone)] pub struct ShardHandle { /* tokio::sync::mpsc::Sender<ShardCommand> */ }
impl ShardHandle { pub fn try_submit(&self, s: Submit) -> Result<(), (Submit, ErrorCode)>; } // Full → QueueFull { scope: Global, bound: queue_capacity }
pub fn shard<S: CompletionSink>(cfg: ShardConfig, sink: S) -> (ShardHandle, impl Future<Output = ()>); // caller runs it inside a LocalSet
pub fn spawn_shard_thread<S: CompletionSink + Send>(cfg: ShardConfig, sink: S) -> std::io::Result<ShardHandle>; // dedicated thread: current_thread runtime + LocalSet
```

**Behaviour (decided here):**
- Shard state is `Rc<RefCell<ShardState>>`: a FIFO of pending `Submit`s, a count of live connections, a read-buffer free list and a `Notify` for idle connections. No borrow is held across `.await`.
- The command loop uses `recv_many` into a reused `Vec` and pushes onto the FIFO. If no connection is idle and `live < max_connections`, it spawns one connection task (`spawn_local`) per missing connection. Otherwise it wakes one idle connection. Requests do not spawn tasks (§5.1).
- A connection task runs this loop:
  1. Take the next `Submit`.
  2. `write_request` into the connection's write `Vec`, then `write_all`.
  3. Read into a pooled read buffer and parse.
  4. Read the CL body.
  5. Hand off the read buffer if the response fills ≥ ¼ of it. Otherwise copy it out into an exact-size `Vec` and keep the buffer.
  6. Push the completion, and return `reclaimed` buffers to the free list.
  7. Take the next request, or park on `Notify`.
- `TCP_NODELAY` is on.
- **Reuse rules in P0** (§5.1 subset): close and never reuse the connection when bytes remain after the response, on `Connection: close`, on HTTP/1.0 without keep-alive, on close-delimited responses, and on any error. Other connections are reused.
- `Framing::Chunked` completes with `Protocol(Other)` and closes the connection. Mark it `// removal: P1a chunked FramingDecoder (#issue)`.
- **Error mapping:**
  - Connect refused → `Network(Refused)`. Other connect errors → `Network(Other)`.
  - EOF or reset before any response byte → `Network(ClosedBeforeResponse)`. The stale-connection re-send rule arrives in P1a.
  - EOF mid-body → `Protocol(IncompleteBody)`.
- Timeouts arrive in P1a.

- [ ] **Step 0: Research gate** (see "Process rules"). The research agent writes `docs/notes/shard-and-connection.md`, and it is reviewed before any code in this task.
- [ ] **Step 1: Write the failing tests:**
  - `get_round_trip_returns_status_head_body`
  - `keep_alive_reuses_one_connection`: 100 sequential requests with `max_connections = 1`; the server counts exactly 1 accept.
  - `never_exceeds_max_connections`: 200 concurrent requests with `max_connections = 8`; peak accepted ≤ 8.
  - `connection_close_reconnects`
  - `leftover_bytes_close_connection`: the server appends junk after the response; the next request uses a new connection.
  - `server_closed_idle_connection_settles_with_network_error` (Review Focus 2): the server closes after the first response; the second request settles with `Network(ClosedBeforeResponse)` within 1 s; the third succeeds on a new connection.
  - `eof_mid_body_settles_incomplete_body` (Review Focus 2)
  - `refused_connect_settles_refused`
  - `queue_full_fails_fast` (Review Focus 3): `queue_capacity = 4` and the shard future not yet polled; the 5th `try_submit` returns `QueueFull { scope: Global, bound: 4 }` immediately.
  - `each_key_completes_exactly_once`: 10,000 requests with `max_connections = 50`; the sink records keys and expects no duplicate and no missing key.
  - `small_response_copied_large_handed_off`: compare `buf.capacity()` with `read_buf_size`.
  - `reclaimed_buffer_is_reused`: the sink returns the buffer, and the next large completion's `buf.as_ptr()` equals the returned one.
  - `request_buffer_returned_in_completion`: `completion.req.as_ptr()` equals the submitted one.
- [ ] **Step 2: Run** `cargo nextest run -p engine-core --test shard_loopback`. Expected: FAIL.
- [ ] **Step 3: Implement** `buf.rs` (free list with a max size of `max_connections × 2`), then `completion.rs`, `connection.rs` and `shard.rs`. Check the tokio APIs (`mpsc::Receiver::recv_many`, `LocalSet`, `task::spawn_local`, `Notify`) on docs.rs for the locked version.
- [ ] **Step 4: Run** the tests, clippy and `cargo llvm-cov nextest -p engine-core --fail-under-lines 90`. Expected: PASS.
- [ ] **Step 5: Commit** `Add prototype shard with connection tasks, buffer handoff and completion sink` and open a PR.

---

### Task 7: `engine-bench` core with blazehttp and raw-loop clients

**Files:**
- Create: `crates/engine-bench/Cargo.toml` (bin; features `count-allocs` for the `allocation-counter` global allocator; dev-dependency `bench-server`)
- Create: `crates/engine-bench/src/{main.rs, cli.rs, driver.rs, pacing.rs, metrics.rs, work.rs}`, `crates/engine-bench/src/clients/{mod.rs, blaze.rs, raw.rs}`
- Test: `crates/engine-bench/tests/clients.rs`, plus unit tests in `work.rs`, `pacing.rs` and `metrics.rs`
- Create: `docs/notes/bench-method.md`

**Interfaces:**
- Consumes: `engine_core::{shard, ShardConfig, CompletionSink, Submit, write_request…}`, and `bench_server::serve` (Task 9)
- Produces:

```rust
pub enum Work { Fnv, Serde }                    // Fnv: fnv1a64(body) == expected; Serde: Payload { id == 42 } (Rust-only cells)
pub fn fnv1a64(bytes: &[u8]) -> u64;            // offset 0xcbf29ce484222325, prime 0x100000001b3
pub enum Load { Closed { concurrency: usize }, Open { rate: u32, max_in_flight: usize } }
pub trait Driven {                               // one driver task per run, same current_thread runtime as the connections (§16.2 symmetric harness)
    fn start(&mut self, id: u32);
    async fn completions(&mut self, out: &mut Vec<(u32, Result<(), BenchError>)>); // waits for ≥1
}
pub struct RunResult { pub client: String, pub cell: String, pub load: String, pub rps: f64, pub p50_us: f64, pub p99_us: f64,
                       pub cpu_us_per_req: f64, pub allocs_per_req: Option<f64>, pub requests: u64 }   // printed as one JSON line (serde)
```

**CLI:** `engine-bench --client <blaze|raw|hyper|reqwest|ureq|isahc> --addr 127.0.0.1:3999 --path /1k --work fnv --expect <u64> (--closed 50 | --open <rate> --max-in-flight 50) --warmup-secs 2 --secs 10`

**Method (decided here):**
- **CPU:** `utime + stime` from `/proc/self/stat` over the measured window. The window is ≥ 5 s so 10 ms ticks stay below 0.2% error. Benchmarks run only in Linux containers.
- **Open loop:** intended start time `tᵢ = t₀ + i/rate`. Latency is measured from `tᵢ` (wrk2-style coordinated-omission correction). When in-flight requests reach `max_in_flight`, the request waits in the driver's queue, and that wait counts toward latency.
- **Allocations:** a separate run built with `--features count-allocs` divides the allocation count by the number of requests. Timed runs never use the counting allocator. Check `allocation-counter`'s API on docs.rs, including whether it counts per thread. If it does not count per thread, record that in `bench-method.md` and count over the whole process in the single-threaded runtime.
- **Raw loop:** a port of the spike's 40-line tokio + `httparse` loop (`spike/g0/src/main.rs` `raw_conn`). This is the §16.2 budget baseline. It switches from `serde` to `Work`.
- **blaze client:** `shard()` on the same `LocalSet` with a `ChannelSink`. `ChannelSink` is an `Rc<RefCell<VecDeque<Completion>>>` plus a `Notify` and a return-`Vec` for buffers. Request `Vec`s are recycled from completions, so steady state allocates nothing for request bytes.

- [ ] **Step 1: Write the failing tests:**
  - `fnv1a64_known_values`: `fnv1a64(b"") == 0xcbf29ce484222325`; `fnv1a64(b"a") == 0xaf63dc4c8601ec8c`.
  - `open_loop_schedule_is_rate_exact`: a pure function `intended_start(i, rate, t0)`.
  - `percentiles_from_sorted_latencies`
  - In `tests/clients.rs`, `blaze_and_raw_complete_1000_requests_with_valid_checksums` starts `bench_server::serve` in-process and runs both clients in closed and open loop.
- [ ] **Step 2: Run** `cargo nextest run -p engine-bench`. Expected: FAIL.
- [ ] **Step 3: Implement** the driver, pacing, metrics and the two clients.
- [ ] **Step 4: Run** the tests, clippy, and `cargo hack check -p engine-bench --each-feature`. Expected: PASS.
- [ ] **Step 5: Commit** `Add engine benchmark driver with blazehttp and raw-loop clients` and open a PR.

---

### Task 8: Rust G0 competitors

**Files:**
- Create: `crates/engine-bench/src/clients/{hyper.rs, reqwest.rs, ureq.rs, isahc.rs}`, `bench/competitors.json` (name, crate, exact version, features; written by `cargo metadata` in a small `tools/pack`-style script, or by hand and checked in CI against `Cargo.lock`)
- Modify: `crates/engine-bench/tests/clients.rs`

**Interfaces:**
- Consumes: the `Driven` trait and the blocking-runner signature `fn run_blocking(threads: usize, work: impl Fn() -> Result<(), BenchError> + Sync)` (added here)
- Produces: four more `--client` values

**Rules (§16.2):**
- `hyper`: `client::conn::http1::handshake` with a minimal keep-alive pool, a `Vec<SendRequest>` stack with checkout and return. Each connection task is spawned on the same runtime.
- `reqwest`: `pool_max_idle_per_host = c`.
- `ureq`: thread-per-request, with threads = c and the label `blocking`.
- `isahc`: multi interface with the max connections set to c.
- Every client uses its latest stable version, the release profile, and plaintext only in P0. HTTPS competitor cells start in P1a.

- [ ] **Step 1: Extend the test** to `all_rust_competitors_complete_1000_requests_with_valid_checksums`, covering every client in closed loop at c = 8.
- [ ] **Step 2: Run** it. Expected: FAIL (unknown client).
- [ ] **Step 3: Implement** the four adapters and `competitors.json`.
- [ ] **Step 4: Run** the tests, clippy and `cargo deny check`. Expected: PASS. isahc's `curl-sys` licence must be listed in `deny.toml`'s allow list with a comment saying it is a benchmark-only dependency.
- [ ] **Step 5: Commit** `Add hyper, reqwest, ureq and isahc as G0 benchmark competitors` and open a PR.

---

### Task 9: `bench-server`, the fixed-response server

**Files:**
- Create: `crates/bench-server/Cargo.toml` (lib + bin; tokio multi-thread, tokio-rustls, `rcgen` for a self-signed certificate at start-up, `flate2` to pre-gzip at start-up)
- Create: `crates/bench-server/src/{lib.rs, main.rs, cells.rs}`, `crates/bench-server/tests/serve.rs`
- Create: `bench/cells.json` (generated by `bench-server --print-cells`: path, response bytes length, header count, FNV-1a of the body)

**Interfaces:**
- Produces:
  - `pub struct ServerConfig { pub plain: SocketAddr, pub tls: Option<SocketAddr>, pub threads: usize, pub stall: bool }`
  - `pub async fn serve(cfg: ServerConfig, ready: tokio::sync::oneshot::Sender<()>) -> std::io::Result<()>`
  - `pub fn cells() -> &'static [Cell]`, where `Cell { path: &'static str, response: Vec<u8>, body_fnv: u64 }`

**Cells (all response bytes pre-built at start-up):**
- `/1k`: the spike payload (`spike/g0/body.json`, 847 bytes, `"id":42`, generated deterministically by the same shape as `spike/httpbench/goserver/main.go`) with 3 headers: `content-type`, `content-length` and a fixed `date`.
- `/1k-h20`: the same payload with 20 headers.
- `/1k-gzip`: the payload pre-gzipped, with `content-encoding: gzip`.
- `/1k-chunked`: chunked framing.
- `/1m`: a 1 MiB body.

**Request handling:** find `\r\n\r\n`, take the target between the first two spaces, and write the pre-built bytes. An unknown path gets a pre-built 404. A request body named by `content-length` is skipped. `--stall` accepts and reads but never answers. The server never parses JSON. It parses nothing beyond what is needed to find the target.

- [ ] **Step 1: Write the failing tests:**
  - `each_cell_returns_prebuilt_bytes`
  - `keep_alive_serves_10k_requests_on_one_connection`
  - `tls_port_serves_same_cells`
  - `stall_mode_never_answers`: reads with a 300 ms timeout and receives 0 bytes.
  - `cells_json_matches_cells`: compares against the committed `bench/cells.json`.
- [ ] **Step 2: Run** `cargo nextest run -p bench-server`. Expected: FAIL.
- [ ] **Step 3: Implement** it and generate `bench/cells.json`.
- [ ] **Step 4: Run** the tests and clippy. Expected: PASS.
- [ ] **Step 5: Commit** `Add fixed-response benchmark server with pre-built cells` and open a PR.

---

### Task 10: Benchmark harness orchestration (Docker, validity, netem, statistics)

**Files:**
- Create: `bench/docker/rust.Dockerfile` (multi-stage build of `bench-server` and `engine-bench`; Rust image pinned by digest), `bench/docker/node.Dockerfile` (`node:26-bookworm-slim` and `node:24-bookworm-slim` by digest), `bench/docker/netem.Dockerfile` (debian slim + iproute2)
- Create: `bench/harness/{package.json, cli.ts, docker.ts, validity.ts, stats.ts, loadpoints.ts, netem.ts, results.ts, report.ts, config.ts}`, `bench/harness/{stats,validity,loadpoints,results}.test.ts`

**Interfaces:**
- Produces:

```ts
export interface Sample { client: string; cell: string; load: string; round: number; rps: number; p50Us: number; p99Us: number;
  cpuUsPerReq: number; allocsPerReq?: number; serverCpuPct: number; noiseCpu: number; valid: boolean; invalidReason?: string }
export function median(xs: readonly number[]): number;
export function ratioOfMedians(num: readonly number[], den: readonly number[]): number;
export function bootstrapRatioCi(num: readonly number[], den: readonly number[], o: { resamples: number; seed: number; confidence: number }):
  { point: number; lo: number; hi: number };                       // resample each side's rounds independently; seeded PRNG (xorshift128+ in stats.ts)
export function validateRun(m: { serverCpuPct: number; noiseCpu: number; loadAvgBefore: number }, t: ValidityThresholds): { valid: boolean; reason?: string };
export function pairwiseLoadPoint(satA: number, satB: number): number;   // 0.8 × min(satA, satB), rounded down to 100 req/s
export function g0Verdict(blazeCpu: readonly number[], rivalCpu: readonly number[], kind: 'engine-bound' | 'shared-limit'): { pass: boolean; ci: …; bound: 1.0 | 0.97 };
// ratio = rival CPU µs / blaze CPU µs (higher = we are cheaper)
```

**Validity (decided here; defaults in `config.ts`, tuned by benchmark):**
- Server CPU is Δ`usage_usec` from `docker exec srv cat /sys/fs/cgroup/cpu.stat` divided by (Δwall × server CPUs). It must be < 60%.
- `noiseCpu` is Δ busy jiffies from the VM's `/proc/stat` minus the server's and client's cgroup usage, averaged over the run in CPUs. It must be ≤ 0.10.
- The 1-minute load average before the start must be ≤ 1.0.
- Invalid rounds are re-run, up to 3 extra times per round. Then the cell reports `insufficient valid rounds`.

**Gate mode vs dev mode:**
- Gate mode refuses to start outside the configured window (`BENCH_WINDOW`, unset until the owner chooses).
- It aborts if any container that is not labelled `blazehttp-bench` is running. It never stops anything.
- `--dev` runs anywhere and marks results `dev: true`. Dev results are never gate evidence.

**Topology:**
- Server container: `--cpus=4 --cpuset-cpus` pinned to 4 CPUs.
- Client containers: `--cpus=1`, pinned to a disjoint CPU, sharing the server's network namespace (`--network container:srv`).
- netem runs in a sidecar with `--cap-add NET_ADMIN`: `tc qdisc replace dev lo root netem delay <rtt/2>ms`, because loopback traverses `lo` twice per round trip. Check at the first run that Docker Desktop's kernel has `sch_netem`. If it does not, stop and report to the owner.

**Results:** one JSONL per run, `bench/results/<yyyy-mm-dd>-<host>-<label>.jsonl`. Each line is a `Sample` plus run metadata: git SHA, image digests, Node version, `docker info` CPUs and kernel, and competitor versions. `report.ts` prints the markdown tables used in the journal, including the cross-language leaderboard table format (the G8 skeleton; the published page comes in P5).

- [ ] **Step 1: Write the failing tests:**
  - `median_odd_even`
  - `ratio_of_medians`
  - `bootstrap_ci_is_deterministic_for_seed_and_brackets_point`
  - `bootstrap_ci_narrows_with_less_noise`
  - `validity_rejects_server_at_60_percent`: 59.9% is valid, 60.0% is invalid.
  - `validity_rejects_noise_and_busy_host`
  - `pairwise_load_point_uses_slower_client`
  - `g0_verdict_engine_bound_needs_lower_bound_at_least_1`: a fixture where the point estimate is 1.05 but the lower bound is 0.98 → fail.
  - `results_round_trip_jsonl`
- [ ] **Step 2: Run** `pnpm vitest run bench/harness`. Expected: FAIL.
- [ ] **Step 3: Implement** the pure modules, then `docker.ts` (a thin `execFile('docker', …)` wrapper) and `cli.ts` (subcommands `g0`, `g1`, `xlang`, `lowload`, `memory`; options `--cells`, `--clients`, `--rounds 10`, `--dev`).
- [ ] **Step 4: Run** the tests. Then run a dev smoke run on the local Mac: `pnpm bench g0 --dev --rounds 2 --cells 1k --clients raw,blaze`. Expected: 4 valid samples printed and a JSONL written. This shows the Docker path works end to end. Do not commit dev results.
- [ ] **Step 5: Commit** `Add Docker benchmark harness with validity checks and bootstrap statistics` and open a PR.

---

### Task 11: Quick cross-language baseline (top four non-Rust G0 clients)

**Files:**
- Create: `bench/xlang/{cpp,c,zig,go}/` ported from `spike/httpbench/xlang/{cpp,c,zig}` and `spike/httpbench/goclient`. Each has its own Dockerfile and pins its toolchain (Zig stays on the spike's 0.17.1). The ports:
  - replace JSON parsing with FNV-1a over the body bytes and compare it with `--expect`;
  - read CPU from `/proc/self/stat`;
  - accept `--path`, `--concurrency` and `--secs`;
  - print one `RunResult`-shaped JSON line.
- Modify: `bench/harness/cli.ts` (`xlang` subcommand)
- Create: `docs/notes/xlang-baseline.md`

The four clients are the G0-eligible non-Rust rows of §1-E in order: C++ Boost.Beast, Zig `std.http.Client` (blocking, threads = c, labelled), C libcurl multi and Go `net/http`. Bun is excluded because it is not a G0 competitor. fasthttp, libh2o and cpp-httplib come in P1a (E7). Build flags per §16.2: C/C++ `-O3`, Zig `ReleaseFast`, Go defaults.

- [ ] **Step 1: Port each client and smoke-test it.** Run `pnpm bench xlang --dev --rounds 1 --clients cpp,c,zig,go,raw`. Expected: 5 valid samples, every checksum matches `bench/cells.json`.
- [ ] **Step 2: Run the baseline in the nightly window.** Run `pnpm bench xlang --rounds 10 --cells 1k --closed 50 --clients cpp,c,zig,go,raw` and commit the results JSONL. If the owner has not yet set a window, use a `--dev` run, label it provisional in the note, and repeat in the first window.
- [ ] **Step 3: Write the result.** Add a journal entry and fill `docs/notes/xlang-baseline.md` with a table of each competitor's CPU µs/req ÷ the raw loop's CPU µs/req, with the 95% CI. **If any competitor's ratio CI includes or falls below 1.10**, open `docs/decisions/p0-io-uring.md` for the owner. It states the data and the question "fund `io_uring`/own-reactor research?" (§15 P0 acceptance) and is linked from the P0 report.
- [ ] **Step 4: Commit** `Add cross-language baseline clients with checksum-equal work` and open a PR.

---

### Task 12: `node-binding` (napi bridge, TSFN wake, completion Vec swap)

**Files:**
- Create: `crates/node-binding/{Cargo.toml (crate-type cdylib; napi 3, napi-derive 3, napi-sys, parking_lot), build.rs (napi-build), src/lib.rs, src/env_state.rs, src/sink.rs, src/codec.rs, src/tsfn_ref.rs}`
- Modify: `policy/spec.json` (add `wire`), and regenerate with `pnpm specgen`
- Create: `crates/node-binding/test/binding.test.ts`, `crates/node-binding/test/fixtures/{idle-exit.mjs, pending-alive.mjs}`, `crates/node-binding/package.json` (private workspace package with a `build` script: `napi build --platform --release --no-js --output-dir ./dist`)
- Create: `docs/notes/completion-path.md`

**`wire` section (u32 words, generated into `SUBMIT_RECORD`/`COMPLETION_RECORD` index constants in TS and Rust):**
- Submit record, 9 words: `keyLo, keyHi, method, targetOff, targetLen, headersOff, headersLen, bodyOff, bodyLen`. Offsets index the submit arena.
- Completion record, 9 words: `keyLo, keyHi, status, errWord, errArg, headOff, headLen, bodyOff, bodyLen`. Offsets index the completion arena. `errWord = 0` means success, otherwise it is `ErrorCode::to_wire().0 + 1`.
- `takeCompleted` returns `count | (more << 31)`.
- `arenaBytes: 4194304`, `batchRecords: 4096` (§11.5; Task 19 confirms or retunes them).

**Interfaces:**
- Consumes: `spawn_shard_thread`, `CompletionSink`, `Submit`, `ErrorCode` (Tasks 2 and 6)
- Produces (JS-visible, typed by hand in Task 13's `binding.ts`):
  - `init(config: { host: string; port: number; maxConnections: number; queueCapacity: number }, wake: () => void): void`. P0 has one shard per environment; P1a replaces it with `control()` and profile leases.
  - `submit(records: Uint32Array, count: number, arena: Uint8Array): void`
  - `takeCompleted(records: Uint32Array, arena: Uint8Array): number`

**Rules (§4 steps 4, 6, 7 and §11.3):**
- Per-environment state lives in napi instance data.
- `TsfnSink::push` locks the mutex, pushes into the active `Vec`, drains this shard thread's return `Vec` into `reclaimed`, and records `was_empty`. It unlocks, and then calls the TSFN if `was_empty`. No code that can panic runs under the lock.
- `takeCompleted` swaps active and spare under the lock (the spare must be empty). It copies outside the lock into the JS arrays and arena. Records that do not fit stay in a carry-over and set `more`. Consumed read buffers go into the return `Vec` under the lock.
- The response head (excluding the status line) and the body are copied into the arena. Bodies larger than the arena are capped in P0 at `arenaBytes` and settle `TooLarge(ResponseBody)`; external buffers arrive in P1a.
- The binding validates the codec input: offsets and lengths must be within the arena and the method id must be in range. A bad record produces an `Engine(Internal)` completion for that key and is never a panic.
- When `try_submit` returns `QueueFull`, a completion is queued with that error. It is never a JS throw (Review Focus 3).
- **Ref/unref (Review Focus 5):** use the safe wrapper `TsfnRef` over `napi_ref_threadsafe_function` and `napi_unref_threadsafe_function`, called through `napi-sys`. Every `unsafe` block carries `// SAFETY:`. `ThreadsafeFunction::aborted()` is the only source of truth. The TSFN is ref'd while pending > 0 and unref'd at 0. Pending is incremented in `submit` and decremented in `takeCompleted`.
- The env cleanup hook drops the TSFN clones and never calls `abort()` (§11.3). Verify each napi-rs 3 API on Context7 before use, and confirm none is deprecated.
- Every export uses `#[napi(catch_unwind)]`. Add `const _: () = assert!(cfg!(panic = "unwind"));`.

- [ ] **Step 0: Research gate** (see "Process rules"). The research agent writes `docs/notes/completion-path.md`, and it is reviewed before any code in this task.
- [ ] **Step 1: Write the failing tests** (Vitest, with a `node:http` loopback server in the test):
  - `round_trip_single_request`
  - `batch_of_1000_requests_each_key_completes_once`
  - `queue_full_is_a_completion_not_a_throw`: `queueCapacity: 8` against a stalled `net` server; submit 100 → ≥ 92 completions carry the `QUEUE_FULL` wire code with `bound` 8.
  - `more_flag_when_arena_is_small`: a 4 KiB arena and ten 847-byte bodies → several takes, every key exactly once.
  - `malformed_record_completes_engine_internal`
  - `process_exits_when_idle` (Review Focus 5): spawn `idle-exit.mjs`, which runs 10 requests and returns; the child exits with code 0 within 2 s.
  - `process_stays_alive_while_pending` (Review Focus 5): `pending-alive.mjs` against a server that answers after 500 ms; the child exits after the response, not before, with the result printed.
- [ ] **Step 2: Run** `pnpm --filter node-binding build && pnpm vitest run crates/node-binding`. Expected: FAIL.
- [ ] **Step 3: Implement** it.
- [ ] **Step 4: Run** the tests with `--throw-deprecation`, then clippy on `node-binding`. Expected: PASS.
- [ ] **Step 5: Commit** `Add napi binding with batched submit and swapped completion buffers` and open a PR.

---

### Task 13: Native loader, platform detection, integrity and the `NativeSession` seed

**Files:**
- Create: `packages/blazehttp/src/native/{platform.ts, loader.ts, binding.ts, session.ts, slot-table.ts, wire-decoder.ts}`, `packages/blazehttp/src/errors/transport-error.ts`
- Create (gitignored; `pnpm build:native` writes it for the binary it just built; with no binaries, `tools/pack/integrity.ts --empty` writes an empty table so the `ts` CI job type-checks, and every load then fails with `integrity`. There is no bypass.): `packages/blazehttp/src/generated/integrity.ts`
- Create: `npm/<target>/package.json` ×8 (`name: @blazehttp/<target>`, `os`, `cpu`, `libc` for linux, `main: blazehttp.<target>.node`, `files`, `license`)
- Modify: `packages/blazehttp/package.json` (`optionalDependencies` on all 8 with `workspace:*`; pnpm rewrites these to exact versions on publish)
- Test: `packages/blazehttp/test/{platform,loader,slot-table,session}.test.ts`

**Interfaces:**
- Consumes: the binding API (Task 12), the generated `ERROR_WIRE` and `SUBMIT_RECORD`/`COMPLETION_RECORD` (Task 2/12)
- Produces:

```ts
export type Target = 'darwin-arm64' | 'darwin-x64' | 'linux-x64-gnu' | 'linux-x64-musl' | 'linux-arm64-gnu' | 'linux-arm64-musl' | 'win32-x64-msvc' | 'win32-arm64-msvc';
export function detectTarget(p: { platform: string; arch: string; glibcVersionRuntime: string | undefined }): Target | undefined;
export type LoadResult = { ok: true; binding: NativeBinding; target: Target }
  | { ok: false; reason: 'unsupported-platform' | 'binary-load' | 'integrity'; cause?: unknown };
export function loadNative(deps?: LoaderDeps): LoadResult;      // LoaderDeps = { resolve, readFile, dlopen, hashes, platformInfo } for tests; defaults to real ones
export class TransportError<C extends HttpErrorCode = HttpErrorCode> extends Error { readonly code: C; readonly detail: ErrorDetailMap[C] }
export class SlotTable<T> { alloc(v: T): number /* index; generation via generationOf(index) */; generationOf(index: number): number;
  take(index: number, generation: number): T | undefined /* stale generation → undefined */ }   // wire keyLo = index, keyHi = generation; no bigint, no per-request object
export interface RawResponse { status: number; head: string /* latin1, exact size */; body: Uint8Array /* copied; no arena view escapes */ }
export class NativeSession {
  constructor(binding: NativeBinding, cfg: { host: string; port: number; maxConnections: number; queueCapacity: number });
  request(method: Method, target: string, headers: string, body?: Uint8Array): Promise<RawResponse>;   // rejects only with TransportError
}
```

**Rules:**
- musl is detected when `process.report.getReport().header.glibcVersionRuntime` is `undefined` on Linux, called once. Verify this in the Node docs.
- The loader:
  1. resolves `@blazehttp/<target>/blazehttp.<target>.node`;
  2. checks SHA-256 (`node:crypto`) against `integrity.ts` before loading;
  3. loads with `process.dlopen` on a `{ exports: {} }` module object.
  There is no dev bypass: the native build script writes `integrity.ts` for the binary it just built.
- `NativeSession` encodes requests into a reused `Uint32Array` and arena. It writes header and target strings with `arena.write`-equivalent `Buffer.prototype.write(str, off, 'latin1')` and asserts the returned length. It flushes **once per `setImmediate`** (§4 step 3), or early at 4,096 records or a full arena.
- The wake callback loops `takeCompleted` while `more` is set. Errors decode through `WireDecoder` (a `Record<code, DetailParser>` lookup built from `ERROR_WIRE`). The head becomes a string with `buf.toString('latin1', s, e)`, and the body is copied with `.slice()`.

- [ ] **Step 0: Research gate** (see "Process rules"). The research agent writes `docs/notes/native-loader-and-session.md`, and it is reviewed before any code in this task.
- [ ] **Step 1: Write the failing tests:**
  - `detectTarget maps all 8 targets and returns undefined for freebsd/ia32`
  - `unsupported_platform`
  - `missing_package_returns_binary_load`
  - `tampered_binary_returns_integrity`: copy the built `.node` to a temp dir, flip one byte, inject `resolve`, and expect `integrity`. `dlopen` must never be called.
  - `loads_real_binary`
  - `slot_table_rejects_stale_generation`
  - `slot_table_reuses_indices_with_new_generation`
  - `requests_in_one_macrotask_flush_in_one_submit`: a fake binding counts `submit` calls; 100 requests in one tick → 1 call.
  - `requests_across_macrotasks_still_batch_per_immediate`
  - `queue_full_rejects_with_transport_error_QUEUE_FULL_global`
  - `real_round_trip_through_session`: `node:http` server; status, head and body correct.
- [ ] **Step 2: Run** `pnpm vitest run packages/blazehttp`. Expected: FAIL.
- [ ] **Step 3: Implement** it. Add `tools/pack/integrity.ts` (`hashBinaries(dir): Record<Target, string>` and writing the TS module), and wire it into the root `build:native` script.
- [ ] **Step 4: Run** the tests with coverage (≥ 95%), knip, lint and typecheck. Expected: PASS.
- [ ] **Step 5: Commit** `Add native loader with integrity check and batched NativeSession` and open a PR.

---

### Task 14: G1 Node runner, reproduction of the §1 numbers, spike retirement

**Files:**
- Create: `bench/g1/{package.json, run.ts, clients.ts}`. Dependencies are exact-pinned: `undici`, `axios`, `ky`, `ofetch`, all latest stable.
- Modify: `bench/harness/cli.ts` (`g1` subcommand), `bench/docker/node.Dockerfile`
- Delete: `spike/` (whole directory, at the end of this task)

**Interfaces:**
- Consumes: `NativeSession`, `loadNative` (Task 13), and the harness (Task 10)
- Produces: `node bench/g1/run.ts --client <name> --path /1k --concurrency 50 --secs 10`, which prints one JSON line with `rps, p50Us, p99Us, cpuMsPer1k, mainElu, peakRssMb, v8HeapMb, externalMb`

**Clients (a lookup map in `clients.ts`):**
- `undici`: `Pool.request` + `JSON.parse`, the G1 baseline.
- `node-fetch`: global `fetch`.
- `axios`, `ky`, `ofetch`.
- `blaze-proto`: `NativeSession` + `JSON.parse`.
- `spike-proto`: loads a build of `spike/httpbench/rustnode`, marked `// removal: deleted in this task after reproduction (#issue)`.

- [ ] **Step 1: Implement** `run.ts` and `clients.ts`, measuring the way `spike/httpbench/bench2.mjs` does: 3,000 warm-up requests, then a timed window.
- [ ] **Step 2: Smoke run:** `pnpm bench g1 --dev --rounds 1 --cells 1k --clients undici,axios,blaze-proto,spike-proto`. Expected: all valid, every body `id === 42`.
- [ ] **Step 3: Reproduce §1-A′ on the reference host** in the nightly window, with 10 rounds on Node 26. Pass criteria:
  - undici and spike-proto are each within ±15% of 32,644 and 61,027 req/s;
  - their ratio CI overlaps 1.6–2.1×.
  Commit the results. Record in the journal the spike prototype's re-measured median. The reference-cell gate stays ≥ max(63,790, that median) (§16.1). If reproduction fails, stop and report to the owner with the data. Do not delete `spike/`.
- [ ] **Step 4: Record the first own-engine G1 number.** Run `blaze-proto` on the reference cell and record it (informational in P0; the gate is P1a).
- [ ] **Step 5: Retire the spike.**
  1. Confirm that Task 11 has landed and that `bench/` has ported the payload, the raw loop and the xlang clients.
  2. Remove the `spike-proto` client.
  3. Tag the last commit that contains the spike: `git tag p0-spike-final && git push origin p0-spike-final`. The x64 re-measure of the spike prototype (§16.1, "re-measured on that same host") runs from this tag whenever the VM arrives.
  4. Run `git rm -r spike/`.
  5. Run `pnpm knip && pnpm policy` (expected: clean).
- [ ] **Step 6: Commit** `Add G1 Node benchmark runner and retire the spike` and open a PR.

---

### Task 15: CI on 8 targets

**Files:**
- Modify: `.github/workflows/ci.yml`

**Jobs (all actions on current majors, pinned by SHA, `concurrency` cancels superseded runs):**
- `policy`, `ts` (from Task 1), plus `pnpm specgen --check`, `node tools/vectors/check-index.ts` and `notices.ts --check`.
- `rust` on `ubuntu-24.04`:
  - `cargo fmt --check`;
  - `cargo clippy --workspace --all-targets` with `RUSTFLAGS=-D warnings`;
  - `cargo deny check` (RUSTSEC unmaintained and yanked = error), `cargo audit`, `cargo shear`, `cargo hack check --workspace --each-feature`;
  - `cargo nextest run --workspace`;
  - `cargo llvm-cov nextest -p engine-core --fail-under-lines 90`.
- `fuzz-smoke`: pinned nightly, `h1_response` for 120 s.
- `native` matrix, one entry per target. Each entry builds `node-binding` with `napi build --platform --release --no-js --target <triple>`, generates `integrity.ts`, then runs the `node-binding` and `packages/blazehttp` tests on Node 22.19, 24 and 26. The musl entries run inside `node:<v>-alpine` through `docker run` on the matching Ubuntu runner. Each entry uploads the `.node` as an artefact.
  | Target | Runner |
  |---|---|
  | darwin-arm64 | `macos-15` |
  | darwin-x64 | `macos-15-intel` |
  | linux-x64-gnu, linux-x64-musl | `ubuntu-24.04` |
  | linux-arm64-gnu, linux-arm64-musl | `ubuntu-24.04-arm` |
  | win32-x64-msvc | `windows-2025` |
  | win32-arm64-msvc | `windows-11-arm` |
- `alpine-load`, on both musl targets:
  - load the musl artefact through the loader inside Alpine and run `real_round_trip_through_session`;
  - `glibc_binary_on_musl_returns_binary_load`: place the gnu artefact under the musl package name with a matching integrity hash. Expect `{ ok: false, reason: 'binary-load' }` and no crash (Review Focus 4).

Coverage for `node-binding` (instrumented `.node` driven by the JS tests) is set up in P1a E2, and the P0 report records this as open. On musl the allocator is mimalloc with `local_dynamic_tls` (§5.3, §14), added to `node-binding` behind `cfg(target_env = "musl")` in this task.

- [ ] **Step 1: Write the workflow.** Push the branch, and open a draft PR to run it.
- [ ] **Step 2: Iterate until every job is green on the PR.** Fix causes, not jobs: no `continue-on-error` and no skipped targets.
- [ ] **Step 3: Set the required checks.** Add every job name as a required check on `main` and `milestone/p0` (`gh api … /protection`).
- [ ] **Step 4: Commit** `Run CI on all eight native targets with Alpine load tests`, then mark the PR ready.

---

### Task 16: Packaging dry-run, release workflow and placeholders

**Files:**
- Modify: `packages/blazehttp/package.json`:
  - `exports` with `.` → `{ types, node: { import, require }, default: { import, require } }`, with `types` first (§14). The `bun`, `deno`, `worker` and `browser` keys arrive with the P4 kits.
  - `files`, `sideEffects: false`, `publishConfig.provenance: true`.
- Create: `packages/blazehttp/tsdown.config.ts` (ESM + CJS + d.ts)
- Create: `tools/pack/{pack-all.ts, placeholders.ts, pack.test.ts}`
- Create: `.github/workflows/release.yml` (`workflow_dispatch` with an input `dry-run` that defaults to `true`; `permissions: id-token: write`; it collects the 8 artefacts from `ci.yml`, runs `pack-all`, then `npm publish --provenance --access public` for the 9 packages, with `--dry-run` unless the input is `false`)

**Interfaces:**
- Produces:
  - `packAll(artefactsDir: string, outDir: string): Promise<{ tarballs: string[] }>`. It copies each `.node` into `npm/<target>/`, regenerates `integrity.ts`, builds with tsdown, rewrites the `optionalDependencies` to the exact version, and runs `npm pack`.
  - `writePlaceholders(outDir: string): string[]`, which generates 9 `0.0.0` packages with a "reserved" README in a temp dir and commits nothing.

- [ ] **Step 1: Write the failing tests:**
  - `tarball_file_lists_are_exact`: the main package contains only `dist/**`, `package.json`, `README.md` and the licences, with no `src/**`. Each platform package contains only its `.node`, `package.json` and the licences.
  - `optional_dependencies_are_exact_pinned_to_main_version`
  - `integrity_module_matches_packed_binaries`
  - `no_install_scripts_in_any_tarball`
  - `placeholders_are_0_0_0_and_reserved`
- [ ] **Step 2: Run** `pnpm vitest run tools/pack`. Expected: FAIL.
- [ ] **Step 3: Implement** it. Then run `npx publint` and `npx @arethetypeswrong/cli --pack` on the packed main package (add both as root devDependencies and invoke them in `pack.test.ts`).
- [ ] **Step 4: Run** the tests. Then run `release.yml` with `dry-run: true` on the PR branch (`gh workflow run`). Expected: 9 packages packed, publint and attw clean, and the `npm publish --dry-run` output lists 9 packages.
- [ ] **Step 5: Owner checkpoint.** Give the owner the 9 placeholder tarballs (`node tools/pack/placeholders.ts --out $TMPDIR/placeholders`) and these steps:
  1. Create the npm org `blazehttp`.
  2. `npm publish --access public` each placeholder.
  3. On each package, configure the trusted publisher: GitHub repo `<login>/blazehttp`, workflow `release.yml`.

  Record completion in the P0 report.
- [ ] **Step 6: Commit** `Add packaging pipeline, release dry-run workflow and placeholder generator` and open a PR.

---

### Task 17: Wake-latency study and G1-low data (§9.4, §22)

**Files:**
- Create: `bench/harness/lowload.ts` (`lowload` subcommand: c ∈ {1, 2, 4, 8} × netem RTT ∈ {1 ms, 10 ms} × clients {undici, blaze-proto}), `docs/notes/wake-latency.md`
- Modify (only for strategies that win): `crates/engine-core/src/shard.rs`, `crates/node-binding/src/sink.rs`

- [ ] **Step 1: Baseline in the nightly window.** Run `pnpm bench lowload --rounds 10` and commit the results. Compute p50 and p99 deltas against undici with CIs, and compare them with the G1-low limits: p50 ≤ undici + 0.1 ms, p99 ≤ undici + 0.3 ms.
- [ ] **Step 2: Test strategies, one at a time.** Write each in the journal as idea → hypothesis → measurement → kept or dropped, and run each on its own branch. Measure the low-load cells and the G1 reference cell, so each strategy is checked for regressions. The candidates:
  1. On the shard thread, a short spin (≤ N µs, N swept over 0/20/50/100) on the command channel before parking.
  2. The tokio `current_thread` builder's `event_interval` and `global_queue_interval` (check docs.rs for the locked version).
  3. `SO_BUSY_POLL` on client sockets (check whether it needs `CAP_NET_ADMIN` above the sysctl; document it).
  4. Merge only measured wins. Each must keep every engine, binding and session test green.
- [ ] **Step 3: Write up the results.** `docs/notes/wake-latency.md` gets the final table of G1-low at 1 ms and 10 ms with the kept strategies, and a one-line owner-facing summary for the P0 report: met, or missed by X ms (§18, likely an owner decision after P1).
- [ ] **Step 4: Commit** `Measure low-load wake latency and keep winning strategies` (or `Record low-load wake latency baseline` if nothing won) and open a PR.

---

### Task 18: Memory measurements: RSS per pending request, allocator choice, burst recovery

**Files:**
- Create: `bench/memory/{pending-cost.ts, burst.ts}`, `.github/workflows/measure.yml` (`workflow_dispatch`; runs `pending-cost.ts` on all 8 targets and uploads JSON)
- Modify: `policy/spec.json` (`measured.pendingCostBytes`: `{ "<target>": <bytes> }`, data only; the generator emits it from P1a, when the budget code consumes it)
- Create: `docs/notes/allocator.md`

**Method:**
- **`pending-cost.ts`:**
  - Start a stalled server (`bench-server --stall`, or a `net` server that never answers when Docker is unavailable on a runner) in a child process.
  - In the measuring process, submit pending requests through `NativeSession` in steps of 10k up to 100k. Set `maxConnections` to 256 and `queueCapacity` to 200,000 so that the queue holds them.
  - After each step, force GC (`--expose-gc`), wait 1 s and sample RSS.
  - The result is the least-squares slope of RSS against pending count, in bytes per pending request, reported with R².
  - The P0 number under-counts the P2 facade objects. The P2 plan re-measures with the full client, and this is noted next to the value.
- **`burst.ts`:**
  - Concurrency c = 2 → 500 → 2 on the `/1k` cell, with RSS sampled every second.
  - It passes when RSS is within 10% of the pre-burst baseline 60 s after the burst (G2).
- **Allocator candidates (§11.1, §22):**
  - system allocator;
  - mimalloc with purge-after-idle (its `purge_delay` option; check the mimalloc docs);
  - on glibc, system + `malloc_trim(0)` after 30 s idle, called from `node-binding` with `// SAFETY:`.
  - Judge each by G0 reference-cell CPU µs (`engine-bench --features` per allocator) **and** burst recovery, per target class: Linux gnu, Linux musl (mimalloc `local_dynamic_tls` is fixed; confirm only), macOS, Windows.

- [ ] **Step 1: Implement** both scripts with one test each in `bench/memory/*.test.ts`:
  - `slope_of_linear_fit`: a pure helper with fixture points;
  - `recovery_check_within_10_percent`.
  Run them. Expected: PASS.
- [ ] **Step 2: Run** `measure.yml` on all 8 targets, and run `pending-cost` and `burst` on the reference host. Write `measured.pendingCostBytes` into `spec.json` with the R² in the journal.
- [ ] **Step 3: Run the allocator comparison** on the reference host, plus a CI runner for macOS and Windows (trend only). Decide per target in `docs/notes/allocator.md`. **Keep only the chosen allocator code** and delete the losing variants before merging (§14.2). Burst recovery here reflects the allocator only, because pool and Vec idle trimming arrive in P1a. Note this.
- [ ] **Step 4: Commit** `Measure pending-request memory cost and choose allocators per target` and open a PR.

---

### Task 19: Remaining P0 decisions: TLS provider, external memory, arena sizes, facade budget

**Files:**
- Modify: `crates/engine-bench/src/clients/raw.rs`. Add an HTTPS mode over tokio-rustls behind the features `tls-ring` and `tls-aws-lc`. After the decision, only the winner's feature remains.
- Modify: `policy/spec.json` `wire.arenaBytes` / `wire.batchRecords` (only if Step 3 changes them)
- Create: `docs/notes/tls-provider.md`, `docs/notes/external-memory.md`

- [ ] **Step 1: `ring` vs `aws-lc-rs`.**
  - Run the raw loop over HTTPS on `/1k` at c = 50, 10 rounds on the reference host, with CPU µs/req for each provider.
  - Add a temporary CI job that builds both providers on all 8 targets. It records whether each builds and how long it takes (`aws-lc-rs` needs extra toolchain on Windows). Remove the job after the decision.
  - Decide by CPU µs first and build portability second. Write the decision in `tls-provider.md`, citing the rustls docs for the locked version.
- [ ] **Step 2: `napi_adjust_external_memory` double counting.**
  - On a throwaway branch `exp/external-memory` (pushed, never merged; cite its SHA in the note), create an external `Buffer` through napi-rs with and without an explicit adjust.
  - Read `v8.getHeapStatistics().external_memory` and `process.memoryUsage().external` before and after, and after GC.
  - Decide whether P1a's external-buffer path must call adjust itself.
- [ ] **Step 3: Arena and batch sizes, and the small-body threshold.**
  - Sweep arena sizes {1, 4, 8} MiB × batch caps {1,024, 4,096} on the G1 reference cell and the `/1m` cell with `blaze-proto`.
  - Keep the §11.5 defaults unless a variant wins measurably. Record the result.
  - The small-body lane threshold (64 KiB, §11.1) is confirmed or retuned with the same sweep on bodies of 16, 64 and 256 KiB served through a `bench-server` query parameter `?size=`. Add it to the cells, then regenerate and commit `bench/cells.json`.
- [ ] **Step 4: Facade budget per core cell** (§16.3, §22).
  - Measure undici baselines for all 12 core cells: {plaintext, HTTPS} × {1k, 1k-h20, 1k-gzip} × {c = 50, 500}.
  - For each plaintext non-gzip cell, budget µs/req = (undici CPU µs/req ÷ 1.4) − blaze-proto CPU µs/req.
  - The HTTPS and gzip budgets need the engine's TLS and decompression. The report records their undici baselines now and the formula, and says they are computed in P1a when TLS lands. This is an explicit open item, not a silent gap.
  - Put the table in the journal.
- [ ] **Step 5: Commit** `Decide TLS provider and record P0 sizing and facade budget measurements` and open a PR.

---

### Task 20: G0 reference gate, P0 report, review gate and milestone PR

**Files:**
- Create: `docs/reports/p0.md`, `bench/results/<date>-arm64-ref-g0.jsonl` (and an x64 file if the VM is available)
- Modify: `docs/perf/journal.md`, `THIRD_PARTY_NOTICES` (regenerate)

- [ ] **Step 1: G0 reference cell gate run** in the nightly window on the reference host: `pnpm bench g0 --rounds 10 --cells 1k --work fnv --clients blaze,hyper,reqwest,ureq,isahc,raw`.
  - Load points are pairwise (Task 10): saturation is measured first, and the gate metric is CPU µs/req at the fixed offered load.
  - Every round must be valid (server < 60%, idle host).
  - **Acceptance:** `g0Verdict(blaze, hyper, 'engine-bound').pass`, meaning the CI lower bound ≥ 1.0.
  - Also report allocations per request (`count-allocs` run) and max throughput.
  - If it misses, keep optimising (owner rule, §15 gate misses). Every idea goes through the journal, and the run repeats. The owner may approve P0 functionality with the speed gate marked open.
- [ ] **Step 2: x64.** If the owner has provided the VM: start it, re-measure the spike prototype from tag `p0-spike-final` (the x64 reference-cell floor, §16.1), run the same G0 gate, stop the VM, and record the cost against the budget cap. Otherwise record "x64 pending owner action" in the report.
- [ ] **Step 3: Stale-code and docs pass.**
  - Run `pnpm knip && pnpm policy && cargo shear && cargo hack check --workspace --each-feature`. Expected: clean.
  - Every component built has a note in `docs/notes/`: response-parser, request-writer, shard-and-connection, completion-path, vectors, bench-method, xlang-baseline, wake-latency, allocator, tls-provider, external-memory, toolchain.
  - The journal has an entry per measurement task.
- [ ] **Step 4: Write `docs/reports/p0.md`.**
  - A table of every P0 acceptance criterion with its evidence: commit, results file, CI run URL.
    - The addon loads on 8 targets, including the Alpine load test.
    - The prototype is ≥ hyper in the G0 plaintext reference cell.
    - G1-low 1 ms data is on the gate host.
    - The cross-language baseline result, with the 1.10× trigger.
  - A §22 decisions table: every open item and what was decided, or why it stays open (HTTPS/gzip facade budgets, node-binding coverage).
  - The schedule deviation (wake study in week 2).
  - The owner decisions needed.
- [ ] **Step 5: Multi-lens review gate.** Dispatch five independent reviewers on the full `milestone/p0` diff, each given the design path, this plan and `.claude/CLAUDE.md`:
  1. performance;
  2. architecture and patterns (§13);
  3. security (§5.2, §12, supply chain);
  4. feasibility against P1a;
  5. builder (readability, DRY, stale code).

  Fix every blocker and high finding at its root, then re-review until none remain. Append the review report to `docs/reports/p0.md`.
- [ ] **Step 6: Milestone PR.** Run `gh pr create --base main --head milestone/p0 --title "P0 Foundations" --body-file docs/reports/p0.md`. Ask the owner to review and approve it. **Do not merge without the owner's approval.**
