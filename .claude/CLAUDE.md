# blazehttp — project instructions

## What this is
blazehttp: public npm HTTP client with an axios-style TypeScript API and our **own HTTP/1.1 engine written from scratch in Rust** (napi-rs binding), undici fallback transport, fetch/XHR for browsers. Own HTTP/2 comes in version 1.1.

**Source of truth:** `docs/specs/2026-11-06-blazehttp-design.md` (reviewed in 10 rounds; read it fully before planning or coding). Evidence and prototypes: `spike/` (P0 Task 14 deletes the folder; the tag `p0-spike-final` keeps it).

## Status and next step
- Design complete. The **P0 build plan** is `docs/superpowers/plans/2026-11-06-p0-foundations.md`: its "Plan status" table says which tasks are merged, open or not started, and its "Process rules" bind every task.
- Next: build P0 task by task from that plan (`implementation-plan` skill), through the phase review gate (plan Task 20; design §16). Performance work follows the `performance-engineering` skill (prior-art research, measured wins only, `docs/perf/journal.md`).

## Owner decisions (do not change without asking)
- Name `blazehttp`; licence MIT OR Apache-2.0; public repo on the owner's GitHub; owner's npm account and npm org `blazehttp` (`@blazehttp/<target>` platform packages).
- Engine from scratch: never use hyper/reqwest/undici as the engine; only infrastructure crates (tokio, rustls, decoders, url/idna, small utilities, napi-rs).
- Headline goal G0: our engine beats the fastest HTTP client engines in every language (Rust, C, C++, Zig, Go, Java, .NET) in a fair harness. Full Node client ≥ 63,790 req/s at 1 CPU and #1 among Node clients. RAM bounded and auto-sized; works in 128 MiB.
- One native engine at every load; missed speed gate → keep optimising, no release on a fallback.
- Built by Claude Code with sub-agents; owner approves phases.
- Later decisions (2026-11-07 to 2026-11-13; full text in the plan's "Process rules", recorded in design §21):
  - First in every benchmark, component and cell, shared-limit cells included. A lost cell is an open speed item with an owner task.
  - `ResponseParser` and `write_request` are each faster than every rival with equal work, on arm64 and x64 (statistic: design §16.4 "Component micro-gates").
  - A task merges only after a verification pass reports zero findings, nits included, and meets the per-task definition of done.
  - Research before code: every task writes its `docs/notes/<component>.md` first. Our own algorithms: study the fastest code, rewrite, never copy.
  - Machine resource limits: at most 2 helper agents alive; fuzzing, mutation testing, benchmarks and long tests only one at a time in a memory-capped container.
  - x64 runs stay inside the owner's monthly cap ($20, about 10 instance-hours); the VM is stopped after every run and never terminated.
- Open owner decisions are listed in design §21 "Open owner decisions" and in the plan's "Owner decisions needed". No agent decides them.

## Rules (enforced by CI per the design)
- 10/10 quality: no N+1, no time/space complexity issues, DRY, readable, OOP, genuine GoF patterns (design §13).
- No deprecated libraries, functions or APIs anywhere (design §14.1). No stale or unused code — delete it (§14.2; knip, cargo-shear, `-D warnings`).
- Rust knowledge from the official Rust project docs first (doc.rust-lang.org, rust-lang.github.io) and cite them (§6.3). Edition 2024, stable toolchain (Miri/fuzz on a pinned nightly only).
- Learn from proven code (hyper, undici, llhttp, picohttpparser, curl, Go net/http, axios…), port their test cases as data vectors with licence notices, never copy code verbatim (§6).
- Always look for speed: measure every idea, keep only measured wins, record in `docs/perf/journal.md` (§7).
- Every task is reviewed until a verification pass reports zero findings of any severity (plan "Process rules").
- Every phase also passes the multi-lens review gate (performance, architecture/patterns, security, feasibility, builder) until no blocker/high remains; a medium or low finding from the phase gate is fixed in the same round when its fix is local, and otherwise listed in the phase report with an owner task, never dropped.

## Git (this repo only — overrides the owner's global one-step rule)
- Claude may commit, push feature branches and open PRs on its own.
- Small PRs merge into a **milestone branch** after green CI + agent review gate.
- The owner approves one milestone PR into `main` every 1–2 weeks (with summary + review report) and every release.
- Commit messages: imperative mood, describe the change, never mention Claude/AI/agents. `main` protected.

## Benchmarks
- arm64 reference: the owner's Mac, Docker Desktop at 8 CPUs. **Gate runs only in the fixed nightly window** (owner keeps other containers stopped — never stop the owner's containers yourself). Dev runs outside the window are labelled as dev runs and are never gate evidence.
- x64: cloud VM that Claude may start/stop itself within the owner's monthly budget cap; every run is logged in `bench/aws-cost-log.md`.
- Never run a benchmark while other agents may be running: the quiet-machine rule in the plan's "Process rules".

## Owner actions pending (P0)
Create npm org `blazehttp` + OIDC trusted publishing; choose the nightly benchmark window; set an AWS budget alert. (Docker at 8 CPUs: done 2026-11-06. x64 VM and its budget cap: done 2026-11-07.)
