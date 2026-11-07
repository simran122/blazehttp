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

### 2026-11-07: `ResponseParser` vs the fastest head parsers in Rust, C, C++, Zig and Go (cross-language bench, dev numbers)

- **Superseded by the "fix round 1" entry below.** This run compared our parser **without** the per-field split that
  every other parser hands its caller (our API returns the field block as one range), so its "we lead" claims do not
  compare like with like; C/C++ were built without `-DNDEBUG` (GCC 12.2), and the gate did not check the timed path.
  Kept as the record of that run.
- **Idea:** answer "how does our parser compare with the fastest parsers in every language?" on the same bytes and
  CPU: `bench/parsers/` (README there), one Docker image per language, differential gate before timing.
- **Hypothesis:** at P0 (safe scalar Rust) we lead on short heads, are at parity with httparse on 20-header heads,
  and are linear under 1-byte feeds where restart-style parsers are not.
- **Method:** `pnpm bench:parsers` (harness commit `d813c22` after the rebase onto the parser's fix round; the runner's "refuse if other containers run" check
  was added after this run and changes nothing measured). Linux arm64, Docker Desktop 28.5.1 on the owner's M2,
  kernel 6.11.11-linuxkit, every invocation `--cpus=1 --network=none`, no other container running. Same CPU baseline
  for all: generic Armv8-A (Rust `target-cpu=generic`, C/C++ `-march=armv8-a`, Zig `-mcpu=baseline`, Go
  `GOARM64=v8.0`); Rust release + fat LTO + 1 CGU (rustc 1.99.1), C/C++ GCC 12.2 `-O3 -flto`, Zig 0.17.1
  `ReleaseFast`, Go 1.27.3 default. Parsers: blazehttp `ResponseParser` (`p0/parser` @ `e65a950`), httparse 1.10.1,
  picohttpparser `dd573d8` (HEAD, 2026-10-27), llhttp 9.4.3 (strict), Boost.Beast 1.92.0 (`response_parser<empty_body>`
  and a `basic_parser<false>` subclass), Zig `std.http` (`HeadParser` + `Client.Response.Head.parse`), Go
  `net/http.ReadResponse`. Gate: identical status, head length, Content-Length, field count and field bytes on all 7
  workloads (passed). 7 interleaved rounds (start parser rotated per round), 100 ms warm-up + 300 ms timed per
  invocation; median of per-round ratios ours/theirs (< 1 = ours faster). **Host load average 3.7 → 16.7 during the
  run** (other work on the Mac, outside the nightly window): dev numbers, wide min–max, no gate claim.
- **Numbers (median ns/head; ours/theirs median, min–max):**

  | parser | 3 headers (109 B) | 20 headers (779 B) | rotated 64 (mixed) | allocs/head (3 / 20) |
  |---|---|---|---|---|
  | **blazehttp** (Rust) | **55.1** | **268.9** | 252.3 | 0 / 0 |
  | httparse (Rust) | 62.0 — 0.82 (0.47–1.26) | 300.3 — 0.96 (0.76–1.12) | **244.3** — 0.95 (0.83–2.85) | 0 / 0 |
  | picohttpparser (C) | 60.8 — 0.86 (0.52–1.01) | 390.8 — 0.65 (0.17–0.89) | 326.3 — 0.67 (0.59–2.14) | 0 / 0 |
  | llhttp (C) | 84.3 — 0.62 (0.39–0.93) | 390.9 — 0.66 (0.57–0.86) | 339.1 — 0.70 (0.53–2.08) | 0 / 0 |
  | Beast `basic_parser` (C++) | 117.1 — 0.50 | 614.5 — 0.43 | 504.9 — 0.45 | 0 / 0 |
  | Beast `response_parser` (C++) | 186.6 — 0.28 | 1322.5 — 0.19 | 1970.4 — 0.11 | 4 / 21 |
  | Zig `std.http` | 358.2 — 0.16 | 2329.2 — 0.12 | 1975.5 — 0.11 | 0 / 0 |
  | Go `net/http` | 909.3 — 0.06 | 4081.7 — 0.07 | 3415.2 — 0.07 | 10 / 40 |

  1-byte feeds (median ms, (a) 60 KB value / (b) 99 × 600 B / (c) 60 KB name / (d) 8 KiB status line): blazehttp
  0.36 / 0.53 / 0.56 / 0.079; picohttpparser (re-parses once complete, `last_len` scan meanwhile) 0.50 / 0.55 / 0.77 /
  **0.068**; llhttp 0.42 / 0.60 / 0.77 / 0.086; Zig 0.47 / 0.64 / 0.63 / 0.082; Go 1.42 / 2.52 / 2.63 / 0.25;
  httparse (re-parses from byte 0) 175 / 235 / 344 / 118; Beast (keeps lines, re-parses a partial line)
  1,009 / 25 / 2,085 / 38 (`response_parser`), similar for `basic_parser`.
- **Where we win / lose (plain row only; see the caveat at the top):** fastest by median on the 3-header head (0.82× httparse, 0.86× picohttpparser;
  single rounds range 0.47–1.26) and on the 20-header head (0.96× httparse, ≈ 0.65× the C parsers); 2–16× faster than the C++, Zig
  and Go parsers, which allocate (Beast: one per field plus one, Go map) or split lines generically (Zig). **Parity, not a lead, with
  httparse** on 20 fields and on the mixed rotation (median ratio 0.95–0.96 but httparse's median ns on the rotation is
  lower, 244 vs 252, and the ranges overlap): this is the gap P1a must close. On feeds we are linear like llhttp,
  picohttpparser and Zig; picohttpparser is marginally faster on (d) (0.068 vs 0.079 ms, within the run's noise).
- **What P1a SIMD must close:** a clear lead over httparse on 20-field and mixed heads (target ≤ 0.85× on `1k-h20`,
  note §8.4) on arm64 and x64. x64 is **not measured yet** (deferred to the owner's x64 VM); there picohttpparser's
  SSE4.2 paths switch on, so its standing will change.
- **Kept / dropped:** bench kept (`bench/parsers/`); results `bench/results/2026-11-07-arm64-dev-parsers.jsonl`.
- **Commit:** `p0/parser-bench`.

### 2026-11-07: Cross-language parser bench, fix round 1 (`blazehttp+fields`, gated timed path, GCC 14, dev numbers)

- **Idea:** compare like with like. Every other parser hands its caller each field; ours returns the field block as one
  offset range. New row `blazehttp+fields` = `ResponseParser` + a zero-allocation name/value split into a reused span
  array (`bench/parsers/rust/src/engines.rs` `split_fields`, `memchr` per line), the output httparse gives. Also:
  the gate checks the timed `parse` path (status, head length, length, keep-alive) against the input bytes and ours,
  plus a chunked / `Connection: close` / trailing-OWS head; C/C++ with GCC 14.2 and `-DNDEBUG`, direct calls in every
  timed loop; containers pinned with `--cpuset-cpus=2`; the 64-variant rings of each fixture timed as well.
- **Hypothesis:** with the split, ours is still ahead of httparse on short heads and at parity on 20 fields.
- **Method:** `pnpm bench:parsers --out …-gcc14.jsonl` at commit `12651b2` (clean tree), then `--cc gcc-12` for the
  C/C++ rows. Linux arm64 Docker Desktop 28.5.1 on the owner's M2, `--cpus=1 --cpuset-cpus=2 --network=none`, no
  other container at start; 7 interleaved rounds × 300 ms after 100 ms warm-up; feeds 5 rounds. Gate passed on all 10
  workloads in both runs (it caught one real difference while being built: llhttp keeps trailing OWS in values; its
  field view now trims it as a consumer must). **Host load average 8–33 during both runs** (other agents' builds and
  load tests outside the containers): absolute ns are ≈ 3× a quiet run (ours on `1k.http`: 149 ns in the run vs 47 ns
  measured alone just before), so only the **per-round ratios** are usable, and their ranges are wide.
- **Numbers (GCC 14 run; median per-round ratio blazehttp+fields / theirs, > 1 = theirs faster; workloads `1k` /
  `1k-h20` / `1k` × 64 / `1k-h20` × 64 / mixed 64):**

  | vs | ours+fields / theirs | plain ours / theirs |
  |---|---|---|
  | httparse | **1.16 / 1.64 / 1.73 / 1.96 / 2.02** | 0.70 / 0.89 / 0.87 / 1.08 / 0.86 |
  | picohttpparser | **1.21 / 1.33 / 1.78 / 1.34 / 1.57** | 0.78 / 0.74 / 0.80 / 0.71 / 0.73 |
  | llhttp | **1.27 / 1.08 / 1.13 / 1.13 / 1.54** | 0.56 / 0.63 / 0.66 / 0.65 / 0.79 |
  | Beast `basic_parser` | 0.86 / 0.51 / 0.95 / 0.93 / 0.89 | 0.47 / 0.31 / 0.56 / 0.46 / 0.54 |
  | Beast `response_parser` | 0.33 / 0.35 / 0.57 / 0.42 / 0.35 | 0.23 / 0.22 / 0.30 / 0.23 / 0.19 |
  | Zig `std.http` | 0.22 / 0.19 / 0.27 / 0.23 / 0.21 | 0.15 / 0.11 / 0.18 / 0.13 / 0.14 |
  | Go `net/http` | 0.08 / 0.10 / 0.07 / 0.11 / 0.10 | 0.04 / 0.06 / 0.04 / 0.05 / 0.07 |

  The split itself costs as much as the parse: plain / with-fields = 0.50–0.63. Allocations per head: 0 for every
  parser except Beast `response_parser` (4 / 21 / 15.2) and Go (10 / 40 / 31.7). Feeds (median ms, (a)/(b)/(c)/(d)):
  ours 0.52 / 0.66 / 0.41 / 0.076 (with fields 0.49 / 0.54 / 0.40 / 0.097), picohttpparser 0.56 / 0.56 / 0.74 / 0.082,
  llhttp 0.62 / 0.73 / 0.52 / 0.073, Zig 0.73 / 0.65 / 0.60 / 0.069, Go 2.05 / 2.03 / 1.95 / 0.30, httparse
  252 / 429 / 314 / 161, Beast 25 ms–2.2 s. **GCC 12.2 vs 14.2** (same rows, second run): no compiler effect resolvable
  above this noise (picohttpparser ratio on `1k-h20` 1.08 vs 1.33, llhttp 1.37 vs 1.08: they move in opposite
  directions between runs).
- **Where we win / lose (headline, `blazehttp+fields`):** **httparse is ahead on every workload** (ours+fields takes
  1.2–2.0× its time), and so are picohttpparser (1.2–1.8×) and llhttp (1.1–1.5×). We stay ahead of Beast (1.05–3× faster
  across its two APIs), Zig `std.http` (3.7–5.4×) and Go `net/http` (9–14×). Without the split (`blazehttp`, what the engine pays when it needs
  only the framing facts) we are ahead of all of them except httparse on the 20-field ring (1.08). On 1-byte feeds we
  are linear and on par with the other resumable parsers; httparse and Beast are quadratic.
- **What P1a must close:** the second pass. Our parser already finds every colon and line end; a span-emitting API
  (names/values recorded during the one scan) removes the split, and SIMD scanning must then put us ahead of httparse:
  target ≤ 0.85× httparse on `1k-h20` **with fields**. x64 still deferred (picohttpparser's SSE4.2 path is off on arm64).
- **Kept / dropped:** `blazehttp+fields` kept as the headline row; runs kept in
  `bench/results/2026-11-07-arm64-dev-parsers-gcc14.jsonl` and `…-gcc12.jsonl`. Re-run both in a quiet window.
- **Commit:** `p0/parser-bench` (fix round 1).
