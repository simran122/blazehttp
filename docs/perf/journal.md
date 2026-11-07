# Performance journal

Record every measured idea. Keep only measured wins.

## Entry template

### YYYY-MM-DD: <idea>

- **Idea:**
- **Hypothesis:**
- **Method:**
- **Numbers (with 95% CI):**
- **Kept / dropped:**
- **Commit:**

### 2026-11-06: Response head parser design (scratch prototype, dev numbers)

- **Idea:** fused span scan resumable at any byte (design G) vs line-at-a-time `memchr` + validate (C), byte state
  machine (A), httparse 1.10.1. Detail: `docs/notes/response-parser.md` §5, §8.
- **Hypothesis:** G is linear under any split and at least as fast as httparse.
- **Method:** scratch `parser-bench` (not committed), Linux arm64 Docker on the owner's M2, `--cpus=1`, rustc 1.99.1,
  fat LTO; 9 × 2 M iterations, median per run; 3 runs; outside the nightly window (host load 2.5–4), one identical
  head per loop. Dev numbers only: no CI, no confidence interval; the committed `examples/h1_parse_bench.rs` (Task 4)
  and P1a's criterion bench replace them.
- **Numbers (median of 3 runs; 3 headers / synthetic 20 / `/1k-h20` 794 B):** G 46.6 / 280.7 / 270.9 ns; C + SWAR
  53.3 / 351.9 / 306.9 ns; A 77.2 / 666.4 / 577.0 ns; httparse NEON 53.9 / 292.1 / 292.9 ns. 60 KB fed 1 B at a
  time: G 0.26 ms, httparse 67 ms.
- **Kept / dropped:** G kept (parity with httparse on 20 headers, 0.86× on 3); C dropped; `memchr_iter` dropped (no
  difference); `memchr` later dropped from the design (status line fused; unmeasured until Task 4's bench).
- **Commit:** none (research note only).

### 2026-11-06: Target CPU changes reference rankings (dev numbers)

- **Idea:** check whether the macOS `apple-m1` default target CPU skews comparisons.
- **Method:** same source built for macOS default and `-C target-cpu=generic`, one run each.
- **Numbers:** httparse 20-header head 439–458 ns (`apple-m1`) vs 282–284 ns (generic); our designs moved < 7%.
- **Kept / dropped:** rule kept: build every engine for the shipped target CPU in the same run (design §16.2).
- **Commit:** none.

### 2026-11-06: Bounds-check removal in safe Rust (assembly audit)

- **Idea:** `as_chunks::<8>()` and `get` instead of `chunks_exact` + indexing and `buf[a..b]` by stored offsets.
- **Method:** scratch `asm-audit`, `--emit asm` for aarch64-apple-darwin and x86_64-unknown-linux-gnu; listed every
  call target.
- **Numbers:** only `buf[pos..lf]` kept a panic call (`slice_index_fail`) on both targets; all other forms none.
- **Kept / dropped:** `get` for stored offsets, `as_chunks` for SWAR (kept); no runtime numbers.
- **Commit:** none.

### 2026-11-06: `ResponseParser` (Task 4) vs httparse 1.10.1 (committed bench, dev numbers)

- **Idea:** the built design G (fused span scan, byte-wise status prefix, no `memchr`) against httparse 1.10.1 +
  the same fact extraction, on the shared fixtures `tests/fixtures/h1-heads/*.http`.
- **Hypothesis:** P0 budget (`docs/notes/response-parser.md` §8.4): median ratio ≤ 0.95× on `1k.http`, ≤ 1.05× on
  `1k-h20.http`; every 1-byte feed ≤ 1 ms; the byte-wise status line does not regress against the prototype's
  `memchr` status line.
- **Method:** `cargo run --release -p engine-core --example h1_parse_bench` (fat LTO, `codegen-units = 1`); per
  fixture 64 same-length variants cycled, 100 ms warm-up per parser, 7 runs × 4 interleaved blocks × 50,000 parses,
  median of per-run ratios; the binary was run 3 times per host. Linux arm64: Docker Desktop on the owner's M2,
  `rust:1-bookworm` (rustc 1.99.1), `--cpus=1` (`cpu.max = 100000 100000`), kernel 6.11.11-linuxkit, generic
  `aarch64` target CPU, no other container running. macOS: same M2, default target CPU (`apple-m1`), plus one run
  with `-C target-cpu=generic`. Host load average 4.7–5.5 (outside the nightly window): dev numbers, no gate claim.
- **Numbers (Linux arm64, 3 runs; ours / httparse ns, median ratio):** `1k.http` 45.0 / 52.0 (0.841), 44.1 / 52.4
  (0.840), 43.1 / 51.8 (0.830); `1k-h20.http` 228.9 / 235.5 (0.963), 231.3 / 236.2 (0.976), 229.7 / 237.3 (0.966);
  `synthetic-h20.http` 0.939 / 0.935 / 0.929. 1-byte feeds: (a) 60 KB value 0.18–0.21 ms (httparse 68–70 ms),
  (b) 99 × 600 B 0.20–0.21 ms, (c) 60 KB name 0.19–0.20 ms, (d) 8 KiB status line 0.03–0.08 ms.
  **macOS `apple-m1`:** ratios 0.64–0.65 / 0.65–0.66 / 0.55–0.56 (httparse is slowed by this target CPU, as found in
  the research); **macOS generic:** 0.875 / 0.966 / 0.955. Feeds on macOS: 0.17–0.21 ms; httparse (a) 67–69 ms.
- **Kept / dropped:** kept. Budget met on the Linux reference container (0.83–0.84× and 0.96–0.98×, feeds ≤ 0.21 ms).
  The byte-wise status prefix did not regress: 3-header head 43–45 ns vs the prototype's 46.6 ns with `memchr`.
- **Commit:** Task 4 PR (`p0/parser`).

### 2026-11-07: Scan seam `token_end`/`value_end(window, pos)` (SIMD survey §7 item 3)

- **Idea:** phrase the parser's span scans as `end = token_end(window, pos)` / `value_end(window, pos)` so P1a's
  bitmaps drop in without touching the phase logic.
- **Hypothesis:** behaviour-neutral and not slower.
- **Method:** same-container A/B on Linux arm64 (Docker, `--cpus=1`, rustc 1.99.1, generic aarch64): base and
  candidate built side by side, binaries alternated, committed bench (7 runs × 4 blocks, median per-run ratio vs
  httparse 1.10.1 + facts) incl. the new varied ring. Host load 6–26 (busy; quietest rounds used).
- **Numbers (median ratio, quiet rounds):** `1k-h20.http` base 0.958 / 0.964 vs seam 0.988 / 0.997 (≈ +3%);
  with the seam reverted the same tree measured 0.959–0.977 vs base 0.950–0.982 (neutral). `1k.http` and the varied
  ring were neutral.
- **Kept / dropped:** dropped (≈ 3% slower on `1k-h20`); P1a adds the seam with the bitmaps if their shape needs it.
- **Commit:** none (reverted before commit).

### 2026-11-07: `value_len` jumps to the lowest SWAR flag with `trailing_zeros` (SIMD survey §7 item 2)

- **Idea:** the lowest SWAR flag is exact (borrows only propagate upward), so `trailing_zeros(flags) / 8` is the stop
  byte unless it is HTAB; drop the per-word `position` re-scan.
- **Hypothesis:** a win on varied heads (unpredictable stop lanes), no regression on `1k`/`1k-h20`.
- **Method:** same-container A/B as above (base = the parser without the seam), 4 alternated rounds, N = 7 runs each.
- **Numbers (median ratio per round, base → candidate):** varied ring 0.814 / 0.807 / 0.808 / 0.843 →
  **0.661 / 0.672 / 0.666 / 0.663** (≈ 18% less time); `1k.http` 0.823 / 0.819 / 0.795 / 0.819 → 0.790 / 0.811 /
  0.822 / 0.804 (neutral); `1k-h20.http` 0.962 / 0.982 / 0.960 / 0.964 → 0.963 / 0.967 / 0.955 / 0.958 (neutral);
  `synthetic-h20.http` 0.883 / 0.930 / 0.932 / 0.928 → 0.963 / 0.962 / 0.973 / 0.961 (≈ +3–4%: long uniform values,
  not a keep criterion).
- **Kept / dropped:** kept (win on the varied ring, no regression on `1k`/`1k-h20`). Exhaustive lane and pair unit
  tests and the fuzz differential cover correctness.
- **Commit:** fix round 1 (`p0/parser`).

### 2026-11-07: `ResponseParser` after fix round 1 (committed bench, dev numbers)

- **Idea:** re-measure the parser after fix round 1 (single-source status prefix, `trailing_zeros` value scan) with the
  varied ring added.
- **Hypothesis:** P0 budget still met (≤ 0.95× `1k.http`, ≤ 1.05× `1k-h20.http`, feeds ≤ 1 ms).
- **Method:** as on 2026-11-06 (`cargo run --release -p engine-core --example h1_parse_bench`, 7 runs × 4 blocks,
  median per-run ratio), binary run 3× per host. Linux arm64: Docker on the owner's M2, `--cpus=1`, rustc 1.99.1,
  generic `aarch64`, no other container running. macOS: default `apple-m1` target CPU. Host load 4–7 (dev numbers).
- **Numbers (Linux arm64, 3 runs; median ratio):** `1k.http` 0.824 / 0.822 / 0.792 (43–44 ns); `1k-h20.http` 0.953 /
  0.962 / 0.962 (227–250 ns); `synthetic-h20.http` 0.951 / 0.965 / 0.963; varied ring 0.650 / 0.669 / 0.666 (403–413
  ns vs httparse 614–622 ns). Feeds: (a) 0.19–0.38 ms (httparse 67–73 ms), (b) 0.21–0.25, (c) 0.18–0.23, (d)
  0.03–0.04 ms. **macOS `apple-m1`:** 0.64–0.66 / 0.66 / 0.57 / 0.69–0.70; feeds 0.18–0.22 ms (httparse (a) 65–67 ms).
- **Kept / dropped:** budget met on the Linux reference container.
- **Commit:** fix round 1 (`p0/parser`).


### 2026-11-07: bench-server runtime: multi-thread vs per-core `current_thread` + `SO_REUSEPORT`

- **Idea:** one tokio multi-thread runtime (design A) vs one `current_thread` runtime and one `SO_REUSEPORT` listener
  per core (design B). Detail: `docs/notes/bench-server.md` §4, §7.
- **Hypothesis:** B costs less server CPU per request (no cross-thread wakes or work stealing).
- **Method:** Linux arm64, Docker Desktop on the owner's M2 (8 CPUs), server `--cpus=4 --threads 4`, wrk 4.1.0
  `--cpus=3 -t3 -c50` in the server's network namespace, `GET /1k`, 5 s warm-up + 20 s, server CPU from its cgroup
  `cpu.stat`; A and B alternated, 3 rounds each. Host **not idle** (load average 14–28 from other work), so dev only.
- **Numbers (no CI, n = 3):** server CPU per request A 10.17 / 9.74 / 10.64 µs (median 10.17), B 11.05 / 10.13 /
  13.67 µs (median 11.05); 85–87% of it is kernel time in both.
- **Kept / dropped:** A kept (simpler, no measured loss); B dropped.
- **Commit:** see the PR that adds `crates/bench-server`.
