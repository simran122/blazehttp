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

## Shard and connection tasks (P0 survey, 2026-11-07; detail in `docs/notes/shard-and-connection.md`)

Dev measurements on the owner's M2 (macOS loopback, loaded host, in-process 4-thread server, client = one
`LocalRuntime` thread, closed loop, thread CPU clock, thread-local counting allocator), scratch crate `connbench`. Not
yet in `journal.md`: re-measure in the G0 harness on Linux (Tasks 7/8, nightly window).

| Idea | Hypothesis | How to measure | Status |
|---|---|---|---|
| Exchange inside the connection task; requests as FIFO entries (design A) | no channel hop or oneshot per request: fewer wakes and 0 allocations vs hyper | G0 c = 50/500: CPU µs per request, allocations per request, wake counters | Dev: 0.000 vs hyper 10.0 allocations per request; CPU median 12.1 vs raw loop 13.7 vs hyper 17.7 µs (±20–30% noise); **adopted for P0** |
| Checkout idle check with `try_read` (syscall only when the reactor flagged the socket) | free on a clean connection | micro + G0 | Dev: 5.1 ns per check (no syscall); **adopted** |
| Forced `recv(MSG_PEEK)` per reuse (h2o, Bun) to close the post-reactor-turn windows W1/W2 | +1 syscall per request (full) or per parked checkout only (parked-only, closes W1) | Task 6 `shard_bench` on Linux: full / parked-only / none, ≥ 10 rounds | Dev: 2,130 ns per check on macOS; Linux unmeasured; **deferred: adopted if ≤ 2% CPU per request** (note §12 item 1) |
| "Receiver baton": a parked connection awaits the command channel itself; the command loop runs only while every connection is busy | removes one local wake + task poll per request at low load (≈ 150–300 ns, unverified) | `shard_bench` / G0 c = 1, G1-low | Dev: c = 1 costs 1.000 command-loop wakes per request (batch 1.00) vs 0.061–0.065 at c = 50 (batch 15.5–16.4); 3 client-thread wakes per request vs hyper's 2; P1a if the c = 1 cell misses |
| Inline drain: a connection about to park `try_recv`s the shared receiver | saves the command-loop round when commands are already queued; nothing at c = 1 | `shard_bench` c = 8–50 | Unverified |
| engine-bench-only local submit (FIFO + `notify_last`, no channel) | removes the channel and command-loop wake in G0 | diagnostic only | Rejected for gate runs: Node never takes that path |
| Direct hand-off to a LIFO idle-connection stack (per-slot mailbox + waker) instead of `Notify` | avoids the `Notify` waiter mutex; exact LIFO | G0 + profile of `notify_last` | Unverified; P1a if `Notify` shows in profiles |
| Release the read buffer when a connection parks, take it at checkout (fasthttp per-request reader pool) | RSS proportional to busy, not idle, connections (c = 500, 128 MiB cell) | G2 memory cells | Unverified; costs a pool round trip per request at low load |
| Hand-off threshold below ¼ of the buffer (e.g. 4 KiB) | saves a memcpy of 4–16 KiB per response | G0 with 4/8/16 KiB bodies + RSS | Unverified (≈ 1.6 µs per 16 KiB copy, unmeasured); trades memory amplification (up to 16×) |
| Uninitialised read-buffer growth (`read_buf` into spare capacity) instead of zero-extending | no memset when a buffer is created or grows | G0 1 MB cell | Unverified; `try_read_buf` loses tokio's short-read readiness clear, so only for growth, never the steady state |
| `Builder::event_interval` below 61 on shard threads | fresher idle readiness under load (smaller desync window), more `epoll_wait` calls | G0 c = 500 + syscall count | Unverified |
| `Builder::max_io_events_per_busy_tick` (new in tokio 1.54.0) | caps I/O events processed per busy tick; may smooth latency at c = 500 | G0 c = 500 p99 | Unverified |
| `notify_last` (LIFO) vs `notify_one` (FIFO) for idle wakes | LIFO keeps fewer connections hot (cache, server idle timers) | G0 low load + connection count | Unverified; LIFO adopted by reasoning (Go, hyper-util) |
| `TCP_QUICKACK` after reads (Linux) | lower latency when the server waits for an ACK | G1-low netem 1 ms | Unverified |
