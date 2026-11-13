# HTTP/1.1 parser and writer speed: research note (input to Tasks 4c, 5b, 5c and P1a)

Design §5.2 (strict rules, unchanged), §6 (learn from proven code, never copy), §7 (speed programme), §13 (code
structure and patterns), §16 (gates). Builds on `docs/notes/response-parser.md` (§8.4 budget, §8.6 cross-language
race), `docs/notes/simd-scanning-research.md` (SIMD survey; referred to, not repeated) and, on branch `p0/writer`,
`docs/notes/request-writer.md` and `docs/perf/journal.md`. Access date for every link: **2026-11-08**. **Verified** =
checked against the pinned source, the primary text of a paper, an official doc page, or a run recorded in this note
with its command, seed and output (the two runs are the compile-only asm check and the executable model in
`tools/h1-research/`, §4.1 and §5.0; the model verifies only what its checks cover, §4.1 "Coverage limits");
**unverified** = everything else (unverified ideas go only to `docs/perf/ideas.md`). No benchmark was run for this
note: every speed figure for a candidate is an **estimate (unmeasured)** that only orders what is measured first
(§7, §8); it is never evidence that a gate is reachable. Measured figures name their source.

## 0. Summary and decision

### 0.1 The problem in numbers (quiet re-run, parser note §8.6 on branch `p0/parser-bench-rerun`, PR #21)

Dev run of 2026-11-08 at commit `8ff4b87` (`bench/results/2026-11-08-arm64-dev-parsers-gcc14.jsonl`), Linux arm64,
generic Armv8-A; median per-round ratio ours / theirs (> 1 = theirs faster) for `1k`, `1k-h20`, `1k` ring64,
`1k-h20` ring64, mixed 64. Dev numbers (rounds 1–4 loaded), not gate results:

| Rival | ours **with fields** / theirs | ours plain / theirs | P-A projection (plain × 1.075–1.10, §5.0; estimate) |
|---|---|---|---|
| httparse 1.10.1 | 1.23, 1.74, 1.39, 1.95, 1.78 | 0.80, 0.93, 0.79, 0.94, 0.93 | 0.86–0.88, **1.00–1.02**, 0.85–0.87, **1.01–1.03**, **1.00–1.02** |
| picohttpparser `dd573d8` | 1.19, 1.30, 1.42, 1.42, 1.33 | 0.72, 0.68, 0.80, 0.71, 0.68 | 0.77–0.79, 0.73–0.75, 0.86–0.88, 0.76–0.78, 0.73–0.75 |
| llhttp 9.4.3 (strict) | 0.81, **1.09**, 0.85, **1.18**, **1.11** | 0.51, 0.55, 0.49, 0.56, 0.57 | 0.55–0.56, 0.59–0.61, 0.53–0.54, 0.60–0.62, 0.61–0.63 |
| Beast `basic_parser` | 0.72, 0.81, 0.82, 0.97, 0.90 | 0.46, 0.45, 0.45, 0.46, 0.46 | ≈ 0.48–0.51 |
| Beast `response_parser`, Zig, Go | 0.08–0.53 | 0.06–0.30 | — |

1-byte feeds: we are the fastest of all nine parsers (0.03–0.20 ms; httparse 48–90 ms; Beast 9–878 ms).

| Component | Gate (owner; statistic defined once in design §16.4 (component micro-gates)) | Today | Gap |
|---|---|---|---|
| `ResponseParser` with per-field spans | **#1 against every rival** (httparse, picohttpparser, llhttp, Beast, Zig, Go and any row added): the cell passes when the design §16.4 CI upper bound is < 1.0; every workload (1k, 1k-h20, ring64, mixed, trickle), arm64 reference host **and** x64 VM; met by the **scalar** parser and checked by Task 4c (owner decision 2026-11-13, §0.2) | loses to httparse (1.23–1.95×), picohttpparser (1.19–1.42×), and llhttp on 20-field and mixed heads (1.09–1.18×) | 1.1–2.0× |
| `write_request` | the same statistic against every writer of the Task 5c race (§3.13) at equal work (§3.12); `post1k` ≤ 2.3× copy floor (provisional) | `post1k` 3.01–3.07× floor, `varied` 2.58–2.63×, `get` 2.43–2.51× (measured: journal "Request writer after review fix round 1", branch `p0/writer`, PR #18, head `786d341`, dev run, not a gate run); 1.16–1.22× hyper encode-only (earlier journal entry of 2026-11-08) | ≈ 25% on the floor ratio, on a floor that is not yet a stable measure (§6) |

Constraints that stay: the §5.2 checks, `#![forbid(unsafe_code)]` in `engine-core`, `unsafe` SIMD only in
`engine-simd`.

### 0.2 Decision **[author's call]**

- **P-A alone is not a win.** Emitting spans in the scan removes the second pass and wins clearly against
  picohttpparser and llhttp (projected 0.53–0.88) but is projected to **lose to httparse by 0–3%** on the three
  20-field cells (1.00–1.03, estimate, §5.0). It is the necessary first step, not the answer.
- **Task 4c builds P-A + P-B** (§5.2–§5.3; steps, order and acceptance: plan "Task 4c"): spans in the scan, one loop
  body per line on locals, the STOP-set name search with an off-critical-path tchar fold, caller-owned spans. P-B's
  gain is **unmeasured**: the §5.3 arithmetic gives ≈ 0.4–0.85× httparse on 20-field heads on arm64 depending on
  whether the name search saves anything (the SIMD note measured a SWAR name path as slower in the chain, §5.3), and
  x64 has **no data**. The first step of 4c is a profile of plain G that replaces the guessed split.
- **Owner decision (2026-11-13): the gate is met without SIMD.** Task 4c passes only when our **scalar** parser (no
  `engine-simd` kernel) beats every rival in every cell on arm64 and x64, with equal work, by the design §16.4
  statistic (CI upper bound < 1.0). Rivals run as shipped with their own SIMD: httparse NEON and AVX2,
  picohttpparser `pcmpestri` (the race builds every row for `x86-64-v3`, so that path is on). While a cell loses,
  Task 4c is not closed: it continues as research (profile, study the rival, design our own algorithm, measure) and
  reports progress to the owner. **Where the estimates stand:** they do not show P-A + P-B winning every cell (a
  scalar name scan may not beat httparse's NEON name scan on arm64 or picohttpparser's `pcmpestri` on x64), so the
  research loop is expected to be needed; §5.7 lists where it starts. **SIMD is an extra layer:** P1a's P-D
  (bit-stream validator over `engine-simd` bitmaps, §5.5) is built on top of a scalar parser that already wins and is
  kept only where it measures faster than that parser. It is never the way to meet the gate. Its speed is unknown
  until prototyped (the only stage-1 measurement, SIMD note §4.2, was not faster than scalar on a fixed head), and
  P1a chooses its lead from a prototype comparison (plan "P1a entry conditions recorded in P0"; §5.5).
- **Task 5b builds W-A** (shared known-name classifier, STOP search; §6.2) without the authority-id item, whose
  `TemplateId` binding moves to P1a (plan "P1a entry conditions recorded in P0", item 9): in P0 a shard has one
  template, so Task 6 keeps the authority compare. **W-B** (layout hints) moves to **P2** (it needs the
  P2 wire contract). **Task 5c** builds the writer race (§3.13). **P1a** builds `classify_block` and P-D; W-D is
  optional in P1a, behind a measurement (§6.5).
- Expected x64 position of every candidate: §5.2–§5.5 (parser) and §6.7 (writer).

### 0.3 Design changes proposed by this note **[author's call]**

Each is applied by the task that builds it, in the same PR as the code:
1. `ResponseParser::parse(&mut self, buf, spans: &mut FieldSpans)` — caller-owned spans (§5.1); `ResponseHead` gains
   `fields: u16`; our bench row emits spans itself, and `blazehttp+fields` is deleted after one pinned-baseline
   comparison (§5.7).
2. Optional padded read buffer: `parse` receives the received length and a slice with initialised slack (§5.3,
   measured first).
3. Parser note §6.3 "body bytes are never examined": P-D loads up to 63 body bytes into registers when a block
   straddles the head end; they are masked and never interpreted (§5.5). The semantic rule is unchanged.
4. P2 wire format: optional per-line layout hints next to the delta (W-B, §6.3); interned name ids in
   `policy/spec.json` (W-C, §6.4).
5. Design §16.4 (component micro-gates): the parser and writer micro-gate statistic, defined there once and computed
   by one function, `pairedRatioCi` in `bench/parsers/stats.ts` (added by Task 4c, reused by Task 5c).
6. The `[bool; 256]` class tables move from `engine-core`'s `grammar.rs` into `engine-simd` (the lower crate), and
   `engine-core` re-exports them, so the scalar and SIMD kernels share one source of truth (§5.5; P1a).
7. Plan: the plan sections "Task 4c", "Task 4d", "Task 5b", "Task 5c" and "P1a entry conditions recorded in P0"
   hold the steps, order and acceptance of the work this note researches; this note keeps the research.
   `bench/writer/` is replaced by the Task 5c harness; W-B moves to P2; W-D is P1a (optional, measured).

## 1. References (pinned)

Every reference is pinned to its latest published release on 2026-11-08: the GitHub Release (`releases/latest`) where
the project publishes them, otherwise its newest release **tag** (Seastar, Beast/Boost, Go, Zig on Codeberg,
llparse, ureq-proto), then `commits/<tag>` for the commit. Projects with neither are pinned to the default branch's
head commit (picohttpparser, h2o, simdcsv).
Files were fetched as raw files at these commits and read; nothing was built or run.

| Project | Tag | Commit | Link base (append the path and `#L<n>`) |
|---|---|---|---|
| httparse | `v1.10.1` | `9f29e79f9832dbd0ae5220acb17c1866745bdecd` | https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/ |
| picohttpparser | none (HEAD, 2026-10-27) | `dd573d8fa9264c646a400e96ce9f07dea6f156e4` | https://github.com/h2o/picohttpparser/blob/dd573d8fa9264c646a400e96ce9f07dea6f156e4/ |
| llhttp (DSL) | `v9.4.3` | `45c8699d0ca8431ab366c8706e613b2e2ac62c04` | https://github.com/nodejs/llhttp/blob/45c8699d0ca8431ab366c8706e613b2e2ac62c04/ |
| llhttp (generated C) | `release/v9.4.3` | `0e815792b167a9bd8ace259b95b7da953776c288` | https://github.com/nodejs/llhttp/blob/0e815792b167a9bd8ace259b95b7da953776c288/ |
| llparse | `v7.3.1` | `963d45058e55a0aa35d6dae355d6669ed53b64aa` | https://github.com/nodejs/llparse/blob/963d45058e55a0aa35d6dae355d6669ed53b64aa/ |
| uWebSockets | `v20.81.0` | `045bbc11ef4f3ec933c112579d3e8922302a798b` | https://github.com/uNetworking/uWebSockets/blob/045bbc11ef4f3ec933c112579d3e8922302a798b/ |
| h2o | none (releases stopped; master 2026-11-06) | `f35c735ebe0657235702c2b91912542052cc05db` | https://github.com/h2o/h2o/blob/f35c735ebe0657235702c2b91912542052cc05db/ |
| nginx | `release-1.31.8` | `140aa942711868a8daeb95810bc9a81f272ada72` | https://github.com/nginx/nginx/blob/140aa942711868a8daeb95810bc9a81f272ada72/ |
| fasthttp | `v1.77.0` | `61bdcc30d0484032c9b2af7c16f6ec0f2089d288` | https://github.com/valyala/fasthttp/blob/61bdcc30d0484032c9b2af7c16f6ec0f2089d288/ |
| Seastar | `seastar-25.05.0` | `8df8212e53577e1d8477a5c901457cd61d88afc7` | https://github.com/scylladb/seastar/blob/8df8212e53577e1d8477a5c901457cd61d88afc7/ |
| Bun | `bun-v1.4.4` | `38afee6c84fcf48c0edb2466aa4b27d287a59c30` | https://github.com/oven-sh/bun/blob/38afee6c84fcf48c0edb2466aa4b27d287a59c30/ |
| Netty | `netty-4.2.20.Final` | `4fb6a5f308382b4bee1bff3ef80b45ea8b35166d` | https://github.com/netty/netty/blob/4fb6a5f308382b4bee1bff3ef80b45ea8b35166d/ |
| Go | `go1.27.3` | `2aae1000ea1d4b28923b96db28acf4e3b99d6aff` | https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/ |
| Zig (Codeberg) | `0.17.1` | `be248cc1fdc3cc654f9ec8ba68514bcb93b204fe` | https://codeberg.org/ziglang/zig/src/commit/be248cc1fdc3cc654f9ec8ba68514bcb93b204fe/ |
| hyper | `v1.12.0` | `7f67699ce32d717081deb7018b4da7d679277ac0` | https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/ |
| http (hyper's types) | `v1.5.0` | `16fc9a7b840c2181e7f8b37397c107b0ffcd050d` | https://github.com/hyperium/http/blob/16fc9a7b840c2181e7f8b37397c107b0ffcd050d/ |
| undici | `v8.12.4` | `779ee3e12cb8c1f365f6e69ee34b088159402d2f` | https://github.com/nodejs/undici/blob/779ee3e12cb8c1f365f6e69ee34b088159402d2f/ |
| curl | `curl-8_23_0` | `3163ec065643ba20709f190111f42c3e1fe95273` | https://github.com/curl/curl/blob/3163ec065643ba20709f190111f42c3e1fe95273/ |
| Boost.Beast | `boost-1.92.0` | `7c1e061f91e2ef542217b76286c314d006c0c8fc` | https://github.com/boostorg/beast/blob/7c1e061f91e2ef542217b76286c314d006c0c8fc/ |
| simdjson | `v5.1.1` | `40a686ab89dc5ee58ec10d0514dd5c99abbc472b` | https://github.com/simdjson/simdjson/blob/40a686ab89dc5ee58ec10d0514dd5c99abbc472b/ |
| simdutf | `v9.3.3` | `59589cd3df6823cbfdbd253cde3377e7c3e1fe9b` | https://github.com/simdutf/simdutf/blob/59589cd3df6823cbfdbd253cde3377e7c3e1fe9b/ |
| simdcsv | none (HEAD, 2021-01-19) | `a9494a85c83525bb42ba368d8f9bcf3acf53a8d1` | https://github.com/geofflangdale/simdcsv/blob/a9494a85c83525bb42ba368d8f9bcf3acf53a8d1/ |
| Hyperscan | `v5.4.2` | `bc3b191ab56055e8560c7cdc161c289c4d76e3d2` | https://github.com/intel/hyperscan/blob/bc3b191ab56055e8560c7cdc161c289c4d76e3d2/ |
| Vectorscan | `vectorscan/5.4.13` | `acd7363aadea43da9c5246542d9969db843dd132` | https://github.com/VectorCamp/vectorscan/blob/acd7363aadea43da9c5246542d9969db843dd132/ |
| cpp-httplib | `v0.65.0` | `7e5d0683ac326788e52ce3c44585c1af24b04532` | https://github.com/yhirose/cpp-httplib/blob/7e5d0683ac326788e52ce3c44585c1af24b04532/ |
| ureq-proto (ureq's writer) | `0.7.1` (crates.io 0.7.1) | `acb5cc3ee1261709316acd6538be08e82f84f3c6` | https://github.com/algesten/ureq-proto/blob/acb5cc3ee1261709316acd6538be08e82f84f3c6/ |
| Jetty | `jetty-12.1.14` | `15cecbd757a2f6b0fdc6a8944f4c6d0020b17da0` | https://github.com/jetty/jetty.project/blob/15cecbd757a2f6b0fdc6a8944f4c6d0020b17da0/ |

Line references below (`L<n>`) are to the file named in each paragraph at these commits.

## 2. Parsers: why each one is fast

Each entry records the fields the brief asks for: passes, branch structure, tables, end-of-head detection, where
validation happens, field output, partial input. It then gives the reasons it is fast, ranked, and **what it takes
for us to be faster than that parser specifically, with equal work**. Equal work means the same output: status,
version, head length, per-field name/value spans with OWS trimmed, framing facts and keep-alive. All claims are
verified against the pinned source unless marked otherwise. Gains are not; they are hypotheses for §7.

### 2.1 httparse `v1.10.1` (Rust; the parser hyper uses)

- **Flow** (`src/lib.rs` L653–714): it skips leading empty lines byte by byte (L558–580). The version is one `u64`
  compare against `HTTP/1.0`/`HTTP/1.1` when ≥ 8 bytes are present (L759–775), otherwise byte by byte. Then exactly
  one SP, 3 digits (000–999 accepted, L933–941), and the reason scanned one byte at a time with no SIMD (L839–884;
  any obs-text makes the reason `""`, L853–859).
- **Cursor** (`src/iter.rs`): three raw pointers; `next()` is one compare plus a load (L187–198); `advance`/`bump`
  are unchecked `unsafe` (L79–92); `slice()` hands out `start..cursor` and moves `start` (L105–129), so a span
  costs nothing until it ends.
- **Header loop** (`src/lib.rs` L1073–1246), one loop body per field line:
  1. first-byte check (CRLF or bare LF ends the head; else `TOKEN_MAP`, L1106–1136);
  2. name via `match_header_name_vectored`, then `next!` must be `:` (L1141–1152; `from_utf8_unchecked`, no UTF-8
     pass);
  3. OWS skipped one byte at a time (L1173–1196);
  4. value via `match_header_value_vectored`, which validates and stops at the first non-value byte in one step
     (L1201–1212); CR must be followed by LF, a bare LF is accepted;
  5. trailing whitespace trimmed with `rposition`, usually one byte (L1229–1239);
  6. one 32-byte store `Header { name: &str, value: &[u8] }` into the caller's array (L1241–1245).
  `TooManyHeaders` is detected only after the line is parsed (L1044, L1223). Config flags are runtime `bool`
  checks; there are no branch hints.
- **SIMD selection** (`build.rs` L33–94, `src/simd/mod.rs`): aarch64 always uses NEON, chosen at compile time
  (mod.rs L141–153). x86_64 uses AVX2 or SSE4.2 only when those features are enabled at compile time; the default
  x86_64 target goes through `runtime.rs`, which caches the detected feature in a static `AtomicU8` (L20–31) and
  checks it on every value scan (L48–57). **Names on x86 are never vectorised**: always the SWAR module (L33–35).
- **SWAR** (`src/simd/swar.rs`): values: `lt = (x − 0x20…) & !x`, the same test on `x ^ 0x7F…` for DEL, then
  `offsetnz`, which finds the first flagged byte **with a byte loop** (L163–176; the comment at L161 says
  `trailing_zeros` measured slower for them). Names: `match_block` is a plain table loop over the 8 bytes
  ("naive fallback", L95–104). The loop bound is the remaining buffer, not the token (L40): a 5-byte value still
  reads a whole word.
- **"SSE4.2"** (`src/simd/sse42.rs` L57–84) uses only SSE2 plus `lddqu`: per 16 bytes `lddqu`, `cmpeq(max_epu8(x,
  0x20), x)`, `cmpeq(x, 9)`, `cmpeq(x, 0x7F)`, `andnot`, `movemask`, `trailing_zeros`. Under 16 bytes left it falls
  back to SWAR (L54). **AVX2** (`avx2.rs` L100–140): the same on 32 bytes; under 32 bytes left, SWAR, not SSE
  (L93–94).
- **NEON** (`src/simd/neon.rs`): names use the Muła nibble LUT (L84–122): `idx = x & 0x8F` (TBL returns 0 for
  indices ≥ 16, so every byte ≥ 0x80 is rejected), `row = tbl(bitmap, idx)`, `bit = tbl([1,2,4,…,128]×2, x >> 4)`,
  `vceqq(row & bit, bit)`: 6 vector ops per 16 B, constants reloaded with `vld1q` per call. Values: `vcleq(0x20, x) |
  vceqq(x, 9)` minus `vceqq(x, 0x7F)` (L139–154). The stop position: two `vgetq_lane_u64` extracts, then a byte loop
  per lane (L157–189), not `shrn`.
- **Record:** one forward pass per call, restarting at byte 0 on every call (`Bytes::new(buf)`, L660); end of head
  found inline at the empty line; validation fused with scanning; output 32 B per field into the caller's array,
  whose length caps the field count (`TooManyHeaders`); partial input = re-parse from 0 (quadratic under a trickle:
  48–90 ms on the 1-byte feeds in the quiet re-run, §0.1).
- **Looser than §5.2:** bare LF (L692, L862, L1113, L1208), status 000–999, leading empty lines, and opt-in
  obs-fold, space before the colon and multiple SPs. (An empty reason is also allowed by §5.2, so it is not a
  difference.)
- **x86 scope:** the value scan stops with `movemask` + `trailing_zeros` on both the SSE2 (`sse42.rs` L83) and AVX2
  (`avx2.rs` L144) paths; only the SWAR and NEON paths use byte loops for the stop. The runtime `AtomicU8` dispatch
  exists only when the build enables neither feature at compile time (an x86-64-v1 baseline); with `-C
  target-cpu=x86-64-v2`/`v3` the path is fixed at compile time.
- **Why it is fast, ranked:** (1) the value scan validates and finds the CR in one fused step; (2) block width
  bounded by the buffer, not the token; (3) a raw-pointer cursor with no index arithmetic or bounds checks; (4) the
  version as one `u64` compare; (5) one 32-byte store per field and no UTF-8 pass; (6) the SIMD path fixed at compile
  time on aarch64. **Weak spots:** scalar names on x86, a byte-loop stop position on the SWAR and NEON paths, a
  runtime-dispatch call per value on an x86-64-v1 build, restart from 0.
- **To be faster than httparse with equal work.** Its per-field path on a short head is: first-byte table check,
  name scan (NEON LUT on arm64, table on x86), `:` check, OWS loop (usually 1 iteration), one SIMD/SWAR block that
  stops at the CR, CR/LF check, one `rposition` step, one store. **What it skips that we do:** status range,
  framing facts with duplicate/conflict rules, keep-alive, head and field limits, bare-LF rejection and
  resumability. The fair opponent is therefore httparse **plus** the facts pass the bench already adds (hyper does
  the same work in a second pass over the `Header` array). We must: emit spans where the CRLF is found (no second
  pass); collapse the per-part dispatch into one loop body per line; keep the framing dispatch at the CRLF; on
  arm64 match its NEON name LUT (scalar alone may not, §5); on x86 use inlined SWAR or SSE2 (baseline) and so avoid
  its per-call dispatch.

### 2.2 picohttpparser `dd573d8` (C; also h2o's and Bun's head parser)

- **Primitives:** `IS_PRINTABLE_ASCII(c) = (c − 040u) < 0137u` (`picohttpparser.c` L61); `CHECK_EOF` returns −2,
  `EXPECT_CHAR` −1 (L63–77); `likely`/`unlikely` hints (L39–53); a 256-entry `char` `token_char_map` (L104–111).
- **`findchar_fast`** (L113–140), SSE4.2 only: `left = (buf_end − buf) & ~15` makes the loop condition a counter;
  one `_mm_cmpestri(ranges, n, b16, 16, LEAST_SIGNIFICANT | CMP_RANGES | UBYTE_OPS)` per 16 bytes. **Without
  `__SSE4_2__` it returns `buf` unchanged** (L133–139): every arm64 build is scalar (there is no NEON code).
- **Ranges:** values (L147–149): `[00–08] [0A–1F] [7F]`, so the scan stops at any CTL except HTAB, or DEL; obs-text
  passes. Names (`parse_token` L258–265, 8 ranges, the `pcmpestri` maximum): `[00–20] [22] [28–29] [2C] [2F] [3A–40]
  [5B–5D] [7B–FF]` — the non-tchar bytes, except that `|` and `~` fall inside `[7B–FF]` because a ninth range does
  not fit (comment L256–257). After a SIMD stop a scalar loop checks `:` and the table (L272–281); a name containing
  `|` or `~` stays scalar to its end.
- **`get_token_to_eol`** (L142–203), scalar build: `while (likely(buf_end − buf >= 8))` runs 8 unrolled
  `if (unlikely(!IS_PRINTABLE_ASCII(*buf))) goto NonPrintable; ++buf;` — one subtract-compare per byte and one
  end check per 8 bytes (L155–178). HTAB and obs-text re-enter the loop; any other CTL goes to `FOUND_CTL` (CR must
  be followed by LF; bare LF accepted; L188–199).
- **`parse_headers`** (L306–361): a line starting with CR or LF ends the head; obs-fold yields an entry with
  `name = NULL` (L323, L340–343); `parse_token(':')`; OWS loop with `CHECK_EOF` per byte (L333–339); value; a
  backward trim loop; four stores into `phr_header { name, name_len, value, value_len }` (L346–358).
- **Status line** (L437–480): 9 bytes required up front, then unchecked expects; any minor digit 0–9 (L288–304);
  multiple SPs after the version (L449–452); first status digit 1–5 (L241–250, L454–458).
- **Partial input:** re-parse from 0; with `last_len` set, `is_complete` first scans only the new bytes (from
  `last_len − 3`) for two line ends in a row (L205–231, L497) — so a complete head costs one extra scan of the
  last bytes plus a full parse. It accepts mixed LF/CRLF terminators.
- **Looser than §5.2:** bare LF, obs-fold (NULL-name entries), `HTTP/1.2`–`HTTP/1.9`, multiple SPs. The field count
  is capped by the caller's `max_headers` (−1 when exceeded).
- **Why it is fast, ranked:** (1) fused classify-and-stop; (2) **on arm64 it is pure scalar** — the 8× unrolled
  loop, end check per 8 bytes, branch hints and the cursor in locals passed through return values — and it still
  beats us 1.19–1.42× with fields (quiet re-run), so **our gap is loop structure and per-field overhead, not vector width**
  (inference from source + parser note §8.6); (3) on x86 one `pcmpestri` per 16 B (high latency, 8 µops, but one
  instruction); (4) no EOF checks inside the up-front-checked status prefix; (5) four stores per field.
- **To be faster than picohttpparser with equal work.** Its per-field path on arm64: scalar name loop (`:`, table,
  EOF check per byte), OWS loop, 2–4 trips of the unrolled value loop plus a scalar tail, four stores. On x86: two
  `pcmpestri` per field, three when the value is longer than 16 B (the common case on `1k-h20`), whose ~11-cycle
  latency dominates short tokens. **It skips:** bare-LF and obs-fold
  rejection, version 1.0/1.1 only, framing facts and keep-alive, limits, resumability (Bun's wrapper rejects
  obs-fold; the bench adds the facts pass). We must: keep SWAR `value_len` (already ≥ the 8× byte loop above ≈ 8 B)
  and its exact `trailing_zeros` stop; make the name scan no costlier than its scalar loop (a padded buffer removes
  the end check, §2.4); bring per-field overhead to its level — one store, cursor in locals, no phase round trips.
  On x86, SSE2/SWAR with `tzcnt` beats `pcmpestri` latency on short tokens (uops.info, SIMD note §3.4).

### 2.3 llhttp `v9.4.3` + llparse `v7.3.1` (C generated from a TypeScript DSL; Node's parser)

- **How states compile** (llparse `src/implementation/c/`): `llhttp__internal__run` enters with one `switch` on
  `state->_current` (generated `src/llhttp.c` L1225–1230); every state change after that is a `goto` (`node/base.ts`
  `tailTo` L63–74); the state is stored **only on return** (base.ts L59–61; execute L10264). Each consuming state
  starts with `if (p == endp) return` (base.ts L47–57): one end check per byte. Single nodes compile to
  `switch (*p)`; table-lookup nodes to a `static uint8_t lookup_table[256]` plus a switch (`node/table-lookup.ts`
  L45–69, L360–386); sequences call `llparse__match_sequence_*`, one byte at a time with `s->_index` to resume
  (L254–345).
- **SIMD emission** (table-lookup.ts): only a node with a single self-loop edge and no transform (L72–93). SSE4.2
  up to 2 × 8 ranges, `_mm_cmpestri(… NEGATIVE_POLARITY)`; a value that ends mid-block costs a second `pcmpestri`
  at the stop byte (generated L2782–2786). NEON only for ≤ 3 ranges (`MAX_NEON_RANGES = 6` numbers, L14): `vcgeq`/
  `vcleq` per range, `vshrn_n_u16(mask, 4)`, `ctzll >> 2` (L245–254). **tchar needs 9 ranges, so names are never
  NEON**: on arm64, about 2 branches per name byte. The wasm path uses signed compares (L321–323; suspected bug,
  below).
- **Header grammar** (generated `llhttp.c`, DSL `src/llhttp/http.ts`): `header_field_start` (L3327–3349; CR → head
  end, LF an error unless lenient); `header_field` walks a trie over `connection`, `content-length`,
  `proxy-connection`, `transfer-encoding`, `upgrade` on the lowercased first bytes (L3289–3316; http.ts L582–586)
  and falls into `header_field_general` (table per byte + SSE4.2 ranges, L3039–3106) with no rescan; OWS skipped by
  `header_value_discard_ws` (L2972–2999); **framing values are parsed during the scan** (`Content-Length` with
  overflow-checked multiply-add, `chunked`, `Connection` tokens; http.ts L722–829); `header_value` is the hot loop
  (L2743–2875); CR ends the span and LF must follow (L2219–2234, L2412–2429); obs-fold rejected unless lenient
  (L2201–2218). Trailing OWS is **not** trimmed from value spans (http.ts L672, L849).
- **Output:** spans by callback — `span_start` records the position, `span_end` calls `cb(start, p)`, loaded through
  `settings` with a null check and an indirect call (`src/native/api.c` L7–30, L407–425 at the llhttp DSL pin): up to four calls per field. An open
  span is flushed at the end of each `execute` (L10266–10276), so a field split across reads arrives in two
  callbacks and the embedder must join them.
- **Response line** (http.ts L338–428): one SP, status 000–999; the reason accepts every byte except CR/LF,
  control characters and NUL included (L411–422).
- **Record:** exactly one pass across all calls; resumable at any byte; validation fused into the state machine;
  strict by default.
- **Why it is fast, ranked:** (1) direct-threaded `goto`s, state written only on return; (2) vectorised value
  loops inside the state (NEON `shrn` + `ctz >> 2`); (3) span bookkeeping costs nothing per byte; (4) framing
  headers classified by a trie and `Content-Length` parsed during the scan; (5) no re-parse. **Costs:** an end check
  per byte, byte-at-a-time names on arm64, indirect callbacks, a double `pcmpestri` at each value end on x86.
- **What strict llhttp still skips** (equal-work list; the skew is against us, so the comparison is conservative):
  leading CR/LF before the status line (`llhttp.c` L7058–7075); the status range (000–999); reason byte checks (NUL,
  CTLs and DEL accepted); version and protocol checks (`HTTP/0.9`, `HTTP/2.0`, RTSP and ICE start lines parse); all
  head, field-count and 1xx limits; trailing-OWS trimming of value spans.
- **To be faster than llhttp with equal work.** It is resumable and computes framing facts in its scan, like us; it
  beats us today with fields on 20-field and mixed heads (1.09–1.18, §0.1) only because of our second pass — plain we
  are at 0.49–0.57. We must: store spans directly (P-A) instead of its four indirect calls; scan names faster than its
  one byte with two branches on arm64; save resume state only at exits; keep one length match per field for the
  framing names. After P-A the projection is 0.53–0.63 (§0.1, estimate). On x64 llhttp scans names with two
  `pcmpestri` per 16 B (blob8 + blob9) and values with `pcmpestri` plus a second one at each value end; our P-B is
  expected to stay ahead (≈ 0.6–0.8, estimate).
- **Issues (suspected, by reading):** under wasm `simd128` with `HEADER_VALUE_RELAXED`, the range `[0x0e, 0xff]` is
  emitted with signed `wasm_i8x16_ge/le` (0xff = −1), so no byte matches (L2374–2377) and the
  `header_value_relaxed` → `test_lenient_flags_19/_20` → `header_value_relaxed` path can loop without advancing for
  a byte such as VT (L2382–2385, L8435–8452); root cause llparse table-lookup.ts L321–323 (fix: `wasm_u8x16_*`);
  strict mode unaffected. The reason phrase accepts CTLs.

### 2.4 uWebSockets `v20.81.0` (C++ server; request parser only)

- **Fence** (`src/HttpParser.h`): `MINIMUM_HTTP_POST_PADDING = 32` (L42); after the data it writes `'\r', 'a'`
  (L505–508), so the value scan always stops at the fence CR and needs no end check — `p[1] == '\n'` doubles as
  "inside the data" (L441–443, L474–477); the name scan stops at the fence and `p == end` tells partial input from an
  error (L413–420). 8-byte reads may run 7 bytes past the fence; the padding covers them.
- **Loops:** values (L355–364): an 8-byte `memcpy` plus `hasLess(word, 32)` (L231–233); on a hit the word is
  rescanned byte by byte (no `tzcnt`); HTAB skipped, any other non-CR byte → 400 (L426–440). Names (L275–301):
  runs of `[A-Z]` lowercased in place (`|= 32`), runs of `[a-z]` skipped, `:` returns, `-` advances, anything else
  to a branchy generic tchar check (L253–273). The SWAR helpers `hasBetween`/`notFieldNameWord` (L239–249) have no
  callers. The request line checks the target with `hasLess | hasMore` and the version with one 11-byte `memcmp`
  (L304–349; only HTTP/1.1).
- **Lookup:** a 256-bit bloom filter sampling 4 key bytes (`BloomFilter.h` L30–79), then a linear `strncmp`
  scan.
- **Record:** one pass per attempt plus the bloom pass and trims; partial input copied into a fallback
  `std::string` (≤ 4 KiB) and re-parsed from 0 per segment (L705–784); ≤ 100 fields; keys lowercased in place;
  **accepts DEL in values** (L359–360); rejects bare LF, obs-fold and space before the colon.
- **Why it is fast:** (1) the fence removes every end check; (2) one SWAR test per 8 bytes; (3) range runs for
  names; (4) one `memcmp` for the version; (5) O(1) lookup misses via the bloom filter.
- **To be faster than uWS (request heads; it cannot parse responses).** Copy the fence safely: pooled read buffers
  with initialised slack and a sentinel, scan the padded slice with no tail loop, clamp results to the received
  length; use `trailing_zeros` instead of its byte rescan; keep framing fused (uWS calls `getHeader` with a linear
  `strncmp` afterwards). Do not copy in-place lowercasing (JS receives the bytes) or accepting DEL.
- **Issues (suspected):** a bloom false positive makes a request with one `Host` header a 400 (L526–529,
  `mightHave` before `add`); on the fallback path the fence is written past `size()` (L505–508 via L715).

### 2.5 h2o client (C; master `f35c735`)

- `lib/common/http1client.c` calls `phr_parse_response` with `last_len = 0` (L423–426): **every read re-parses
  from byte 0**. The vendored `deps/picohttpparser/picohttpparser.c` is byte-identical to the pinned pico (same blob
  `c94753d6`). The server side passes `last_len` (lib/http1.c L650–653).
- Per field it adds (L446–465): obs-fold → error; `h2o_strdup` copies the name; `h2o_strtolower` lowercases it in
  place; `h2o_lookup_token` (generated `token_table.h` L273+, from `misc/tokens.pl` L144–147) switches on length,
  then the last character, then `memcmp` — not a perfect hash. Framing checks later are pointer compares (L505–531).
- **Issues:** duplicate `Content-Length` — the last wins (L523–528); `chunked` with `Content-Length` not rejected
  (L529–530); minor versions 1–9 map to 1.1 (L443).
- **To be faster than h2o:** it is picohttpparser's work plus a copy, a lowercase pass and a lookup per field, and a
  full re-parse per read. Beating pico beats h2o. Worth reusing: the *length → one discriminating byte → compare*
  shape for name dispatch (§5, §6).

### 2.6 nginx `release-1.31.8` (C; upstream responses in the proxy)

- **Status line** (`src/http/ngx_http_parse.c` `ngx_http_parse_status_line` L1675–1892): `for (p = b->pos; p <
  b->last; p++) switch (state)` over 13 states, one byte per step behind a jump table (the comment at L105 says so).
  `HTTP/` is five one-compare states; major 1–999 and minor 0–999 (L1758–1811, so `HTTP/2.0` passes); any number of
  SPs before the code and between its digits (`sw_status` skips SP, L1815–1817); after the code SP, `.` (IIS `403.1`), CR or bare LF (L1833–1849); the reason
  accepts every byte except CR/LF, NUL and CTLs included (L1852–1861). On buffer end it saves `r->state` and returns
  `NGX_AGAIN` (L1875–1878). The proxy copies the reason into the pool and falls back to HTTP/0.9 on a parse error
  (`ngx_http_proxy_module.c` L1823–1865).
- **Field lines** (`ngx_http_parse_header_line` L870–1145, one call per line): **one table, two jobs** —
  `lowcase[256]` maps `A-Z` to `a-z`, `a-z0-9-` to themselves and everything else to 0, so one load validates the
  common name alphabet and yields the lowercased byte (L889–897, L964–971); the name is **hashed during the scan**
  (`ngx_hash(key, c) = key * 31 + c`, `ngx_hash.h` L114) and its lowercase copied into `lowcase_header[32]`.
  Looseness that buys speed: other name bytes only set `r->invalid_header`, which the proxy never reads (L953–955,
  L1022); only SP is skipped after the colon (HTAB becomes a value byte, L1029, L1043–1046); in values only SP, CR,
  LF and NUL are special (L1051–1068), so the loop is 4 compares per byte with no table; bare LF ends lines and the
  head (L918–920, L1000–1004); `CR CR LF` accepted (L1101–1107). Obs-fold is rejected (a line starting with SP fails
  `ch <= 0x20`, L948–951). Output: pointer pairs plus the hash written into the request as it scans; fully
  resumable, nothing re-scanned (state, hash and lowercase index saved, L1123–1128, restored L899–901).
- **Lookup and framing** (`ngx_http_upstream.c`): `ngx_hash_find` — bucket `key % size`, length, bytes
  (`ngx_hash.c` L12–49) — costs one modulo and one bucket because the hash was built during the scan; the proxy
  layer allocates one pool block per field and copies name, value and lowercase name (proxy L1908–1938). Duplicate
  `Content-Length`, CL with TE, and TE other than `chunked` are errors (upstream.c L5008–5045, L5489–5530).
- **Chunked** (`ngx_http_parse_chunked` L2216–2485): chunk data is never scanned (L2339–2341); extensions and
  trailers are skipped to CR **without validation** (L2322–2330, L2398–2406); `ctx->length` keeps the minimum bytes
  still needed, a read-sizing hint (L2427–2471).
- **Why it is fast:** no separate end-of-head pass and no re-parse; a 4-compare value loop; one load to validate
  and lowercase a name byte; header dispatch reduced to one bucket probe. **Why not faster:** one byte per step behind
  an indirect jump, no SWAR/SIMD, a serial multiply chain for the hash, an allocation and 2–3 copies per field in
  the proxy.
- **To be faster than nginx with equal work.** It skips: CTL rejection in values and reason, non-token name
  rejection, HTAB as OWS, bare-LF and `CR CR LF` rejection. Our values already scan 8 bytes per step against its 1;
  our per-byte name loop costs about the same as its loop minus the hash and lowercase store, and a word-at-a-time
  name search (§5) puts us clearly ahead. Do not add a second pass for spans. Expected comfortable win (unverified;
  nginx is not in the race).

### 2.7 fasthttp `v1.77.0` (Go)

- **Driver** (`header.go` `Read`/`tryRead` L2711–2773): on `ErrNeedMore` it retries with one more buffered byte and
  **re-parses from byte 0** (`h.parse(b)`, L2767). v1.77 adds `blockTerminated`, a `bytes.Contains(b[start:],
  "\r\n\r\n")` that searches only bytes not searched before (L37–39, L58–67), so a split head costs one partial
  attempt, an incremental terminator search and one full parse.
- **Status line** (`parseFirstLine` L3450–3507): `bytes.IndexByte` (assembly SIMD) for `\n` (bare LF accepted,
  L4050–4060), `' '` and the code end; multiple SPs skipped; reason not validated; protocol and reason copied.
- **Field lines** (`headerscanner.go` `next` L41–152): the name is a byte loop testing `== ':'` then
  `validHeaderFieldByteTable[c]` (the 77 token characters, `bytesconv_table.go` L11); spaces inside and before the
  colon are accepted and trimmed (L109–113; header.go L3632, L3642–3646). OWS loop (L121–123). **Values: 8 bytes per
  step with validation folded in** (`scanValueLine` L359–394): `anyByteIsCTL(w)` (`swar.go` L48–51: `x := w ^
  0x7f…; ((w − 0x20…) &^ w | (x − 0x01…) &^ x) & 0x80… != 0`) — the same formula as our `value_len`; a flagged word
  drops to a byte loop (HTAB counts as a CTL here, so any word with a TAB takes the slow loop). Trailing OWS and CR
  trimmed backwards (L131–137). It needs the byte after each LF to rule out obs-fold (L138–145), and **accepts
  obs-fold by joining continuation lines in place, overwriting the input** (L190–293).
- **Dispatch and storage** (header.go L3618–3764, `args.go`): switch on `key[0] | 0x20`, then a length check and a
  per-byte `toLowerTable` compare (cookie.go L742–752); special headers copied into dedicated fields; duplicate CL
  and TE are errors, only `chunked`; **CL with TE accepted, TE wins** (L3676–3679, L3725); other headers are copied
  and canonicalised into slots reused from the previous response (`appendArgNormalized`, args.go L472–519): zero
  allocations in steady state but **two copies per field**; lookup by a linear `bytes.Equal` scan.
- **To be faster than fasthttp with equal work.** It skips bare-LF, space-in-name, obs-fold and CL+TE rejection and
  reason validation (all free in our grammar). Its scans are the same kind as ours, so we win by never copying and
  never re-scanning. A ready SWAR lowercase is `lowercaseWord` (swar.go L60–65). Not in the race; it is the right
  "fastest Go" row (§2.13).

### 2.8 Seastar `seastar-25.05.0` (C++, Ragel)

- The grammar is Ragel regular expressions (`src/http/response_parser.rl` L91–119: `tchar`, `field_vchar = graph |
  obs_text`, `header_1st = (field ':' sp_ht* <: value crlf) %assign_field`) compiled with `ragel -G2`
  (`CMakeLists.txt` L428): every state is a label with compare/jump trees, one transition per byte, no SIMD.
- Resumable through `_fsm_cs` (`include/seastar/core/ragel.hh` L101–142); a token split across buffers is rebuilt
  by copying (`sstring_builder`, L69–96). **Every token becomes an `sstring`** (L45–84); duplicate names are merged
  as `old + "," + value` (L63–72), so two `Content-Length: 5` become `"5,5"`. The header map is an
  `unordered_map<sstring, sstring, case_insensitive_hash, …>` (`reply.hh` L149) whose hash takes the key **by value**
  and calls `tolower` per byte (`string_utils.hh` L57–62).
- Strict on names (`tchar+ ':'`), values (no CTLs) and CRLF; loose on start-line separators (Ragel `space` accepts
  `\t\v\f\n\r`, L115), the reason (`(any − cr − lf)*`), obs-fold (joined) and duplicates (merged).
- **To be faster than Seastar:** it spends one transition per byte, two `sstring` constructions per field (heap
  allocations only for tokens over the 15-byte small-string buffer, so most names and short values do not
  allocate), a by-value key copy plus a `tolower` pass per hash, and a map insert (which allocates a node); any
  span-emitting parser that never allocates wins by a wide margin (estimate, unverified).

### 2.9 Bun `bun-v1.4.4` (client in Rust over picohttpparser; uWS fork for its server)

- **Client** (`src/picohttp/lib.rs` L42–80, `src/http/lib.rs`): `phr_parse_response` through FFI into a
  `#[repr(C)]` ptr/len array; pico built at commit `f4d94b48` with a chunked-decoder patch
  (`scripts/build/deps/picohttpparser.ts` L13–31), x64 with `-march=nehalem` (SSE4.2, so `pcmpestri` is compiled
  in; `scripts/build/flags.ts` L73–76), arm64 scalar. **It passes `last_len = 0` on every call** (http/lib.rs
  L3818, L3826–3830): every short read re-parses from 0 and pico's `is_complete` shortcut is never used. Obs-fold is
  rejected in the wrapper (picohttp/lib.rs L519–531). Name dispatch hashes each lowercased name with Wyhash and
  compares it with `hash_header_const(b"Content-Length")`-style values that are **computed at run time** (the comment
  says Wyhash is not a const fn; L684–689, L1032–1039, L5010–5103); names are matched by 64-bit hash only.
  Conflicting CLs rejected (L5019–5031), `Connection: close` sticky (L5070–5077).
- **Server** (`packages/bun-uws/src/HttpParser.h`): uWS's fence (L1257–1260) and `hasLess` SWAR value scan
  (L1047–1064, L760–762); Bun keeps wire casing (upstream lowercases in place), raises the field limit to 200, and in
  native mode accepts DEL (L1058–1061). Requests only.
- **To be faster than Bun:** its client is picohttpparser plus a Wyhash per field plus a full re-parse per short
  read, so beating pico while emitting spans beats Bun's client.

### 2.10 Netty `netty-4.2.20.Final` (Java)

- `HttpObjectDecoder` (`codec-http/src/main/java/io/netty/handler/codec/http/HttpObjectDecoder.java`) switches on
  a `State` (L371–424). Each line: `buffer.indexOf` for LF (L1315), `strictCRLFCheck` rejects a bare LF by default
  (`DEFAULT_STRICT_LINE_PARSING = true`, L156–157, L1327–1335), then the line is copied into a scratch buffer
  (L1347–1348). `indexOf` is SWAR (`ByteBufUtil.firstIndexOf` L574–612): `SWARUtil.applyPattern` = `input = word ^
  pattern; tmp = (input & 0x7F7F…) + 0x7F7F…; ~(tmp | input | 0x7F7F…)` — an **exact** per-byte zero test with no
  false positives (`common/src/main/java/io/netty/util/internal/SWARUtil.java` L38–42), index `trailing zeros >>> 3`
  (L54–57).
- `splitHeader` (L1162–1209) loops byte by byte to the colon and, **for responses, tolerates whitespace before the
  colon** (L1179–1185); the name becomes a new `AsciiString` (copy; the response decoder interns nothing), the value
  a new `String` (L1148–1160, L1205–1213); obs-fold accepted by concatenation (L780–785); framing is read **after**
  all lines are stored, by map lookups (`getAll(CONTENT_LENGTH)`, `contains(TRANSFER_ENCODING)`, L813–842). Complete
  lines are never re-parsed; a partial line is re-searched from its start on every call (L1315).
- **To be faster than Netty:** three copies and two allocations per field plus a map dominate; spans with no copies
  win by several times (estimate). Worth taking: the exact SWAR equality test.

### 2.11 Go `go1.27.3` (`net/http` client: `ReadResponse` → `textproto`)

- `bufio.Reader.ReadSlice('\n')` uses `bytes.IndexByte` (assembly SIMD) and never re-searches searched bytes
  (`src/bufio/bufio.go` L348–387); `ReadLine` drops `\n` or `\r\n` (bare LF accepted, L405–438). The status line is
  a `string` allocation, `strings.Cut`, `Atoi` (`src/net/http/response.go` L158–189).
- `upcomingHeaderKeys` **pre-counts** the buffered lines (`src/net/textproto/reader.go` L630–652) to size one shared
  `[]string` backing array and the map (L531–540) — a whole extra pass to save allocations.
- Per line: `IndexByte(':')` (L619–624); obs-fold joined (L136–188); `canonicalMIMEHeaderKey` validates, then
  canonicalises in place, then probes the `commonHeader` interning map without allocating (L757–804); value
  validated byte by byte (L567–571), trimmed, allocated (L579), inserted into the map (L581–604). The token test is a
  **128-bit bitmask held in two `u64` constants** (`((1<<c) & lo) | ((1<<(c-64)) & hi)`, L693–745) — branch-free, no
  memory load; in Rust it needs `checked_shl` or a guarded `u128` shift, about the cost of our table.
- **To be faster than Go:** already ≈ 10× in the race (parser note §8.6). Its cost is 1–2 allocations per field,
  a map insert and ≈ 6 passes per line. Rule for us: no per-field allocation on the engine path.

### 2.12 Zig `0.17.1` (`std.http`)

- **`HeadParser.feed`** (`lib/std/http/HeadParser.zig` L20–328) only finds the end of the head, resumably (state
  `start`/`seen_n`/`seen_r`/`seen_rn`/`seen_rnr` carried across calls, L5–12): per block of
  `max(suggestVectorLength(u8), 8)` bytes (L21: 16 on NEON and SSE2, 32 on an AVX2 target) it compares with
  splats of `\r` and `\n`, ORs, and **counts** matches with `@reduce(.Add)` (L119–123). Zero matches: skip. 1–3:
  only the last 1–3 bytes are checked to carry state (L126–168). ≥ 4: an `inline for` over offsets compares each
  4-byte window as a `u32` against `\r\n\r\n` (and its top half against `\n\n`) (L169–183). The integer views
  (`int16/24/32`, L330–347) give the partial-terminator states from the same load.
- **`Response.Head.parse`** (`lib/std/http/Client.zig` L515–612) is a second pass: `splitSequence("\r\n")`, one
  `u64` compare for `HTTP/1.x`, `parseInt3` with **no digit check** (L697–702), `splitScalar(':')`, `trim(" \t")`,
  a chain of up to 7 `eqlIgnoreCase` calls. **No name or value validation**; a bare LF inside a value is kept.
  Fields are not returned; `iterateHeaders` re-splits on demand (`HeaderIterator.zig` L13–48), a third pass.
- **Suspected bug (by reading, bare-LF terminator only):** with ≤ 3 CR/LF bytes in a block only the tail is
  checked, so a `\n\n` in the middle of a block (e.g. `HTTP/1.0 200\n\nAB…`) is missed (the engine review confirmed
  it by simulation). CRLF heads are unaffected. A second end-detection quirk affects invalid heads only (mixed
  terminators); it does not change valid-head results.
- **To be faster than Zig:** it validates nothing and hands no fields out, so the fair row is our plain parser
  (bench README). Our single pass must cost no more than its two; its end scan is 16 B per step, but the split
  walks every byte again.

### 2.12a Boost.Beast `boost-1.92.0` `basic_parser` (C++; in the race)

- `find_fast` is a **no-op** (`http/detail/basic_parser.ipp` L109–117: returns `{buf, false}`; the SSE4.2 path is
  gone), so names are a per-byte `is_token` table loop with a `need_more` end check per byte (L507–530), and values
  go through `parse_token_to_eol` (L208–241): one `is_print` test per byte, a second test for HTAB/DEL on
  non-printables, CR then LF required (bare LF rejected; the `#if 0` block at L243 shows it was considered). Leading
  OWS one byte at a time; trailing OWS via `trim_back`; one byte of lookahead after each CRLF for obs-fold, which is
  **accepted** and unfolded into a `char_buffer` (L560–600).
- Per field (`http/impl/basic_parser.ipp` `inner_parse_fields` L360–405): `string_to_field(name)` (a lookup of the
  known-field enum), `do_field` (framing facts: duplicate CL, TE, Connection, Upgrade), then
  `on_field_impl(f, name, value)` — a virtual call in the `basic_parser` subclass the bench uses, a `fields` insert
  with an allocation in `response_parser`. A partial head returns `need_more` and the current line is re-parsed on
  the next call (9–878 ms on the 1-byte feeds in the quiet re-run, §0.1).
- **To be faster than Beast with equal work:** it is a scalar byte loop with an end check per byte plus a name
  lookup and an indirect call per field; we already beat it with the second pass (with fields 0.72–0.97 against
  `basic_parser`, 0.30–0.53 against `response_parser`, §0.1), and P-A widens the margin (≈ 0.48–0.51, estimate).
  Nothing to copy except the explicit enum for known names, which the shared classifier (§6.1) provides.

### 2.13 Parsers not yet in the race

The race (`bench/parsers/`) has httparse, picohttpparser, llhttp, Beast (two APIs), Zig and Go. Candidates:

| Parser | Response-head API to time | Container build | Plausibly faster than httparse? |
|---|---|---|---|
| **fasthttp** v1.77.0 | `ResponseHeader.Read` on a reused, `Reset` `bufio.Reader`, fields via `VisitAll` | trivial (the Go image, module pinned) | no, but the fastest Go row; several × `net/http` (estimate) |
| **nginx** release-1.31.8 | `ngx_http_parse_status_line`, then `ngx_http_parse_header_line(r, b, 1)` until `NGX_HTTP_PARSE_HEADER_DONE`, recording pointer pairs; facts by the shared extraction pass | moderate: configure + build at the tag, link the harness to `ngx_http_parse.o` and core objects | unlikely (1 B/step), but the "C server state machine" reference |
| **Netty** 4.2.20.Final | `EmbeddedChannel(new HttpResponseDecoder())`, `writeInbound`/`readInbound`, JMH | JDK + Maven image; JIT and GC noise | no (allocates per field) |
| **Seastar** 25.05.0 | `http_response_parser p; p.init(); p(temporary_buffer<char>(head)); get_parsed_response()` | heavy (Ragel 6.10, Seastar, fmt, boost; tight under the 2 GiB cap) | no (allocates per field) |
| **uWebSockets** v20.81.0 | `HttpParser::getHeaders` / `consumePostPadded` — **requests only** | easy (header-only) | plausibly, on requests (fence + SWAR); not applicable to responses |

h2o's client and Bun's client are picohttpparser (already in the race). **Among the parsers surveyed here, none
outside the race is plausibly faster than httparse.** Not surveyed yet: the response parsers of the engines that PR
#20 added to G0 — Jetty `HttpParser` (jetty-12.1.14), the JDK's `java.net.http` `Http1HeaderParser`, and .NET's
`SocketsHttpHandler` `HttpConnection` header parsing — deferred to the Task 4c research step (§10), with fasthttp
and nginx added as race rows in Task 4c (fasthttp closes "every Go parser", nginx is the C server reference); Netty,
Seastar and the JVM/.NET parsers are leaderboard rows (G8) unless the survey finds one faster than httparse.

### 2.14 What every fast parser does that we do not (against `crates/engine-core/src/h1/parser/mod.rs`)

1. **Spans are stored at the CRLF, in the same pass, everywhere** (httparse L1241, pico L357, uWS L445; llhttp by
   callback). Our `value()` holds `line_start`, `colon` and the value end at the CRLF, hands them to `facts.field`
   and drops them; the bench's `split_fields` rebuilds them in a second pass. This explains most of the 1.2–2.0×
   gap (plain / with-fields = 0.48–0.65 in the quiet re-run).
2. **One loop body per field line with the cursor in locals.** Our `step()` goes round the phase loop three times per
   field (`LineStart`, `Name`, `Value`), each returning a `Result<Option<ParseStatus>, ErrorCode>` (a large value),
   re-slicing `window.get(self.pos..)` and writing `self.phase`/`pos`/`line_start`/`colon`; about eight
   `checked_add`/`ok_or` branches per field. No reference has these.
3. **The status prefix** is checked by a per-byte closure; httparse uses one `u64` compare for `HTTP/1.x`.
4. **No end checks** (uWS fence) and no scalar tails.
5. **Names are scalar on every platform.** Rivals vectorise names on some targets: httparse with NEON on arm64;
   picohttpparser and llhttp with `pcmpestri` on x86 when SSE4.2 is enabled at compile time (scalar on arm64 and on an
   x86-64-v1 baseline). httparse's x86 names are a table loop like ours.

Where we are already ahead: an exact first-flag `trailing_zeros` where httparse's SWAR/NEON paths and uWS use byte
loops (its SSE2/AVX2 paths use `trailing_zeros` too), resumability without re-parse (fastest on every 1-byte feed),
and strictness. (Our lack of an OWS loop is not a lead under equal work: every rival skips OWS in 0–1 iterations.)

## 3. Writers and serialisers: why each one is fast

Same treatment as §2, one level deeper than `docs/notes/request-writer.md` §2 (branch `p0/writer`). Topics per
writer: when validation runs (construction or write), pre-serialised blocks, integer formatting, the copy pattern,
vectored vs copied output, word-at-a-time validation; then **what it takes to be faster than that writer with equal
work** (equal work is defined in §3.12 and used by the race in §3.13).

**Our baseline, from two different benches (journal 2026-11-08; do not mix their ratios):**
- *Committed bench* (`h1_write_bench`, writer vs copy floor, the gated ratio), measured after Task 5's review fix
  round 1 (journal entry "Request writer after review fix round 1", branch `p0/writer`, PR #18, head `786d341`; dev
  run on a loaded host, not a gate run): `post1k` writer 135.5–166.6 ns, floor 45.2–54.5 ns, ratio **3.01–3.07×**
  (gate ≤ 2.3×); `varied` 2.58–2.63×; `get` 2.43–2.51×. (The earlier entry's 2.51–2.58× is superseded.) The floor's
  time moved 13–21% between two binaries although its code is unchanged (review docs-review-4, M1/M2), so a ratio to
  that floor is not yet a stable measure (§6).
- *Scratch ablation* (writer source copied per variant into one crate, so absolute numbers differ): `post1k` full
  74–79 ns; without name classification 58–65 ns (classification ≈ 15 ns); without any delta scan 44–46 ns
  (`token_len` + `value_len` ≈ 14–19 ns). The ablation gives the *shares* of the cost; the committed bench gives the
  gate ratio. The remaining ≈ 8–10 ns are the target check, limits, the authority compare, framing, the checked sum
  and `try_reserve_exact`. These shares predate fix round 1 (which added rules, e.g. the `_`-folding name compare).

**One correction to `request-writer.md` §2** (verified at the pin; it lives on branch `p0/writer`, so it is handed to
the Task 5 fix list rather than edited here, §10): curl does **not** cut a custom header at the first newline.
`Curl_add_custom_headers` uses `curlx_str_untilnl` only to decide whether the value is blank (`lib/http.c` L1877)
and then writes the whole original string with `curlx_dyn_addf(req, "%s\r\n", origp)` (L1934), so an embedded CRLF
reaches the wire. (Writer note L77, L94 and L119 say otherwise.) For reference: http 1.5.0 finds standard names
with a `const fn` `match` on byte-string literals after lowercasing into a 64-byte stack buffer
(`src/header/name.rs` L97–104, L1073–1100).

### 3.1 hyper `v1.12.0` + http `v1.5.0`

- **Validation at construction, never at encode.** `HeaderValue::from_bytes` → `try_from_generic`, a branch-free
  fold `bad |= !is_valid(b)` written "so the loop vectorizes" (`src/header/value.rs` L212–228), with `is_valid(b) =
  b >= 32 && b != 127 || b == b'\t'` (L562–565) — arithmetic, not a table — then `Bytes::copy_from_slice`
  (allocates). `from_static` validates in a `const fn` (L62–76). `HeaderName::from_bytes` maps every byte through
  the 256-entry `HEADER_CHARS` table (lowercase or 0) into a `[MaybeUninit<u8>; 64]`, then `StandardHeader::
  from_bytes`; a standard name is an enum (no allocation), a custom one is checked for 0 and allocated
  (`name.rs` L1009–1150).
- **Per request at encode** (`src/proto/h1/role.rs` `Client::encode` L1215–1262): no byte validation, but 2–4 hash
  operations for framing (`content_length_parse_all`, `entry(TRANSFER_ENCODING)`, `set_content_length` inserting
  `HeaderValue::from(len)` — which **allocates a 20-byte `BytesMut` per request** and formats with `itoa`
  (`value.rs` L401–437)); the connection layer additionally runs `connection_any_close` (a lookup plus a `to_str`
  scan) per request (`src/proto/h1/conn.rs` L629, not inside `encode`). `dst.reserve(30 + n × 30)` is a guess (L32, L1226); the
  target goes through `write!(FastWrite(dst), "{} ", uri)` (L1231, with a `TODO … encode without std::fmt`); four
  appends per field (`write_headers` L1629–1636). `HeaderMap` iteration walks the insertion-ordered `entries`
  `Vec`, following `extra_values` only for repeated names (`src/header/map.rs` L2323–2353) — cheap. The cost is
  building a map per request. Output: `WriteStrategy::Queue` → one `writev` of ≤ 64 slices (`io.rs` L59–64,
  L278–316). No SIMD.
- **To be faster than hyper with equal work.** Equal work = per request `HeaderName`/`HeaderValue::from_bytes` for
  every delta field, a `Uri` parse, a new map, then encode: an allocation per field, per map and for
  Content-Length, and a hash per name. The measured 0.38–0.41× (journal) is against hyper's *clone + encode*
  emulation (a pre-built `HeaderMap` cloned per request, then encoded), which validates nothing per request — so it
  is neither EW0 nor EW1; the EW1 row (§3.12) has not been measured yet. Against hyper's **encode-only** emulation
  (pre-built map, no clone, no validation) our *validating* writer is 1.16–1.22×; the equal opponent of encode-only
  is our copy floor (EW0), measured at ≈ 0.4–0.5× of it (journal: floor ≈ 36–40 ns vs encode-only ≈ 75–82 ns, dev).
  To beat encode-only even while validating we must remove the classification and line-search cost (§6), which
  hyper does not pay because each name became an enum once.

### 3.2 Go `go1.27.3` `(*Request).Write`

- **Validation at write time, twice.** `Transport.roundTrip` runs `validateHeaders` (names: `isTokenTable` lookup;
  values: reject `isCTL && !isLWS`; `transport.go` L594–608, `httpguts/httplex.go` L196–311). `(*Request).write`
  (`request.go` L592–784) adds `PunycodeHostPort`, `ValidHostHeader` (Host blanked or error), `RequestURI()` (new
  string), `stringContainsCTLByte` on the target; then `writeSubset` (`header.go` L190–225) checks every name
  **again** and runs `headerNewlineToSpace.Replace` over every value (a second scan; allocates only when it
  replaces). Excluded names (`Host`, `User-Agent`, `Content-Length`, …) are found by an `exclude[k]` map lookup —
  one string hash per header (Go's name classification).
- **Serialisation:** `fmt.Fprintf` for the request line and `Host`; keys **sorted every request** (pooled
  `headerSorter`, L160–181); four `WriteString`s per field; `newTransferWriter` allocates; Content-Length is
  `strconv.FormatInt(n,10)+"\r\n"` (allocates). Output: a 4 KiB `bufio.Writer`, one `Flush` per request (head and
  small body coalesced). Measured earlier: 5 / 10 allocations (`get` / `post1k`).
- **To be faster than Go:** it does more byte work than we do (two passes over names and values) plus `fmt`, a sort
  and 5–10 allocations. We are ≈ 15–20× faster today; rules: never a second pass over values, never allocate per
  request. Equal work for Go = `validateHeaders` + `Request.Write` (the bench must call both, since validation lives
  in `roundTrip`).

### 3.3 fasthttp `v1.77.0` `RequestHeader`

- **Validation at set time, and it repairs instead of rejecting.** `Set`/`Add` → `initHeaderKV`
  (`header.go` L2353, L4062–4083): `removeNewLines` uses two `bytes.IndexByte` and replaces every CR/LF with SP
  (L4134–4160); NUL, DEL and other CTLs are **not** checked; `normalizeHeaderKey` uses `validHeaderFieldByte` only to
  decide whether to canonicalise case (an invalid name is still stored, L4085–4132). Special names are classified at
  set time (switch on `key[0] | 0x20`, then a `toLowerTable` compare, L1982–2045) into dedicated fields;
  `Transfer-Encoding` is dropped. A non-special `Set` scans the existing headers linearly (`args.go` L449–463).
- **Serialisation (corrected after review):** `(*Request).Write` (`http.go` L2058–2067) calls `SetContentLength`
  on **every** write: `strconv.AppendUint` into a reused slice plus a linear `delAllArgs` scan that removes any
  `Transfer-Encoding` (`header.go` L339–343). `SetBody` (`http.go` L1063–1067) formats nothing. When the Host header
  is empty the URI is re-parsed on write (`http.go` L2006–2017). `AppendBytes` (`header.go` L3253–3305) appends the
  stored special fields and arguments. The head is appended into the socket buffer's free space
  (`w.AvailableBuffer()`, then `w.Write(buf)`, L3193–3203), falling back to the `bufV` scratch slice and a copy when
  the head is larger than the free space (L3194–3196). Requests and writers are pooled; zero allocations in steady
  state; a small body leaves in the same `bufio` flush.
- **To be faster than fasthttp with equal work.** Its write path is *not* a pure append: per request it formats the
  Content-Length and scans its argument list for `Transfer-Encoding`, so its EW0 (`req.Write` on a reused request
  with Host and URI preset, §3.12) sits above our copy floor. Equal work (EW1) adds `httpguts.ValidHeaderFieldName/
  Value` before each `Set`. Our budget therefore is: validation ≤ the EW1 checks plus its per-write `SetContentLength`
  and `delAllArgs` scan — at least one pass over every value plus a linear argument scan; a single branch-free pass
  over the delta (§6.3) and one word compare per name (§6.1) fit it with margin (estimate). Our `out` already is the
  buffer that is sent.

### 3.4 undici `v8.12.4` (`writeH1`)

- **Validation at `Request` construction:** path `invalidPathRegex = /[^!-ÿ]/` (`lib/core/request.js` L29,
  L118–126); names are **interned first** — `processHeader` (L472–570) looks the name up in
  `headerNameLowerCasedRecord`, a null-prototype record of every well-known name in original and lowercase spelling
  (`lib/core/constants.js` L105–115), so a known name skips the token check; unknown names use a char table below 12
  characters and `tokenRegExp` above (`lib/core/util.js` L728–748); values one `headerCharRegex` scan (L759–767).
- **Serialisation:** string concatenation `header += …` (`lib/dispatcher/client-h1.js` L1343–1371), a cached
  per-client `host:` line (`kHostHeader`, `client.js` L317), then `cork`, `write(head + "content-length: n\r\n\r\n",
  'latin1')`, `write(body)`, `uncork` (L1536–1564): a latin1 encode plus one `writev`.
- **To be faster than undici:** send bytes, not strings (the P2 layer must hand Rust one latin1 byte region);
  borrow the interning idea — known names as small integer ids that Rust expands from its own table without checks
  (§6.4).

### 3.5 curl `curl-8_23_0`

- **Validation:** none for custom header values (correction above; `lib/http.c` L1808–1942); some names are skipped
  by `curlx_str_casecompare` chains (L1895–1925); each default header probes the custom list linearly with
  `Curl_checkheaders` (`lib/transfer.c` L84–99): O(defaults × custom) per request.
- **Serialisation:** a new `dynbuf` per request (`curlx_dyn_init` L3096, freed L3162) doubling from 32 B
  (`lib/curlx/dynbuf.c` L29, L67–113; ≈ 5 reallocations for a 400 B head); ~20 `http_add_hd` slots, several and all
  custom headers through `curlx_dyn_addf` (curl's printf, `dynbuf.c` L187–215); integers one digit at a time
  (`lib/mprintf.c` L896–900). With a body, the head is copied into the 64 KiB `sendbuf` together with body bytes
  (`lib/request.c` L380–425). The proxy h1 serialiser is `Curl_h1_req_write_head` (`lib/http1.c` L322–345) +
  `Curl_dynhds_h1_dprint` (`lib/dynhds.c` L352–375).
- **To be faster than curl:** it loses on allocations, printf and O(k·n) probing while validating less; nothing to
  copy beyond coalescing head and body (which we do).

### 3.6 h2o client (`lib/common/http1client.c` `build_request` L747–837) and server (`lib/http1.c`)

- **Client:** no validation; names are interned tokens with class flags (`include/h2o/token.h` L32–48), so
  classification is a pointer compare or a bit test; buffer from the request's pool, sized method + path +
  authority + 512, doubled by `RESERVE` before each header (L753–767); `h2o_memcpy` plus single-byte stores for
  `": "` and CRLF, no per-append capacity check (L774–783); `writev` of head and body (L839–895).
- **Server, the two-pass pattern:** `flatten_headers_estimate_size` sums name + value + 4 per header plus fixed
  bounds (L944–954), one pool allocation (L1090–1092), `flatten_headers` copies unchecked (L956–999). Token lookup:
  switch on length, then the **last byte**, then `memcmp` (`token_table.h` L273+).
- **To be faster than h2o:** without validation its copy equals our floor. Take the tokens (§6.4) and the
  last-byte discriminator (at length 10, `connection`/`keep-alive`/`user-agent` end in `n`/`e`/`t`).

### 3.7 Boost.Beast `boost-1.92.0` serializer

- **Storage:** one allocation per field holding the pre-formatted `name: value\r\n` line
  (`http/impl/fields.hpp` L291–305, L968–994); only size checks and a trim at insertion, **no CR/LF or CTL check**
  (L974–985); `set` is `string_to_field` + an intrusive multiset `lower_bound` with `iequals` + an insertion-order
  list (L584–599): O(log n) and one allocation per field. Content-Length via `raw_to_string`, one digit per
  iteration (L934–945).
- **Serialisation:** the head is never copied — `basic_fields::writer` (L32–222) is a `buffers_cat_view` of method,
  target, `" HTTP/1.1\r\n"` and one buffer per field; the serializer appends the body buffers
  (`http/impl/serializer.hpp` L86–106): a `writev` with one iovec per field. Chunk headers use `std::make_shared`
  per chunk (`chunk_encode.hpp` L68–117).
- **To be faster than Beast:** with equal work (build the message per request, then `buffer_copy` into a flat
  buffer) its per-field allocations dominate; on a socket, one contiguous write should beat a 10–20-iovec `writev`
  for small heads (unverified beyond 2 iovecs, `request-writer.md` §8.3). Our template is "format once" without the
  per-field allocations and iovecs.

### 3.8 nginx `release-1.31.8` `ngx_http_proxy_create_request`

- **Compiled template, built once at config time:** `ngx_http_proxy_init_headers` (L4186+) compiles
  `proxy_set_header` and the defaults (L761–772) into two bytecode arrays — `lengths` codes and `values` copy codes
  with literal bytes inline — and a static hash of the names it sets.
- **Two-pass write** (L1184–1600): pass 1 sums every length (request line, escapes, Host, script codes, incoming
  headers not found by `ngx_hash_find` — classification is one probe because the request parser hashed the name
  while scanning it); one exact `ngx_create_temp_buf(len)` (L1370); pass 2 `ngx_cpymem` (a byte loop below 17 bytes
  on Intel, `ngx_string.h` L97–140) plus single-byte stores; the body is chained, not copied, and sent with
  `writev`. No write-time validation (inputs were validated by its parser).
- **To be faster than nginx:** we already use its pattern (sum, `try_reserve_exact`, appends); copy the
  classification-during-scan idea (§6.1).

### 3.9 cpp-httplib `v0.65.0` (commit `7e5d0683ac326788e52ce3c44585c1af24b04532`, resolved 2026-11-08)

- **Validates twice:** `set_header` keeps a field only if `is_field_valid` (`httplib.h` L12136–12139,
  L8513–8516); `check_and_write_headers` checks every field again (L13305–13318); `write_request_line` checks the
  method (`is_token`) and target (`is_request_target`) (L8980–8992); the checks are branchy per-character loops
  (L10620–10666).
- **Serialisation:** headers in an `insertion_ordered_multimap` of `std::string` pairs; Content-Length via
  `std::to_string` (L16725–16740); a temporary string for the request line and **per field** (L8986–9020) into a
  `BufferStream`; **head and body in two sends** for a small POST (L16769–16773, L16855–16860).
- **To be faster than cpp-httplib:** heavy allocation and copying; keep head and body in one write end to end.

### 3.10 Zig `0.17.1` `std.http.Client` `Request.sendHead`

- **Validation:** only in `request()` and only with `runtime_safety` (Debug/ReleaseSafe): names non-empty without
  `:`, neither name nor value containing the two-byte `"\r\n"` — a lone CR or LF passes (`lib/std/http/Client.zig`
  L1699–1711). **ReleaseFast checks nothing.**
- **Serialisation** (L991–1090): `writeAll` per piece into a buffered `std.Io.Writer` (default 1024 B, L53,
  L257–264); `writeVecAll` of 3 slices for overridable defaults (L1289–1299); Content-Length via `w.print("{d}")`.
  `post1k` overflows the 1 KiB buffer (more than one drain; unverified).
- **To be faster than Zig:** its CPU path is short because it checks nothing; with equal work (a Go-level validator
  loop added) we need validation at about one vectorised pass; we also win on syscalls for `post1k`.

### 3.11 Netty `netty-4.2.20.Final` `HttpRequestEncoder`

- **Validation at set time, on by default:** `validateToken` for names, `validateValidHeaderValue` for values
  (first byte ≥ 0x21 and not DEL, then `b < 0x20 && b != 0x09 || b == 0x7F` fails;
  `DefaultHttpHeadersFactory.java` L30–74, `HttpHeaderValidationUtil.java` L104–139); `isConnectionHeader` is the
  same length switch + `contentEqualsIgnoreCase` as our classifier (L50–74). Encode does not validate again.
- **Serialisation:** an EWMA of past head sizes (start 256, weight 1/5, ×4/3) sizes one pooled buffer
  (`HttpObjectEncoder.java` L66–83, L329–345, L635–637); small content (≤ max(128, estimate/8)) fused into the head
  buffer (L58, L320–333); per field one `ensureWritable(name + value + 4)`, then absolute sets, `": "` and CRLF as
  16-bit stores (`HttpHeadersEncoder.java` L32–46); request line `HttpRequestEncoder.java` L39–78.
- **To be faster than Netty:** its encode is close to our floor; equal work = `DefaultHttpHeaders` with validation
  plus encode; our classifier must cost less per candidate than `contentEqualsIgnoreCase` (§6.1).

### 3.12 Equal work for writers (definitions)

| Tier | Work per request | Ours |
|---|---|---|
| **EW0** serialise only | request line, headers, framing, one buffer; every input object pre-built and reused | the copy floor |
| **EW1** Go-level checks | EW0 + token names, field-content values (reject CTLs except HTAB, and DEL), target CTL/SP check on every per-request field; inputs built per request from bytes | — |
| **EW2** ours | EW1 + forbidden names, singleton names, line count, authority binding, reject (never repair) | `write_request` |

**One definition per rival** (the timed call is in §3.13):

| Rival | EW0 | EW1 lift (their own code, run per request) | EW2 lift |
|---|---|---|---|
| hyper 1.12 (+ reqwest, which serialises through hyper) | `Client::encode` of a pre-built `HeaderMap`. `encode` empties the map (`msg.head.headers.clear()`, `role.rs` L1260) and inserts Content-Length, so a reused map would be empty after the first op: the harness pre-clones a batch of maps **outside** the timed region (criterion `iter_batched`-style) and times only `encode` over them; the clone itself is reported as its own row | `HeaderName::from_bytes` + `HeaderValue::from_bytes` per delta field, `Uri` parse, new map, `encode` | + `contains_key` per forbidden name |
| ureq 3.5 (`ureq-proto` 0.7.1) | `Flow` prelude write of a pre-built `http::Request` | `http::Request` built from bytes (`HeaderName`/`HeaderValue::from_bytes`) + prelude write | + forbidden-name lookups |
| Go net/http | none (its checks cannot be skipped) | native: `validateHeaders` (copied from `roundTrip`) + `req.Write` | + `exclude`-style map lookups |
| fasthttp | `req.Write` on a reused request with Host and URI preset (still formats Content-Length and scans for TE per write, §3.3) | + `httpguts.ValidHeaderFieldName/Value` before each `Set` | + first-byte switch it already has |
| libcurl (+ isahc, which drives libcurl) | origin head assembly (`Curl_http`) with defaults only | + a Go-equivalent byte loop + tchar table (curl validates nothing) | + name compares |
| h2o client | `build_request` | + Go-equivalent loop | + token flags it already has |
| Boost.Beast | `serializer` over a pre-built message, `buffer_copy` | message built per request + Go-equivalent loop (Beast checks no CR/LF) | + name compares |
| cpp-httplib | not separable (validates in `set_header` and again on write) | native | + name compares |
| Zig std.http (ReleaseFast) | `sendHead` port | + Go-equivalent loop | + name compares |
| Netty | `HttpRequestEncoder.encode` of a pre-built request with non-validating headers | default validating `DefaultHttpHeaders` built per request + `encode` | + `isConnectionHeader` it already has |
| Jetty 12.1 | `HttpGenerator.generateRequest` of pre-built `MetaData.Request` (it *repairs* CTLs to SP while copying, `putSanitisedValue`) | validating `HttpFields` built per request + `generateRequest` | + name compares |
| undici | advisory only in 5c (§3.13) | — | — |

Report three ratios per rival: **ours@EW2 / theirs@EW1** (we do more and must still win — the headline),
ours@EW2 / theirs@EW2, and ours@EW0 (copy floor) / theirs@EW0. Allocations per request are reported beside every
number. A rival's repair (fasthttp, Go's newline-to-space, Jetty's sanitising) is recorded in the injection table,
not lifted.

### 3.13 Task 5c — cross-language writer race

**Goal:** show, per rival, that `write_request` is faster with equal work (§3.12), on arm64 and x64, as
`bench/parsers` does for parsers. It replaces the ad-hoc `bench/writer/` comparisons (`hyper-emul/`, `go/`) with one
harness.

**Rivals** (pinned at the latest release in §1, re-baselined at P1b and each release candidate, design §16.2). Every
G0 engine (design §2, incl. the Java and .NET engines added on 2026-11-08) has a row or a stated mapping:

| Rival | Timed API (in memory, reused sink, no socket) | Runnable as specified? |
|---|---|---|
| hyper v1.12.0 | `role::Client::encode` is `pub(crate)`, so the harness vendors hyper at the pin with **one added `#[doc(hidden)] pub fn` that calls `Client::encode` unchanged** (the patch is a recorded diff in the bench folder; no other line changes). Sink: a reused `Vec`. **Check mode:** the bytes must equal real hyper's, captured untimed through `client::conn::http1::handshake` over an in-memory duplex with a canned response. A separate **end-to-end** row (send + canned response through the duplex) is reported as advisory: hyper writes the next head only after reading a response (`proto/h1/dispatch.rs` L356–363, L709–724), so a capture-only duplex cannot time it. Fallback if the patch does not build: `bench/writer/hyper-emul` with the same byte-equality check | yes |
| reqwest | serialises through hyper's encoder — mapped to the hyper row (end-to-end in G0) | mapping |
| ureq (`ureq-proto` 0.7.1) | the client `Flow` prelude write into a caller buffer (`src/client/sendreq.rs` `try_write_prelude`, `write!` per line) | yes |
| Go net/http | `validateHeaders` (copied, it lives in `roundTrip`) + `req.Write(bw)` on a reused 8 KiB `bufio.Writer` over `io.Discard`, `Flush` per op | yes |
| fasthttp | pooled `Request`, `Reset`, then `SetNoDefaultContentType(true)` again (`Reset` clears it), `SetRequestURI`, `SetMethod`, `Set` × n, `SetBody`, `req.Write(bw)`. The URI is re-encoded on write when it was parsed; that is part of fasthttp's per-request work and is timed | yes |
| libcurl | origin heads are built in `Curl_http` (`lib/http.c` ≈ L3082–3162) with `Curl_add_custom_headers`; timing it alone needs an easy handle and a connection with a capturing send filter, built against curl's internal headers in a test build (curl's `tests/unit` harness style). **Time-boxed to one day in 5c**; if it cannot be made to run, the curl row is end-to-end only (the G0 race, libcurl multi), with the reason recorded. `Curl_h1_req_write_head` is the CONNECT-proxy serialiser (`lib/cf-h1-proxy.c` L259 is its only caller) and is not timed | yes, once runnable (then gated like every other row; curl's per-request auth and host setup in `Curl_http` are labelled in the report) |
| isahc | drives libcurl — mapped to the curl row | mapping |
| h2o client | `build_request` (static in `lib/common/http1client.c`: the harness compiles a vendored copy at the pin) with a pool cleared per op | yes |
| Boost.Beast | `request<string_body>` (pre-built for EW0, built per op for EW1) + `serializer` + `buffer_copy` into a reused `flat_buffer` | yes |
| cpp-httplib | `Request` + `set_header` × n + `detail::write_request_line` + `detail::check_and_write_headers` + body append into `BufferStream` | yes |
| Zig std.http | port of `sendHead` into `Writer.fixed`, `ReleaseFast` | yes |
| Netty | a harness subclass of `HttpRequestEncoder` calls the protected `encode(ctx, msg, out)` with a stub `ChannelHandlerContext` (the request's content is `retain()`ed per op, since the encoder releases it) whose `alloc()` is the pooled allocator — no `EmbeddedChannel` pipeline in the timed region; JMH | yes |
| Jetty | `HttpGenerator.generateRequest(info, header, null, content, true)` (public, `jetty-http`) into a reused `ByteBuffer`; JMH | yes |
| `java.net.http` | its writer (`jdk.internal.net.http.Http1Request`) is internal and needs an exchange object → **end-to-end only** (G0) | mapping |
| .NET `SocketsHttpHandler` | its writer (`HttpConnection`) is internal → **end-to-end only** (G0) | mapping |
| undici | **advisory in 5c**: undici's timed path starts from a JS headers object (`new Request` + string concatenation + latin1 write) while ours starts from a delta already serialised by our (not yet written) P2 layer. The equal-work comparison — the same JS headers object through to bytes — moves to G1-JS once P2 exists | advisory |

**Forced default fields** (the gate compares field multisets, so every rival must emit the same set):

| Rival | Field it adds on its own | Handling |
|---|---|---|
| undici | `connection: keep-alive` (`client-h1.js` L1351–1357) | advisory row; byte difference reported |
| Go | `User-Agent: Go-http-client/1.1` | set `User-Agent` explicitly to the template's value |
| fasthttp | default `Content-Type` on requests with a body (`header.go` L3270–3276) and its `User-Agent` | `SetNoDefaultContentType(true)`; set `User-Agent` |
| Zig | default `user-agent`, `accept-encoding`, `connection` | set each to `.omit` |
| libcurl | `Accept: */*` (unverified at the pin) | custom header `Accept:` to remove it |
| cpp-httplib | `Accept`, `User-Agent` (unverified at the pin) | override with the template's values |
| hyper, ureq, h2o, Beast, Netty, Jetty | none found in the paths read | the check-mode dump confirms |

Fields that cannot be turned off are added to **our** template for that row, so both sides emit the same bytes.

**Harness shape — reuse `bench/parsers`:** one image per language (Rust, C/C++, Go, Zig, Node, Java), base images by
digest, rival sources by SHA-256 and tag commit, `docker run --cpus=1 --cpuset-cpus=<n> --memory=2g --memory-swap=2g
--network=none`, the runner refuses to start while another container runs, the same CPU baseline everywhere
(generic Armv8-A; on x64 every row, ours and every rival, is built for `x86-64-v3`, as in §5.7), release flags per
design §16.2 (Java: JMH after warm-up), row order shuffled per round from a recorded seed (design §16.4), JSONL with
provenance
(`bench/results/<date>-<arch>-dev-writers.jsonl`), and `check`/`time` modes with the same protocol (`harness
<writer> <mode> <workload> [run ms]`).

**Differential gate before timing:** in `check` mode every harness prints, per request, the bytes it produced; the
runner parses each output with **httparse's `Request` parser as the oracle** (as the writer note's fuzz target does)
and requires: the same method, target, Content-Length and body; the same field multiset after the forced-field
handling above (names compared case-insensitively — Go and fasthttp canonicalise case — values byte-exact); no
`transfer-encoding`; exactly one `content-length` where the writer note's §6.5 requires one. A second gate feeds the
**injection vectors** (`tests/vectors/h1-request/`) through each rival's EW1 path and records reject / repair /
pass — a table, never timed.

**Workloads** (exported once as JSON by `crates/engine-core/examples/h1_write_bench.rs`'s generator so every
language loads identical requests): `get` (`GET /data`, template only), `post1k` (`POST /data`, defaults +
`content-type: application/json` + `x-request-id` (36-character UUID) + `authorization: Bearer` + 40 characters,
1 KiB JSON body), `varied` (the 256-request ring, fixed xorshift seed), plus `h20` (20 per-request fields, 400–800 B
of headers — where validation dominates). Per-request inputs are fresh each op (cycled from the ring), so
construction-time validators run per request.

**Metrics and statistic:** ns per request (median, min–max over rounds), ratios to our copy floor and to ours,
allocations per op (Go `-benchmem`, JMH `gc.alloc.rate.norm`, Rust counting allocator, C `--wrap=malloc`).
**Gate (5c):** for every rival row marked "yes" and every workload, ours@EW2 / theirs@EW1 passes the design §16.4
(component micro-gates) statistic, on the arm64 reference host (nightly window) **and** the x64 VM; `post1k` ≤ 2.3×
floor. The statistic is computed by `pairedRatioCi` in `bench/parsers/stats.ts`, added by Task 4c (the dev runner
computes only medians today, its README says so) and reused here, never re-implemented. Task 5c evaluates "every
rival@EW1 beaten" for Task 5b's W-A.

## 4. Papers and algorithms

The SIMD note (§3) already covers the nibble LUT (Muła, simdjson §3.1.1), NEON movemask emulation, `pcmpestri` cost,
SWAR, two-stage bitmaps (simdjson stage 1, Cloudflare 2014) and Keiser–Lemire UTF-8. This section adds what changes
our design. Status per source in §9.

### 4.1 Parabix bit streams: ScanThru and MatchStar — the carry primitive for HTTP lines

- **Source (verified, primary text):** Cameron, Shermer, Shriraman, Herdy, Lin, Hull, Lin, *Bitwise Data
  Parallelism in Regular Expression Matching*, PACT 2014, pp. 139–150, doi:10.1145/2628071.2628079. Text is represented as one
  bit stream per character class ("parallel bit streams"); markers advance through a class with an **addition**,
  whose carries move every marker through its run at once: `MatchStar(M, C) = (((M ∧ C) + C) ⊕ C) ∨ M` (§3), which
  "differs from ScanThru of the Parabix tool chain in that it finds all matches, not just the longest". Carries
  between 64-bit words are kept per addition by the Pablo compiler; the paper adds long-stream addition for blocks
  ≥ 256 bits. Reported: 0.95, 1.22 and 1.6 CPU cycles per byte for its test expressions on SSE2, and 0.63 on AVX2
  for the simplest expression (the 0–4 axis beside the results is instructions per cycle).
- **ScanThru** (the longest-match form we need; source: Cameron et al., *Parallel Scanning with Bitstream Addition:
  An XML Case Study*, Euro-Par 2011, pp. 2–13, cited by the PACT paper, which names ScanThru but does not define it;
  the Euro-Par text was not fetched): `ScanThru(M, C) = (M + C) ∧ ¬C` moves each marker in `M` to the
  first byte after the run of class `C` that starts at it. For a field line: markers at line starts, `C` = tchar →
  lands on the byte after the name, which must be `:`; markers after the colon, `C` = field-content → lands on the
  value's end, which must be CR, followed by LF. **All lines of a 64-byte block are validated with two additions and
  a handful of AND/OR/XOR, with no per-line branch.**
- **Checked for HTTP by an executable model (runs 2026-11-08 and 2026-11-09; what it does not test: "Coverage
  limits" below):** `tools/h1-research/scanthru_model.py` implements
  exactly the P-D pseudocode of §5.5 (six carry bits including the pending empty-line CR, the head-end limit
  `bits ≤ h + 1`, flatten inputs masked by the limit and the received length) and the W-D writer form of §6.5, and
  checks them against scalar references that emit **spans**:
  - `timeout 300 python3 -I tools/h1-research/scanthru_model.py` (seed 9112, one single-process run after the
    resource preflight) printed `parser {'more': 16132, 'ok': 66481, 'err': 17387}`,
    `parser regressions + boundary sweep + field limits 39`, `trickle bytes classified by P-D 0`,
    `writer accepted 23730 of 100000; W-B hint checks 23730` (≈ 16 s, run 2026-11-09).
  - Field limit (review R2-2): heads with 97–102 fields (8-byte lines, and 16-byte lines that put the 101st line at
    bit 0 of a block), whole and split, equal the reference (the 101st field is
    Malformed at its first byte); the model asserts that no span stream ever exceeds the 100 slots.
  - Trickle: a 4 KB line plus 40 fields fed one byte per call; P-D classifies 0 bytes because the scalar parser owns
    every partial line (linear by construction; §5.5).
  - Writer: W-D and W-B (§6.3) against the W-A reference **including forbidden and singleton names** (review M2-1);
    W-B's hint corruption and the review's tiled smuggle (R2-1) are rejected.
  - Parser: 100,000 mutated heads, each parsed whole and split at 0–3 random points through the line-start hand-off
    between P-D and the scalar parser (§5.5); result, head end, **every span** and the exact first-error offset must
    equal the reference. Plus the three review counterexamples (empty-line CR at bit 63 followed by a body line, a
    bare CR after the last CRLF, a body line after the head end) and a boundary sweep (paddings 0–139 × five tails ×
    **every single split point**).
  - Writer: 100,000 mutated deltas with declared line counts off by −1/0/+1, plus the smuggling and unterminated
    deltas and empty names (§6.5).
  - Scope: the model does not cover the 64 KiB head limit or the status line (both unchanged scalar code). The
    first version of the model (an earlier survey script) checked only accept/reject and error offsets in one call;
    it is superseded.
  - **Coverage limits (mutation pass on the model, review docs-review-1 F-04, 2026-11-13; 30 mutants):** the
    model's checks genuinely test the core error equations, the carries and the line-ownership rule (21 mutants
    killed). They do **not** test: W-B's tiling end `pos == block.len()` and W-B's CRLF check (both survive, so the
    tiled-smuggle rejection above rests on other checks); that the P-D fast path is ever taken on valid input (a P-D
    that always re-runs the scalar parser passes); the 2-bit colon carry of W-D at bits 61–63 (no deterministic
    boundary sweep; killed in 2 of 11 seeds); `& received` against non-zero padding (the model zero-pads). Its
    "error offsets equal the reference" is not independent evidence: on error the same scalar function is re-run.
    These cases are **required tests of the Rust implementation**, not additions to the model, which is deleted in
    P1a (issue #24: port the bit-stream model to `engine-core` tests and remove `tools/h1-research`): P-D and W-D in
    P1a (plus "fast path taken, zero scalar fallbacks on valid heads ≥ 64 B" and the §5.5 four-cursor regression
    input), W-B in P2.
- **Cost:** ≈ 32 `u64` operations per 64-byte block on top of the class bitmaps (P-D's validator reads five: LF, CR,
  colon, T, V; `BlockMasks` has seven, adding `sp` for W-D and `ows` for the trims; the SIMD note's prototype built
  two); 6 carry bits between blocks (parser form).
- **Rust fit:** the bit-stream arithmetic is plain `u64` code: `u64::carrying_add` is stable since 1.91.0 (const:
  unstable, so not in a `const fn`) and
  compiles on 1.99.1 under `#![forbid(unsafe_code)]` (compile-only check, `tools/h1-research/asmcheck`, function
  `scan_thru`; [std docs](https://doc.rust-lang.org/1.99.1/std/primitive.u64.html#method.carrying_add)). Only the
  **classification** (bytes → bitmaps) needs SIMD: rustc 1.99.1 does not auto-vectorise a safe 64-byte
  compare-to-bitmask loop (§5.0). So: **classification in `engine-simd`, validation and span extraction in safe
  `engine-core`.**
- **Resumability:** every check is causal (a bit depends only on bytes at or before it, plus the one-byte CR→LF
  lookahead carried as a bit). §5.5 uses this for a hand-off at line starts rather than a carry checkpoint, so that
  every line is committed exactly once.

### 4.2 simdjson stage 1 in depth (verified, source at `v5.1.1`)

- Strings: `prefix_xor` turns quote bits into "inside a string" regions — by carry-less multiply on x64
  (`_mm_clmulepi64_si128`, `include/simdjson/haswell/bitmask.h` L18–22) and by a shift-XOR cascade on arm64 (the
  `vmull_p64` form is commented out as slower, `include/simdjson/arm64/bitmask.h` L17–30). **HTTP has no quoting in
  the head grammar we validate** (quoted strings only appear inside opaque values), so prefix-XOR is not needed —
  ScanThru is the right primitive (the line structure is "run of class X, then a delimiter").
- Index extraction (`src/generic/stage1/json_structural_indexer.h` `bit_indexer::write`, L93–122): returns early on
  an empty mask (a branch the comment calls sometimes mispredicted, sometimes helpful), takes the popcount, writes
  indexes in unrolled steps of 4 up to 24 (`write_indexes_stepped`, `tzcnt` + clear-lowest-bit), and loops for the
  rest only when the popcount exceeds 24 (`simdjson_unlikely`). For HTTP: four bitmaps with one bit per line (name
  start, colon, value start, value end) → four index arrays → spans, with no dependent chain from one span to the
  next; with ≤ 16 lines per 64-byte block (the minimum line `a:\r\n` is 4 bytes) one step-of-4 unroll to 16 covers a
  block.
- Padding: stage 1 reads whole blocks; inputs are padded (SIMD note §3.6).

### 4.3 "Validate fast, locate on error" (simdutf, verified at `v9.3.3`)

`generic_validate_ascii_with_errors` (`src/generic/ascii_validation.h` L6–28) checks whole blocks with a cheap
any-bad test and only on failure calls the scalar `validate_with_errors` to find the exact position. For us: the
SIMD/bitmap path only has to be exact about **acceptance**; on any anomaly in a block it hands the block to the scalar
parser, which produces the error (and its class: `Malformed`, `SmugglingRisk`, `TooLarge`) exactly as today. Errors
are rare, so the cost is irrelevant, and the scalar parser stays the single source of error semantics — the
differential risk (SIMD note §3.9) shrinks to "the fast path accepts only what the scalar path accepts".

### 4.4 Data-parallel FSMs and SHENG — rejected for the head

- Mytkowicz, Musuvathi, Schulte, *Data-Parallel Finite-State Machines*, ASPLOS 2014 (primary text read): run the
  FSM from all states at once with a byte shuffle as a table gather, relying on state convergence. PACT 2014 (§4.1)
  summarises it as "one or two shuffles per input symbol" and notes it still consumes one byte at a time.
- Hyperscan/Vectorscan SHENG (`src/nfa/sheng_impl.h` at `vectorscan/5.4.13`, L55–94): a ≤ 16-state DFA whose
  transition is `cur_state = pshufb(shuffle_mask[c], cur_state)` per byte — a **dependent** shuffle chain, ≈ 1–2
  cycles per byte of latency.
- Our scalar parser already runs ≈ 1 cycle per byte; P-D's speed is unknown until prototyped (§5.5). Neither FSM
  form wins for a grammar this regular. (Teddy/shufti/truffle match literal sets and byte classes — the
  classification side, already covered by the nibble LUT.)

### 4.5 SIMD CSV and line scanning

- simdcsv (Langdale, README at the pin, verified): simdjson's quote handling plus comma/CR-LF bitmaps over padded,
  cache-line-aligned buffers; "the only new wrinkle … is the fact that we have to detect a CR followed by a LF" —
  the same `CR ∧ (LF ≫ 1)` pairing we need, with the cross-block bit. No HTTP-specific idea beyond that.
- Numbers stay scalar: Content-Length and chunk sizes are 1–7 digits in practice; SWAR 8-digit parsing (Muła, Lemire)
  and a hex table for chunk sizes are enough (ideas.md).

### 4.6 Prior art on SIMD HTTP parsing (none peer-reviewed)

No peer-reviewed paper on SIMD HTTP/1 parsing or serialisation was found (survey of arXiv, ACM DL, USENIX, IEEE,
2026-11-08). Industrial work: Cloudflare's 2014 picohttpparser AVX2 bitmaps (SIMD note); QUICHE Balsa (Envoy's
HTTP/1 parser) is not vectorised (survey statement, unverified); Bun's client uses picohttpparser; uWS uses SWAR + a
fence. Open-source projects found by search and **not read** (unverified, listed in `ideas.md` for a later look):
[Kourier](https://kourier-server.github.io) (C++ server, "uses SIMD instructions extensively" in a strict parser),
[hparse](https://github.com/floatdrop/hparse) (Zig, SIMD, streaming), [xocomil](https://docs.rs/crate/xocomil/0.3.0)
(Rust request parser and response writer; SIMD use not stated) and [simdzone](https://github.com/NLnetLabs/simdzone)
(DNS zone files, simdjson-style). Two names reported by the papers survey (cursed-http, simdhttp) could not be found
by search and are dropped.

## 5. Our parser: candidates

### 5.0 Where the time goes today (from measured data; no new runs)

- **The split is a second pass.** `blazehttp+fields` costs 1.5–2.1× plain `blazehttp` (plain / with fields =
  0.48–0.65 in the quiet re-run, §0.1): the bench's `split_fields` re-reads every field byte (`memchr` CR,
  `position` `:`, OWS trim; `bench/parsers/rust/src/engines.rs` L78–103) although `value()` already held all four
  boundaries (§2.14).
- **Projection once spans are fused** (estimate, unmeasured): plain ratios × 1.075–1.10, i.e. ≈ 3–4 cycles per
  field of the ≈ 40 below for the four span stores plus the two OWS checks (one compare each in the common case) —
  the same figure §5.3 uses **[author's call: one span-cost figure for both sections]**. From §0.1: vs httparse
  0.86–0.88 / **1.00–1.02** / 0.85–0.87 / **1.01–1.03** / **1.00–1.02**; vs picohttpparser 0.73–0.88; vs llhttp
  0.53–0.63; vs Beast `basic_parser` ≈ 0.48–0.51. **P-A wins against everything except httparse, to which it is
  projected to lose by 0–3% on the three 20-field cells** (break-even for the 0.94 cell is × 1.064). This also
  assumes caller-owned spans (§5.1); a parser-owned 800-byte array re-initialised by `reset` on every head would cost
  more.
- **Where a 20-field head's time goes in plain G** (arm64; plain `1k-h20` ≈ 227–250 ns for 20 fields, journal
  2026-11-07, ≈ 40 cycles per field at ≈ 3.5 GHz; the split is an estimate from reading the code, not a profile):
  three phase-loop trips per field, each returning a large `Result<Option<ParseStatus>>` and writing
  `self.phase/pos/line_start/colon` (≈ 10–15 cycles); the name as a byte loop with one table load and branch per
  byte (≈ 14 cycles for 14 bytes); the value as 2–3 SWAR words (≈ 6–8); CRLF checks, field count and the facts
  length switch (≈ 5); ≈ 8 `checked_add`/`ok_or` branches (≈ 2–4). §5.3 attacks the first, second and last items.
  The first step of Task 4c measures this split (a profile of plain G on `1k-h20`: `perf stat`/sampling on the x64
  VM, Instruments on arm64); until then every per-item figure here is a guess.
- **Safe-Rust bitmaps do not pay** (compile-only check, run 2026-11-08: `tools/h1-research/asmcheck`, rustc 1.99.1,
  fat LTO, `codegen-units = 1`, commands in its `Cargo.toml`, targets `aarch64-apple-darwin -C target-cpu=generic`
  and `x86_64-unknown-linux-gnu` baseline and `x86-64-v3`): a 64-byte `fold(|m, (i, &b)| m | (u64::from(b == c) <<
  i))` stays a scalar byte loop (`ldrb`, `cmp`, `cset`, `lsl`, `orr` per byte) on every target; a 16-byte-chunk form
  is unrolled scalar; an exact SWAR equality mask with a multiply gather costs 76 (aarch64) / 110 (x86_64)
  instructions per class per 64 B, so five classes cost ≈ 6–9 instructions per byte — several times today's cost.
  **Bitmaps need explicit SIMD (`engine-simd`).** A branch-free `fold` with table lookups compiles to a tight loop
  with no data-dependent branch (`ldrb`, `ldrb [table]`, `and`, `subs`, `b.ne`).

### 5.1 The span output (shared by every candidate) **[author's call]**

```rust
/// One field line as offsets from the start of its head (u16: a head is ≤ 64 KiB, design §5.2).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct FieldSpan { pub name_start: u16, pub name_end: u16, pub value_start: u16, pub value_end: u16 }
/// Caller-owned span storage, sized to the field limit; created once per connection, never re-initialised.
pub struct FieldSpans([FieldSpan; H1_LIMITS.max_header_fields]);
const _: () = assert!(H1_LIMITS.max_head_bytes <= u16::MAX as usize + 1);
// ResponseParser::parse(&mut self, buf: &[u8], spans: &mut FieldSpans) -> Result<ParseStatus, ErrorCode>
```
- **Caller-owned (httparse's shape)**, so `new`/`reset` stay as cheap as today and no 800-byte array is re-initialised
  per head; the connection owns one `FieldSpans` for its lifetime. Contract: the same `spans` is passed on every call
  of one exchange (like the same `buf`); passing another is the caller's bug, documented like the `buf` prefix rule.
  `ResponseHead` gains `fields: u16` (the count); spans `[0, fields)` are valid after `Complete`.
- Spans are written only when a line's CRLF is confirmed, so a `Partial` never exposes half a line and a resumed call
  continues writing at `fields`. After an `Interim` the count restarts at 0 for the next head (offsets are relative to
  each head's own start, unchanged contract).
- `value_start` skips leading OWS; `value_end` trims trailing OWS backwards from the CR (SP/HTAB only; obs-text never
  trimmed) — the httparse/pico output, and the bench's `split_fields`.
- **Our own idea:** 4 × `u16` per field can cross the napi boundary as a `Uint16Array`, so the JS layer builds
  `headers` without scanning the bytes again (P2 hand-off; unmeasured).
- **Test plan (every candidate):** span equality, whole vs **every split point**, for every vector of
  `tests/vectors/h1-response/` and the fixtures (extends `tests/h1_response.rs`'s split-invariance test, which today
  compares only `Outcome`); `fields` reset after an `Interim`; OWS edges (SP, HTAB, empty value, value of only OWS,
  obs-text at both ends never trimmed); the bench's differential gate compares our spans with every rival's fields.

### 5.2 Candidate P-A — "spans in G" (minimal change)

Keep Design G (parser note §6.3) unchanged; in `value()` at the confirmed CRLF, write the span (four stores + the
OWS trims) and increment `fields`.

- **Pseudocode:** `Value: e = pos + value_len(rest); [CR, LF, ..] at e → vs = skip_ows(colon+1, e); ve =
  trim_ows_back(vs, e); spans[fields-1] = (line_start, colon, vs, ve); facts.field(...)`.
- **Time:** O(head bytes); per field + 4 stores + two OWS checks (usually 1 byte each). **Space:** the caller's 800 B.
  **Branches per byte:** unchanged ≈ 0.6.
- **Expected position (estimate, unmeasured):** arm64 — §5.0 (wins except httparse on 20-field cells, a projected
  0–3% loss). x64 (every row built for `x86-64-v3`, §5.7) — vs httparse ≈ 0.85–1.0 (its x86 names are a table loop
  like ours, values AVX2); vs picohttpparser ≈ 0.9–1.1 (two to three `pcmpestri` per field); vs llhttp ≈ 0.5–0.7.
- **Risks:** none new for security (spans come from bytes already validated); partial input unaffected. **Not a win
  on its own** against httparse on 20-field heads (projected 1.00–1.03).

### 5.3 Candidate P-B — "line kernel" (scalar, safe Rust; our own design)

The references' common structure (§2.14: one loop body per line, cursor in locals) plus two ideas of ours:
decoupling **locate** (the critical path) from **validate** (off the critical path), and a **stop-set** search for
names. Resumable mid-line exactly like G (phase, scanned-to offset and colon persist), so every byte is examined O(1)
times however the input is split.

```text
state (persisted only on exit): phase ∈ {LineStart, Name, Value}, line_start, pos (scanned to), colon, fields, facts
fn scan_lines(window, st) -> Outcome {           // locals in the loop; written back to `self` only when returning
  loop { match st.phase {
    LineStart => match window[st.pos..] {
        [CR, LF, ..] => return HeadEnd(st.pos + 2),
        [] | [CR]    => return More,
        [CR, ..]     => return Err(Malformed),
        _ => { st.fields += 1; if st.fields > MAX_FIELDS { return Err(Malformed) }      // at the line's first byte, as G
               st.line_start = st.pos; st.phase = Name } },
    Name => {
        // locate: first byte in STOP = {≤ 0x20, ≥ 0x7F, ':'} (SWAR, lowest flag exact); resumes at st.pos
        let n = stop_len(&window[st.pos..]);
        // validate the newly scanned name bytes BEFORE any return: branch-free tchar fold, one branch per call
        if !all_tchar(&window[st.pos .. st.pos + n]) { return Err(Malformed) }       // catches ( ) , / ; < = > ? @ [ \ ] { } "
        st.pos += n;
        match window.get(st.pos) {
            None => return More,                                                      // partial name, already validated
            Some(b':') if st.pos > st.line_start => { st.colon = st.pos; st.pos += 1; st.phase = Value }
            _ => return Err(Malformed) } },                                          // empty name, SP, CR, LF, CTL, DEL, ≥ 0x80
    Value => {
        let e = st.pos + value_len(&window[st.pos..]);                                // today's SWAR, resumes at st.pos
        match window[e..] {
            [CR, LF, ..] => { emit_span_and_facts(st.line_start, st.colon, e)?; st.pos = e + 2; st.phase = LineStart }
            [] | [CR]    => { st.pos = e; return More }
            _            => return Err(Malformed) } } } }
}
```
- **Equivalence with G (verdict for verdict):** STOP ⊂ not-tchar, so "first non-tchar is `:` and the name is
  non-empty" ⇔ "first STOP byte is `:`, the name is non-empty and every byte before it is tchar". The fold runs over
  the newly scanned bytes before any `More`, so a delimiter such as `(` is rejected in the same call as G rejects it
  (review counterexample `HTTP/1.1 200 OK\r\na(b:xyz` → Malformed in both); the field count is checked at the first
  byte of a line, as G does, so 100 fields followed by `x` is Malformed in both (never `More` or `TooLarge`). Whole
  and split parsing agree byte for byte (parser note §6.3 invariant); the differential fuzz target runs P-B vs G.
- **Linear under a trickle:** `pos` advances through a partial name or value and is persisted with `phase`, so a
  64 KB line fed one byte per call costs O(64 KB) in total (the review's 502–32,000× counterexample came from an
  earlier draft that restarted at the line start). Test: 1-byte feeds of a 60 KiB name and of a 60 KiB value assert
  the work is linear (feeds (a) and (c) already exist).
- **Status line:** the complete 12-byte prefix as `u64::from_le_bytes(first 8) & 0x00FF_FFFF_FFFF_FFFF ==
  "HTTP/1."` (7 bytes) plus byte checks for the minor (`0`/`1`), SP and three digits; a partial prefix stays byte by
  byte (G's rule). (A plain `u64` + `u32` equality cannot check the minor and the digits.)
- **Arithmetic:** offsets are bounded once per call (`window.len() ≤ 65,536`), so the ≈ 8 `checked_add` per field
  become `wrapping_add` with that bound as the documented reason (the pattern `value_len` already uses under
  `clippy::arithmetic_side_effects`).
- **Optional padded window (uWS fence, safe form; measured separately, API change §0.3 item 2):** the pooled read
  buffer keeps ≥ 8 initialised slack bytes after the data, the first being CR and the second a non-LF byte, so the
  SWAR loops run over `as_chunks::<8>()` with no scalar tail. Rules: every match (STOP byte, CR, LF, `:`, end of head)
  counts only at a position < `min(received length, head budget)` — a stop at or beyond it means `More` (or
  `TooLarge` at the budget, as in G); the sentinel CR is never
  followed by LF, so stale slack can never complete a line; a CR at the last received byte is `More`, a CR followed
  by a received non-LF byte is `Malformed` (uWS stalls instead; we reject).
- **STOP search formula (SWAR, per `u64` word `x`):** `hasless(x, 0x21) | (x & 0x8080…80) | haszero(x ^ 0x7F7F…7F)
  | haszero(x ^ 0x3A3A…3A)`. Only the **lowest** flag is exact (borrows propagate upward; higher flags can be false,
  70 seen in 4,032 byte pairs, review docs-review-1 F-13), so the code uses `trailing_zeros` of the flags and nothing
  else (no popcount, no highest flag, no backward scan).
- **What we know about the name search (measured, dev):** the SIMD note measured a SWAR name path in this dependent
  chain as **slower**: "SWAR names + SWAR+tz" 321.2 ns vs 185.8 ns for the table on the repeated head and 425.3 vs
  395.0 ns on varied heads (SIMD note §4.2–§4.3; its §7 item 4 keeps the table in P0). A related seam was also
  measured and dropped: phrasing the scans as `token_end`/`value_end(window, pos)` was ≈ 3% slower on `1k-h20`
  (journal 2026-11-07 "Scan seam"). P-B's STOP search differs in its test (four SWAR tests for a small stop set
  instead of exact per-lane alphabet ranges with a table fallback) and moves the per-byte table work off the exit
  branch, but it is the same shape — a word test in the dependent chain — and the fold still loads, looks up and ANDs
  every name byte. So the name search is an **independent A/B inside P-B** (table loop vs STOP search), not part of
  the kernel's definition.
- **Cycle budget (arm64, estimate, unmeasured):** ≈ 40 cycles per field − dispatch (10–15) − name locate (6–8) −
  checked arithmetic (2–4) + span stores and OWS checks (3–4, as §5.0) = ≈ 16–26 cycles per field, i.e. ≈ 0.4–0.6×
  httparse on `1k-h20` (scaling the plain 0.93×) **if every subtraction holds**. If the name search saves nothing (as
  measured above) and half the dispatch remains (resume state, the field-count check), the same arithmetic gives
  ≈ 32–37 cycles, ≈ 0.75–0.85×. So P-B's position is ≈ 0.4–0.85× httparse, **unmeasured**; the Task 4c profile and the
  A/B decide (vs picohttpparser and llhttp the same scaling keeps P-B ahead).
- **x64 (no x64 data; estimate, unmeasured):** httparse's x86 names are a table loop like ours and its values stop
  with AVX2 `tzcnt` (on the `x86-64-v3` race build), so P-B's position against it should be no worse than on arm64;
  picohttpparser spends three `pcmpestri` per field in the common `1k-h20` case (name, and two for a value longer
  than 16 B; ≈ 30+ cycles of latency); llhttp adds a second `pcmpestri` per value end and indirect callbacks. The
  arm64 range applies with more doubt; against llhttp P-B should stay well ahead (it does strictly more per field
  than picohttpparser). The decision is made on the §5.7 measurements.
- **Time:** O(head bytes). **Space:** as G (+ nothing). **Branches per byte:** names ≈ 1 per 8 bytes + 1 exit,
  values ≈ 1 per 8 bytes + 1 exit, ≈ 5 per line fixed → ≈ 0.25–0.3 (from ≈ 0.6).
- **Risks:** (1) security: STOP must contain every byte that can end a line or hide a separator (CR, LF, SP, HTAB,
  NUL, CTLs, DEL, ≥ 0x80) — it does by construction (≤ 0x20 ∪ ≥ 0x7F); the tchar fold is the final word; exhaustive
  256 × lane tests and the differential fuzz target against G; (2) no `unsafe`; (3) clippy rule 9 (≤ 60 lines):
  `scan_lines` splits into `line_start`, `name`, `value` helpers marked `#[inline]`, kept only if the split costs
  ≤ 2% (§13.1 rule 9's hot-path exception otherwise).

### 5.4 Candidate P-C — stage-1 bitmaps + per-line `tzcnt` walk (P1a, `engine-simd`)

The SIMD note's lead candidate (its §3.6, §6.1), now with spans; one of the three P1a prototypes (P-C, P-D, scalar)
from which the lead is chosen by measurement (§5.5).
- **Stage 1** (`engine-simd`, `unsafe` inside, safe API): `classify_block` → `BlockMasks` (the seam, §5.5).
- **Stage 2** (safe, `engine-core`), per line: if the line's first byte is CR → end-of-head check (LF next, else
  Malformed); else `name_end = next_set(not_tchar, line_start)`, require **`name_end > line_start`** (an empty name
  `:x` is Malformed, as in G) and the colon bit at `name_end`; `value_start = next_clear(ows, colon + 1)`;
  `value_end = next_set(not_field, colon + 1)`, require the CR bit at `value_end` and the LF bit after it; trim
  trailing OWS back from `value_end` (§5.1); check the 100-field limit at the line start; span; facts.
  `next_set(m, p)`: `t = (m >> (p % 64)).trailing_zeros()`; `t == 64 - p % 64` or more (no bit left in this block)
  means "refill with the next block, or `More` at the end of the received data" — never an offset.
- **Space:** masks for the current block only (lazy, 7 × `u64`); whole-window masks would be up to 64 KiB / 64 × 7 ×
  8 B = 56 KiB per parser, so they are not used.
- **Measured precedent (dev, SIMD note §4):** whole head 0.42× the scalar chain on varied heads, 1.04× on one
  repeated head; lazy per block 0.55× varied, 0.88× repeated.
- **Time:** stage 1 ≈ 1.5 vector ops per byte (throughput-bound); stage 2 ≈ 15 scalar ops and 3 dependent `tzcnt`
  per line. **Branches per byte:** ≈ 0.13.
- **Risks:** a dependent `tzcnt` chain per line remains; dispatch per call, not per span; trickle as P-D (scalar for
  partial blocks, §5.5 hand-off).

### 5.5 Candidate P-D — bit-stream line validator (P1a; our own design on Parabix ScanThru)

**Seam (`engine-simd`, one definition):**
```rust
pub struct BlockMasks { pub lf: u64, pub cr: u64, pub colon: u64, pub sp: u64, pub ows: u64, pub tchar: u64, pub field: u64 }
pub struct Classifier { /* private: the kernel chosen by detect(); not constructible otherwise */ }
impl Classifier { pub fn detect() -> Self; pub fn classify(&self, block: &[u8; 64]) -> BlockMasks; pub fn kernel(&self) -> &'static str }
```
- The `[bool; 256]` class tables move to `engine-simd` (the lower crate: `engine-core` depends on it, design §13.1
  rule 7) and `engine-core::h1::grammar` re-exports them — still one source of truth, now reachable by the scalar
  kernel, the SIMD kernels and their exhaustive tests. `detect()` picks NEON (baseline on aarch64), SSSE3 or AVX2
  (x86, runtime detection) or the scalar kernel; a `Classifier` cannot be built any other way, so safe callers can
  never invoke a kernel their CPU lacks. `sp` is needed by the writer (W-D), `ows` by the span trims.
- **Observability:** each parser counts scalar re-runs (errors and fallbacks) per kernel; the bench records
  `kernel()` and the counter in its provenance, so a silent fallback to scalar is visible in every result file.

**Validator (`engine-core`, safe `u64` code; the algorithm `tools/h1-research/scanthru_model.py` executes, which keeps
four independent span lists):**
```text
state: lf_in, cr_in, name_c, colon_c, value_c, head_end_pending        // 6 bits
cursors: c_ns, c_col, c_vs, c_ve = fields                               // one write cursor per stream (F-07)
for each 64-B block (base), received = mask of bytes actually received in this block:
    if head_end_pending: require LF at bit 0 (else scalar re-run → Malformed); head ends at base + 1; stop
    S  = (LF << 1) | lf_in                                  // line starts (lf_in = 1 at the hand-off line start)
    A  = (CR << 1) | cr_in                                  // bytes after a CR
    (s1, name_c)  = (S & T).carrying_add(T, name_c);   N = s1 & !T          // byte after each name
    P  = (N << 1) | colon_c                                 // byte after each colon
    (s2, value_c) = P.carrying_add(V, value_c);        E = s2 & !V          // byte after each value
    err = (N & !COLON) | (E & !CR) | (A ^ LF) | (S & !(T | CR))
    H  = S & CR & received                                  // empty lines (head end candidates)
    limit = if H == 0 { all } else { bits ≤ lowest(H) + 1 } // keeps the LF check at h + 1
    err &= limit & received
    if err != 0 → re-run the scalar parser from p, the P-D run's start (it reports the exact first error)
    if c_ns + popcount(S & T & limit & received) > 100 → re-run the scalar parser from p   // before flatten
    stage[0..16] ← flatten(S & T & limit & received);  stage[16..32] ← flatten(N & limit & received)   // 4 × 16-entry
    stage[32..48] ← flatten(P & limit & received);     stage[48..64] ← flatten(E & limit & received)   // stack buffers
    write each stage's own count of entries into its stream of FieldSpans at that stream's cursor (name_start at
    c_ns, name_end at c_col, value start at c_vs, value end at c_ve) and advance that cursor by that count
    complete = min(c_ns, c_col, c_vs, c_ve)    // fields with all four entries; a line crossing the block keeps its
                                               // earlier entries at index `complete` until the rest arrive
    if H != 0: h = lowest(H); if h == 63 { head_end_pending = 1; continue }
               if h + 1 < received length { head ends at base + h + 2; stop } else hand off at line start base + h
    lf_in = LF >> 63; cr_in = CR >> 63; colon_c = N >> 63
spans[k] = (name_start[k], colon[k], skip_ows(value_from[k]), trim_ows(value_end[k])) for k < complete at the
commit (line start L, below); facts per field (scalar)
```
- **Four cursors, not one (review docs-review-1 F-07):** the four streams do not advance by the same count in a block
  when a line straddles a block boundary, so one shared cursor puts later entries into the wrong field. Required
  regression test of the Rust implementation (P1a): from a line start, `"N" × 70 + ":v\r\n" + "a:b\r\n" × 14 +
  "\r\n"` must give spans `(0, 70, 71, 72), (74, 75, 76, 77), …` (a single cursor gives `(0, −, −, −), (74, 70, 71,
  72), …`).
- **Head end (review R1-1, now enforced):** the sixth bit carries an empty-line CR at bit 63 into the next block,
  where only its LF is checked before stopping; `limit` keeps bit `h + 1` so a bare CR after the last CRLF fails
  `A ⊕ LF`; `H` and every flatten input are masked by `limit & received`. The review's counterexamples are in the
  model's regression list and pass: `N:` + 59 × `v` + `\r\n\r\n` + `Content-Length: 0\r\n\r\n` ends at 65 with one
  field (the body line is not a header); `a:b\r\n\rX` is Malformed at the `X`; `a:b\r\n\r\nc:d\r\n` has one span.
- **Resume = hand-off at line starts (review R1-2):** P-D runs from a line start `p` over the complete 64-byte blocks
  of the window and **commits only complete lines** (spans, facts, field count) up to the last line start `L` inside
  the processed blocks; it never commits a line it has not seen end. The scalar parser (P-B or G) continues from `L` in
  its `LineStart` phase and commits lines one by one; **in the same call**, once the scalar parser has committed the
  line it owns and is back at a line start with ≥ 64 received bytes after it, P-D runs again (the model's `pd_split`;
  this covers far more of a multi-read head than waiting for a later call, e.g. 60% vs 16% of the bytes of a 99 × 600
  B head in 700 B reads, review docs-review-1 F-11). Each line is committed by exactly one of the two, so no fact is
  folded twice and the field count is exact. **[author's call]** P-D processes complete 64-byte blocks only; the last
  partial block of the received data is left to the scalar parser (`received` is all-ones in the first version; the
  mask stays for the padded-window variant, §0.3 item 2, measured in P1a). Trickles and short tails stay scalar
  (linear, cheap). The model checks this hand-off with 0–3 random splits on 100,000 heads and **every single split
  point** in the boundary sweep: spans and head end equal the whole-input reference (error offsets too, but those are
  not independent evidence, §4.1 "Coverage limits").
- **When P-D runs:** only when the scalar parser is in its `LineStart` phase and does not own a partial line.
  Once the scalar parser has returned `More` inside a line it keeps that line until its CRLF, so P-D never
  re-classifies a partial line and a 1-byte trickle stays linear (model: 0 bytes classified by P-D).
- **Scalar re-run after an error:** from the P-D run's start `p` (a line start), with every uncommitted index
  discarded; it reports the first error exactly, with G's semantics.
- **Field limit (review R2-2):** before the flatten of each block,
  `c_ns + popcount(S & T & limit & received) > 100` → scalar re-run from `p` (Malformed at the 101st line's first
  byte, as G). `c_ns` (the name-start cursor, which leads the other three) counts committed lines plus the lines of
  this run not yet committed. The flatten writes into four 16-entry stack buffers per block (≤ 16 lines fit in 64
  bytes) and copies each stream's count into `FieldSpans` at that stream's cursor, so no stepped write can ever touch
  slot 100 or beyond.
- **What it validates:** token names ending at `:` (an empty name fails `S & !T`), field-content values ending at
  CR, CR always followed by LF and LF always preceded by CR (`A ⊕ LF`), no line starting with SP/HTAB (obs-fold), end
  of head at `CRLF CRLF`. The status line stays scalar.
- **Time:** stage 1 ≈ 90–100 vector ops per 64 B for the five masks the validator reads (≈ 127 for all seven of
  `BlockMasks`; recount in review docs-review-1 F-02); validator ≈ 32 `u64` ops per 64 B; flatten ≈ 4 × (popcount +
  a step-of-4 unrolled write) per block; per field ≈ 6 scalar ops for OWS and the span; facts by length switch.
  **Branches per byte:** ≈ 0.05. **No dependent chain between lines.** **Space:** 6 carry bits; four 16-entry stack
  buffers per block, copied by count into the caller's `FieldSpans` (≤ 100 entries per stream, checked before the
  flatten).
- **Speed: unknown until prototyped.** The only measurement of a stage-1 design in the repo (SIMD note §4.2, dev,
  two masks only) was **not faster than scalar on a fixed head**: whole-head stage 1 194.4 ns vs 185.8 ns (1.04×),
  lazy per block 162.9 ns (0.88×), ≈ 0.86 and ≈ 0.72 cycles per byte at ≈ 3.5 GHz; its 0.42× win is on varied heads,
  where the scalar parser mispredicts. The gate cells `1k`, `1k-h20` and `ring64` are fixed-shape heads. ≈ 100 vector
  ops per block on a four-pipe vector back end is ≥ 25 cycles, ≥ 0.39 cycles per byte for stage 1 alone with every
  pipe busy (≥ 0.78 on a two-pipe core); with the validator, flatten and per-field work (≈ 200 instructions per 64
  B) a realistic derived range is **≈ 0.6–1.0 cycles per byte on arm64 (estimate, unmeasured)**, i.e. parity to a
  modest gain on fixed heads. simdjson's stage 1 alone costs 0.5–1 cycle per byte on AVX2 (32-byte vectors). x64: no
  data.
- **Expected position:** unknown. P-D is not what closes a gate cell: the scalar parser must already win every cell
  in Task 4c (owner decision 2026-11-13, §5.7). P-D is an extra layer, kept only where it measures faster than that
  scalar parser. P1a starts by prototyping P-D stage 1 + validator, P-C and scalar on the fixed `1k-h20` head and the
  `varied-256` ring on both arches and chooses the lead from the data (plan "P1a entry conditions recorded in P0").
- **Our own additions** beyond Parabix/simdjson: error handling by scalar re-run (§4.3), so the fast path is only an
  acceptance filter and the scalar parser keeps all error semantics; the line-start hand-off; `u16` spans for JS.
- **Risks:** (1) security — the filter must never accept what the scalar parser rejects: classes from the single
  tables, exhaustive 256 × lane × alignment tests per kernel, the model ported as a Rust test, fuzz SIMD vs scalar vs
  httparse; (2) `unsafe` only inside the `engine-simd` kernels (`&[u8; 64]` loads are in bounds by type; the feature
  argument is `detect()`); (3) body bytes in a straddling block are loaded but masked (§0.3 item 3); (4) a classifier
  bug would show up as accepted-but-wrong spans only if the scalar re-run is not reached — hence the differential
  fuzz and the fallback counter.

### 5.6 Rejected

- **Per-span 16-byte SIMD (httparse shape):** slower in our dependent chain (SIMD note §4.2).
- **SHENG / data-parallel FSM:** dependent shuffle per byte (§4.4).
- **Safe-Rust bitmaps in `engine-core`:** no auto-vectorisation (§5.0).
- **Name hashing during the scan (nginx):** a serial multiply per byte; a length switch is cheaper for 3 names.
- **A P-D carry checkpoint with a scalar tail that commits independently:** folds facts twice (review R1-2);
  replaced by the line-start hand-off.

### 5.7 Measurement plan and acceptance (parser)

- **Equal work:** every row produces status, version, head length, per-field name/value spans with OWS trimmed,
  framing facts and keep-alive. Once P-A lands, our row emits spans itself. `blazehttp+fields` then stays for one
  comparison, as the pinned baseline row `blazehttp+fields@4d` (the parser Task 4d leaves, plan Task 4c acceptance
  line 2), and is deleted after it; the plain row stays as "facts only" for Zig.
- **Workloads:** `1k`, `1k-h20`, `ring64` (both), `mixed`, `trickle` (feeds (a)–(d), ≤ 1 ms each), **plus a
  `varied-256` ring** (names shuffled, value lengths 0.5–1.5×) because `ring64` varies only digits.
- **Statistic (owner gate, §0.1):** design §16.4 (component micro-gates), per rival and workload, computed by
  `pairedRatioCi` in `bench/parsers/stats.ts` (added by Task 4c before any candidate is judged); arm64 reference
  host (nightly window) and x64 VM. **x64 build baseline [author's call]:** every row, ours and every rival, is built
  for `x86-64-v3` (design §16.4 states it as the one exception to §16.2's shipped-baseline rule; the flags are the
  same for every row); this turns on each rival's best path the host has (SSE4.2
  `pcmpestri` in picohttpparser and llhttp at compile time, AVX2 in httparse). The gated row of ours is the scalar
  parser on both arches. Which x64 build a later SIMD row of ours uses, the v3 build or the shipped build (baseline
  `x86-64` + runtime dispatch), is decided in P1a with the dispatch design. Superseded: the unpaired ratio-of-medians
  statistic and `response-parser.md`'s "≤ 0.85× httparse on 1k-h20" line.
- **Task 4c acceptance (plan "Task 4c" is the source):** its first step profiles plain G on `1k-h20`; the paired CI
  and the `varied-256` ring are added before any candidate is judged. P-A has no equal-work baseline in the A/B
  harness, so it is judged in a 10-round dev race against the pinned `blazehttp+fields@4d` row, with the A/B rule's
  thresholds. Each P-B change (locals / one loop per line; the STOP search as its own A/B against the table loop;
  the masked `u64` prefix; the shared classifier) is kept only if it passes the plan's A/B rule in the same
  container (`crates/engine-core/examples/h1_parse_bench.rs`, 10 alternated rounds, `pairedRatioCi(new, old)`) and
  dropped otherwise. Spans equal the reference, and the new parser is faster than the pinned baseline row on every
  timed workload, with no feed slower. The race on both arches then records every cell against httparse,
  picohttpparser, llhttp, Beast, Zig, Go, fasthttp and nginx, with a Clang build of every C and C++ row beside the
  GCC one. **Owner decision
  (2026-11-13): Task 4c passes only when the scalar parser, with no `engine-simd` kernel, wins every one of those
  cells on both arches** (CI upper bound < 1.0; rivals as shipped, with their own SIMD). The estimates above do not
  show P-A + P-B reaching that against httparse (and possibly picohttpparser on x64), so open cells after the first
  race are likely. An open cell keeps Task 4c open: the task continues as research and reports progress to the
  owner. The gate is not relaxed and the cell is not handed to P1a. P1a runs the race again after every parser
  change.
- **Scalar ideas for a cell that stays open (Task 4c research loop; ranked; each an estimate until measured and
  kept only through the A/B rule):** (1) specialised short-name paths (a known-name word compare, §6.1, that locates
  and validates common names in one step); (2) the padded read window of §5.3, which removes the scalar tails;
  (3) for each open cell, a profile of the rival that wins it and our own redesign of the step where it leads
  (study, rewrite, never copy).
- **SIMD ideas (P1a only; never a way to close a Task 4c cell):** on x64, wider stage-1 kernels — AVX2 32-byte,
  then AVX-512 64-byte only after AVX2 is measured (SIMD note §1.1); lazy per-block stage 1, the only stage-1
  variant measured faster than scalar on a fixed head (0.88×, SIMD note §4.2, dev). Each is kept only where it is
  faster than the scalar parser.
- **Secondary metrics** (x64 VM): `perf stat -e instructions,branches,branch-misses` per head to check the
  branches-per-byte claims; `--emit asm` audit of each kernel (no panic paths; parser note §8.5).
- **Design §16.4 (component micro-gates)** holds the parser and writer micro-gate statistic (§0.3 item 5).

## 6. Our writer: candidates

Target: on the committed bench, `post1k` from 3.01–3.07× (measured, §3 intro) to ≤ 2.3× the copy floor, and every
rival@EW1 beaten (§3.12–3.13; evaluated by the Task 5c race). The scratch ablation (§3 intro) gives the shares: name
classification ≈ 20% of the writer's time, the delta scan ≈ 20–25%, the other checks ≈ 10%. Every effect below is
an **estimate (unmeasured)**. **First step of Task 5b:** fix the floor measurement (same binary, alternated order,
the design §16.4 CI, a varied-delta workload), because the floor moved 13–21% between two binaries with unchanged
code; a ratio to it is not yet a stable measure. Replacing the ratio-to-floor line by a more stable measure is a
proposal with the owner (not decided here); until then the `post1k` ≤ 2.3× line stands as set.

### 6.1 Building block: one known-name classifier, shared by parser and writer

`h1/field_name.rs` (a shared module, design §13.1 rule 2):
```rust
pub enum KnownName { ContentLength, TransferEncoding, Connection, Host, Te, Expect, Upgrade, KeepAlive,
                     ProxyConnection, ProxyAuthorization, Authorization, Cookie, ContentType, UserAgent }
pub enum Spelling { Exact, Underscore }   // Underscore: the name matched with '_' standing for '-'
pub fn classify(name: &[u8], has_underscore: bool) -> Option<(KnownName, Spelling)>;
```
- **Algorithm:** `match name.len()` (jump table) → for lengths with several candidates, one discriminating byte
  (the last byte, h2o: at length 10 `n`/`e`/`t`; at 6 `e`/`t`) → **compare 1–3 overlapping little-endian `u64`
  words OR-ed with `0x2020…20`** against `const` canonical words (`u64::from_le_bytes` of `name.first_chunk::<8>()`
  and `name.last_chunk::<8>()`; names under 8 bytes zero-extended). No indexing, no panic path. The `const`
  canonical words are built the same way — zero-extended, **then OR-ed with `0x2020…20`** — because the OR turns the
  input's zero padding into `0x20`; without it `te` and `host` would never match and a forbidden name would pass. The
  last-byte discriminator is case-folded too (`last | 0x20`, so `TE` and `te` take the same branch).
- **Exactness:** names are validated tokens before classification; canonical names contain only `a–z` and `-`;
  among tchar bytes, `|0x20` maps only `A–Z`/`a–z` into `a–z` (`^`→`~` and `_`→DEL land outside the canonical
  bytes), so the folded compare ⇔ ASCII case-insensitive equality. Over all 256 bytes the only false fold onto a
  canonical byte is CR (`0x0D | 0x20` = `-`), which is not a tchar: **validation before classification is
  load-bearing**, so P-B and W-A classify a name only after its tchar fold has passed. **Underscore spellings**
  (writer note W10/X9) **[author's call]:** a second set of `const` canonical words with `_` in place of `-`, built by
  the same OR fold (`_ | 0x20` = `0x7F`, which no validated tchar other than `_` produces), compared only when the
  name scan saw a `_` (a flag folded during the scan, as nginx hashes during its scan); the input is never rewritten.
- **Users:** the writer uses every variant (an `Underscore` match is always forbidden). **The parser matches only
  `Spelling::Exact`** for its three framing names (an underscore name is an ordinary field on the response side,
  as today), replacing `facts.rs`'s length switch + `eq_ignore_ascii_case`.
- **Cost:** a switch, ≤ 1 byte test, 1–3 word compares — ≈ 1–2 ns per name (estimate). **Tests:** every special
  name in every case and underscore spelling, every length 0–64, random tokens, all compared with today's
  `classify`.

### 6.2 Candidate W-A — scalar, safe Rust (Task 5b, P0)

1. **Names:** the parser's STOP search + tchar fold (§5.3) — the same primitive in `grammar.rs` for both (DRY),
   with the `_` flag folded into a `[u8; 256]` flags table (tchar bit, underscore bit) so the fold yields both.
2. **Classification:** §6.1.
3. **Values:** today's SWAR `value_len`, then `": "` before and CRLF after checked with slice patterns.
4. **Authority binding by id — not part of Task 5b or Task 6; a P1a entry condition (plan item 9), kept only if an
   A/B shows it faster:** instead of a 10–30-byte compare per request, the connection
   stores the `TemplateId` it was checked out for; ids are `(slot, generation)` pairs whose generation increments on
   every slot reuse and never wraps within the process (`u64`), so a stale or recycled id can never match a template
   built for another authority (the credential-leak case). −2–4 ns (estimate).
5. Limits checked once on the summed length.
- **Time:** O(target + delta) validation + O(head + body) copy. **Space:** 0 extra. **Branches per delta byte:**
  ≈ 0.25 (from ≈ 0.6). **Allocations:** 0.
- **Expected (estimate, unmeasured):** the ablation's ≈ 21–22 ns saving (classification share applied as if it all
  disappears; the new classifier and STOP search still cost something; it includes item 4's 2–4 ns, now P1a)
  gives `post1k` ≈ 2.15–2.7× floor, depending on which baseline it is applied to: ≈ 2.35–2.4× as a ≈ 22% share of
  the fix-round-1 ratio; ≈ 2.5–2.7× as 21–22 ns on the fix-round-1 rounds (writer 135.5–166.6 ns, floor 45.2–54.5
  ns, loaded host); ≈ 2.15–2.7× on the review's derived figures (docs-review-1 F-05). **W-A alone straddles the 2.3×
  line at best.** x64: the same shares (no SIMD involved).
- **Better than hyper/Go because:** EW1's validation plus our EW2 checks with no allocation, map, `fmt` or second
  pass; one word compare per name where hyper pays a map entry and Go a map hash.
- **Risks:** classification must equal today's (tests above); the X3 line count stays.

### 6.3 Candidate W-B — validate against the writer's own layout (scalar; our design from the hint idea; P2)

Built in **P2**, not in P0: it needs the P2 wire contract. The JS layer knows each line's name and value length. It
passes them as **untrusted hints** (`[u16; 2]` per line next to `header_lines`; ≤ 13,107 lines, the `u16`
`header_lines` bound for 64 KiB of 5-byte lines, so ≤ 52 KiB of hints in the worst case and 4 B per line normally;
the writer at `786d341` enforces no field-count limit — the 100-field limit of design §5.2 is the parser's — so
13,107 stays the bound unless P2 adds one, which would make it 100 lines / 400 B). Rust then needs no search:
```text
pos = 0
for (name_len, value_len) in hints:                                   // O(lines); every range via get(), never [..]
    require name_len ≥ 1                                               // no empty name (review R1-5 (1))
    name  = block.get(pos .. pos + name_len)?                          // None → Engine(Internal): no panic (R1-5 (2))
    sep   = block.get(pos + name_len .. +2)? == b": "
    value = block.get(pos + name_len + 2 .. + value_len)?
    crlf  = block.get(end_of_value .. +2)? == b"\r\n"
    require sep && crlf                                                // review R2-1: both are mandatory
    require all_tchar(name) && all_field_content(value)               // field-content excludes CR and LF
    classify(name)                                                     // §6.1
    pos = end_of_value + 2
require pos == block.len()  &&  hints.len() == header_lines
```
- **Injection-exact without counters (review R1-5 (3)):** every byte of the block is in exactly one of: a name range
  (tchar only), a checked `": "`, a value range (field-content: no CR, LF, NUL, CTLs except HTAB, DEL), or a checked
  CRLF; the ranges tile the block. So a value carrying `\r\nx-forwarded-for: 1.2.3.4` fails its field-content fold,
  whatever the hints say, and no counter can wrap. (A whole-block fold with CR/LF *counts* would also be exact, but
  only with `usize` counters; it is not used.) The model does **not** test the tiling end `pos == block.len()` or the
  CRLF check (§4.1 "Coverage limits"); required P2 Rust tests: correct hints for a prefix followed by trailing bytes
  (e.g. a smuggled line after the last hinted line), hints whose value range ends before or after the CRLF, and
  wrong separators.
- **Time:** O(delta) with no data-dependent loop exits: the folds have hint-given lengths (SWAR, 8 B per step) +
  O(lines) constant-time checks. **Branches per byte:** ≈ 1 per 8 bytes; ≈ 6 per line.
- **Expected (estimate, unmeasured):** W-B still folds every byte; it only removes data-dependent loop exits, which a
  fixed `post1k` delta lets the predictor learn (SIMD note §1), so any saving shows on varied deltas, not on `post1k`.
  No `post1k` figure is claimed.
- **Precondition:** measure the JS-side cost of producing the hints in P2 before adopting (the lengths are known
  while serialising, so it should be ≈ 0, unverified).
- **Risks:** a P2 wire-format change (§0.3 item 4); fuzz target: random hints × random blocks vs today's
  `scan_fields` (must agree on accept and reject); vectors: `": x\r\n"` with hints (0, 1) and (1, 0), out-of-range
  hints, a hint that ends inside the block.

### 6.4 Candidate W-C — validate-once interned names (cross-layer)

As undici's lowercase record, h2o's tokens and hyper's `StandardHeader`: the JS layer sends a small integer id for
each **known** name (the §6.1 set plus common ones such as `accept`, `content-encoding`); Rust writes the canonical
bytes from its own static table and takes the class from it — no name scan or classification for those lines.
Unknown names still go through W-A/W-B. **Risk:** the id table is generated from `policy/spec.json` so JS and Rust
cannot drift; a JS bug sending `content-length` as raw bytes is still caught by classification.

### 6.5 Candidate W-D — SIMD delta validation (P1a, `engine-simd`)

The **writer form** of the P-D validator (review R1-3, R1-4), exactly what `tools/h1-research/scanthru_model.py`
`wd()` executes:
```text
state: lf_in = 1 (the block starts at a line start), cr_in, name_c, value_c, colon_hi (2 bits: colons at 62, 63)
for each 64-B block, received = bytes of the delta in this block; the class masks LF, CR, COLON, SP, T, V are
    AND-ed with `received` before use, so stale padding never feeds a carry (else `a: b\r\n` + padding `a…` is
    falsely rejected; required Rust test, P1a):
    S = (LF << 1) | lf_in;  A = (CR << 1) | cr_in
    (s1, name_c)  = (S & T).carrying_add(T, name_c);   N = s1 & !T                 // byte after each name
    N1 = (N << 1) | (colon_hi >> 1)                     // byte after the colon: must be SP (exact ": ")
    P  = (N << 2) | colon_hi                            // value starts after ": "
    (s2, value_c) = P.carrying_add(V, value_c);        E = s2 & !V                 // byte after each value
    err = (N & !COLON) | (N1 & !SP) | (E & !CR) | (A ^ LF) | (S & !T)              // S & !T: empty name, empty line
    if err & received != 0 → reject (Engine(Internal), as today)
    stage[0..16] ← flatten(S & T & received);  stage[16..32] ← flatten(N & received)   // 2 × 16-entry stack buffers
    classify each line whose name start and colon are both known (a name crossing a block keeps its start in
    `pending_name_start`, one u16 of state, until its colon arrives); no allocation on the hot path
    lines += popcount(LF & received); carry out lf_in, cr_in, colon_hi = N >> 62
accept iff the delta is empty or ends with LF, name_c == 0, value_c == 0, lines == header_lines,
       and every line's classify(block[name_start .. colon]) was admitted                        // review M2-1
```
- **Per-line classification (review M2-1):** the bit-stream grammar alone would accept `content-length: 5\r\n` or
  `transfer-encoding: chunked\r\n`. W-D therefore flattens the name bounds (as P-D does) and runs the §6.1
  classifier with the singleton bits on every line, exactly as W-A: forbidden names, underscore spellings of forbidden
  and singleton names, and repeated singletons are refused. The model's reference now carries these rules and the
  four forbidden-name vectors (`content-length`, `Transfer-Encoding`, `content_length`, a repeated `cookie`) plus
  `user_agent` are rejected by W-A, W-B and W-D alike.
- **No head end in the writer form:** an empty line inside the delta (`S & CR`) is an error (`S & !T`), never an end
  marker, so `a: b\r\n\r\nGET /admin HTTP/1.1\r\nhost: internal\r\n` is rejected; the end condition (last byte LF,
  no run in flight) rejects `a: ` + 61 × `v` (64 bytes, no CRLF); empty names (`: x\r\n`) fail `S & !T`; the X3 line
  count stays. All are in the model's checks and pass; 100,000 mutated deltas agree with the scalar W-A reference.
  The 2-bit colon carry has no deterministic boundary sweep in the model (§4.1 "Coverage limits"); required P1a Rust
  test: a colon at bits 61, 62 and 63 followed by an invalid byte in the next block.
- **Engine tests (review suggestion 5):** the smuggle, unterminated, empty-name, forbidden-name and duplicate-
  singleton deltas go into `tests/vectors/h1-request/blazehttp-rules.json` (run by `crates/engine-core/tests/
  h1_writer.rs` `injection_vectors` on every writer path), and the writer fuzz target
  (`crates/engine-core/fuzz/fuzz_targets/h1_writer.rs`) gains a W-D vs W-A differential when W-D lands.
- **With W-B hints** the validator's own outputs are additionally compared with the layout: `N == hinted colons`,
  `E == hinted line ends` — **in addition to** the validator, never instead of it. (Comparing the raw `:` class
  would reject values containing `:` such as URLs and times; comparing CR alone would miss a bare LF or NUL — the
  review's counterexamples — which `A ⊕ LF` and `E & !CR` catch.)
- **Optional fused copy:** `copy_classified(dst: &mut Vec<u8>, src)` loads each block once, stores it into `dst` and
  classifies it; the uninitialised-capacity write is `unsafe` inside `engine-simd` only, with a `// SAFETY:` argument
  that `dst` was reserved for `src.len()` before the loop. Because this reverses today's check-then-append, the
  writer calls `out.clear()` on every error path, so rejected bytes never stay in the buffer. Measured, not assumed:
  at 100–400 B the delta is in L1 after a plain copy anyway.
- **Cost (re-derived from the P-D model, estimate, unmeasured):** ≈ 150 ops per 64-byte block (stage 1 for six
  masks ≈ 115 — P-D's five ≈ 101 plus SP — and a validator ≈ 35, against P-D's ≈ 101 + 32); at a sustained 6 ops
  per cycle that is ≈ 7 ns per block, optimistic for a four-wide vector back end (§5.5), and ≈ 11–14 ns at 3–4. Plus
  the §6.1 classification at ≈ 1–2 ns per line: a 100-byte `post1k` delta (2 blocks, 3 lines) ≈ 17–35 ns, a 400-byte
  delta (7 blocks, ≈ 10 lines) ≈ 55–120 ns. That is no better than W-A's scalar folds on small deltas; W-D can pay
  only on large deltas (`h20`), so it is **optional in P1a, behind a measurement**, kept only where it measures faster
  (if anywhere, above ≈ 256 B).

### 6.6 Rejected

- Fused copy + validate in safe `engine-core` (needs `unsafe` `set_len`, or a capacity check per word).
- Hashing names (a multiply chain per byte costs more than 1–3 word compares).
- A per-client cache of validated delta blocks keyed by content (hashing ≈ the validation it saves; YAGNI).
- `writev` of head + small body (journal: equal up to 4 KiB, worse under TLS; writer note §8.3).
- Raw-class bitmap equality as a replacement for the W-D validator (review R1-4).

### 6.7 Measurement plan and acceptance (writer)

- **Workloads:** `get`, `post1k`, `varied` (256-ring), plus `h20` (20 per-request fields); arm64 and x64.
- **A/B:** `crates/engine-core/examples/h1_write_bench.rs` (writer vs copy floor, same container, alternated
  binaries, 10 rounds, the A/B rule of plan "Task 4c"); ablations per check (classification off, delta scan off,
  authority off).
- **Task 5b acceptance (plan "Task 5b" is the source):** first the floor measurement is fixed (§6 intro); W-A is
  kept only if faster in that A/B and dropped otherwise; `post1k` ≤ 2.3× floor on both arches (the owner's line,
  unchanged; W-A alone is projected to straddle it, §6.2); 0 allocations (`tests/h1_writer_alloc.rs`). "Every
  rival@EW1 beaten" is evaluated by Task 5c, not inside 5b.
- **Task 5c acceptance:** the design §16.4 statistic for ours@EW2 / theirs@EW1 against every runnable rival, every
  workload, both arches (§3.13). **Expected x64 position:** the writer has no SIMD in P0, so its position against Go,
  hyper, fasthttp and the C/C++ writers should match arm64's; a rival with SSE-specific write paths was not found.

## 7. Measurement plan (the single summary; details in §5.7, §6.7, §3.13)

| What | Where | Workloads | Arches | Statistic / acceptance |
|---|---|---|---|---|
| P-A, P-B (keep or drop) | `h1_parse_bench` A/B, same container | 1k, 1k-h20, ring64, mixed, trickle, varied-256 | arm64, x64 | the A/B rule of plan "Task 4c" (10 alternated rounds, `pairedRatioCi`) |
| Plain G profile (4c step 1) | `perf stat`/sampling (x64 VM) or Instruments (arm64) | 1k-h20 | either | measured per-field split replaces §5.0's guess |
| Parser race (4c; run again in P1a) | `bench/parsers` + fasthttp, nginx rows + `pairedRatioCi` | same | arm64 ref + x64 VM (`x86-64-v3`) | design §16.4 statistic per rival and cell; Task 4c passes only when the scalar parser wins every cell (owner decision 2026-11-13) |
| P-C, P-D, scalar | P1a prototypes (lead chosen from data), then the race | 1k-h20, varied-256, then all | both | kept only where faster than the scalar parser; every cell of the race still passes |
| W-A (keep or drop) | fixed floor measurement first, then `h1_write_bench` A/B + ablations | get, post1k, varied, h20, varied-delta | both | faster; `post1k` ≤ 2.3× floor; 0 allocations |
| Writer race (5c) | §3.13 harness | get, post1k, varied, h20 | both | design §16.4 statistic, ours@EW2/theirs@EW1, per runnable rival |
| W-D (optional) | P1a criterion | post1k, h20 | both | kept only where faster than W-A |
| W-B | P2 A/B | varied-delta, h20 | both | kept only if faster |
| Branch claims | `perf stat` | 1k-h20, varied-256, post1k | x64 VM | instructions, branches, branch-misses per op |

Every kept change goes to `docs/perf/journal.md` with numbers; anything not measured stays in `ideas.md`.

## 8. Decision table

Ranked within each component by expected gain per risk. Correctness and gain are separate columns: correctness is
verified as stated; **every gain is an unmeasured estimate** (only P-C's has a dev measurement) until §7 runs, and
each task's first step is the measurement that turns it into a measured figure, with a drop rule.

| Rank | Candidate | Component | Correctness: verified by … | Gain (unmeasured estimate unless marked) | Risk | Build in |
|---|---|---|---|---|---|---|
| 1 | **P-A spans in the scan** (caller-owned `FieldSpans`) | parser | spans come from bytes G already validates; span tests (§5.1) | removes the second pass: vs httparse 0.85–1.03 (a 0–3% loss on 20-field cells), vs pico 0.73–0.88, vs llhttp 0.53–0.63 | none new | **Task 4c** |
| 2 | **P-B line kernel** (locals, resumable mid-line, STOP search + tchar fold, masked `u64` prefix, shared classifier) | parser | review docs-review-1: exhaustive and mutated inputs vs an RFC reference (no Rust yet); differential fuzz vs G planned | arm64 ≈ 0.4–0.85× httparse (§5.3); x64 no data | equivalence with G (fuzz), clippy rule 9 | **Task 4c**, each change kept only if faster |
| 3 | P-B padded window (fence) | parser | rules in §5.3; not tested yet | removes scalar tails; small | API change; fence rules (§5.3) | Task 4c option, measured; else P1a |
| 4 | **P-D bit-stream validator** (6-bit state, line-start hand-off, scalar error re-run) | parser | the model (core equations, carries, line ownership; limits in §4.1) and the review's independent tests | unknown until prototyped; derived ≈ 0.6–1.0 cycles per byte on arm64 (§5.5) | `unsafe` only in `engine-simd` kernels; acceptance-only fast path | **P1a** prototype; lead chosen from data |
| 5 | P-C stage-1 + `tzcnt` walk | parser | prototype differential + Miri (SIMD note §5) | **measured** (dev, SIMD note §4): 0.42–0.55× on varied heads, 0.88–1.04× on a repeated head | simpler than P-D | **P1a** prototype |
| 1 | **W-A shared classifier + STOP search** | writer | classification tests vs today's `classify` (§6.1) | `post1k` ≈ 2.15–2.7× floor (straddles 2.3×, §6.2) | low | **Task 5b** |
| 2 | **W-B layout hints, tiled folds** | writer | the review's tests; the model misses two checks (§4.1) | on varied deltas only; no `post1k` figure | P2 wire-format change; hints untrusted | **P2** |
| 3 | W-C interned name ids | writer | — | known names cost ≈ 0 | generated table; JS/Rust drift | P2 |
| 4 | W-D SIMD writer-form validator with per-line classification (+ optional fused copy) | writer | the model and the review's tests (colon-carry sweep owed, §6.5) | ≈ 17–35 ns at 100 B, ≈ 55–120 ns at 400 B: can pay only on large deltas | `unsafe` only in `engine-simd`; `out.clear()` on errors | **P1a**, optional, behind a measurement |
| — | Task 5c cross-language writer race (reuses `pairedRatioCi`) | writer | check mode + injection table (§3.13) | the "every writer" evidence | curl micro row time-boxed | **Task 5c** |
| — | fasthttp + nginx parser rows; `pairedRatioCi` in `bench/parsers` | parser race | differential gate of the bench | the "every parser" evidence | none | **Task 4c** |

**What each task builds** (steps, order and acceptance: the plan sections "Task 4c", "Task 4d", "Task 5b", "Task 5c"
and "P1a entry conditions recorded in P0"; this note does not repeat them):
- **Task 4c (parser, scalar, P0; its first PR merges before Task 6 because it changes `parse`):** first the profile
  of plain G and
  `pairedRatioCi` + the `varied-256` ring; then P-A and each P-B change, each dropped unless faster in the A/B; the
  fasthttp and nginx rows; the survey of the Jetty/JDK/.NET response parsers (§10); span tests (§5.1). Acceptance:
  §5.7: the task passes only when the scalar parser wins every cell on both arches (owner decision 2026-11-13);
  until then it stays open as research. Task 4d runs mutation testing on the merged parser.
- **Task 5b (writer, P0):** first the fixed floor measurement (§6 intro); then W-A without the authority id (P1a),
  dropped unless faster; `post1k` ≤ 2.3× floor. W-B is P2; W-D is P1a.
- **Task 5c:** the writer race (§3.13), replacing `bench/writer/`; it evaluates "every rival@EW1 beaten".
- **P1a (SIMD):** first the prototype comparison (P-D stage 1 + validator, P-C, scalar; fixed `1k-h20` and the
  varied ring; both arches) that chooses the lead, the x64 runtime-dispatch design, Miri for the kernels, the
  padded-window API; then `engine-simd` `Classifier`/`BlockMasks` (NEON; SSSE3/AVX2; scalar), the class tables moved
  into `engine-simd`, the chosen parser path, W-D only if measured faster, and the §4.1 required tests (issue #24).
  Every SIMD path is an extra layer on the scalar parser of Task 4c and is kept only where it is faster.

## 9. Sources

All accessed 2026-11-08. Source code: verified at the commits of §1 (file and line in §2–§3). Rust std: version
1.99.1 pages.

| # | Source | Link | Status |
|---|---|---|---|
| 1 | Cameron, Shermer, Shriraman, Herdy, Lin, Hull, Lin, *Bitwise Data Parallelism in Regular Expression Matching*, PACT 2014, pp. 139–150 | https://doi.org/10.1145/2628071.2628079 ; https://www2.cs.sfu.ca/~ashriram/papers/2014_PACT_GREP.pdf | verified (primary text: MatchStar, the MatchStar-vs-ScanThru sentence, long-stream addition, 0.95/1.22/1.6 cycles per byte on SSE2 and 0.63 on AVX2); the ScanThru formula is not in it (#1a) |
| 1a | Cameron et al., *Parallel Scanning with Bitstream Addition: An XML Case Study*, Euro-Par 2011, pp. 2–13 (source of `ScanThru(M, C) = (M + C) ∧ ¬C`) | cited in #1's reference list | unverified (text not fetched); the formula is exercised by the model and the review's independent tests |
| 2 | Mytkowicz, Musuvathi, Schulte, *Data-Parallel Finite-State Machines*, ASPLOS 2014, pp. 529–542 | https://doi.org/10.1145/2541940.2541988 (cited as [11] in #1; PDF read by the papers survey) | verified (primary text: abstract and shuffle mechanism) |
| 3 | Langdale, Lemire, *Parsing Gigabytes of JSON per Second*, VLDB Journal 28(6), 2019 | https://arxiv.org/abs/1902.08318 | verified (SIMD note) |
| 4 | simdjson `v5.1.1`: `include/simdjson/haswell/bitmask.h`, `include/simdjson/arm64/bitmask.h`, `src/generic/stage1/json_structural_indexer.h` | https://github.com/simdjson/simdjson/tree/40a686ab89dc5ee58ec10d0514dd5c99abbc472b | verified (source) |
| 5 | simdutf `v9.3.3` `src/generic/ascii_validation.h` | https://github.com/simdutf/simdutf/blob/59589cd3df6823cbfdbd253cde3377e7c3e1fe9b/src/generic/ascii_validation.h | verified (source) |
| 6 | Vectorscan `5.4.13` `src/nfa/sheng_impl.h` | https://github.com/VectorCamp/vectorscan/blob/acd7363aadea43da9c5246542d9969db843dd132/src/nfa/sheng_impl.h | verified (source) |
| 7 | Wang et al., *Hyperscan*, NSDI 2019 | https://www.usenix.org/system/files/nsdi19-wang-xiang.pdf | verified (SIMD note) |
| 8 | Langdale, simdcsv README | https://github.com/geofflangdale/simdcsv/blob/a9494a85c83525bb42ba368d8f9bcf3acf53a8d1/README.md | verified (text) |
| 9 | Krasnov (Cloudflare), *Improving PicoHTTPParser further with AVX2*, 2014 | https://blog.cloudflare.com/improving-picohttpparser-further-with-avx2/ | verified (SIMD note); numbers unverified for us |
| 10 | uops.info `PCMPESTRI`, `PSHUFB`, `PMOVMSKB` | https://uops.info/html-instr/PCMPESTRI_XMM_XMM_I8.html | verified (SIMD note) |
| 11 | Rust 1.99.1: `u64::carrying_add` (stable since 1.91.0), `slice::as_chunks`, `Vec::set_len` (`unsafe`), `Vec::spare_capacity_mut` | https://doc.rust-lang.org/1.99.1/std/primitive.u64.html#method.carrying_add ; https://doc.rust-lang.org/1.99.1/std/primitive.slice.html#method.as_chunks ; https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html#method.set_len ; https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html#method.spare_capacity_mut | verified (docs; `carrying_add` also by compile under `forbid(unsafe_code)`) |
| 12 | Rust Reference 1.99.1, code generation attributes (`target_feature`) | https://doc.rust-lang.org/1.99.1/reference/attributes/codegen.html | verified (SIMD note) |
| 13 | Compile-only asm check | `tools/h1-research/asmcheck` (commands in its `Cargo.toml`) | run 2026-11-08 (compile-only, not re-run by the review; scope in §5.0) |
| 14 | Executable model of P-D and W-D | `tools/h1-research/scanthru_model.py` (seed 9112; output in §4.1) | run 2026-11-08/09; tests the core equations, carries and line ownership; coverage limits in §4.1; removed in P1a (issue #24) |
| 15 | Quiet parser race, 2026-11-08 | branch `p0/parser-bench-rerun` (PR #21), `docs/notes/response-parser.md` §8.6, `bench/results/2026-11-08-arm64-dev-parsers-gcc14.jsonl` | verified (recorded run; dev numbers) |
| 16 | Prior art: Kourier, hparse, xocomil, simdzone | https://kourier-server.github.io ; https://github.com/floatdrop/hparse ; https://docs.rs/crate/xocomil/0.3.0 ; https://github.com/NLnetLabs/simdzone | unverified (not read) |
| 17 | Every parser and writer of §1 | §1 link bases | verified (source at the pins) |

## 10. Deferred, hand-offs and pre-existing issues

**Deferred (with owner task):**
- Survey of the Jetty `HttpParser`, JDK `Http1HeaderParser` and .NET `HttpConnection` response parsers (added to G0
  by PR #20) — Task 4c research step; race rows only if one is plausibly faster than httparse.
- The writer EW1 hyper row and the Netty/Jetty JMH rows — Task 5c.
- JS-side cost of W-B hints and the `Uint16Array` span hand-off — P2.
- The micro-gate statistic now lives in design §16.4 (component micro-gates); `pairedRatioCi` — Task 4c.

**Hand-offs to other branches (not editable here):**
- `request-writer.md` L77, L94, L119 (branch `p0/writer`): curl does not cut custom headers at a newline (§3 intro)
  — Task 5 fix list.
- The plan's Task 4 `parse` signature and `response-parser.md` L320 — owner plan section **"Task 4c"** (updated when
  §5.1's caller-owned spans land); `request-writer.md` §8.4 — owner plan section **"Task 5c"** (updated when the
  writer race replaces `bench/writer/`); `response-parser.md`'s ≤ 0.85× httparse line — superseded by design §16.4
  (fixed in a later PR).
- `response-parser.md` §8.6 on `milestone/p0` still shows the noisy 2026-11-07 table; PR #21 replaces it.

**Pre-existing issues found by the review, fixed on this branch:** `bench/parsers/README.md` and
`bench/parsers/c/Dockerfile` said llhttp's NEON path runs only under `LENIENT_HEADER_VALUE_RELAXED` (strict
`header_value` has NEON too, `llhttp.c` L2789–2829); `simd-scanning-research.md` linked simdjson through `/master/`
and claimed the `engine-core` tables are the source of truth for `engine-simd` without resolving the crate direction
(now: tables move to `engine-simd`, §5.5); `response-parser.md` L49 linked llhttp's generated C by tag name instead of
commit.
