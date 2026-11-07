# SIMD byte scanning for the HTTP/1.1 head: research survey (input to P1a `engine-simd`)

Design §5.3 (speed techniques), §6.3 (authoritative references), §7 (speed programme), §14 (no deprecated APIs),
G7 (`unsafe` only in `engine-simd`, every block with `// SAFETY:`, Miri + fuzz). Builds on
`docs/notes/response-parser.md` (branch `p0/parser`, commit `736eb75`): the P0 parser is a fused, resumable span
scanner whose primitives are `token_len` (TCHAR run, 256-entry table) and `value_len` (field-content run, safe SWAR,
8 bytes per step). Review round 1 of that note dropped `memchr` (the status line is scanned like a field line), so
`find_lf` is no longer a P0 primitive; it is covered here only for P1a chunk-size lines.

Access date for every link: **2026-11-06**. "Verified" = checked against the primary source, an official Rust doc
page for 1.99.1, the pinned crate source, or a measurement run for this note. "Unverified" = stated by a source but
not reproduced here. All numbers in §4 are **dev measurements** (owner's M2 under normal desktop load, macOS,
not the nightly benchmark window, not the Linux reference container).

## 1. Summary

**The headline finding is about structure, not instructions.** Replacing each P0 primitive with a 16-byte SIMD
version of the same `fn(&[u8]) -> usize` shape (what httparse does) makes the parser **slower** in our fused,
dependent scan: every span's end feeds the next span's start, so the parser is bound by latency
(load → classify → mask → vector-to-GPR move → count trailing zeros → next load ≈ 25–30 cycles per span), not by
throughput. The scalar table/SWAR loops avoid that chain because the branch predictor speculates through the loop
exits. Measured on the `/1k-h20` head (§4.3): per-span NEON 383 ns vs P0 scalar 186 ns (same head every time) and
512 vs 527 ns (256 varied heads).

What wins is the **simdjson / Cloudflare two-stage shape**: classify the head in 64-byte blocks into bitmaps
(stage 1, independent work per block, throughput-bound), then let the parser walk the bitmaps with
`trailing_zeros` (stage 2, a 1–3-cycle scalar chain per span). Prototype: **222 ns vs 527 ns (0.42×) on varied
heads, 194 vs 186 ns (1.04×) on one repeated head; a lazy per-block variant is 163 ns (0.88×) on the repeated head
and 288 ns on varied heads.** Cloudflare reached the same conclusion for picohttpparser in 2014: a single-match AVX2
scan "gave no gain because HTTP fields are short"; 128-byte bitmaps walked with `tzcnt` gave 1.68–1.79× (§3.6).

A second finding changes how we benchmark: a micro-benchmark that parses **one head over and over** lets the
branch predictor memorise every loop exit. The same scalar parser takes 186 ns on a repeated head and 436–585 ns
when 256 realistic variants rotate (§4.3). The P0 parser budget (note §8.4) and the G0 cells use a fixed head;
both should also report a varied-head ring.

### 1.1 Most promising techniques per primitive

| Primitive | Technique (best first) | aarch64 (NEON baseline) | x86_64 | Expected gain vs P0 scalar | Status |
|---|---|---|---|---|---|
| `token_len` (TCHAR) | 1. Stage-1 bitmap: nibble-LUT classification (`tbl`/`pshufb` ×2, AND, compare-zero) of 64-byte blocks, walk with `trailing_zeros` | `vqtbl1q_u8` ×2 + `vandq` + `vceqzq` per 16 B; 64-bit mask by AND-with-powers-of-two + 3× `vpaddq_u8` | SSSE3 `pshufb` (not baseline: needs detection or an x86-64-v2 build); AVX2 `vpshufb` 32 B; `pmovmskb` gives masks directly | names: 3.6× in isolation (28 vs 101 ns per head); whole field scan 0.42× time on varied heads | verified (arm64 dev); x86 correctness verified (Rosetta + Miri), x86 speed unverified |
| | 2. Per-span 16-B nibble-LUT (httparse NEON shape) | 7 vector ops/16 B | SSSE3 7 ops/16 B, not inlinable without the feature | **slower** in the dependent chain (§4.3) | verified (arm64 dev) |
| | 3. Scalar: table, or SWAR `[A-Za-z0-9-]` fast path with table fallback | — | — | SWAR alone: slower in the chain (321 vs 186 ns, repeated head) | verified (dev) — keep the table in P0 |
| `value_len` (field-content: HTAB, SP–`~`, obs-text; reject CTL/DEL) | 1. Stage-1 bitmap from range compares: `x < 0x20` OR `x == 0x7F`, minus `x == 0x09` | `vcltq` + 2× `vceqq` + `vorrq` + `vbicq` (5 ops/16 B) | SSE2 baseline: `pminub`+`pcmpeqb` (≤ 0x1F) + 2× `pcmpeqb` + `pandn` + `por` + `pmovmskb` (7 ops/16 B, inlinable, no detection) | same as above (values and names share the stage-1 loads) | verified (arm64 dev; SSE2 asm checked) |
| | 2. SWAR with `trailing_zeros` jump to the first flagged byte (exact for the lowest flag) | — | — | 395 vs 527 ns on varied heads (0.75×), 198 vs 186 ns on a repeated head | verified (dev) — optional P0 input (§7) |
| | 3. Per-span 16/32-B SIMD (httparse SSE4.2/AVX2/NEON shape) | 1 load + 5 ops + `shrn` + `fmov` + `rbit`/`clz` | needs dispatch for AVX2 | **slower** in the dependent chain | verified (arm64 dev) |
| `find_lf` (P1a chunk-size lines; not a P0 primitive any more) | `memchr` crate (SSE2/AVX2/NEON, ifunc dispatch) or the value-class bitmap (LF is outside field-content, so the stage-1 mask already stops at CR/LF) | — | — | 2.3 ns (`memchr`) vs 6.3 ns (`iter().position`) for a 17-B status line; inline NEON 1.5 ns | verified (dev) |

AVX-512: not worth it for P1a (spans are short; `_mm512_*_mask` compares and masked loads would remove tail code, but
Miri "only supports very few AVX512 intrinsics" and AVX-512 availability on client x64 is uneven). Revisit after
AVX2 stage 1 is measured on the x64 VM.

## 2. Problem shape (why HTTP is not JSON)

- `/1k-h20` (note §8.3): 794 B, 20 names (275 B, mean 13.8 B, max 27 B), 20 values (420 B, mean 21 B, max 55 B).
  Typical heads are 100–800 B; values are mostly 3–40 B.
- The P0 parser is resumable at any byte and scans forward once. Each span's length decides where the next span
  starts → **a serial dependency chain of ~40 spans per head**.
- A 16-byte vector covers most spans in one step, so "bytes per cycle" is not the limit; **latency per span** and
  **branch predictability** are. That is why per-span SIMD gains little (httparse NEON vs SWAR: within 4% on our
  heads, note §8.2) and why stage 1 (latency-free classification) helps.

## 3. Technique catalogue

Each entry: source → mechanism → cost → fit for short HTTP spans → safe API shape → testability → status.

### 3.1 Nibble-LUT ("vectorized classification") — simdjson, Muła, Hyperscan shufti
- **Sources:** Langdale & Lemire, *Parsing Gigabytes of JSON per Second*, VLDB Journal 28(6), 2019
  ([arXiv 1902.08318](https://arxiv.org/abs/1902.08318), [HTML v7](https://arxiv.org/html/1902.08318v7)); Muła,
  *SIMDized check which bytes are in a set*, 2018-10-18, updated 2019-03-27
  ([0x80.pl](http://0x80.pl/notesen/2018-10-18-simd-byte-lookup.html), read over plain HTTP because the HTTPS
  certificate does not match the host); Wang et al., *Hyperscan*, NSDI 2019
  ([PDF](https://www.usenix.org/system/files/nsdi19-wang-xiang.pdf)).
- **Mechanism (verified, simdjson §3.1.1):** two `vpshufb` lookups, one indexed by the low nibble and one by the
  high nibble (after a 4-bit shift), AND-ed; up to 8 classes in the 8 result bits; a class must be a union of
  (low-nibble set × high-nibble set) products. simdjson classifies structural characters and whitespace this way in
  "two `vpshufb` instructions, a shift and a few bitwise logical operations".
- **Muła's universal algorithm (verified):** any set of byte values: a 16×16 bitmap; row = `pshufb(bitmap, lo)`,
  bit = `pshufb([1,2,4,…,128,1,…], hi)`, member ⇔ `row & bit != 0`. Full 0–255 needs two row tables; Langdale's
  variant removes the blend (`pshufb` zeroes lanes whose index has bit 7 set): 3 shuffles, 3 AND, OR, XOR, compare.
- **For TCHAR (derived and verified by test):** TCHAR ⊂ 0x21–0x7E, so one row table suffices (Muła's "0–7" half):
  bytes ≥ 0x80 must give 0. NEON: index `x & 0x8F` (TBL returns 0 for indices ≥ 16); SSSE3: index `x` itself
  (`pshufb` uses bits 0–3 and zeroes on bit 7, ignoring bits 4–6). Cost per 16 B: NEON `and, ushr, tbl, tbl, and,
  cmeq #0` (6 ops, compiled code inspected); SSSE3 `pshufb, psrlw, pand, pshufb, pand, pcmpeqb` (6 ops). httparse's
  NEON `match_header_name_char_16_neon` uses exactly this table (httparse 1.10.1, `src/simd/neon.rs`, verified).
  The TCHAR rows decompose into 6 distinct low-nibble sets (one per high nibble 2–7), so the simdjson two-table
  form also fits (≤ 8 classes) with the same cost.
- **Hyperscan (verified from the paper text, §4–5):** FDR, a bucketed shift-or multi-string matcher using SIMD;
  "small string-set (<80) matching: a shift-or algorithm using the SSSE3 PSHUFB instruction applied over a small
  number (2–8) of 4-bit regions" (Teddy in the Hyperscan source; the paper does not name it); NFA/DFA "cyclic state
  acceleration" searches for the first exceptional 1–2 byte sequence with SIMD (shufti/truffle in the source). Not
  needed for HTTP heads (we match classes, not literal sets); cited for the shufti lineage only.
- **Short spans:** fine inside stage 1 (amortised over 64 B); per-span it loses to scalar in the dependent chain.
- **API:** private to `engine-simd`; exposed only through stage-1 masks.
- **Tests:** exhaustive 256-value × every-lane × tail differential vs the table (done in the prototype: 636,160
  single-byte cases and 2 M random buffers, arm64 native and x86 under Rosetta).

### 3.2 Range classification for field-content (`value_len`)
- **Sources:** httparse 1.10.1 `src/simd/{sse42,avx2,neon}.rs` (verified); Cloudflare 2014 (§3.6).
- **Mechanism:** NEON has unsigned compares: `bad = (vcltq(x, 0x20) | vceqq(x, 0x7F)) & !vceqq(x, 0x09)`.
  x86 lacks unsigned compares: `x <= 0x1F` ⇔ `pcmpeqb(pminub(x, 0x1F), x)` (httparse uses `pmaxub` for `>=`).
  All SSE2 → **baseline on every x86_64 target**, so it inlines with no detection (verified: `rustc --print cfg
  --target x86_64-unknown-linux-gnu` lists only `fxsr, sse, sse2`; asm inspected, 7 vector ops + `pmovmskb`).
  httparse names its x86 module `sse42` but uses only SSE2/SSE3 instructions (`lddqu`), not the SSE4.2 string ops.
- **Alternative:** the nibble LUT also works (3 classes: CTL row 0 minus HTAB, CTL row 1, DEL) at the same cost on
  x86 but needs SSSE3; range compares are preferred.

### 3.3 Bitmask extraction on NEON ("movemask" emulation)
- **Sources:** Kutenin, *Porting x86 vector bitmask optimizations to Arm NEON*, Arm community blog, 2022-08-29
  ([link](https://developer.arm.com/community/arm-community-blogs/b/servers-and-cloud-computing-blog/posts/porting-x86-vector-bitmask-optimizations-to-arm-neon));
  `memchr` 2.8.3 `src/vector.rs` (`NeonMoveMask`, verified); Lemire, *Scanning HTML at Tens of Gigabytes per Second
  on ARM Processors*, arXiv 2503.01662, 2025 ([HTML](https://arxiv.org/html/2503.01662)).
- **`shrn` (16 B → 64-bit, 4 bits per byte):** `vshrn_n_u16(reinterpret(cmp), 4)` then `fmov` to a GPR; first
  match = `trailing_zeros / 4` (`rbit` + `clz`). Replaces the ≥ 4-instruction SSE2NEON emulations; 10–15% on glibc
  `strlen` (Kutenin, unverified for us). `memchr` checks "any match" with `vpmaxq_u8` first, then `shrn`. httparse
  1.10.1 instead extracts two 64-bit lanes and loops over bytes (`offsetnz`) — a cheaper formula is available.
- **64 B → 64-bit, 1 bit per byte (Lemire 2025, simdjson NEON):** AND each compare result with
  `[1,2,4,…,128]×2`, then three rounds of `vpaddq_u8`; one `fmov` per 64 bytes. Lemire reports 3–4× over the
  WebKit/Blink 16-byte loops and up to 36 GB/s on Graviton 4 / M2 (unverified for us). This is what the stage-1
  prototype uses (verified correct; Miri run §5).
- **Cost note:** the vector → GPR move is the expensive step on Arm (Kutenin); stage 1 does it once per 64 B per
  class, per-span code does it once per span.

### 3.4 SSE4.2 `pcmpestri` / `pcmpistri` (picohttpparser, llhttp) — avoid
- **uops.info (verified):** `PCMPESTRI xmm, xmm, imm8`: 8 µops; latency ≤ 12 (data) / 15–17 (lengths) cycles on
  Skylake, Ice Lake, Alder Lake-P; ≤ 10–11 / 12–15 on Zen 3/4; reciprocal throughput 3–4 cycles
  ([page](https://uops.info/html-instr/PCMPESTRI_XMM_XMM_I8.html)). Compare `PSHUFB`: 1 µop, latency 1 (2 measured
  on Zen 4), throughput 0.5–1 ([page](https://uops.info/html-instr/PSHUFB_XMM_XMM.html)); `PMOVMSKB`: 1 µop,
  latency ≤ 3 (Intel) / ≤ 5–6 (Zen 3/4) ([page](https://uops.info/html-instr/PMOVMSKB_R32_XMM.html)).
- Matches Cloudflare's "11 cycles, ≈ 1.45 B/cycle" (2014). **Decision input:** no SSE4.2 string instructions in
  `engine-simd`; design §5.1's "SSE4.2/AVX2" wording should read "SSE2/SSSE3/AVX2".

### 3.5 SWAR (safe Rust, every target)
- **Sources:** Bit Twiddling Hacks (`haszero`, `hasless`, `hasbetween`; Anderson, Stanford, unverified page not
  fetched — formulas re-derived and verified by exhaustive test); httparse `src/simd/swar.rs` (verified).
- **`value_len` P0 form:** `lt20 = (x − 0x20…) & !x & 0x80…`, `eq7f` likewise on `x ^ 0x7F…`. Borrows only
  propagate upward from a truly flagged byte, so **the lowest flag is exact** (verified by test); only HTAB is a real
  false positive. So `trailing_zeros(mask) / 8` jumps straight to the stop byte; only an HTAB needs the table.
  This removes the per-word unpredictable `position` loop (`swar_tz` in §4).
- **`token_len` SWAR fast path** (exact per-lane range tests on 7-bit lanes: `y + (127 − m)` and `(127 + n) − y`
  cannot carry across lanes when `y ≤ 0x7F`; alpha via `y | 0x20` ∈ (0x60, 0x7B) whose preimage is exactly
  `A–Z ∪ a–z`): verified correct, but **slower** than the table in the parser chain (321 vs 186 ns, repeated head;
  425 vs 395 ns with `swar_tz` values, varied heads). Not recommended.

### 3.6 Two-stage structural indexing — simdjson stage 1 / Cloudflare picohttpparser AVX2
- **simdjson (verified, §3–4 of the paper):** stage 1 classifies 64-byte blocks branch-free into bitmasks and
  converts them to indexes (`tzcnt` + `blsr`, 8 unconditional writes per step); stage 2 walks the indexes. Stage 1
  costs 0.5–1 cycle/byte excluding UTF-8 validation; "running time is split evenly between our two stages".
  simdjson requires `SIMDJSON_PADDING` bytes after the input because it "may access bytes beyond the end of the
  string but before the end of the allocated memory"
  ([basics.md](https://raw.githubusercontent.com/simdjson/simdjson/master/doc/basics.md), verified).
- **Cloudflare, *Improving PicoHTTPParser further with AVX2*, V. Krasnov, 2014-12-18 (verified text,
  [link](https://blog.cloudflare.com/improving-picohttpparser-further-with-avx2/)):** a single-match AVX2 version
  "gave no throughput gain because HTTP fields are short"; the fix built **bitmaps for 128 bytes at a time** for
  the name/value delimiter and newline together and walked them with `tzcnt`: 1.79× / 1.68× over `pcmpestri` on
  Haswell (unverified for us; their workload, 2014 hardware).
- **For us:** stage 1 = per 64-byte block, two masks: `not_tchar` and `not_field_content` (the latter also marks
  CR, LF, NUL, DEL; LF is in it, so chunk-size line ends come for free). Stage 2 = the existing parser loop with
  `token_len`/`value_len` replaced by "next set bit at or after `pos`" (`mask[w] >> (pos % 64)`, `trailing_zeros`).
- **Resumability:** classification is per block and position-independent, so it composes with the resumable parser:
  either classify the whole received window once per `parse` call (fastest on varied heads), or classify lazily
  per 64-byte block the scan reaches and cache only the current block's two masks (16 bytes of parser state;
  fastest on repeated heads in the prototype). Bytes after the received data: classify a zero-padded copy of the
  last partial block (NUL is in neither class) or, better, rely on the pooled read buffer having ≥ 64 initialised
  bytes of slack (simdjson-style padding) and clamp results to the received length.
- **Dispatch cost disappears:** one stage-1 call covers 64 B or the whole head, so a function-pointer dispatch
  (§6.1) costs ~1 ns per head instead of ~0.4–1.4 ns per span (§4.2).

### 3.7 UTF-8 validation (Keiser & Lemire) — not needed, but its block structure is
- *Validating UTF-8 In Less Than One Instruction Per Byte*, Software: Practice and Experience 51(5), 2021
  ([arXiv 2010.03090](https://arxiv.org/abs/2010.03090), [HTML v5](https://arxiv.org/html/2010.03090v5),
  verified): three nibble lookups (high and low nibble of the previous byte, high nibble of the current) AND-ed into
  7 error bits; ASCII fast path per 64-byte block (OR the registers, test the sign bits); 0.21–0.97
  instructions/byte; >10 GiB/s on x64. HTTP field values are opaque bytes (obs-text allowed, no UTF-8 rule), so we
  do not validate UTF-8; the **per-64-byte ASCII fast path** is reusable: a head block without bytes ≥ 0x80 or < 0x20
  can skip the value-class work (a measured idea, `ideas.md`).

### 3.8 Related parsers
- *Parsing Millions of URLs per Second*, Nizipli & Lemire, SPE 54(5), 2024 ([arXiv 2311.10533](https://arxiv.org/abs/2311.10533)):
  "vector algorithms" in ada-url (abstract only verified; details not read).
- Lemire 2025 (above): one-lookup NEON classification for HTML's 4 special characters; first-match via `shrn` +
  `rbit`/`clz` (Blink) or `vmaxvq`/`vminvq` (WebKit); all variants fall back to scalar below 16/64 bytes and report
  no short-input measurements.
- No academic paper on vectorised **HTTP** parsing was found (search of arXiv/ACM/USENIX, 2026-11-06) beyond
  Cloudflare's blog and httparse/picohttpparser/llhttp source.

### 3.9 Parser-differential security constraints (what SIMD must not change)
- *T-Reqs* (Jabiyev, Sprecher, Onarlioglu, Kirda, CCS 2021; [repo](https://github.com/bahruzjabiyev/t-reqs)):
  grammar-based mutation, same request to two servers, compare. *HDiff* (DSN 2022; description only via the HTTP
  Garden paper — unverified). *The HTTP Garden* (Kallus, Anantharaman, Locasto, Smith, [arXiv 2405.17737](https://arxiv.org/abs/2405.17737)):
  coverage-guided differential fuzzing of request streams; >100 bugs, 68 fixed, 39 exploitable (abstract verified).
- **Constraint for `engine-simd`:** a vector path must accept exactly the bytes the table accepts, at every lane,
  every alignment and every block/tail boundary — a one-byte disagreement (e.g. treating `0x7F` or `0x0B` as valid
  in one path only) is a request/response-splitting differential between our own two code paths. Hence: the
  `[bool; 256]` tables stay the single source of truth; every SIMD classifier is tested against them exhaustively
  (§6.3).

## 4. Measurements (dev, arm64)

**Method.** Scratch crate (not in the repo):
`/private/tmp/claude-503/-Users-simransinghjatana-blazehttp/ed887b7f-c97a-4e33-8886-2a62e7f7a0d7/scratchpad/simd-research/scanbench`
(`src/scan.rs` primitives, `src/main.rs` harness). rustc 1.99.1, edition 2024, `lto = "fat"`, `codegen-units = 1`,
`std::hint::black_box` on inputs, 11 samples × 2 M (heads) or 5 M (sweeps) iterations, median. Host: Apple M2,
macOS (Darwin 25.4), **load average 3–6 from other processes** (not the nightly window). Built with
`-C target-cpu=generic` (what the Linux arm64 package ships; three runs, median of the three medians) and once with
the default `apple-m1` CPU. Correctness: every variant asserted equal to the table reference on every case before
timing.

### 4.1 Independent calls (throughput view), `/1k-h20` head, starts precomputed

| Variant | names (20 calls) | values (20 calls) |
|---|---|---|
| table (P0 names) | 101.0–117.0 ns | 154.4–165.6 ns |
| table8 (8 lookups AND-ed per word) | 87.6–93.2 | — |
| SWAR `[A-Za-z0-9-]` + table fallback | 77.4–81.8 | — |
| SWAR (P0 note §6.2) | — | 57.7–61.0 |
| SWAR + `trailing_zeros` | — | 53.1–59.0 |
| NEON 16 B per span | **28.0–28.8** | **36.0–37.6** |
| NEON via function pointer (runtime-dispatch shape) | — | 47.5–54.1 |
| SWAR + tz via function pointer | — | 60.3–62.9 |

In isolation NEON looks 3.6× (names) and 1.6× (values) faster. A function-pointer call adds ≈ 0.6–0.9 ns per span.

**Run-length sweep** (one call per measurement, haystack = run + stop byte + 256 B; predictor sees a fixed length,
best case for scalar; generic CPU, run 1):

| len | value SWAR | value SWAR+tz | value NEON | token table | token NEON |
|---|---|---|---|---|---|
| 1 | 1.50 | 1.57 | 1.88 | 1.52 | 1.81 |
| 5 | 2.11 | 1.53 | 1.78 | 3.31 | 1.81 |
| 8 | 1.80 | 2.13 | 1.78 | 4.20 | 1.82 |
| 12 | 2.41 | 2.08 | 1.79 | 5.51 | 1.99 |
| 16 | 2.67 | 2.99 | 2.07 | 8.59 | 2.29 |
| 24 | 3.05 | 3.33 | 2.17 | 10.88 | 4.75 |
| 32 | 3.31 | 3.89 | 3.06 | 17.17 | 3.60 |
| 64 | 5.31 | 6.63 | 3.63 | 21.96 | 5.89 |
| 128 | 10.09 | 11.74 | 6.18 | 50.91 | 8.23 |

Break-even (throughput view): NEON ≥ SWAR for values from ~8–12 bytes, 1.4–1.6× from 16 bytes; NEON ≥ table for
tokens from ~3 bytes. These are **not** the parser's break-even (§4.3).

### 4.2 Dependent chain on one repeated head (mini fused parse: name run, `:`, OWS, value run, CRLF ×20)

| Variant | median ns/head (3 runs, generic) |
|---|---|
| table names + SWAR values (P0 note) | **185.8** |
| table + SWAR+tz | 198.1 |
| SWAR names + SWAR+tz | 321.2 |
| NEON names + NEON values (per span) | 383.0 |
| NEON names + SWAR values | 260.8 |
| table names + NEON values | 280.1 |
| NEON + NEON via function pointers | 366.9–419.7 |
| **stage-1 bitmaps, whole head** | 194.4 |
| **stage-1 bitmaps, lazy per block** | **162.9 (0.88×)** |

### 4.3 Dependent chain on 256 varied heads (names shuffled, value lengths 0.5–1.5×; ≈ 881 B incl. padding)

| Variant | run 1 / 2 / 3 (generic) | median | `apple-m1` |
|---|---|---|---|
| table + SWAR (P0 note) | 526.9 / 584.5 / 436.2 | **526.9** | 468.5 |
| table + SWAR+tz | 395.0 / 417.7 / 365.6 | 395.0 (0.75×) | 358.7 |
| SWAR names + SWAR+tz | 425.3 / 451.6 / 425.1 | 425.3 | 425.4 |
| NEON + NEON (per span) | 511.5 / 494.5 / 515.5 | 511.5 | 468.8 |
| NEON names + SWAR values | 524.9 / 434.1 / 464.1 | 464.1 | 420.7 |
| **stage-1 bitmaps, whole head** | 246.1 / 204.5 / 221.7 | **221.7 (0.42×)** | 218.9 |
| stage-1 bitmaps, lazy per block | 342.0 / 285.0 / 287.8 | 287.8 (0.55×) | 307.4 |

Readings: (1) the same parser costs 2.8× more when heads vary (branch mispredictions); (2) per-span SIMD does not
fix that; (3) stage 1 is the only variant that is good on both, and it is the only one whose speed does not depend
on the predictor memorising the head. Noise is visible (varied runs spread ±15%); repeat in the Linux reference
container in the nightly window before any decision, and on the x64 VM.

### 4.4 Machine code (verified by `--emit asm`)
- NEON token loop (generic aarch64): `and, ushr, tbl, tbl, and, cmeq #0, shrn, fmov, cbnz` per 16 B; value loop:
  `cmhi, cmeq, cmeq, orr, bic, shrn, fmov, cbnz`. Exit: `rbit, clz, add …, lsr #2`. No bounds checks.
- x86_64 SSE2 value loop inlined into the caller (`pminub, pcmpeqb ×3, pandn, por, pmovmskb, bsf`).
- x86_64 SSSE3 token loop stays an **out-of-line function** (it has `#[target_feature(enable = "ssse3")]`, the
  caller does not): confirms the Reference rule in §6.1.

## 5. Correctness and Miri results (verified)

- Differential (prototype): 636,160 single-byte cases (every byte value at every position, lengths 0–70) + 2 M
  random buffers over a hostile alphabet (CR, LF, NUL, DEL, HTAB, obs-text, `:` …): all variants equal to the table,
  incl. the stage-1 masks bit-for-bit. Run natively on arm64 and as an `x86_64-apple-darwin` binary under Rosetta 2
  (SSE2 and SSSE3 paths; Rosetta reported `avx2 = false`, so AVX2 was not executed natively).
- **Miri** (`nightly-2026-11-05`, `rustc 1.101.0-nightly 249c1e4e3`, Miri master `5f09e81`), reduced case set
  (lengths ≤ 34, 15 interesting byte values, 50 random buffers):
  - aarch64 (host): **passes** with `vqtbl1q_u8`, `vshrn_n_u16`, `vceqzq_u8`, `vcltq_u8` (≈ 3 min). Miri's shim
    list contains `neon.tbl1.v16i8`, `neon.umaxp.v16i8`, `neon.addp.*` (source read).
  - `--target x86_64-unknown-linux-gnu` with `RUSTFLAGS="-C target-feature=+ssse3,+sse4.2,+avx2"`: **passes**
    (SSE2, SSSE3 `pshufb`, AVX2 paths). Miri's x86 shims include `pshuf.b.128`, `pshuf.b` (AVX2), all `pcmp*str*`.
  - Cost: Miri runs ≈ 50 single-byte cases per second for these loops, so CI Miri jobs must use a small case set;
    the exhaustive differential runs natively and in fuzzing.
  - Stage 1 on aarch64 (`vpaddq_u8` mask reduction, checked bit-for-bit against the tables on the 50 random
    buffers): **passes** under Miri. The x86 stage 1 was not prototyped (x86 is correctness-only here).

## 6. Recommendations for P1a `engine-simd`

### 6.1 API and dispatch
- **Official rules (verified, Rust Reference 1.99.1 "Code generation attributes"):** "Functions marked with
  `target_feature` are not inlined into a context that does not support the given features. The `#[inline(always)]`
  attribute may not be used with a `target_feature` attribute." Safe `#[target_feature]` functions are callable
  safely only from callers that enable the same features. `std::arch` docs: dynamic detection with
  `is_x86_feature_detected!` and an `unsafe` call into a `#[target_feature]` function; the macro "expands to `true`"
  when the feature is enabled at compile time. `core::simd` / `std::simd` is nightly-only (`portable_simd`,
  #86656) → not allowed in shipped code (§6.3). `slice::as_chunks` is stable since 1.88.0.
- **Seam:** expose **block classification**, not per-span lengths:
  ```rust
  // engine-simd (safe API; unsafe inside, each block with // SAFETY:)
  pub struct BlockMasks { pub not_tchar: u64, pub not_field_content: u64 }
  pub fn classify_block(block: &[u8; 64]) -> BlockMasks;          // dispatched once per block
  pub fn classify(blocks: &[[u8; 64]], out: &mut [BlockMasks]);   // dispatched once per call
  ```
  The parser keeps the masks of the block it is in (or of the whole received window) and computes
  `token_end(pos)` / `value_end(pos)` with `trailing_zeros`. The `[bool; 256]` tables remain the single source of
  truth and the scalar fallback (`classify_block_scalar`, also the Miri/fuzz oracle).
- **Dispatch:** aarch64: NEON is a default target feature (verified: `rustc --print cfg --target
  aarch64-unknown-linux-gnu` → `neon`) → static, no detection. x86_64: SSE2 baseline is enough for
  `not_field_content`; `not_tchar` needs SSSE3 (`pshufb`) or an SSE2 range formulation (≈ 15 ops/16 B). Select the
  implementation once (`std::sync::OnceLock<fn(..)>` or the `memchr`-style self-replacing `AtomicPtr` that
  `memchr` 2.8.3 and httparse's `AtomicU8` use) — one indirect call per head is ~1 ns. Ship order to measure:
  SSE2+SSSE3 (16 B ×4) → AVX2 (32 B ×2). **Owner decision to raise:** whether the x64 npm binary may assume
  x86-64-v2 (SSSE3/SSE4.2 static) — not needed if dispatch is per block.
- **Do not** dispatch per span (measured +0.6–0.9 ns per span, ≈ 40 spans per head) and do not ship per-span
  16-byte loops as the P1a primitive (slower than scalar in the chain, §4.2).

### 6.2 Benchmark first (P1a criterion, both arches)
1. Stage-1 whole-window classification + bitmap walk (NEON; SSE2/SSSE3; AVX2) vs P0 scalar, on **both** the
   repeated `/1k-h20` head and a ≥ 256-head varied ring; plus the 3-header head and the 1-byte feed.
2. Lazy per-block variant (current-block cache) vs whole-window (they trade places between repeated and varied).
3. Mask extraction on NEON: `vpaddq` ×3 (1 bit/byte, 64 B) vs `shrn` per 16 B (4 bits/byte, 4 words).
4. ASCII/CTL-free block fast path (skip the value class when a block has no byte < 0x20 or ≥ 0x7F).
5. Padding: pooled buffers with ≥ 64 initialised slack bytes vs a zero-padded copy of the tail block.

### 6.3 Testing
- Unit: exhaustive 256 × 64-lane × alignment differential of every `classify_block` implementation vs the tables;
  all-ones/all-zeros/alternating blocks; a block straddling the received-data end.
- Fuzz (`cargo-fuzz`, pinned nightly): `classify` vs scalar on arbitrary bytes and offsets; parser-level
  differential (SIMD build vs scalar build vs httparse oracle) as already planned for the fuzz crate.
- Miri: per design §6.3 (`--target x86_64-unknown-linux-gnu -C target-feature=+sse4.2,+avx2`, plus aarch64) with
  a **reduced** case list (Miri speed, §5); keep `+ssse3` in the flags (implied by `sse4.2`). Measured to work today.
- Every `unsafe` block: `// SAFETY:` naming the bounds argument (fixed-size `&[u8; 64]` arrays make loads in-bounds
  by type; the only feature argument is the dispatcher's detection).

### 6.4 Risks
| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Gains measured on a loaded M2 do not reproduce in the Linux container / on x64 | M | H | re-run in the nightly window and on the x64 VM before adopting; keep scalar as the fallback |
| Stage 1 changes the parser seam (masks instead of `fn(&[u8]) -> usize`) | H | M | introduce the seam in P0 (optional, §7) or as the first P1a story |
| SIMD/table disagreement → parser differential (smuggling) | L | H | exhaustive + fuzz differential, tables as single source of truth |
| Miri too slow for exhaustive SIMD tests | H | L | reduced Miri set; exhaustive natively and in fuzz |
| Function-pointer dispatch blocks inlining | M | M | dispatch per block/window only |
| Varied-head speed depends on the branch predictor (CPU-specific) | M | M | benchmark both rings on both arches |

## 7. Optional input for the scalar P0 parser (Task 4)

None of this is required for P0; the P0 design is sound. Each item is cheap and measured above (dev):
1. **Add a varied-head ring to Task 4's benchmark** (e.g. 256 heads: shuffled `/1k-h20` names, value lengths
   0.5–1.5×). The repeated head hides branch mispredictions: 186 vs 527 ns for the same code (§4.3).
2. **`value_len`: jump with `trailing_zeros`** instead of re-scanning the flagged word with the table: the lowest
   SWAR flag is exact (§3.5), so `p = mask.trailing_zeros() / 8`; if `w[p] != HTAB` the run ends at `p`, else
   continue from `p + 1` with the table. 395 vs 527 ns on varied heads (0.75×), 198 vs 186 ns on the repeated head
   (+6%, within noise of the loaded host). Measure in Task 4's bench on both rings; adopt if the varied ring wins.
3. **Seam shape:** if convenient, phrase the parser's calls as `pos = scan.token_end(window, pos)` /
   `scan.value_end(window, pos)` (a small `Scanner` value the parser owns) rather than free `fn(&[u8]) -> usize`.
   P0's implementation is the table/SWAR; P1a's stage-1 bitmaps then slot in without touching the phase logic.
4. Keep the TCHAR **table** for names in P0: table8 and a SWAR `[A-Za-z0-9-]` fast path were slower in the chain.

## 8. Sources

| # | Source | Link | Accessed | Status |
|---|---|---|---|---|
| 1 | Langdale, Lemire, *Parsing Gigabytes of JSON per Second*, VLDB J. 28(6), 2019 | https://arxiv.org/abs/1902.08318 , https://arxiv.org/html/1902.08318v7 | 2026-11-06 | verified (text) |
| 2 | Keiser, Lemire, *Validating UTF-8 In Less Than One Instruction Per Byte*, SPE 51(5), 2021 | https://arxiv.org/abs/2010.03090 , https://arxiv.org/html/2010.03090v5 | 2026-11-06 | verified (text) |
| 3 | Muła, *SIMDized check which bytes are in a set*, 2018 (upd. 2019) | http://0x80.pl/notesen/2018-10-18-simd-byte-lookup.html | 2026-11-06 | verified (text; HTTPS cert mismatch, read over HTTP) |
| 4 | Muła, `simd-byte-lookup` repo | https://github.com/WojciechMula/simd-byte-lookup | 2026-11-06 | verified (README) |
| 5 | Wang et al., *Hyperscan*, NSDI 2019 | https://www.usenix.org/system/files/nsdi19-wang-xiang.pdf | 2026-11-06 | verified (text §4–5); Teddy/shufti names from source, not paper |
| 6 | Lemire, *Scanning HTML at Tens of Gigabytes per Second on ARM Processors*, 2025 | https://arxiv.org/html/2503.01662 | 2026-11-06 | verified (text); speed numbers unverified for us |
| 7 | Nizipli, Lemire, *Parsing Millions of URLs per Second*, SPE 54(5), 2024 | https://arxiv.org/abs/2311.10533 | 2026-11-06 | abstract only |
| 8 | Kutenin, *Porting x86 vector bitmask optimizations to Arm NEON*, 2022-08-29 | https://developer.arm.com/community/arm-community-blogs/b/servers-and-cloud-computing-blog/posts/porting-x86-vector-bitmask-optimizations-to-arm-neon | 2026-11-06 | verified (text); % gains unverified for us |
| 9 | Krasnov (Cloudflare), *Improving PicoHTTPParser further with AVX2*, 2014-12-18 | https://blog.cloudflare.com/improving-picohttpparser-further-with-avx2/ | 2026-11-06 | verified (text); 1.68–1.79× unverified for us |
| 10 | httparse 1.10.1 `src/simd/{neon,avx2,sse42,swar,runtime,mod}.rs` | https://github.com/seanmonstar/httparse/tree/9f29e79f9832dbd0ae5220acb17c1866745bdecd/src/simd | 2026-11-06 | verified (source) |
| 11 | memchr 2.8.3 `src/vector.rs`, `src/arch/x86_64/memchr.rs`, `src/arch/generic/memchr.rs` | https://docs.rs/crate/memchr/2.8.3/source/ | 2026-11-06 | verified (source) |
| 12 | uops.info: PCMPESTRI, PSHUFB, PMOVMSKB | https://uops.info/html-instr/PCMPESTRI_XMM_XMM_I8.html , …/PSHUFB_XMM_XMM.html , …/PMOVMSKB_R32_XMM.html | 2026-11-06 | verified |
| 13 | Rust Reference, Code generation attributes (`target_feature`) | https://doc.rust-lang.org/reference/attributes/codegen.html | 2026-11-06 | verified |
| 14 | `std::arch` (1.99.1) | https://doc.rust-lang.org/std/arch/index.html | 2026-11-06 | verified |
| 15 | `is_x86_feature_detected!` (1.99.1, since 1.27.0) | https://doc.rust-lang.org/std/macro.is_x86_feature_detected.html | 2026-11-06 | verified |
| 16 | `std::simd` (nightly-only, `portable_simd` #86656) | https://doc.rust-lang.org/std/simd/index.html | 2026-11-06 | verified |
| 17 | `slice::as_chunks` (stable 1.88.0) | https://doc.rust-lang.org/std/primitive.slice.html#method.as_chunks | 2026-11-06 | verified |
| 18 | Rust 1.89.0 release notes (AVX-512 intrinsics and target features) | https://blog.rust-lang.org/2025/08/07/Rust-1.89.0/ | 2026-11-06 | verified (lists AVX512 intrinsics under stabilized APIs) |
| 19 | Miri README (target features, cross-interpretation, "very few AVX512 intrinsics") | https://github.com/rust-lang/miri | 2026-11-06 | verified |
| 20 | Miri intrinsic shims at `5f09e81` (`src/intrinsics/aarch64.rs`, `x86/{ssse3,sse42,avx2}.rs`) | https://github.com/rust-lang/miri/tree/5f09e816076dcd5c0f4cc941a37407754ddfb193/src/intrinsics | 2026-11-06 | verified (source) + run (§5) |
| 21 | simdjson `doc/basics.md` (padding) | https://raw.githubusercontent.com/simdjson/simdjson/master/doc/basics.md | 2026-11-06 | verified |
| 22 | Kallus et al., *The HTTP Garden*, arXiv 2405.17737, 2024 | https://arxiv.org/abs/2405.17737 | 2026-11-06 | verified (abstract) |
| 23 | Jabiyev et al., *T-Reqs*, CCS 2021 | https://github.com/bahruzjabiyev/t-reqs | 2026-11-06 | verified (repo description) |
| 24 | HDiff, DSN 2022 | (described in #22) | 2026-11-06 | unverified (secondary) |
| 25 | `rustc --print cfg --target {aarch64,x86_64}-unknown-linux-gnu` (1.99.1) | local | 2026-11-06 | verified (run) |
