# HTTP/1.1 parser and writer speed: research note (input to Tasks 4c, 5b and P1a)

Design §5.2 (strict rules, unchanged), §6 (learn from proven code, never copy), §7 (speed programme), §13 (code
structure and patterns), §16 (gates). Builds on `docs/notes/response-parser.md` (§8.4 budget, §8.6 cross-language
race), `docs/notes/simd-scanning-research.md` (SIMD survey; not repeated here) and, on branch `p0/writer`,
`docs/notes/request-writer.md` and `docs/perf/journal.md`. Access date for every link: **2026-11-08**. **Verified** =
checked against the pinned source, the primary text of a paper, or an official doc page; **unverified** = everything
else (unverified ideas go only to `docs/perf/ideas.md`). This note is research only: no benchmark was run for it.

## 0. The problem in numbers

| Component | Gate (owner) | Today (dev, Linux arm64) | Gap |
|---|---|---|---|
| `ResponseParser` + per-field spans | faster than every parser with equal work | httparse 1.16–2.02× faster, picohttpparser 1.21–1.78× faster (parser note §8.6) | 1.2–2.0× |
| `write_request` | faster than hyper's encoder and Go's `Request.Write` with equal work; `post1k` ≤ 2.3× copy floor | `post1k` 2.51–2.58× floor; 1.16–1.22× hyper encode-only (journal 2026-11-08) | ≈ 10% on the floor ratio; ≈ 20% vs hyper encode-only |

Constraints that stay: the §5.2 checks, `#![forbid(unsafe_code)]` in `engine-core`, `unsafe` SIMD only in
`engine-simd`.

## 1. References (pinned)

Every reference is pinned to its latest published release on 2026-11-08 (resolved with the GitHub and Codeberg APIs:
`releases/latest`, then `commits/<tag>`). Projects without releases are pinned to the default branch's head commit.
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
  found inline at the empty line; validation fused with scanning; output 32 B per field into the caller's array;
  partial input = re-parse from 0 (quadratic under a trickle: 161–429 ms on the 1-byte feeds, parser note §8.6).
- **Looser than §5.2:** bare LF (L692, L862, L1113, L1208), status 000–999, a missing reason, leading empty lines,
  and opt-in obs-fold, space before the colon and multiple SPs.
- **Why it is fast, ranked:** (1) the value scan validates and finds the CR in one fused step; (2) block width
  bounded by the buffer, not the token; (3) a raw-pointer cursor with no index arithmetic or bounds checks; (4) the
  version as one `u64` compare; (5) one 32-byte store per field and no UTF-8 pass; (6) the SIMD path fixed at compile
  time on aarch64. **Weak spots:** scalar names on x86, byte-loop stop position, a runtime-dispatch call per value on
  the default x86_64 build, restart from 0.
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
- **Looser than §5.2:** bare LF, obs-fold (NULL-name entries), `HTTP/1.2`–`HTTP/1.9`, multiple SPs.
- **Why it is fast, ranked:** (1) fused classify-and-stop; (2) **on arm64 it is pure scalar** — the 8× unrolled
  loop, end check per 8 bytes, branch hints and the cursor in locals passed through return values — and it still
  beats us 1.21–1.78× with fields, so **our gap is loop structure and per-field overhead, not vector width**
  (inference from source + parser note §8.6); (3) on x86 one `pcmpestri` per 16 B (high latency, 8 µops, but one
  instruction); (4) no EOF checks inside the up-front-checked status prefix; (5) four stores per field.
- **To be faster than picohttpparser with equal work.** Its per-field path on arm64: scalar name loop (`:`, table,
  EOF check per byte), OWS loop, 2–4 trips of the unrolled value loop plus a scalar tail, four stores. On x86: two
  `pcmpestri` per field whose ~11-cycle latency dominates short tokens. **It skips:** bare-LF and obs-fold
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
  `settings` with a null check and an indirect call (api.c L7–30, L407–425): up to four calls per field. An open
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
- **To be faster than llhttp with equal work.** llhttp does the same work as we do (strict, resumable, framing
  facts), so it is the cleanest comparison; the only thing it skips is trailing-OWS trimming. We must: scan names
  faster than one byte with two branches (a word-at-a-time stop search or a padded buffer); store spans directly
  instead of four indirect calls; save resume state only at exits; keep one length match per field for the
  framing names. Expected to be a comfortable win once spans are fused (unverified).
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
  SPs before the code (L1815–1817); after the code SP, `.` (IIS `403.1`), CR or bare LF (L1833–1849); the reason
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
- **To be faster than Seastar:** it spends one transition per byte plus ≥ 2 allocations and a map insert per field;
  any span-emitting parser that never allocates wins by a wide margin (estimate, unverified).

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
  `start`/`seen_n`/`seen_r`/`seen_rn`/`seen_rnr` carried across calls, L5–12): per 16-byte block it compares with
  splats of `\r` and `\n`, ORs, and **counts** matches with `@reduce(.Add)` (L119–123). Zero matches: skip. 1–3:
  only the last 1–3 bytes are checked to carry state (L126–168). ≥ 4: an `inline for` over offsets compares each
  4-byte window as a `u32` against `\r\n\r\n` (and its top half against `\n\n`) (L169–183). The integer views
  (`int16/24/32`, L330–347) give the partial-terminator states from the same load.
- **`Response.Head.parse`** (`lib/std/http/Client.zig` L515–612) is a second pass: `splitSequence("\r\n")`, one
  `u64` compare for `HTTP/1.x`, `parseInt3` with **no digit check** (L697–702), `splitScalar(':')`, `trim(" \t")`,
  a chain of up to 7 `eqlIgnoreCase` calls. **No name or value validation**; a bare LF inside a value is kept.
  Fields are not returned; `iterateHeaders` re-splits on demand (`HeaderIterator.zig` L13–48), a third pass.
- **Suspected bug (by reading, bare-LF terminator only):** with ≤ 3 CR/LF bytes in a block only the tail is
  checked, so a `\n\n` in the middle of a block (e.g. `HTTP/1.0 200\n\nAB…`) is missed. CRLF heads are unaffected.
- **To be faster than Zig:** it validates nothing and hands no fields out, so the fair row is our plain parser
  (bench README). Our single pass must cost no more than its two; its end scan is 16 B per step, but the split
  walks every byte again.

### 2.13 Parsers not yet in the race

The race (`bench/parsers/`) has httparse, picohttpparser, llhttp, Beast (two APIs), Zig and Go. Candidates:

| Parser | Response-head API to time | Container build | Plausibly faster than httparse? |
|---|---|---|---|
| **fasthttp** v1.77.0 | `ResponseHeader.Read` on a reused, `Reset` `bufio.Reader`, fields via `VisitAll` | trivial (the Go image, module pinned) | no, but the fastest Go row; several × `net/http` (estimate) |
| **nginx** release-1.31.8 | `ngx_http_parse_status_line`, then `ngx_http_parse_header_line(r, b, 1)` until `NGX_HTTP_PARSE_HEADER_DONE`, recording pointer pairs; facts by the shared extraction pass | moderate: configure + build at the tag, link the harness to `ngx_http_parse.o` and core objects | unlikely (1 B/step), but the "C server state machine" reference |
| **Netty** 4.2.20.Final | `EmbeddedChannel(new HttpResponseDecoder())`, `writeInbound`/`readInbound`, JMH | JDK + Maven image; JIT and GC noise | no (allocates per field) |
| **Seastar** 25.05.0 | `http_response_parser p; p.init(); p(temporary_buffer<char>(head)); get_parsed_response()` | heavy (Ragel 6.10, Seastar, fmt, boost; tight under the 2 GiB cap) | no (allocates per field) |
| **uWebSockets** v20.81.0 | `HttpParser::getHeaders` / `consumePostPadded` — **requests only** | easy (header-only) | plausibly, on requests (fence + SWAR); not applicable to responses |

h2o's client and Bun's client are picohttpparser (already in the race). **No response-head parser plausibly faster
than httparse was found outside the race.** For completeness the race should add fasthttp (cheap, closes "every Go
parser") and nginx (the C server reference); Netty and Seastar are optional rows for the leaderboard (G8), not gates.

### 2.14 What every fast parser does that we do not (against `crates/engine-core/src/h1/parser/mod.rs`)

1. **Spans are stored at the CRLF, in the same pass, everywhere** (httparse L1241, pico L357, uWS L445; llhttp by
   callback). Our `value()` holds `line_start`, `colon` and the value end at the CRLF, hands them to `facts.field`
   and drops them; the bench's `split_fields` rebuilds them in a second pass. This alone explains most of the
   1.2–2.0× gap (plain / with-fields = 0.50–0.63, parser note §8.6).
2. **One loop body per field line with the cursor in locals.** Our `step()` goes round the phase loop three times per
   field (`LineStart`, `Name`, `Value`), each returning a `Result<Option<ParseStatus>, ErrorCode>` (a large value),
   re-slicing `window.get(self.pos..)` and writing `self.phase`/`pos`/`line_start`/`colon`; about eight
   `checked_add`/`ok_or` branches per field. No reference has these.
3. **The status prefix** is checked by a per-byte closure; httparse uses one `u64` compare.
4. **No end checks** (uWS fence) and no scalar tails.
5. **Names are scalar on every platform** — but so are httparse's on x86 and pico's and llhttp's on arm64; only
   httparse NEON vectorises names.

Where we are already ahead: an exact first-flag `trailing_zeros` (httparse and uWS use byte loops), resumability
without re-parse, no OWS loop on the hot path, and strictness.

## 3. Writers and serialisers: why each one is fast

_(in progress)_

## 4. Papers and algorithms

_(in progress)_

## 5. Our parser: candidates

_(in progress)_

## 6. Our writer: candidates

_(in progress)_

## 7. Measurement plan

_(in progress)_

## 8. Decision table

_(in progress)_

## 9. Sources

_(in progress)_
