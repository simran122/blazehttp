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
- **Method:** `pnpm bench:parsers` (measured at `ac7d0b4`, which rebases made unreachable; the same harness code is `5797c66` on `p0/parser-bench`, and these results are committed in `4955f1c`; the runner's "refuse if other containers run" check
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

- **Superseded by the 2026-11-08 entry below** for every number; kept as the record of that run.
- **Idea:** compare like with like. Every other parser hands its caller each field; ours returns the field block as one
  offset range. New row `blazehttp+fields` = `ResponseParser` + a zero-allocation name/value split into a reused span
  array (`bench/parsers/rust/src/engines.rs` `split_fields`, `memchr` per line), the output httparse gives. Also:
  the gate checks the timed `parse` path (status, head length, length, keep-alive) against the input bytes and ours,
  plus a chunked / `Connection: close` / trailing-OWS head; C/C++ with GCC 14.2 and `-DNDEBUG`, direct calls in every
  timed loop; containers pinned with `--cpuset-cpus=2`; the 64-variant rings of each fixture timed as well.
- **Hypothesis:** with the split, ours is still ahead of httparse on short heads and at parity on 20 fields.
- **Method:** `pnpm bench:parsers --out …-gcc14.jsonl` at commit `12651b2` as recorded in the JSONL (clean tree; it is `17239b2` on `p0/parser-bench` after the rebase onto `milestone/p0`, identical tree), then `--cc gcc-12` for the
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
  | picohttpparser | **1.21 / 1.32 / 1.78 / 1.34 / 1.57** | 0.78 / 0.74 / 0.80 / 0.71 / 0.73 |
  | llhttp (no field delivery; re-measured 2026-11-08) | 1.27 / 1.07 / 1.13 / 1.13 / 1.54 | 0.56 / 0.63 / 0.66 / 0.65 / 0.79 |
  | Beast `basic_parser` (no field delivery; re-measured 2026-11-08) | 0.86 / 0.51 / 0.95 / 0.93 / 0.89 | 0.47 / 0.31 / 0.56 / 0.45 / 0.54 |
  | Beast `response_parser` | 0.33 / 0.35 / 0.57 / 0.42 / 0.35 | 0.23 / 0.22 / 0.30 / 0.23 / 0.18 |
  | Zig `std.http` | 0.22 / 0.19 / 0.27 / 0.23 / 0.20 | 0.15 / 0.11 / 0.18 / 0.13 / 0.14 |
  | Go `net/http` | 0.08 / 0.10 / 0.07 / 0.11 / 0.10 | 0.04 / 0.06 / 0.04 / 0.05 / 0.07 |

  **Fix round 2 notes:** the split helper is plain scalar code (`memchr` for each line's CR, then `.position()` for
  the colon and a scalar OWS trim), so "the split costs as much as the parse" partly measures this helper, not only
  the second pass itself. The workloads written `1k` × 64 / `1k-h20` × 64 here are the `*-ring64` workloads (the
  JSONL of this run still names them `*-x64`). The llhttp and Beast `basic_parser` rows above were measured without
  field delivery (llhttp registered no field callbacks; Beast's callback recorded nothing), so both their ratio columns
  (with fields and plain) compare unequal work; the 2026-11-08 entry re-measures them with fields.
  The split itself costs as much as the parse: plain / with-fields = 0.50–0.63. Allocations per head: 0 for every
  parser except Beast `response_parser` (4 / 21 / 15.2) and Go (10 / 40 / 31.7). Feeds (median ms, (a)/(b)/(c)/(d)):
  ours 0.52 / 0.66 / 0.41 / 0.076 (with fields 0.49 / 0.54 / 0.40 / 0.097), picohttpparser 0.56 / 0.56 / 0.74 / 0.082,
  llhttp 0.62 / 0.73 / 0.52 / 0.073, Zig 0.73 / 0.65 / 0.60 / 0.069, Go 2.05 / 2.03 / 1.95 / 0.30, httparse
  252 / 429 / 314 / 161, Beast 25 ms–2.2 s. **GCC 12.2 vs 14.2** (same rows, second run): no compiler effect resolvable
  above this noise (picohttpparser ratio on `1k-h20` 1.08 vs 1.32, llhttp 1.37 vs 1.07: they move in opposite
  directions between runs).
- **Where we win / lose (headline, `blazehttp+fields`; by median per-round ratio, unresolved at this noise until the
  quiet re-run):** **httparse is ahead on every workload** (ours+fields takes
  1.2–2.0× its time), and so is picohttpparser (1.2–1.8×). llhttp's row delivered no fields here; see the 2026-11-08
  entry. We stay ahead of Beast `response_parser` (1.75–3× faster), Zig `std.http` (3.7–5.4×) and Go `net/http` (9–14×). Without the split (`blazehttp`, what the engine pays when it needs
  only the framing facts) we are ahead of all of them except httparse on the 20-field ring (1.08). On 1-byte feeds we
  are linear and on par with the other resumable parsers; httparse and Beast are quadratic.
- **What P1a must close:** the second pass. Our parser already finds every colon and line end; a span-emitting API
  (names/values recorded during the one scan) removes the split, and SIMD scanning must then put us ahead of httparse:
  target ≤ 0.85× httparse on `1k-h20` **with fields**. x64 still deferred (picohttpparser's SSE4.2 path is off on arm64).
- **Kept / dropped:** `blazehttp+fields` kept as the headline row; runs kept in
  `bench/results/2026-11-07-arm64-dev-parsers-gcc14.jsonl` and `…-gcc12.jsonl`. Re-run both in a quiet window.
- **Commit:** `p0/parser-bench` (fix round 1).

### 2026-11-08: Cross-language parser bench re-run (fields from every parser except Zig `std.http`; GCC 14 and GCC 12, dev numbers)

- **Idea:** re-measure after review rounds 2–2g: llhttp and Beast `basic_parser` now deliver every field on their
  timed path (Zig `std.http` still hands no fields out), images are built at the commit timed (revision labels
  checked), containers are memory-capped.
- **Hypothesis:** with equal work, P0 is still behind httparse and picohttpparser on 20-field heads.
- **Method:** branch `p0/parser-bench-rerun` at `8ff4b87` (milestone/p0). Three commands, back to back:
  1. `node bench/parsers/run.ts --gate-only` (builds every image at the clean `8ff4b87`; gate passed);
  2. `node bench/parsers/run.ts --no-build --out bench/results/2026-11-08-arm64-dev-parsers-gcc14.jsonl` (7 rounds ×
     300 ms, feeds 5 rounds; meta: clean tree, all 5 image labels `8ff4b87`);
  3. `node bench/parsers/run.ts --cc gcc-12 --parsers picohttpparser,llhttp,beast-response-parser,beast-basic-parser
     --out bench/results/2026-11-08-arm64-dev-parsers-gcc12.jsonl`. This step **rebuilt** its images (Rust, C, C++)
     while step 2's results file was still untracked, so the tree counted as dirty: the meta lists only `??
     bench/results/2026-11-08-arm64-dev-parsers-gcc14.jsonl`, and every image is labelled
     `8ff4b87…+dirty.6224b242e81d`. That file is excluded from every build context (Rust: the
     `bench/parsers/rust/Dockerfile.dockerignore` whitelist on the repo-root context; C/C++: context
     `bench/parsers/<lang>`), so the source is `8ff4b87`, but our parser in the GCC 12 run is a separate build of it.

  Linux arm64 Docker Desktop on the owner's M2, `--cpus=1 --cpuset-cpus=2 --memory=2g --memory-swap=2g
  --network=none`, no other container at start; other agents' cargo builds ran on the host. **Load rule:** `sysctl -n
  vm.loadavg` every 5 s on the host (raw samples: `bench/results/2026-11-08-arm64-dev-parsers-load-samples.txt`); a
  round runs from the runner's previous stderr marker ("differential gate passed" or "round N/7 done", timestamped
  when written) to its own "round N/7 done"; the feed phase runs from "round 7/7 done" to the end of the run; windows
  include both ends (a sample on a boundary counts in both windows); a window is **noisy** when its 1-minute load
  exceeds 6 (`…-round-load.jsonl`, with a meta line stating this rule). GCC 14: rounds 1–4 noisy (6.9–8.6), rounds 5–7
  quiet (≤ 5.6), **feed phase noisy** (max 7.25). GCC 12: every round noisy (8.4–10.9) and the feed phase too (max
  11.14). A first attempt died in round 7 when the host was suspended for ≈ 18 min (`spawnSync … ETIMEDOUT`, no
  results written) and was restarted from step 1.

  **Statistics caveat:** medians cover all 7 rounds, noisy ones included; the JSONL keeps only median, min and max per
  row (no per-round series), so quiet-only medians cannot be computed. A verdict whose per-round range crosses 1 is
  **unresolved at this noise**. Absolute level: our 64-variant `1k` ring took 43.8 ns (the committed Task 4 bench:
  43.1–45.0 ns on the same rotation); httparse 54.4 ns (Task 4: 51.8–52.4 ns). Dev run: no CI, no confidence interval.
- **Numbers (GCC 14; median per-round ratio ours / theirs, > 1 = theirs faster; `1k` / `1k-h20` / `1k` ring64 /
  `1k-h20` ring64 / mixed 64):**

  | vs | ours+fields / theirs | plain ours / theirs | theirs ns (1k / 1k-h20) |
  |---|---|---|---|
  | httparse 1.10.1 | **1.23 / 1.74 / 1.39 / 1.95 / 1.78** | 0.80 / 0.93 / 0.79 / 0.94 / 0.93 | 52 / 260 |
  | picohttpparser `dd573d8` | **1.19 / 1.30 / 1.42 / 1.42 / 1.33** | 0.72 / 0.68 / 0.80 / 0.71 / 0.68 | 58 / 331 |
  | llhttp 9.4.3 (field callbacks) | **0.81 / 1.09 / 0.85 / 1.18 / 1.11** | 0.51 / 0.55 / 0.49 / 0.56 / 0.57 | 81 / 397 |
  | Beast `basic_parser` (spans kept) | **0.72 / 0.81 / 0.82 / 0.97 / 0.90** | 0.46 / 0.45 / 0.45 / 0.46 / 0.46 | 90 / 511 |
  | Beast `response_parser` | **0.44 / 0.44 / 0.53 / 0.49 / 0.29** | 0.29 / 0.23 / 0.30 / 0.23 / 0.15 | 145 / 986 |
  | Zig `std.http` (hands no fields) | **0.23 / 0.23 / 0.26 / 0.24 / 0.22** | 0.15 / 0.12 / 0.15 / 0.12 / 0.11 | 293 / 1891 |
  | Go `net/http` | **0.08 / 0.13 / 0.11 / 0.15 / 0.13** | 0.06 / 0.07 / 0.06 / 0.07 / 0.07 | 784 / 3371 |

  Ours: 41.8 / 220.3 ns plain, 64.7 / 429.5 ns with fields (`1k` / `1k-h20`). The split costs as much as the parse
  (plain / with fields = 0.48–0.65); the split helper is plain scalar code (`memchr` per line, `.position()` for the
  colon, `bench/parsers/rust/src/engines.rs`), so part of that cost is the helper's. Allocations per head: 0 for every
  parser except Beast `response_parser` (4.0 / 21.0 / 15.2) and Go (10.0 / 40.0 / 31.7) (`1k` / `1k-h20` / mixed).

  1-byte feeds (median ms, (a) / (b) / (c) / (d); **noisy phase**): ours 0.198 / 0.204 / 0.179 / 0.028 (with fields
  0.191 / 0.193 / 0.153 / 0.028), picohttpparser 0.302 / 0.225 / 0.238 / 0.030, llhttp 0.269 / 0.276 / 0.330 / 0.033,
  Zig 0.273 / 0.269 / 0.275 / 0.040, Go 0.661 / 0.621 / 0.706 / 0.076, httparse 73.649 / 89.696 / 87.042 / 48.044,
  Beast `basic_parser` 756.629 / 8.871 / 877.760 / 17.861, Beast `response_parser` 810.109 / 8.785 / 832.719 / 17.449.

  **GCC 12.2 vs 14.2** (C/C++ rows): **not resolvable**: the GCC 12 run was noisy in every round, and our row there is
  a separate build. C/C++ medians differ by -6% to +11% in ns, -12% to +10% in ours+fields / theirs (for example Beast
  `response_parser` on the `1k-h20` ring 0.43 vs 0.49), -10% to +12% in plain / theirs, and feeds by -17% to +66%.
- **Where we win / lose with equal work (`blazehttp+fields`; by median per-round ratio, medians include 4 noisy
  rounds; a per-round range crossing 1 is unresolved at this noise; brackets give [min, max] of the 7 per-round
  ratios):**
  - **httparse: we lose on every workload**, 1.23–1.95× its time (every per-round range above 1).
  - **picohttpparser: we lose** on `1k-h20`, both rings and mixed (1.30–1.42×, ranges above 1); on the single `1k`
    head we lose by median (1.19) but the range crosses 1 (unresolved).
  - **llhttp: we lose on `1k-h20`** (1.09, range above 1). By median we win on the 3-header heads and lose on the
    20-field ring and mixed, but those ranges cross 1: 1k-3-headers 0.81 [0.704, 2.255]; 1k-3-headers-ring64 0.85
    [0.776, 1.139]; 1k-20-headers-ring64 1.18 [0.571, 1.470]; mixed-64 1.11 [0.966, 1.322] (unresolved).
  - **Beast `basic_parser`: ahead by median on every workload (0.72–0.97), but every per-round range crosses 1**
    (1k-3-headers 0.72 [0.608, 1.937]; 1k-20-headers 0.81 [0.723, 1.253]; 1k-3-headers-ring64 0.82 [0.742, 1.074];
    1k-20-headers-ring64 0.97 [0.819, 1.201]; mixed-64 0.90 [0.822, 1.058]): unresolved, near parity on the `1k-h20`
    ring and mixed.
  - **Beast `response_parser`: we win on `1k-h20`, both rings and mixed** (0.29–0.53, ranges below 1); `1k` is ahead
    by median but unresolved: 1k-3-headers 0.44 [0.383, 1.254].
  - **Go `net/http`: we win** (0.08–0.15, every range below 1).
- **Unequal work, stated separately:** Zig `std.http` hands no fields out, so compare it with our plain row: we win
  (0.11–0.15, with fields 0.22–0.26). Our plain row (framing facts only) is ahead of every parser by median, but these
  ranges cross 1: 1k-20-headers 0.93 [0.839, 1.309]; 1k-20-headers-ring64 0.94 [0.822, 1.002]; mixed-64 0.93 [0.789,
  1.143] vs httparse (parity, unresolved); 1k-20-headers 0.68 [0.638, 1.041] vs picohttpparser (ahead by median,
  unresolved). 1-byte feeds: fastest by median on all four inputs, and our median is below every rival's minimum; the
  best rival takes 36%, 11%, 33%, 8% longer than our plain row on (a)–(d) (than our row with fields: 41%, 17%, 55%,
  7%), measured in a noisy phase.
- **What must change to beat every parser:** the split doubles our cost, so the parser must emit name/value spans
  during its single scan (Task 4c, "spans in scan"); then P1a SIMD must take it below httparse (≤ 0.85× on `1k-h20`
  with fields), picohttpparser and llhttp, on arm64 and x64 (x64 still unmeasured).
- **Follow-ups:** the quiet-window re-run is still owed (owner's nightly window; this run was not quiet). The runner
  keeps no per-round series or per-round load (so noisy rounds cannot be dropped), uses a default `--out` inside the
  repo (a second run after an untracked results file counts as dirty and rebuilds), and runs each parser's feed rounds
  back to back rather than interleaved: backlog for the bench harness.
- **Kept / dropped:** results `bench/results/2026-11-08-arm64-dev-parsers-gcc14.jsonl`, `…-gcc12.jsonl`, their
  `…-round-load.jsonl` and `…-load-samples.txt`. They supersede the numbers of the 2026-11-07 cross-language entries
  (not the `h1_parse_bench` entries).
- **Commit:** results `7295506`, load metadata `255aa65` (window edges stated in a later commit), measured at
  `8ff4b87`.


### 2026-11-07: bench-server runtime: multi-thread vs per-core `current_thread` + `SO_REUSEPORT`

- **Idea:** one tokio multi-thread runtime (design A) vs one `current_thread` runtime and one `SO_REUSEPORT` listener
  per core (design B). Detail: `docs/notes/bench-server.md` §4, §7.
- **Hypothesis:** B costs less server CPU per request (no cross-thread wakes or work stealing).
- **Method:** Linux arm64, Docker Desktop on the owner's M2 (8 CPUs), server `--cpus=4 --threads 4`, wrk 4.1.0
  `--cpus=3 -t3 -c50` in the server's network namespace, `GET /1k`, 5 s warm-up + 20 s, server CPU from its cgroup
  `cpu.stat`; A and B alternated, 3 rounds each. Host **not idle** (load average 14–28 from other work), so dev only.
- **Numbers (no CI, n = 3):** server CPU per request A 10.17 / 9.74 / 10.64 µs (median 10.17), B 11.05 / 10.13 /
  13.67 µs (median 11.05); 85–87% of it is kernel time in both.
- **Kept / dropped:** A kept (simpler, no measured loss at c = 50). B is not settled: c = 500 was not measured, so the
  comparison is repeated at c = 50 and 500 in the nightly window (#15).
- **Commit:** see the PR that adds `crates/bench-server`.

### 2026-11-08: bench-server TLS batch writes: gather vs copy

- **Idea:** write a TLS batch of several pre-built responses as one vectored write of the static slices (gather)
  instead of copying them into one buffer (copy). Detail: `docs/notes/bench-server.md` §7.2.
- **Hypothesis:** the copy is cheaper because rustls processes each slice of a vectored write separately.
- **Method:** HTTPS `/1k`, c = 50, wrk pipelining 16 requests per write, server `--cpus=4 --threads 4`, wrk
  `--cpus=3`, shared network namespace, 5 s warm-up + 20 s, server CPU from cgroup `cpu.stat`; 6 rounds (rounds 1–3
  copy first, rounds 4–6 alternated). Host not idle (load average 10–15), so dev only.
- **Numbers (no CI, n = 6):** server CPU per request copy median 2.01 µs (1.74–2.12), gather median 1.74 µs
  (1.62–1.87); gather lower in 4 of 6 rounds.
- **Kept / dropped:** gather kept on both ports; the copy path and its per-stream switch were deleted (the hypothesis
  was wrong for this workload).
- **Commit:** see the PR that adds `crates/bench-server`.

### 2026-11-08: bench-server headroom re-measure after fix round 1 (dev; build: fix round 1, see note §7.3)

- **Idea:** confirm the server stays below the 60% CPU limit after the fix round (vectored writes, limits, TLS
  pinning). Detail and raw numbers: `docs/notes/bench-server.md` §7.3.
- **Hypothesis:** about 10 µs of server CPU per `/1k` request, mostly kernel time, so a 1-CPU client keeps the server
  far below 60%.
- **Method:** Linux arm64, Docker Desktop on the owner's M2, server `--cpus=4 --memory=1g --threads 4`, wrk 4.1.0
  `--cpus=3 --memory=512m` in the server's network namespace, 5 s warm-up + 20 s, server CPU from cgroup `cpu.stat`.
  Host not idle (load average 5–20), so dev only.
- **Numbers (no CI):** `/1k` c = 50 median 9.96 µs (≈ 241k req/s at 60%), c = 500 9.77 µs, `/1k-h20` 10.40 µs,
  HTTPS c = 50 12.9 µs, HTTPS c = 500 21.5 µs, `/1m` 415 µs per request; 83–86% kernel time on plaintext.
- **Kept / dropped:** no code change; the bound for the gate cells (a 1-CPU client keeps the server ≈ 30%) and the
  per-run < 60% assertion (Task 10) are recorded in the note.
- **Commit:** see the PR that adds `crates/bench-server`.

### 2026-11-08: `write_request` (Task 5) vs the copy floor, a hyper-1.12 encoder emulation and Go 1.27.3 (committed benches, dev numbers)

*"Copy floor" in the three 2026-11-08 writer entries is the baseline renamed "unchecked appends" on 2026-11-13: it is
not a lower bound, and its time moves with code layout (2026-11-13 entry). "hyper" there means our emulation of
hyper 1.12's encoder (`bench/writer/hyper-emul`); no hyper code runs. Verdicts in these dev entries are indicative,
not gate results.*

- **Idea:** the chosen request writer (`docs/notes/request-writer.md` §1: pre-encoded template + re-validated delta,
  one exact reservation, one buffer) against the copy floor (same bytes, same appends and framing, no checks), an
  emulation of hyper 1.12's encoder (`bench/writer/hyper-emul`) and Go's `(*Request).Write` (`bench/writer/go`).
- **Hypothesis:** within the provisional gates of note §8.4 (`post1k` ≤ 2.3×, `varied` ≤ 2.9× the floor; advisory
  `get` ≤ 60 ns, `post1k` ≤ 250 ns, `varied` ≤ 300 ns) and ≤ 0.5× the hyper emulation.
- **Method:** `cargo run --release -p engine-core --example h1_write_bench` (7 runs × 4 interleaved blocks × 100,000
  requests, 100 ms warm-up, median of per-run ratios) and `cargo run --release --manifest-path
  bench/writer/hyper-emul/Cargo.toml`, each binary run 3 rounds. Linux arm64: Docker Desktop on the owner's M2,
  `rust:1-bookworm` (rustc 1.99.1), `--cpus=1` (`cpu.max = 100000 100000`), kernel 6.11.11-linuxkit, generic `aarch64`.
  Go: `golang:1.27.3-bookworm`, `--cpus=1`, `GOMAXPROCS=1`, `-count 5 -benchmem`. macOS 26.4 on the same M2, default
  target CPU. **Not a gate run:** other agents' containers were running; host load average 4–10.
- **Numbers (Linux arm64, rounds 1 / 2 / 3; writer ns, floor ns, median ratio):** `get` (head 146 B) 20.8 / 9.6
  (2.16), 23.0 / 10.6 (2.16), 55.2 / 26.9 (2.04); `post1k` (head 335 B + 1 KiB) 91.3 / 35.8 (2.55), 99.6 / 39.6 (2.51),
  288.2 / 105.9 (2.79, loaded round); `varied` (mean head 440 B) 100.2 / 48.9 (2.04), 105.7 / 52.2 (2.05), 331.1 /
  137.8 (2.38). An earlier quiet round (2026-11-08T07:14Z): 19.1 / 83.5 / 87.8 ns, ratios 2.19 / 2.58 / 2.04.
  **vs hyper emulation (ours / clone + encode, ours / encode-only):** `get` 0.22–0.25 / 0.47–0.51, `post1k`
  0.38–0.41 / 1.16–1.22, `varied` 0.33–0.34 / 0.98–1.02 — encode-only is hyper's serialisation floor without
  validation, cloning or `content-length` construction, so the second ratio is not equal work (superseded by the fix
  round 1 entry below). **Go** `(*Request).Write`: `get` 792–1,631 ns, 224 B, 5 allocations; `post1k` 1,514–2,210 ns,
  328 B, 10 allocations, 2 of them the harness's body reader (removed in fix round 1) (ours: 0 allocations,
  `tests/h1_writer_alloc.rs`). **macOS:** ratios
  2.11–2.14 / 2.67–2.76 / 2.36–2.56; vs hyper emulation 0.18 / 0.30–0.32 / 0.31–0.33.
- **Kept / dropped:** kept (the design of note §1). Indicative (dev run, not a gate run): below the advisory absolute
  values (`get` ≤ 23 ns, `post1k` ≤ 100 ns, `varied` ≤ 106 ns in the unloaded rounds) and below the hyper-emulation
  target (≤ 0.41×); **above** the provisional `post1k` gate (writer/floor 2.51–2.58× against ≤ 2.3×); `varied`
  (2.04–2.05×) and `get` below theirs. The writer itself is
  faster than the prototype (91 vs 111–220 ns) but the floor is faster too (36 vs 72–112 ns), and the checks added in
  review (singletons, underscore spellings, line count, authority) cost ≈ 15 ns on `post1k` (next entry). The gates are
  provisional until the 10 reference-host rounds (note §8.4); P1a's block classification is the planned lever.
- **Commit:** Task 5 PR (`p0/writer`).

### 2026-11-08: Request writer micro-variants (scratch A/B, not committed)

- **Idea:** where the writer's time goes, and four candidate speed-ups: (a) `#[inline]` on `write_request` so it
  inlines into the caller's loop across the crate boundary; (b) unrolled appends that skip empty pieces instead of a
  loop over the 12-slice piece list; (c) a branch-free name-spelling fold; (d) a first-byte filter before name
  classification; (e) a `name == canonical` fast path before the case-insensitive compare.
- **Hypothesis:** (a) removes call and setup overhead; (b)–(e) shave the classification cost.
- **Method:** a scratch crate with the writer's source copied per variant (same crate, so each inlines), timed on the
  three request sets, 9 runs × 200,000 requests per variant, median; Linux arm64 container `--cpus=1`, 3 rounds. The
  scratch crate was not committed, so (b)–(e) cannot be re-run from the repository; their drops are provisional. (a)
  measured with the committed bench: three rounds of the inlined build, then three of the previous build, 15–20 s
  later (not alternated: rounds 07:14:26–:32Z, then 07:14:47–:54Z).
- **Numbers:** ablations (quiet round): `post1k` 74–79 ns full, 58–65 ns without classification, 44–46 ns without any
  delta scan, 76–78 ns without the target check; `varied` 87–93 / 76–85 / 49–51 / 78–80 ns. (a) committed bench `get`
  19.1 vs 29.7–38.4 ns, `post1k` 83.3–84.9 vs 105.8–123.5 ns, `varied` 87.8–99.5 vs 118.1–136.5 ns (ratios 2.17–2.20 vs
  3.38–3.40, 2.56–2.58 vs 2.82–2.84, 2.04–2.11 vs 2.23–2.32; `docs/perf/logs/writer/bench-linux-inline.log` /
  `docs/perf/logs/writer/bench-linux-3.log`). Between the two blocks the floor (same code in both builds) rose 13–34% on `post1k` and
  `varied`, so the ns differences include host drift; the ratios do not. Both logs also hold a hyper-emulation section
  that panicked at its byte-equality check ("both encoders write the same request", line 48 of each log, before its
  `varied` rows); none of its rows is used here, only the writer rows above. (b) ±5%, mixed sign; (c) 0 to +2%; (d) 0 to −10% on `varied` only;
  (e) −4% on `post1k` in 2 of 3 rounds, +33% in the third.
- **Kept / dropped:** (a) kept: by writer/floor ratio `get` ≈ 35%, `post1k` ≈ 9%, `varied` ≈ 5–12% less time (the
  ns figures first recorded here, 16–50%, included the drift above). (b)–(e) dropped: no win beyond noise
  (provisional, scratch crate not committed).
- **Commit:** Task 5 PR (`p0/writer`), `#[inline]` on `write_request`.

### 2026-11-08: Request writer after review fix round 1 (committed benches, dev numbers)

- **Idea:** re-measure after the review fixes (template line count, OPTIONS content type, `trailer`/`http2-settings`/
  `proxy-` names, host and target rules, sensitive-name spelling, the module split) and replace the hyper comparison
  with equal work (construct+encode, ruling of the review); record the regression found and its fix.
- **Hypothesis:** the added rules cost little; the ≤ 0.5× hyper target still holds with equal work.
- **Method:** as the 2026-11-08 entry above (Linux arm64 Docker `--cpus=1`, rustc 1.99.1; macOS host; Go in
  `golang:1.27.3-bookworm`), 3 rounds each. Regression hunt: the pre-review tree (`fb80d32`, kept reachable by the tag
  `notes/request-writer-task5`; same engine-core source as `12d82d7`) and the current tree built side by side as two
  binaries and alternated in one container (`docs/perf/logs/writer/bench-ab-fr1.log`, `docs/perf/logs/writer/bench-ab-fr1-opt.log`; columns: set, mean
  head B, writer ns, floor ns, median ratio, min, max), plus a scratch crate with ablations of the new writer (9 runs
  × 200,000 requests, median; not committed). Host load average 4–37 during the hunt (other agents' jobs); dev
  numbers, no gate claim.
- **Numbers.** *Regression:* the first fix build was slower than the pre-review writer — `get` 35–38 vs 27 ns,
  `post1k` 149–154 vs 116–119 ns, `varied` 213–231 vs 128–132 ns. Ablations: the target rule's sequential
  in-query state and its `position` pass cost ≈ 22 ns on `varied`; the `_`-folding name compare ≈ 10–15 ns on
  `post1k`; the `proxy-` prefix check and the sensitive-name loop ≈ 0–5 ns each. *Fix:* the target bytes are one
  vectorisable fold with no carried state and the path is searched only when the target holds a backslash; names
  try `eq_ignore_ascii_case` before folding. *After the fix* (A/B, same container, rounds 1 / 2 / 3,
  `docs/perf/logs/writer/bench-ab-fr1-opt.log`; **current vs pre-review**): `get` 38.7 / 42.3 / 33.6 vs 29.1 / 33.3 / 29.7 ns, ratio
  2.49 / 2.42 / 2.45 vs 2.17 / 2.16 / 2.18; `post1k` 148.4 / 175.3 / 137.2 vs 143.5 / 132.1 / 134.5 ns; `varied`
  160.5 / 172.0 / 170.4 vs 167.1 / 168.5 / 160.8 ns. The two trees did not write the same `varied` and `post1k`
  requests: fix round 1 added a `content-type` field to every `OPTIONS` request with a body (mean head 440 → 445 B)
  and changed the `post1k` body bytes. **Regression kept: `get` +12–15% by ratio** (+13% in ns in the quietest round,
  floor flat 13.5 → 13.7 ns); `post1k` +2–4% in ns in rounds 1 and 3. Owner: Task 5b (fix round 2 measured a
  candidate, 2026-11-13 entry).
  *Final rounds (Linux, writer / floor ns, median ratio):* `get` 31.0 / 12.8 (2.43), 33.4 / 13.4 (2.49), 49.6 / 20.3
  (2.51); `post1k` 151.1 / 48.1 (3.02), 135.5 / 45.2 (3.01), 166.6 / 54.5 (3.07); `varied` 166.0 / 62.7 (2.58),
  157.9 / 60.6 (2.62), 198.5 / 71.4 (2.63) (`docs/perf/logs/writer/bench-linux-fr1-final.log`). The floor's code is the same line for
  line in both trees, yet it measured 13–21% faster in the current binary on `post1k` and `varied` in 5 of 6 round × set cells (round 2
  `post1k`, a loaded round, +8%: 51.0 → 54.9 ns, `bench-ab-fr1-opt.log`; A/B above) with
  more bytes to copy: its time moves with code layout, so these ratios are not comparable with the earlier entry's.
  *Hyper-1.12 encoder emulation (ours / construct+encode):* `get` 0.209–0.212, `post1k` 0.239–0.266, `varied`
  0.215–0.223 — the emulation builds its `HeaderMap` from `(name, value)` pairs inside the timed region while our
  writer gets the delta already joined, so this is not equal input; the conservative figure is ours / clone+encode
  0.28–0.29 / 0.41–0.43 / 0.39–0.42; *serialisation floors* (our copy floor / emulated encode-only): 0.23 /
  0.41–0.46 / 0.40–0.41.
  *Go* `(*Request).Write`, harness allocations removed: `get` 727–817 ns, 224 B, 5 allocations; `post1k` 1,294–1,524
  ns, 264 B, 8 allocations (`docs/perf/logs/writer/bench-go-fr1.log`). *macOS:* ratios 2.36–2.37 / 3.13–3.26 / 2.92–3.06; ours /
  construct+encode 0.15–0.16 / 0.17–0.19 / 0.15–0.17 (`docs/perf/logs/writer/bench-macos-fr1.log`).
- **Kept / dropped:** kept. Both speed fixes are wins against the *first fix build*, not against the pre-review
  writer: `get` stays 12–15% slower than before review (above). Indicative (dev runs): below the ≤ 0.5×
  hyper-emulation target, also by the conservative clone+encode ratio; `varied` below ≤ 2.9× on Linux (2.58–2.63)
  and **above it on macOS (2.92–3.06)**; `post1k` above ≤ 2.3× (3.0×), an accepted miss owned by Task 5b, which
  also decides the exact-vs-amortised reservation growth. The owner's goal is to beat
  every writer; the cross-language writer race is Task 5c.
- **Commit:** Task 5 PR (`p0/writer`), fix round 1.

### 2026-11-13: Request writer fix round 2 (one-binary A/B and committed benches, dev numbers, quiet machine)

- **Idea:** measure fix round 2 (per-component target rule, canonical IPv6 hosts) against the pre-review writer and
  fix round 1 in **one** binary, find the cause of the `get` slowdown since fix round 1 (review finding), and keep no
  measured slowdown without a reason.
- **Hypothesis:** the per-component target rule costs little if its byte check stays one vectorised fold.
- **Method:** `docs/perf/harness/writer-ab/` (engine-core at `68c48f0` = pre-review, `abe8c87` = fix round 1, and the
  fix-round-2 head, plus the `shape`, `appends` and `memcpy` baselines, all in one binary, rotated, 7 runs × 4
  blocks, medians; 3 separate launches), then the committed benches. Binaries built first (`run.sh build`); `run.sh
  measure` ran alone: the controller stopped every other agent and watched the machine (only the 3 bench containers,
  one after another); Linux arm64 Docker on the owner's M2, `--cpus=1` pinned to vCPU 2, rustc 1.99.1,
  `target-cpu=generic`. Dev runs on a quiet machine outside the nightly window: **not gate evidence**. Assembly read
  with `cargo rustc --release -- --emit asm` (no timing). Ranges below are the three launches. Commits are named
  after the rebase onto `milestone/p0` (`7a63a27`), which left `crates/engine-core`, `bench/writer` and the harness
  unchanged; the runs built the pre-rebase commits that each log's `provenance.txt` names (`12d82d7` → `68c48f0`,
  `89a028c` → `abe8c87`, `1faf80d` → `d4855a2`, `82e2020` → `692c7bd`, `33a7710` → `caf019e`).
- **Numbers.** *Run 1* (`d4855a2`, 256-entry flag table; load 3.60 → 3.28, 36 s; `docs/perf/logs/writer/fr2-run1/`):

  | set | pre-review | fix round 1 | fix round 2 | shape | appends | memcpy |
  |---|---|---|---|---|---|---|
  | `get` | 18.3–19.1 | 21.2–22.8 | 21.0–22.7 | 20.3–21.7 | 17.8–18.6 | 4.3–4.5 |
  | `post1k` | 79.5–83.3 | 93.4–98.9 | 91.7–97.2 | 34.8–36.2 | 31.9–33.3 | 17.8–18.5 |
  | `varied` | 101.2–104.1 | 103.6–107.4 | 128.6–132.0 | 44.7–45.7 | 41.1–42.5 | 27.3–27.5 |

  The table made `varied` 21–27% slower than fix round 1 (same-launch ratios). Cause, from the assembly: the table lookup is one
  dependent load per byte, so the fold stays scalar (0 NEON instructions and 25 byte loads in `check_request`, against
  55 vector instructions in fix round 1 and 32 pre-review); fix round 1 also calls `memchr` for its backslash search.
  `get` and `post1k` stay 15–19% and 15–17% slower than pre-review (fix round 1: 16–19% and 17–19%). The committed bench in the
  same run: writer / shape / appends / memcpy `get` 22.6 / 11.8 / 9.4 / 4.5, `post1k` 91.7 / 33.9 / 30.4 / 17.7,
  `varied` 113.1 / 43.0 / 39.6 / 27.1 ns. The same `shape` code measured 11.8 ns there and 20.3–21.7 ns in the A/B
  binary: at `get` size, code layout moves a baseline by ≈ 2×, so only same-binary comparisons are used here.
  *Run 2* (`692c7bd`; load 2.68 → 3.08, 37 s; `docs/perf/logs/writer/fr2-run2/`): the flags computed with
  comparisons and selects instead of the table, plus skipping empty request pieces. `get` 41.1–44.3 (pre-review
  17.7–19.0, fix round 1 20.6–22.0), `post1k` 113.8–114.3 (79.4–80.0 / 93.8–94.9), `varied` 133.1–136.1 ns (99.8–102.3
  / 109.0–110.1). The fold vectorised (312 16-lane instructions) but its scalar loop for short targets compiled to
  byte-dependent branches and a switch-table load; the check-free `shape` with the skip got slower in every set
  (`get` 21.7–25.0, `post1k` 43.6–43.7, `varied` 53.2–53.7 ns against 20.3–21.7 / 34.8–36.2 / 44.7–45.7 in run 1).
  *Run 3* (`caf019e`, fix round 1's fold structure with the per-component rule: one `&` fold over the bytes allowed
  in every component, the per-component check only for a target holding `\`, `^`, `` ` ``, `{`, `}` or `'`; load
  2.94 → 2.88, 34 s; `docs/perf/logs/writer/fr2-run3/`), same-launch ratios over the three launches: `varied` 0.90–1.03×
  fix round 1 and 0.94–1.05× pre-review (fix round 1: 1.01–1.09× pre-review; launch 1's pre-review 121.2 ns is an
  outlier against 101.8–104.1 in launches 2–3); `get` 1.01–1.02× fix round 1, 1.18–1.19× pre-review (fix round 1
  1.16–1.18×); `post1k` 1.01–1.03× fix round 1, 1.17–1.21× pre-review (fix round 1 1.16–1.17×). Assembly: the fold
  is vectorised (16 bytes per step, 8 for short targets) and its scalar remainder has no byte-dependent branch. The
  committed bench in the same run: writer − shape `get` 10.0, `post1k` 58.5, `varied` 64.9 ns.
  *Comparisons (runs 1 and 3, committed benches):* ours + join / hyper-emulation construct+encode 0.167–0.207 / 0.293–0.296 /
  0.245–0.252, ours / clone+encode 0.263–0.266 / 0.414–0.416 / 0.386–0.402; Go `(*Request).Write` `get` 643–690 ns,
  224 B, 5 allocations, `post1k` 1,036–1,238 ns, 264 B, 8 allocations.
- **Kept / dropped:** the flag table, dropped (measured +21–27% on `varied`); the comparison-and-select flags,
  dropped (measured: `get` ×2, `varied` no better); skipping empty pieces, dropped in this form (the check-free shape
  measured slower in every set) and kept as a Task 5b open item (`ideas.md`). Both reverted in `24d19ac`
  ([controller's call], 2026-11-13). Kept: fix round 1's fold structure with the per-component rule (run 3: `varied`
  back to fix round 1's time, `get`/`post1k` within 1–3% of fix round 1). **Not recovered, open:** `get` and `post1k`
  stay 1.16–1.21× pre-review since fix round 1's safety checks; hypothesis: a per-call fixed cost in the checks fix
  round 1 added (it shows on `get`, which has no per-request field; the check-free shape is not the cause). Task 5
  merges at fix round 1's speed and Task 5b must win the gap back, then beat pre-review ([controller's call],
  2026-11-13; `ideas.md`). The gate redefinition proposed for the owner is in the note §8.4 (Task 5b).
- **Commit:** Task 5 PR (`p0/writer`), fix round 2 (`caf019e`; losses reverted in `24d19ac`).
