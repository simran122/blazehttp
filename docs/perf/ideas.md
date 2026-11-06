# Performance ideas

Backlog of ideas to measure. Each idea moves to `journal.md` once measured.


## Response head parser (P0 survey, 2026-11-06; detail in `docs/notes/response-parser.md`)

Scratch measurements: Apple M2, macOS, rustc 1.99.1, fat LTO; 3-header head 109 B, 20-header head 993 B.

| Idea | Hypothesis | How to measure | Status |
|---|---|---|---|
| Line-at-a-time scan (`memchr` LF + per-line table validation) | Linear under any split, faster than a byte state machine | criterion `h1_parse` (P1a): both heads + 1-byte feed | Measured in scratch: 58 / 454 ns vs state machine 73 / 631 ns; adopted |
| Safe-Rust SWAR field-value check (8 bytes per word, table fallback per word) | −15% (3 headers) / −30% (20 headers) | same bench, with/without | Measured in scratch: 49 / 319 ns; adopted in P0 |
| NEON / AVX2 value and token validation in `engine-simd` behind `is_field_value` / `token_len` | further −30–50% on the 20-header head (httparse NEON pattern) | criterion, x64 and arm64 | Unverified; P1a |
| Fused single pass that finds CR while validating (skip `memchr` when the whole head is in one read) | −5–10% at 20 headers (scratch F: 51 / 399 ns without SWAR) | criterion vs chosen design | Unverified with SWAR |
| `memchr_iter` instead of one `memchr` per line | fewer setup costs per line | scratch | Measured: no difference (59 vs 58 ns); dropped |
| Name dispatch by length (14/17/10) then `eq_ignore_ascii_case` | cheaper than hashing names (Bun) for 3 names | criterion | Adopted (cheap, no alternative needed) |
| Avoid `pcmpestri` (SSE4.2 string ops, used by picohttpparser/llhttp) in favour of AVX2 range compares | `pcmpestri` is high-latency microcode | criterion on x64 | Unverified; P1a |
| Never re-parse on partial reads (httparse/hyper/pico do) | avoids quadratic cost on slow peers | 1-byte feed of 60 KB | Measured: 0.18 ms vs 67 ms (httparse); adopted |
| Stop at the blank line, never scan body bytes | body bytes untouched by the parser | code review + bench with body in buffer | Adopted |
| Assembly audit for bounds checks in the scan loops | removes residual checks | `cargo-show-asm` | Unverified; P1a |
| PGO including the parser | a few % on the head path | §7 PGO builds | Unverified; P5 |
| Pick the 20-header layout for `/1k-h20` to match real APIs (value lengths drive ns/B) | budget per byte is stable across layouts | Task 9 + criterion | Open |

Reference notes (what makes each fast / tracker findings): picohttpparser — SSE4.2 range scans, `last_len`
anti-slowloris, accepts bare LF, issue #89 (NULL SEGV). httparse — SWAR/NEON/AVX2 blocks, `u64` version compare, not
resumable, accepts bare LF. llhttp — generated state machine, SIMD only for values, open issues #876 (TE `chunked<HTAB>`)
and #877 (bare LF). hyper — httparse + `is_complete_fast` + `HeaderMap` allocations. h11 — resumable blank-line search,
regex validation. Go — `textproto` line reader, accepts bare LF and obs-fold. Bun — picohttpparser via FFI, name hashing.
curl — `memchr` per line with a copy into a dynbuf.
