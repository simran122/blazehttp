# Performance ideas

Backlog of ideas to measure. Each idea moves to `journal.md` once measured.

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
