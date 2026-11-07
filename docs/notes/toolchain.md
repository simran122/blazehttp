# Toolchain

- Rust stable pinned: **1.99.1** (rustc 58cf8f9c3, 2026-10-13), the stable release current when P0 Task 1 ran.
- MSRV = 1.99.1.
- Edition 2024: see https://doc.rust-lang.org/edition-guide/rust-2024/index.html.
- Nightly only for Miri and `cargo-fuzz`, on a pinned nightly date (design §6.3).
- **Pinned nightly: `nightly-2026-11-06`** (rustc 1.101.0-nightly 249c1e4e3 2026-11-05), for `cargo-fuzz` (cargo-fuzz
  0.13.x, `cargo +nightly-2026-11-06 fuzz run h1_response`) and Miri. Fuzz crates are their own workspaces
  (`crates/engine-core/fuzz`, excluded from the main one) and build with debug assertions and overflow checks on.
