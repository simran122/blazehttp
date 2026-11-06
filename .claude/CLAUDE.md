# blazehttp — project instructions

## What this is
blazehttp: public npm HTTP client with an axios-style TypeScript API and our **own HTTP/1.1 engine written from scratch in Rust** (napi-rs binding), undici fallback transport, fetch/XHR for browsers. Own HTTP/2 comes in version 1.1.

**Source of truth:** `docs/specs/2026-11-06-blazehttp-design.md` (reviewed in 10 rounds; read it fully before planning or coding). Evidence and prototypes: `spike/`.

## Status and next step
- Design complete; confirm the owner's approval before planning.
- Next: write the **P0 build plan** (weeks 1–3) from the design using the `superpowers:writing-plans` skill, then build P0 through the review gate (§16 of the design).

## Owner decisions (do not change without asking)
- Name `blazehttp`; licence MIT OR Apache-2.0; public repo on the owner's GitHub; owner's npm account and npm org `blazehttp` (`@blazehttp/<target>` platform packages).
- Engine from scratch: never use hyper/reqwest/undici as the engine; only infrastructure crates (tokio, rustls, decoders, url/idna, small utilities, napi-rs).
- Headline goal G0: our engine beats the fastest HTTP client engines in every language (Rust, C, C++, Zig, Go) in a fair harness. Full Node client ≥ 63,790 req/s at 1 CPU and #1 among Node clients. RAM bounded and auto-sized; works in 128 MiB.
- One native engine at every load; missed speed gate → keep optimising, no release on a fallback.
- Built by Claude Code with sub-agents; owner approves phases.

## Rules (enforced by CI per the design)
- 10/10 quality: no N+1, no time/space complexity issues, DRY, readable, OOP, genuine GoF patterns (design §13).
- No deprecated libraries, functions or APIs anywhere (design §14.1). No stale or unused code — delete it (§14.2; knip, cargo-shear, `-D warnings`).
- Rust knowledge from the official Rust project docs first (doc.rust-lang.org, rust-lang.github.io) and cite them (§6.3). Edition 2024, stable toolchain (Miri/fuzz on a pinned nightly only).
- Learn from proven code (hyper, undici, llhttp, picohttpparser, curl, Go net/http, axios…), port their test cases as data vectors with licence notices, never copy code verbatim (§6).
- Always look for speed: measure every idea, keep only measured wins, record in `docs/perf/journal.md` (§7).
- Every phase passes the multi-lens review gate (performance, architecture/patterns, security, feasibility, builder) until no blocker/high remains.

## Git (this repo only — overrides the owner's global one-step rule)
- Claude may commit, push feature branches and open PRs on its own.
- Small PRs merge into a **milestone branch** after green CI + agent review gate.
- The owner approves one milestone PR into `main` every 1–2 weeks (with summary + review report) and every release.
- Commit messages: imperative mood, describe the change, never mention Claude/AI/agents. `main` protected.

## Benchmarks
- arm64 reference: the owner's Mac, Docker Desktop at 8 CPUs, **only in the fixed nightly window** (owner keeps other containers stopped — never stop the owner's containers yourself).
- x64: cloud VM that Claude may start/stop itself within the owner's monthly budget cap.

## Owner actions pending (P0)
Create npm org `blazehttp` + OIDC trusted publishing; choose the nightly benchmark window; provide the x64 VM access and budget cap. (Docker at 8 CPUs: done.)
