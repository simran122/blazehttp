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
