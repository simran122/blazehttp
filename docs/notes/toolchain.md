# Toolchain

- Rust stable pinned: **1.99.1** (rustc 58cf8f9c3, 2026-10-13), the stable release current when P0 Task 1 ran.
- MSRV = 1.99.1.
- Edition 2024: see https://doc.rust-lang.org/edition-guide/rust-2024/index.html.
- Nightly only for Miri and `cargo-fuzz`, on a pinned nightly date (design §6.3).
- **Pinned nightly: `nightly-2026-11-06`** (rustc 1.101.0-nightly 249c1e4e3 2026-11-05), for `cargo-fuzz` (cargo-fuzz
  0.13.x, `cargo +nightly-2026-11-06 fuzz run h1_response`) and Miri. Fuzz crates are their own workspaces
  (`crates/engine-core/fuzz`, excluded from the main one) and build with debug assertions and overflow checks on.
- **Where heavy tools run** (plan "Process rules", resource limits): `cargo fuzz run` and `cargo mutants` never run
  on the host, and neither does a Miri run of more than about 30 s. Each runs alone in a capped container
  (`docker run --rm --memory=2g --memory-swap=2g --cpus=1 …`, source mounted read-only and copied inside); libFuzzer
  gets `-rss_limit_mb=1536`, below the container's 2 GiB, so that it stops by itself and writes the reproducer before
  the kernel kills the container (2048 is libFuzzer's default and equals the container limit); cargo-mutants runs
  with one job, at most 2 test threads and a test timeout of about 20 s.
  The fuzz command above is the command inside that container. By owner decision (2026-11-13) mutation testing and
  fuzzing move to GitHub Actions CI; a separate PR adds the workflow, and the capped container then stays as the
  local fallback (plan "Process rules", resource limits).
- **cargo-mutants:** no version is pinned yet. P0 Task 4d records the version it uses here when it first runs the
  mutation pass.
