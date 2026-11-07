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
| httparse's faster version parser (`u64` compare), credited with ~10% on short requests in its changelog | our 12-byte prefix check is the same cost class | criterion | Unverified (changelog seen only as a search result) |
| `memchr_iter` instead of one `memchr` per line | fewer setup costs per line | scratch | Measured: no difference (59 vs 58 ns); dropped |
| Name dispatch by length (14/17/10) then `eq_ignore_ascii_case` | cheaper than hashing names (Bun) for 3 names | criterion | Unmeasured; adopted by design (3 names, no table to maintain); measure against a hash in P1a criterion |
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
| Stage-1 bitmaps (simdjson / Cloudflare 2014): classify 64-B blocks into `not_tchar` / `not_field_content` masks, parser walks them with `trailing_zeros` | removes the per-span latency chain; fast whatever the branch predictor learns | P1a criterion `h1_parse`: repeated head + varied ring, 3-header head, 1-byte feed; arm64 + x64 | Dev: 222 vs 527 ns (varied, 0.42×), 194 vs 186 ns (repeated); **lead P1a candidate** |
| Lazy stage 1 (classify only the block the scan reaches, cache its two masks) | keeps parser state tiny and resumable | same | Dev: 163 vs 186 ns (repeated, 0.88×), 288 vs 527 ns (varied) |
| Per-span 16-B SIMD `token_len`/`value_len` (httparse shape) | faster spans | same | Dev: **slower** in the dependent chain (383 vs 186 ns repeated; 512 vs 527 varied) although 3.6× / 1.6× faster in isolation; **drop** |
| Runtime dispatch per span (fn pointer / `is_x86_feature_detected!` + `#[target_feature]` call) | negligible | same | Dev: +0.6–0.9 ns per span (≈ 40 spans/head); `target_feature` fns are never inlined into callers without the feature (Rust Reference); dispatch per block/window only |
| SSE2-only value class on x64 (`pminub`/`pcmpeqb`), SSSE3 `pshufb` only for TCHAR | SSE2 is baseline → inlinable, no detection | x64 VM criterion | asm verified; speed unverified (x64 VM pending) |
| AVX2 32-B stage 1 | 2× fewer ops per 64 B than SSE | x64 VM criterion | Unverified |
| NEON mask: `vpaddq_u8` ×3 (1 bit/B per 64 B) vs `shrn` per 16 B (4 bits/B) | fewer vector→GPR moves | criterion | Unverified (prototype uses `vpaddq`) |
| Skip value-class work for blocks with no byte < 0x20 or ≥ 0x7F (Keiser–Lemire ASCII fast path) | most head blocks only contain CRLFs as specials | criterion | Unverified |
| ≥ 64 initialised slack bytes in pooled read buffers (simdjson padding) | no tail path in stage 1 | criterion + 1-byte feed | Unverified |
| P0 `value_len`: jump to the first SWAR flag with `trailing_zeros` (lowest flag is exact; only HTAB re-checks) | fewer unpredictable branches | Task 4 bench, repeated + varied heads | Dev: 395 vs 527 ns varied (0.75×), 198 vs 186 ns repeated; optional for Task 4 |
| Benchmark parser with a varied-head ring, not only one repeated head | repeated head lets the predictor memorise exits | Task 4 bench / P1a criterion / G0 report | Dev: same scalar code 186 ns repeated vs 436–585 ns varied; proposed rule |
| SWAR `[A-Za-z0-9-]` name fast path, `table8` names | fewer branches than per-byte table | same | Dev: slower than the table in the chain (321 / — vs 186 ns); drop |
| Avoid `pcmpestri`/`pcmpistri` | 8 µops, 12–17-cycle latency (uops.info) vs `pshufb` 1 µop | — | Verified (uops.info); no SSE4.2 string ops in `engine-simd` |

## Request writer (Task 5 research, 2026-11-07; detail in `docs/notes/request-writer.md`)

Dev measurements in a Linux arm64 container (`--cpus=1`) on the owner's M2 while other agents' containers ran (load
13–31); ratios within a run are the usable signal. Not yet in `journal.md`. The provisional ratio gates in the note
(§8.4) are reset after 10 rounds on the reference host and the x64 VM.

| Idea | Hypothesis | How to measure | Status |
|---|---|---|---|
| Classify the whole per-request delta block with P1a `engine-simd` stage 1, then walk masks | delta is contiguous; removes the per-line SWAR loop | `h1_write_bench` `post1k`/`varied`, writer/floor ratio, arm64 + x64 | Unverified; target validation overhead ≤ 50 ns on `post1k` |
| `memchr` for each line's CR + branch-free fold over the value (V3) | vectorised CR search beats SWAR on short values | scratch `writer-bench` | Dev: 7–18% **slower** than SWAR `value_len` (119 vs 111 ns `post1k`, 130 vs 110 ns `varied`); drop |
| Fold `" HTTP/1.1\r\n"` into the template bytes and keep a static `"GET "`-style prefix per method | 2 fewer `extend_from_slice` calls per request | `h1_write_bench` `get` | Unverified (expected ≤ 2–3 ns) |
| Integer formatting: jeaiii / Lemire SIMD instead of `format_into` | faster `Content-Length` | scratch `writer-bench` int | Dev: `format_into`, `itoa` 1.0.18 and a safe pair table within noise (12.7–13.5 ns for 0–10⁴); `core::fmt` ≈ 2×. Literature numbers **unverified** (search summaries only): Lemire 2022 AVX-512 blog "2.5 vs 8.8 ns" for 16 digits; Champagne Gareau & Lemire 2026 (arXiv 2604.26019) "under two nanoseconds", "1.4–2× faster than the closest competitor". Gains are for ≥ 8 digits; Content-Length is 1–7 → drop |
| `write_head` + `writev([head, body])` above a body threshold (plaintext only; owner P1b) | avoids the body copy for large bodies | `bench/writer/` sys-style bench on the reference host (nightly window) + x64 VM | Dev: copy ≈ `writev` ≤ 4 KiB; `writev` wins from 16 KiB (2 of 3 runs), 1.6–3.3× at 64 KiB; threshold to set in 4–16 KiB |
| Large bodies under TLS: head-then-body plain `write`s vs one copied buffer (P1b) | two `write`s give two records but no body copy; copy gives one record | P1b, rustls 0.23.x, 16 KiB–1 MiB bodies, CPU per request + allocation counter | Unverified |
| Hoisting the fixed request-line pieces and the `connection: close` line into static slices | fewer bounds checks per request | `h1_write_bench` | Unverified (expected ≤ 1–2 ns) |
| Never `writev` under TLS | rustls 0.23.46 `write_vectored` collects a `Vec<&[u8]>` per call (verified in source) | allocation counter around a TLS write | Verified (source); keep the single buffer |
| Skip-mask on the template for per-request overrides | avoids template churn when a request overrides a default | P2 template registry stats (overrides per request) | Unverified; only if P2 measures churn |
| `io_uring` send with a registered buffer for the head | one fewer copy into the kernel, fewer syscalls under load | research backlog (design §7) | Unverified |

Reference notes: hyper 1.12 formats the target through `Display` (own `TODO`), needs a `HeaderMap` per request and
`writev`s head + body; Go 1.27.3 `fmt.Fprintf` + sorted header keys + 4 KiB `bufio` (5–10 allocations per request);
curl 8.23.0 builds the head in a `dynbuf` with its own printf and copies head + body into a 64 KiB `sendbuf`; undici
8.12.4 concatenates strings and `cork`s head + body into one `writev`; Bun 1.4.4 copies head + body into a per-request
`Vec`; Boost.Beast pre-encodes each field at insertion and `writev`s one buffer per field (no CR/LF check).
