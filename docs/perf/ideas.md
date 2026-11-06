# Performance ideas

Backlog of ideas to measure. Each idea moves to `journal.md` once measured.


## Response head parser (P0 survey, 2026-11-06; detail in `docs/notes/response-parser.md`)

Scratch measurements (`parser-bench`, not product code), rustc 1.99.1, fat LTO, `codegen-units = 1`. Hosts: Linux
arm64 in Docker on the owner's M2 (`--cpus=1`, generic `aarch64` target CPU, the reference for these numbers) and macOS
on the same M2 (default `apple-m1` target CPU, and `-C target-cpu=generic`). Heads: 3 headers (109 B), synthetic 20
headers (993 B), proposed `/1k-h20` layout (794 B, note §8.3).

| Idea | Hypothesis | How to measure | Status |
|---|---|---|---|
| Fused span scan resumable at any byte (design G: token run, `:`, SWAR value run that stops at the CR; line-level sub-state) | as fast as httparse without re-parsing | criterion `h1_parse` (P1a): all heads + 1-byte feed | Measured (Linux arm64, median of 3): 47 / 281 / 271 ns vs httparse 54 / 292 / 293 ns (0.86–0.96×); **adopted in P0** |
| Line-at-a-time scan (`memchr` LF, then validate the line) (design C) | linear, faster than a byte state machine | same | Measured (Linux arm64): 53 / 352 / 307 ns with SWAR: 1.05–1.20× httparse on 20 headers; replaced by G |
| Safe-Rust SWAR field-value check (8 bytes per word, table fallback per word) | −15% (3 headers) / −30% (20 headers) vs table only | same, with/without | Measured: kept inside G as `value_len` |
| `as_chunks::<8>()` instead of `chunks_exact(8)` + `c[0..8]` | no bounds checks by construction | asm audit (`--emit asm`, aarch64 + x86_64) | Measured: both forms check-free after LLVM; `as_chunks` adopted (does not rely on the optimiser) |
| `get(a..b)` instead of `buf[a..b]` for slices by stored offsets | removes the only panic path the audit found (`slice_index_fail`) | asm audit | Measured: adopted |
| NEON / AVX2 value and token runs in `engine-simd` behind `value_len` / `token_len` | −30–50% on 20-header heads (httparse NEON pattern) | criterion, x64 and arm64 | Unverified; P1a. On Linux arm64 httparse NEON and httparse SWAR measured within 4% of each other on these heads (short values), so the gain may be small for typical heads |
| Bitmask + `trailing_zeros` walk instead of per-word table fallback (Cloudflare AVX2 picohttpparser, 2014: masks for 128 B, `tzcnt`) | fewer branches on HTAB/obs-text words; also the shape for `engine-simd` | criterion | Unverified; P1a |
| SWAR token run for lower-case names (`a-z`, `0-9`, `-` fast path, table fallback) | names are 35% of the `/1k-h20` head bytes | criterion | Unverified; P1a |
| Avoid `pcmpestri` (SSE4.2 string ops, picohttpparser/llhttp) in favour of AVX2/SSE2 range compares | `pcmpestri` is ~11-cycle latency microcode (Cloudflare 2014: AVX2 1.68–1.79× faster) | criterion on x64 | Unverified for us; P1a |
| Measure parser micro-benchmarks with the shipped target CPU | default `apple-m1` vs generic changed httparse by ≈ 1.6× and the ranking of designs | run both | Measured: see note §8; proposed as a benchmark rule |
| `memchr_iter` instead of one `memchr` per line | fewer setup costs per line | scratch | Measured: no difference (59 vs 58 ns); dropped |
| Name dispatch by length (14/17/10) then `eq_ignore_ascii_case` | cheaper than hashing names (Bun) for 3 names | criterion | Adopted (cheap, no alternative needed) |
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
