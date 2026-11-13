# Performance ideas

Backlog of ideas to measure. Each idea moves to `journal.md` once measured.

## Response head parser (P0 survey, 2026-11-06; detail in `docs/notes/response-parser.md`)

Scratch measurements (`parser-bench`, not product code), rustc 1.99.1, fat LTO, `codegen-units = 1`. Hosts: Linux
arm64 in Docker on the owner's M2 (`--cpus=1`, generic `aarch64` target CPU, the reference for these numbers) and macOS
on the same M2 (default `apple-m1` target CPU, and `-C target-cpu=generic`). Heads: 3 headers (109 B), synthetic 20
headers (993 B), proposed `/1k-h20` layout (794 B, note §8.3).

| Idea | Hypothesis | How to measure | Status |
|---|---|---|---|
| Fused span scan resumable at any byte (design G, prototype with a `memchr` status line: token run, `:`, SWAR value run that stops at the CR; line-level sub-state) | as fast as httparse without re-parsing | criterion `h1_parse` (P1a): all heads + 1-byte feed | Measured (Linux arm64, median of 3): 47 / 281 / 271 ns vs httparse 54 / 292 / 293 ns (0.86–0.96×); **adopted in P0** |
| Line-at-a-time scan (`memchr` LF, then validate the line) (design C) | linear, faster than a byte state machine | same | Measured (Linux arm64): 53 / 352 / 307 ns with SWAR: 1.05–1.20× httparse on 20 headers; replaced by G |
| Safe-Rust SWAR field-value check (8 bytes per word, table fallback per word) | −15% (3 headers) / −30% (20 headers) vs table only | same, with/without | Measured: kept inside G as `value_len` |
| `as_chunks::<8>()` instead of `chunks_exact(8)` + `c[0..8]` | no bounds checks by construction | asm audit (`--emit asm`, aarch64 + x86_64) | Measured: both forms check-free after LLVM; `as_chunks` adopted (does not rely on the optimiser) |
| `get(a..b)` instead of `buf[a..b]` for slices by stored offsets | removes the only panic path the audit found (`slice_index_fail`) | asm audit | Measured: adopted |
| NEON / AVX2 value and token runs in `engine-simd` behind `value_len` / `token_len` | −30–50% on 20-header heads (httparse NEON pattern) | criterion, x64 and arm64 | Unverified; P1a. **Risk:** on Linux arm64 httparse NEON and httparse SWAR measured within 4% of each other on these heads (short values), so the P1a ≤ 0.85× target may not be reachable through 16-byte vectors alone |
| Bitmask + `trailing_zeros` walk instead of per-word table fallback (Cloudflare AVX2 picohttpparser, 2014: masks for 128 B, `tzcnt`) | fewer branches on HTAB/obs-text words; also the shape for `engine-simd` | criterion | Unverified; P1a |
| SWAR token run for lower-case names (`a-z`, `0-9`, `-` fast path, table fallback) | names are 35% of the `/1k-h20` head bytes | criterion | Unverified; P1a |
| Avoid `pcmpestri` (SSE4.2 string ops, picohttpparser/llhttp) in favour of AVX2/SSE2 range compares | `pcmpestri` is ~11-cycle latency microcode (Cloudflare 2014: AVX2 1.68–1.79× faster) | criterion on x64 | Unverified for us; P1a |
| Measure parser micro-benchmarks with the shipped target CPU | default `apple-m1` vs generic changed httparse by ≈ 1.6× and the ranking of designs | run both | Measured: see note §8; proposed as a benchmark rule |
| Fused status-line scan (12-byte prefix checked per byte, then the reason as a value run) instead of `memchr` + validate | drops the `memchr` dependency and a second pass over ~17 bytes; rejects non-HTTP bytes at once | committed bench `h1_parse_bench` (Task 4) | Adopted in review round 1 (security S13, review #19); unmeasured until Task 4's bench |
| httparse's faster version parser (`u64` compare), credited with ~10% on short requests in its changelog | our 12-byte prefix check is the same cost class | criterion | Unverified (changelog seen only as a search result); merged into the masked `u64` status-prefix row of the 2026-11-08 section |
| `memchr_iter` instead of one `memchr` per line | fewer setup costs per line | scratch | Measured: no difference (59 vs 58 ns); dropped |
| Name dispatch by length (14/17/10) then `eq_ignore_ascii_case` | cheaper than hashing names (Bun) for 3 names | criterion | Unmeasured; adopted by design (3 names, no table to maintain); measure against a hash in P1a criterion; superseded by the shared known-name classifier row of the 2026-11-08 section |
| Never re-parse on partial reads (httparse/hyper/pico do) | avoids quadratic cost on slow peers | 1-byte feed of 60 KB | Measured (Linux arm64): G 0.26 ms vs httparse 67 ms; adopted |
| Stop at the blank line, never scan body bytes | body bytes untouched by the parser | code review + bench with body in buffer | Adopted |
| PGO including the parser | a few % on the head path | §7 PGO builds | Unverified; P5 |

Reference notes (what makes each fast / tracker findings): picohttpparser: SSE4.2 range scans, `last_len`
anti-slowloris, accepts bare LF, issue #89 (NULL SEGV). httparse: SWAR/NEON/AVX2 blocks, fused validation that
stops at the CR, `u64` version compare, not resumable, accepts bare LF. llhttp: generated state machine, SIMD only
for values, open issues #876 (TE `chunked<HTAB>`) and #877 (bare LF). hyper: httparse + `is_complete_fast` +
`HeaderMap` allocations. h11: resumable blank-line search, regex validation. Go: `textproto` line reader, accepts
bare LF and obs-fold. Bun: picohttpparser via FFI, name hashing. curl: `memchr` per line with a copy into a dynbuf.
Netty 4.1.125/4.2.5: strict CRLF for heads and chunk lines by default (`defaultStrictLineParsing`).

## SIMD scanning (P1a survey, 2026-11-06; detail in `docs/notes/simd-scanning-research.md`)

Dev measurements on the owner's M2 (macOS, loaded host, `-C target-cpu=generic`, scratch crate `scanbench`),
`/1k-h20` head; "varied" = 256 heads with shuffled names and value lengths 0.5–1.5×. Not yet in `journal.md`:
re-run in the Linux reference container (nightly window) and on the x64 VM first.

| Idea | Hypothesis | How to measure | Status |
|---|---|---|---|
| Stage-1 bitmaps (simdjson / Cloudflare 2014): classify 64-B blocks into `not_tchar` / `not_field_content` masks, parser walks them with `trailing_zeros` | removes the per-span latency chain; fast whatever the branch predictor learns | P1a criterion `h1_parse`: repeated head + varied ring, 3-header head, 1-byte feed; arm64 + x64 | Dev: 222 vs 527 ns (varied, 0.42×), 194 vs 186 ns (repeated); now the A/B baseline for the P-D bit-stream validator (h1 speed note §5.4–§5.5) |
| Lazy stage 1 (classify only the block the scan reaches, cache its two masks) | keeps parser state tiny and resumable | same | Dev: 163 vs 186 ns (repeated, 0.88×), 288 vs 527 ns (varied) |
| Per-span 16-B SIMD `token_len`/`value_len` (httparse shape) | faster spans | same | Dev: **slower** in the dependent chain (383 vs 186 ns repeated; 512 vs 527 varied) although 3.6× / 1.6× faster in isolation; **drop** |
| Runtime dispatch per span (fn pointer / `is_x86_feature_detected!` + `#[target_feature]` call) | negligible | same | Dev: +0.6–0.9 ns per span (≈ 40 spans/head); `target_feature` fns are never inlined into callers without the feature (Rust Reference); dispatch per block/window only |
| SSE2-only value class on x64 (`pminub`/`pcmpeqb`), SSSE3 `pshufb` only for TCHAR | SSE2 is baseline → inlinable, no detection | x64 VM criterion | asm verified; speed unverified (x64 VM pending) |
| AVX2 32-B stage 1 | 2× fewer ops per 64 B than SSE | x64 VM criterion | Unverified |
| NEON mask: `vpaddq_u8` ×3 (1 bit/B per 64 B) vs `shrn` per 16 B (4 bits/B) | fewer vector→GPR moves | criterion | Unverified (prototype uses `vpaddq`) |
| Skip value-class work for blocks with no byte < 0x20 or ≥ 0x7F (Keiser–Lemire ASCII fast path) | most head blocks only contain CRLFs as specials | criterion | Unverified |
| ≥ 64 initialised slack bytes in pooled read buffers (simdjson padding) | no tail path in stage 1 | criterion + 1-byte feed | Unverified; merged with the padded-read-buffer row of the 2026-11-08 section (≥ 8 bytes for SWAR, ≥ 64 for stage 1) |
| P0 `value_len`: jump to the first SWAR flag with `trailing_zeros` (lowest flag is exact; only HTAB re-checks) | fewer unpredictable branches | Task 4 bench, repeated + varied heads | Dev: 395 vs 527 ns varied (0.75×), 198 vs 186 ns repeated; optional for Task 4 |
| Benchmark parser with a varied-head ring, not only one repeated head | repeated head lets the predictor memorise exits | Task 4 bench / P1a criterion / G0 report | Dev: same scalar code 186 ns repeated vs 436–585 ns varied; proposed rule |
| SWAR `[A-Za-z0-9-]` name fast path, `table8` names | fewer branches than per-byte table | same | Dev: slower than the table in the chain (321 / — vs 186 ns); drop |
| Avoid `pcmpestri`/`pcmpistri` | 8 µops, 12–17-cycle latency (uops.info) vs `pshufb` 1 µop | — | Verified (uops.info); no SSE4.2 string ops in `engine-simd` |

## HTTP/1.1 parser and writer speed (survey 2026-11-08; detail in `docs/notes/h1-speed-research.md`)

No row here was benchmarked: every speed effect is unverified. Where a row's *correctness* was checked by a recorded run (the executable model in `tools/h1-research/`), the status says so. Candidates and their measurement plan are in the note (§5–§7).

| Idea | Hypothesis | How to measure | Status |
|---|---|---|---|
| P-A: emit `FieldSpan` (u16 × 4, caller-owned) at the confirmed CRLF in `value()` | removes the bench's second pass; with fields vs httparse ≈ 0.83–0.99, pico ≈ 0.71–0.84, llhttp ≈ 0.51–0.60 (quiet-run plain ratios × 1.05) | `h1_parse_bench` A/B + `bench/parsers` | Unverified; Task 4c |
| P-B: one loop body per field line on locals, state written back only on `Partial` | −15–25% scalar (the references all do this; our `step()` dispatches 3× per field with large `Result` returns) | same; `--emit asm` for memory round trips | Unverified; Task 4c |
| STOP-set name search (first byte ≤ 0x20, ≥ 0x7F or `:`; SWAR, lowest flag exact) + branch-free tchar fold off the critical path | names at ≈ 1 branch per 8 B instead of 1 per byte | same | Unverified; Task 4c |
| Masked `u64` compare of `HTTP/1.` + byte checks for minor, SP and digits on the complete 12-byte prefix (httparse's version trick) | fewer branches on every head | same | Unverified |
| Padded read buffer (≥ 8 initialised slack bytes; sentinel CR followed by a non-LF byte; matches count only below the received length; uWS fence, safe form) | no scalar tails in SWAR loops | same | Unverified; API change |
| Known-name classifier: length switch → last byte → 1–3 OR-0x20 `u64` word compares (shared parser/writer) | writer classification ≈ 15 → ≈ 3–5 ns on `post1k` | `h1_write_bench` ablation | Unverified; Task 5b |
| Writer layout hints from JS (`[u16; 2]` per line), tiled name/`: `/value/CRLF ranges checked with `get()` and branch-free folds | removes the delta search; injection-exact without counters | same + fuzz vs `scan_fields` | Unverified; P2 contract change; measure the JS-side cost first |
| Interned name ids from JS for known names (undici record, h2o tokens) | known names cost ≈ 0 in Rust | same | Unverified; P2 |
| Authority binding by a `(slot, generation)` template id at checkout instead of a byte compare per request | −2–4 ns per request | same | Unverified; Task 6 |
| P-D: ScanThru bit-stream validator (6-bit state, line-start hand-off) over `engine-simd` block bitmaps + index flatten | ≈ 0.4–0.6 cycles per byte, predictor-independent | P1a criterion, varied-256 + ring64 | Speed unverified. Correctness checked by the model (`tools/h1-research/scanthru_model.py`: spans, split points, review counterexamples) |
| SIMD fast path accepts only; any anomaly re-runs the block on the scalar parser (simdutf "locate on error") | keeps one error semantics; no SIMD/scalar differential on errors | fuzz SIMD vs scalar | Unverified |
| W-D: writer-form bit-stream validator for the delta (+ optional fused `copy_classified`, `out.clear()` on errors) | ≈ 17–20 ns at 100 B, ≈ 55–70 ns at 400 B including per-line classification: pays only on large deltas | P1a criterion, post1k + h20 | Speed unverified; correctness checked by the model |
| Netty's exact SWAR byte-equality (`SWARUtil.applyPattern`: no false positives above the first match) | exact masks for `;`, CR, `,` searches in chunk sizes and `Connection` lists | criterion | Unverified |
| Spans as a `Uint16Array` to JS | JS builds headers without rescanning bytes | G1 cells, P2 | Unverified |
| SWAR 8-digit parse for Content-Length (Muła/Lemire) | fewer branches than the digit loop | criterion | Unverified; values are 1–7 digits, probably noise |
| Hex table for chunk sizes | cheaper than range compares | criterion | Unverified; P1a chunked |
| nginx's "bytes still needed" hint from the chunked decoder | better read sizing | G0 chunked cells | Unverified; P1a |
| Real hyper's `Client::encode` through a one-function shim (pre-cloned maps, clone timed separately) instead of the hyper emulation in the writer bench; real hyper over an in-memory duplex only for the byte-equality check | equal-work fidelity | Task 5c differential gate | Unverified |
| Prior-art SIMD parsers to study later: Kourier (C++, strict, SIMD), hparse (Zig), xocomil (Rust), simdzone (DNS zone files) | may hold further tricks | read at a pinned release | Unverified (found by search, not read; links in the note §4.6) |
| QUICHE Balsa (Envoy) HTTP/1 parser | not vectorised (survey) — no gain expected | — | Unverified |

## Benchmark server (Task 9, 2026-11-07; detail in `docs/notes/bench-server.md`)

| Idea | Hypothesis | How to measure | Status |
|---|---|---|---|
| Per-core `current_thread` runtimes, one `SO_REUSEPORT` listener each, instead of one multi-thread runtime | fewer cross-thread wakes and stealing atomics → lower server CPU per request | `/1k` c = 50 and 500, server `cpu.stat` per request, wrk in Docker | Partly measured 2026-11-07 (journal): **c = 50 only**, busy dev host, no win (median 11.05 vs 10.17 µs CPU/request), so A is kept for now. c = 500 not measured: compare at c = 50 and 500 in the nightly window (#15) |

## Request writer (Task 5 research, 2026-11-07; detail in `docs/notes/request-writer.md`)

Dev measurements in a Linux arm64 container (`--cpus=1`) on the owner's M2 while other agents' containers ran (load
13–31); ratios within a run are the usable signal. These scratch measurements are recorded in note §8.2 and §8.3,
not as `journal.md` entries; the scratch `writer-bench` and `sys` crates were not committed, so every "drop" below
is **provisional (unreproduced)** until a committed bench repeats it. The committed writer measurements are in
`journal.md` (2026-11-08 and later). The provisional ratio gates in the note (§8.4) are reset after 10 rounds on the
reference host and the x64 VM.

| Idea | Hypothesis | How to measure | Status |
|---|---|---|---|
| Classify the whole per-request delta block with P1a `engine-simd` stage 1, then walk masks | delta is contiguous; removes the per-line SWAR loop | `h1_write_bench` `post1k`/`varied`, `writer − shape` ns (the checks' cost, note §8.1), arm64 + x64 | Unverified; P1a target defined in note §8.4 |
| **Task 5b open:** recover the `get`/`post1k` time lost since fix round 1 (same binary: `get` 15–19%, `post1k` 15–17% slower than pre-review; journal 2026-11-13) | the append shape or the per-call fixed cost, not the target check (`get` targets are 8 bytes) | one-binary A/B (`docs/perf/harness/writer-ab/`), `writer − shape` and `shape − appends` per set | Open; owner Task 5b (validate-once writer) |
| **Task 5b open:** skip empty request pieces in the append loop (`get` has 6 of 12 empty) | 6 fewer `memcpy` calls on `get` | one-binary A/B, `shape` column | Dev (2026-11-13 run 2): the filtered loop is no longer unrolled and the check-free shape got **slower** in every set (`get` 21.7–25.0 vs 20.3–21.7 ns, `post1k` 43.6 vs 34.8–36.2, `varied` 53.2–53.7 vs 44.7–45.7); dropped in this form, reverted; retry only as part of 5b's append-shape work (e.g. framing-specific piece lists) |
| Target check as a 256-entry per-byte flag table | one load and OR per byte, any byte set | one-binary A/B `varied` | Dev (2026-11-13 run 1): `varied` 21–27% **slower** than fix round 1's fold; asm: scalar, one dependent load per byte; dropped |
| Target check as comparison-and-select `u8` flags | vectorises like fix round 1's fold | one-binary A/B | Dev (2026-11-13 run 2): `get` ×2 (41.1–44.3 vs 20.6–22.0 ns), `varied` no better (133.1–136.1 ns); asm: vectorised, but the short-target scalar loop has byte-dependent branches and a switch-table load; dropped |
| `memchr` for each line's CR + branch-free fold over the value (V3) | vectorised CR search beats SWAR on short values | scratch `writer-bench` | Dev: 7–18% **slower** than SWAR `value_len` (119 vs 111 ns `post1k`, 130 vs 110 ns `varied`); drop (provisional, unreproduced) |
| Fold `" HTTP/1.1\r\n"` into the template bytes and keep a static `"GET "`-style prefix per method | 2 fewer `extend_from_slice` calls per request | `h1_write_bench` `get` | Unverified (expected ≤ 2–3 ns) |
| Integer formatting: jeaiii / Lemire SIMD instead of `format_into` | faster `Content-Length` | scratch `writer-bench` int | Dev: `format_into`, `itoa` 1.0.18 and a safe pair table within noise (12.7–13.5 ns for 0–10⁴); `core::fmt` ≈ 2×. Literature numbers **unverified** (search summaries only): Lemire 2022 AVX-512 blog "2.5 vs 8.8 ns" for 16 digits; Champagne Gareau & Lemire 2026 (arXiv 2604.26019) "under two nanoseconds", "1.4–2× faster than the closest competitor". Gains are for ≥ 8 digits; Content-Length is 1–7 → drop (provisional, unreproduced) |
| `write_head` + `writev([head, body])` above a body threshold (plaintext only; owner P1b) | avoids the body copy for large bodies | `bench/writer/` sys-style bench on the reference host (nightly window) + x64 VM | Dev (scratch `sys`, unreproduced): copy ≈ `writev` ≤ 4 KiB; `writev` wins from 16 KiB (2 of 3 runs), 1.6–3.3× at 64 KiB; threshold to set in 4–16 KiB by P1b with a committed bench |
| Large bodies under TLS: head-then-body plain `write`s vs one copied buffer (P1b) | two `write`s give two records but no body copy; copy gives one record | P1b, rustls 0.23.x, 16 KiB–1 MiB bodies, CPU per request + allocation counter | Unverified |
| Hoisting the fixed request-line pieces and the `connection: close` line into static slices | fewer bounds checks per request | `h1_write_bench` | Unverified (expected ≤ 1–2 ns) |
| Never `writev` under TLS | rustls 0.23.46 `write_vectored` collects a `Vec<&[u8]>` per call (verified in source) | allocation counter around a TLS write | Verified (source); keep the single buffer |
| Skip-mask on the template for per-request overrides | avoids template churn when a request overrides a default | P2 template registry stats (overrides per request) | Unverified; only if P2 measures churn |
| `io_uring` send with a registered buffer for the head | one fewer copy into the kernel, fewer syscalls under load | research backlog (design §7) | Unverified |

Reference notes: hyper 1.12 formats the target through `Display` (own `TODO`), needs a `HeaderMap` per request and
`writev`s head + body; Go 1.27.3 `fmt.Fprintf` + sorted header keys + 4 KiB `bufio` (5 allocations per `get`, 8 per
`post1k` request, journal 2026-11-08 fix round 1);
curl 8.23.0 builds the head in a `dynbuf` with its own printf and copies head + body into a 64 KiB `sendbuf`; undici
8.12.4 concatenates strings and `cork`s head + body into one `writev`; Bun 1.4.4 copies head + body into a per-request
`Vec`; Boost.Beast pre-encodes each field at insertion and `writev`s one buffer per field (no CR/LF check).
