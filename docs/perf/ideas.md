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

No row here was benchmarked: every speed effect is unverified. Where a row's *correctness* was checked by a recorded run (the executable model in `spike/h1-research/`), the status says so. Candidates and their measurement plan are in the note (§5–§7).

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
| P-D: ScanThru bit-stream validator (6-bit state, line-start hand-off) over `engine-simd` block bitmaps + index flatten | ≈ 0.4–0.6 cycles per byte, predictor-independent | P1a criterion, varied-256 + ring64 | Speed unverified. Correctness checked by the model (`spike/h1-research/scanthru_model.py`: spans, split points, review counterexamples) |
| SIMD fast path accepts only; any anomaly re-runs the block on the scalar parser (simdutf "locate on error") | keeps one error semantics; no SIMD/scalar differential on errors | fuzz SIMD vs scalar | Unverified |
| W-D: writer-form bit-stream validator for the delta (+ optional fused `copy_classified`, `out.clear()` on errors) | ≈ 14 ns at 100 B, ≈ 45–50 ns at 400 B: pays only on large deltas | P1a criterion, post1k + h20 | Speed unverified; correctness checked by the model |
| Netty's exact SWAR byte-equality (`SWARUtil.applyPattern`: no false positives above the first match) | exact masks for `;`, CR, `,` searches in chunk sizes and `Connection` lists | criterion | Unverified |
| Spans as a `Uint16Array` to JS | JS builds headers without rescanning bytes | G1 cells, P2 | Unverified |
| SWAR 8-digit parse for Content-Length (Muła/Lemire) | fewer branches than the digit loop | criterion | Unverified; values are 1–7 digits, probably noise |
| Hex table for chunk sizes | cheaper than range compares | criterion | Unverified; P1a chunked |
| nginx's "bytes still needed" hint from the chunked decoder | better read sizing | G0 chunked cells | Unverified; P1a |
| Real hyper (in-memory duplex) instead of the hyper emulation in the writer bench | equal-work fidelity | Task 5c differential gate | Unverified |
| Prior-art SIMD parsers to study later: Kourier (C++, strict, SIMD), hparse (Zig), xocomil (Rust), simdzone (DNS zone files) | may hold further tricks | read at a pinned release | Unverified (found by search, not read; links in the note §4.6) |
| QUICHE Balsa (Envoy) HTTP/1 parser | not vectorised (survey) — no gain expected | — | Unverified |
