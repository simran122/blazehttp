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
  the next call (parser note §8.6: 25 ms–2.2 s on the 1-byte feeds).
- **To be faster than Beast with equal work:** it is a scalar byte loop with an end check per byte plus a name
  lookup and an indirect call per field; spans emitted in our scan beat it (they already do: 0.86–0.95 for
  `basic_parser` before Beast delivered fields, 0.33–0.57 against `response_parser`, parser note §8.6). Nothing to
  copy except the explicit enum for known names, which our length switch already matches.

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

Same treatment as §2, one level deeper than `docs/notes/request-writer.md` §2 (branch `p0/writer`). Topics per
writer: when validation runs (construction or write), pre-serialised blocks, integer formatting, the copy pattern,
vectored vs copied output, word-at-a-time validation; then **what it takes to be faster than that writer with equal
work** (equal work is defined in §3.12 and used by the race in §3.13). Our baseline (journal 2026-11-08, quiet
round, `post1k`): full writer 74–79 ns; without name classification 58–65 ns (classification ≈ 15 ns); without any
delta scan 44–46 ns (`token_len` + `value_len` ≈ 14–19 ns); copy floor ≈ 36 ns; the remaining ≈ 8–10 ns are the
target check, limits, the authority compare, framing, the checked sum and `try_reserve_exact`.

**Two corrections to `request-writer.md` §2** (verified at the pins):
1. curl does **not** cut a custom header at the first newline: `Curl_add_custom_headers` uses `curlx_str_untilnl`
   only to decide whether the value is blank (`lib/http.c` L1877) and then writes the whole original string with
   `curlx_dyn_addf(req, "%s\r\n", origp)` (L1934). An embedded CRLF reaches the wire.
2. http 1.5.0 has no `eq!`-macro perfect hash any more: `parse_hdr` lowercases into a 64-byte stack buffer and calls
   `StandardHeader::from_bytes`, a `const fn` `match` on byte-string literals (`src/header/name.rs` L97–104,
   L1073–1100).

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
  (`value.rs` L401–437) — and `connection_any_close`). `dst.reserve(30 + n × 30)` is a guess (L32, L1226); the
  target goes through `write!(FastWrite(dst), "{} ", uri)` (L1231, with a `TODO … encode without std::fmt`); four
  appends per field (`write_headers` L1629–1636). `HeaderMap` iteration walks the insertion-ordered `entries`
  `Vec`, following `extra_values` only for repeated names (`src/header/map.rs` L2323–2353) — cheap. The cost is
  building a map per request. Output: `WriteStrategy::Queue` → one `writev` of ≤ 64 slices (`io.rs` L59–64,
  L278–316). No SIMD.
- **To be faster than hyper with equal work.** Equal work = per request `HeaderName`/`HeaderValue::from_bytes` for
  every delta field, a `Uri` parse, a new map, then encode: an allocation per field, per map and for
  Content-Length, and a hash per name — we already do the same validation with 0 allocations (0.38–0.41×). Against
  hyper's **encode-only** number (pre-built map, no validation) our *validating* writer is 1.16–1.22×; the equal
  opponent of encode-only is our copy path (EW0, §3.12), ≈ 0.5×. To beat encode-only even while validating we must
  remove the classification and line-search cost (§6), which hyper does not pay because each name became an enum
  once.

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
- **Serialisation:** **pre-formatted bytes**: Content-Length is formatted once when the body is set
  (`strconv.AppendUint` into a reused slice, `http.go` L2058–2067); `AppendBytes` (L3253–3305) only appends stored
  bytes. The head is written **straight into the socket buffer's free space** (`w.AvailableBuffer()`, then
  `w.Write(buf)`, L3193–3203) — no intermediate copy. Requests and writers are pooled; zero allocations in steady
  state; a small body leaves in the same `bufio` flush.
- **To be faster than fasthttp with equal work.** Its write path is a pure append of cached bytes, the same as our
  copy floor. Equal work adds `httpguts.ValidHeaderFieldName/Value` before each `Set` (EW1) — so our validation must
  cost about what two `IndexByte` per value cost: one branch-free pass over the whole delta (§6.2) and at most one
  word compare per name (§6.1). Our `out` already is the buffer that is sent.

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
  (§6.3).

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
- **To be faster than h2o:** without validation its copy equals our floor. Take the tokens (§6.3) and the
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

| Tier | Work per request | Ours | Others, lifted with their own code |
|---|---|---|---|
| **EW0** serialise only | request line, headers, framing, one buffer | copy floor | encode with pre-built, reused objects (hyper encode-only, fasthttp `AppendBytes`, Netty encode, h2o `build_request`, Beast `buffer_copy`, Zig ReleaseFast); Go has no EW0 (it cannot skip its checks) |
| **EW1** Go-level checks | EW0 + token names, field-content values (reject CTL except HTAB, DEL), target CTL/SP check, on every per-request field | — | hyper: `from_bytes` + `Uri` parse + new map; Go: native (`validateHeaders` + `Write`); fasthttp: `httpguts` checks before `Set`; undici: `new Request`; Netty: default validating headers; cpp-httplib: native; h2o, Beast, Zig, curl: a Go-equivalent byte loop + tchar table in their language |
| **EW2** ours | EW1 + forbidden names, singleton names, line count, authority binding, no repair | `write_request` | EW1 + their own classifier (Netty `isConnectionHeader`, fasthttp first-byte switch, hyper `contains_key` per forbidden name, Go `exclude` map) |

Report three ratios per rival: **ours@EW2 / theirs@EW1** (we do more and must still win — the headline),
ours@EW2 / theirs@EW2, and EW0 / EW0. Allocations per request are reported beside every number.

### 3.13 Task 5c — cross-language writer race

**Goal:** show, per rival, that `write_request` is faster with equal work (§3.12), on arm64 and x64, as
`bench/parsers` does for parsers. It replaces the ad-hoc `bench/writer/` comparisons with one harness.

**Rivals and versions** (pinned at the latest release, re-baselined at P1b and each release candidate, design
§16.2):

| Rival | Version | Timed API (in memory, reused sink, no socket) | Sink |
|---|---|---|---|
| hyper encoder | v1.12.0 + http v1.5.0 | **real hyper**, not an emulation: `client::conn::http1::handshake` over an in-memory duplex that captures written bytes, `SendRequest::send_request` per request (EW1: request built from bytes per op; EW0: pre-built `HeaderMap` cloned). Keep `bench/writer/hyper-emul` only as a cross-check whose output must equal real hyper's bytes | captured `Vec` |
| Go net/http | go1.27.3 | `validateHeaders` (copied, it lives in `roundTrip`) + `req.Write(bw)` | reused 8 KiB `bufio.Writer` over `io.Discard`, `Flush` per op |
| fasthttp | v1.77.0 | pooled `Request`, `Reset`, `SetRequestURI`, `SetMethod`, `Set` × n, `SetBody`, `req.Write(bw)` (EW1 adds `httpguts` checks) | `bufio.Writer` over `io.Discard` |
| libcurl | curl-8_23_0 | `Curl_h1_req_write_head` + `Curl_dynhds` in a test build (EW0/EW1); the full easy-handle path only in the G0 end-to-end race | `dynbuf`, reset not freed |
| h2o client | master `f35c735` | `build_request` (static in `http1client.c`: compiled from a vendored copy at the pin) with a pool cleared per op; EW1 adds a validator loop | pool |
| Boost.Beast | boost-1.92.0 | `request<string_body>` built per op + `serializer` + `buffer_copy` | reused `flat_buffer` |
| cpp-httplib | v0.65.0 | `Request` + `set_header` × n + `detail::write_request_line` + `detail::check_and_write_headers` + body append | `BufferStream` |
| Zig std.http | 0.17.1 | port of `sendHead` into `Writer.fixed` (`ReleaseFast`; EW1 adds a validator loop) | fixed buffer |
| undici | v8.12.4 | `new Request(...)` (internal `lib/core/request.js`) + `writeH1` with a stub socket whose `cork`/`write`/`uncork` copy into a reused `Buffer` | reused `Buffer` |
| Netty | 4.2.20.Final | `EmbeddedChannel(new HttpRequestEncoder())`, `writeOutbound(DefaultFullHttpRequest)`, `readOutbound` + release, under JMH | pooled `ByteBuf` |

**Harness shape — reuse `bench/parsers` as is:** one image per language (Rust, C/C++, Go, Zig, Node, Java),
base images by digest, rival sources by SHA-256 and tag commit, `docker run --cpus=1 --cpuset-cpus=<n>
--memory=2g --memory-swap=2g --network=none`, the runner refuses to start while another container runs, same CPU
baseline everywhere (generic Armv8-A; the x64 baseline is decided with the x64 host), release flags per design
§16.2 (Java: JIT after JMH warm-up, reported separately), rounds rotated by start position, median of per-round
ratios, JSONL with provenance (`bench/results/<date>-<arch>-dev-writers.jsonl`), and `check`/`time` modes with the
same protocol (`harness <writer> <mode> <workload> [run ms]`).

**Differential gate before timing (the writer analogue of the parser gate):** in `check` mode every harness prints,
per request, the bytes it produced; the runner parses each output with **httparse's `Request` parser as the oracle**
(as the writer note's fuzz target does) and requires: the same method, target, Content-Length and body; the
same field multiset (names compared case-insensitively — Go and fasthttp canonicalise case — values byte-exact);
no `transfer-encoding`; exactly one `content-length` where §6.5 of the writer note requires one. Byte counts of the
field section may differ only by case and order. A second gate feeds the **injection vectors**
(`tests/vectors/h1-request/`, CR/LF/NUL/DEL in values, bad names) through each rival's EW1 path and records which
it rejects, repairs or passes — reported as a table, never timed.

**Workloads** (exported once as JSON by `crates/engine-core/examples/h1_write_bench.rs`'s generator so every
language loads identical requests): `get` (`GET /data`, template only), `post1k` (`POST /data`, defaults +
`content-type: application/json` + `x-request-id` (36-character UUID) + `authorization: Bearer` + 40 characters,
1 KiB JSON body), `varied` (the 256-request ring, fixed xorshift seed), plus `h20` (20 per-request fields, 400–800 B
of headers — where validation cost dominates). Per-request inputs are fresh each op (cycled from the ring), so
construction-time validators run per request.

**Metrics:** ns per request (median, min–max over rounds), ratio to our copy floor and to ours, allocations per op
(Go `-benchmem`, JMH `gc.alloc.rate.norm`, Rust counting allocator, C `--wrap=malloc`, Node heap delta).
**Gate for Task 5c:** ours@EW2 faster than every rival@EW1 on every workload, arm64 and x64, CI lower bound per
design §16.4 once run on the reference host; `post1k` ≤ 2.3× floor.

## 4. Papers and algorithms

The SIMD note (§3) already covers the nibble LUT (Muła, simdjson §3.1.1), NEON movemask emulation, `pcmpestri` cost,
SWAR, two-stage bitmaps (simdjson stage 1, Cloudflare 2014) and Keiser–Lemire UTF-8. This section adds what changes
our design. Status per source in §9.

### 4.1 Parabix bit streams: ScanThru and MatchStar — the carry primitive for HTTP lines

- **Source (verified, primary text):** Cameron, Shermer, Shriraman, Herdy, Lin, Hull, Lin, *Bitwise Data
  Parallelism in Regular Expression Matching*, PACT 2014. Text represents a block of input as one bit stream per
  character class ("parallel bit streams"); markers advance through a class with an **addition**, whose carries move
  every marker through its run at once: `MatchStar(M, C) = (((M ∧ C) + C) ⊕ C) ∨ M` (§3), which "differs from
  ScanThru of the Parabix tool chain in that it finds all matches, not just the longest". Carries between 64-bit
  words are kept per addition by the Pablo compiler, and the paper's contribution is long-stream addition for
  blocks ≥ 256 bits. Reported: 1–4 CPU cycles per byte for full regex matching on SSE2/AVX2.
- **ScanThru** (the longest-match form we need): `ScanThru(M, C) = (M + C) ∧ ¬C` moves each marker in `M` to the
  first byte after the run of class `C` that starts at it. For an HTTP field line: markers at line starts, `C` =
  tchar → lands on the byte after the name, which must be `:`; markers after the colon, `C` = field-content → lands on
  the value's end, which must be CR, followed by LF. **All lines of a 64-byte block are validated with two additions
  and a handful of AND/OR/XOR, no per-line branch.**
- **Verified for HTTP by a differential test (2026-11-08):** `tools/scanthru_check.py` in the session scratchpad
  (written by the papers survey, re-run for this note: `ok {'ok': 129666, 'more': 33812, 'err': 36522}` in 9.4 s)
  implements a strict field-section validator over 64-bit blocks — five class bitmaps (LF, CR, `:`, tchar,
  field-content), two chained ScanThrus with `carrying_add`, an error mask `(N ∧ ¬colon) ∨ (E ∧ ¬CR) ∨ (A ⊕ LF) ∨ (S
  ∧ ¬(tchar ∨ CR))` (A = CR shifted by one, S = line starts) — and matches a scalar RFC 9112 reference on 200,000
  mutated heads **including the exact offset of the first error** and every block-boundary case (empty-line CR at
  bits 62/63/64, runs crossing blocks). ≈ 32 `u64` operations per 64-byte block on top of the five bitmaps; 5 carry
  bits between blocks.
- **Rust fit:** the bit-stream arithmetic is plain `u64` code: `u64::carrying_add` compiles on stable 1.99.1 under
  `#![forbid(unsafe_code)]` (compile-only check, 2026-11-08; the papers survey reports it stable since 1.91.0,
  unverified against the release notes). Only the **classification** (bytes → bitmaps) needs SIMD: rustc 1.99.1 does
  not auto-vectorise a safe 64-byte compare-to-bitmask loop on aarch64 or x86_64 (§5.0), and exact SWAR costs ≈ 76
  (aarch64) / 110 (x86_64) instructions per class per 64 B. So: **classification in `engine-simd`, validation and
  span extraction in safe `engine-core`.**
- **Resumability:** every check is causal (a bit depends only on bytes at or before it, plus the one-byte CR→LF
  lookahead carried as a bit), so the parser checkpoints the carry bits at the last complete block and
  re-classifies at most a 63-byte tail on the next call.

### 4.2 simdjson stage 1 in depth (verified, source at `v5.1.1`)

- Strings: `prefix_xor` by carry-less multiply (`_mm_clmulepi64_si128` on x64, `vmull_p64` on arm64;
  `include/simdjson/haswell/bitmask.h` L18–22, `arm64/bitmask.h` L17–22) turns quote bits into "inside a string"
  regions; the odd-backslash sequence logic carries across blocks. **HTTP has no quoting in the head grammar we
  validate** (quoted strings only appear inside values, which are opaque), so prefix-XOR is not needed — ScanThru is
  the right primitive (the line structure is "run of class X, then a delimiter").
- Index extraction (`src/generic/stage1/json_structural_indexer.h` `bit_indexer::write`, L82–93, `write_indexes`
  L44–76): `tzcnt` + clear-lowest-bit, writing a fixed number of indexes unconditionally, then the rest only when the
  popcount is larger — no data-dependent branch per index. For HTTP: four bitmaps with one bit per line (name start,
  colon, value start, value end) → four independent index arrays → spans, with no dependent chain from one span to
  the next.
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
- Our scalar parser already runs ≈ 1 cycle per byte; bitmaps run ≈ 0.3–0.5 (§5). Neither FSM form wins for a
  grammar this regular. (Teddy/shufti/truffle match literal sets and byte classes — the classification side, already
  covered by the nibble LUT.)

### 4.5 SIMD CSV and line scanning

- simdcsv (Langdale, README at the pin, verified): simdjson's quote handling plus comma/CR-LF bitmaps over padded,
  cache-line-aligned buffers; "the only new wrinkle … is the fact that we have to detect a CR followed by a LF" —
  the same `CR ∧ (LF ≫ 1)` pairing we need, with the cross-block bit. No HTTP-specific idea beyond that.
- Numbers stay scalar: Content-Length and chunk sizes are 1–7 digits in practice; SWAR 8-digit parsing (Muła, Lemire)
  and a hex table for chunk sizes are enough (ideas.md).

### 4.6 Prior art on SIMD HTTP parsing (none peer-reviewed)

No peer-reviewed paper on SIMD HTTP/1 parsing or serialisation was found (survey of arXiv, ACM DL, USENIX, IEEE,
2026-11-08). Industrial work: Cloudflare's 2014 picohttpparser AVX2 bitmaps (SIMD note); QUICHE Balsa (Envoy's
HTTP/1 parser) is not vectorised; Bun's client uses picohttpparser; uWS uses SWAR + a fence. Small open-source
projects (cursed-http, simdhttp, xocomil, simdzone for DNS zone files) report SIMD parsers; their numbers are
unverified and listed only in `ideas.md`.

## 5. Our parser: candidates

### 5.0 Where the time goes today (from measured data; no new runs)

- **The split is a second pass.** `blazehttp+fields` costs 1.6–2.0× plain `blazehttp` (plain / with-fields =
  0.50–0.63, parser note §8.6): the bench's `split_fields` re-reads every field byte (`memchr` CR, `position` `:`, OWS
  trim; `bench/parsers/rust/src/engines.rs` L78–103) although `value()` already held all four boundaries
  (§2.14).
- **Projection once spans are fused** (plain ratios × an assumed 1.05 for four stores per field; not a
  measurement): vs httparse 0.74 / 0.93 / 0.91 / **1.13** / 0.90 (1k, 1k-h20, 1k × 64, 1k-h20 × 64, mixed); vs
  picohttpparser ≈ 0.75–0.84. Every rival in §2 except httparse on the 20-field ring would be behind us; that cell
  needs the loop-structure work (P-B) or SIMD (P-C/P-D).
- **Safe-Rust bitmaps do not pay** (compile-only check, 2026-11-08, scratch crate `asmcheck`, rustc 1.99.1, fat LTO,
  `codegen-units = 1`, `--emit asm` for `aarch64-apple-darwin -C target-cpu=generic` and `x86_64-unknown-linux-gnu`
  baseline and `x86-64-v3`): a 64-byte `fold(|m, (i, &b)| m | (u64::from(b == c) << i))` stays a scalar byte loop
  (`ldrb`, `cmp`, `cset`, `lsl`, `orr` per byte) on every target; a 16-byte-chunk form is unrolled scalar; an exact
  SWAR equality mask with a multiply gather costs 76 (aarch64) / 110 (x86_64) instructions per class per 64 B. Five
  classes ≈ 5–7 instructions per byte — slower than the current ≈ 1 cycle per byte. **Bitmaps need explicit SIMD
  (`engine-simd`).** A branch-free `fold` with table lookups compiles to a tight loop with no data-dependent branch
  (`ldrb`, `ldrb [table]`, `and`, `subs`, `b.ne`).

### 5.1 The span output (shared by every candidate)

```rust
/// One field line as offsets from the start of its head (u16: a head is ≤ 64 KiB, design §5.2).
#[derive(Clone, Copy)]
pub struct FieldSpan { pub name_start: u16, pub name_end: u16, pub value_start: u16, pub value_end: u16 }
const _: () = assert!(H1_LIMITS.max_head_bytes <= u16::MAX as usize + 1);
```
- The parser owns `[FieldSpan; MAX_HEADER_FIELDS]` (800 B; reused for every response on the connection) and
  `ResponseHead` gains `fields: u16` (count). Spans are written when a line's CRLF is confirmed, so a `Partial`
  return never exposes half a line, and a resumed call continues writing at `fields`.
- `value_start` skips leading OWS; `value_end` trims trailing OWS backwards from the CR (both SP/HTAB only; obs-text
  never trimmed) — the httparse/pico output, and the bench's `split_fields`.
- **Our own idea:** the array is laid out to cross the napi boundary as a `Uint16Array` (4 × u16 per field) so the JS
  layer builds `headers` without scanning the bytes again (P2 hand-off; the gain on the JS side is unmeasured).
- Offsets are relative to the head's own start; after an `Interim` the caller drops the consumed bytes and the
  final head starts at 0 again (unchanged contract).

### 5.2 Candidate P-A — "spans in G" (minimal change)

Keep Design G (parser note §6.3) unchanged; in `value()` at the confirmed CRLF, write the span (four stores + the
OWS trims) and increment `fields`.

- **Pseudocode:** `Value: e = pos + value_len(rest); [CR, LF, ..] at e → vs = skip_ows(colon+1, e); ve =
  trim_ows_back(vs, e); spans[fields-1] = (line_start, colon, vs, ve); facts.field(...)`.
- **Time:** O(head bytes); per field + 4 stores + two OWS checks (usually 1 byte each). **Space:** +800 B per
  connection. **Branches per byte:** unchanged ≈ 0.6 (names ≈ 1 per byte, values ≈ 1 per 8 bytes, ≈ 8 per line for
  dispatch, CRLF and facts).
- **Better than httparse because:** no restart from 0 (linear under a trickle), facts fused at the CRLF (httparse +
  hyper pay a second pass over the `Header` array), exact `trailing_zeros` stop.
- **Risks:** none new for security (spans are derived from bytes the parser has already validated); partial input
  unaffected (spans written only after the CRLF). Does not fix the 20-field ring (projection 1.13× httparse).

### 5.3 Candidate P-B — "line kernel" (scalar, safe Rust; our own design)

The references' common structure (§2.14: one loop body per line, cursor in locals) plus two ideas of ours:
decoupling **locate** (the critical path) from **validate** (off the critical path), and a **stop-set** search
for names.

```text
fn scan_lines(window, mut st: LineState /* pos, line_start, fields, facts */) -> Outcome {
    loop {                                   // locals only; nothing written to self inside the loop
        let rest = window[st.pos..];
        match rest { [CR, LF, ..] => return HeadEnd(st.pos + 2),  [] | [CR] => return More(st),
                     [CR, ..] => return Err(Malformed), _ => {} }
        // 1. locate the colon: first byte in STOP = {≤ 0x20, ≥ 0x7F, ':'} (SWAR, lowest flag exact)
        let n = stop_len(rest);                      // 8 B per step; CR/LF/SP/HTAB/NUL/DEL/obs-text/':' all stop
        if n == rest.len() { return More(st) }       // partial name (validated so far: see "partial input")
        if rest[n] != b':' || n == 0 { return Err(Malformed) }
        // 2. validate the name off the critical path: branch-free table fold, result checked once per line
        let name_ok = all_tchar(&rest[..n]);         // catches "(),/;<=>?@[\]{} and '"' which STOP lets through
        // 3. value: today's SWAR value_len (stops at the first non-field-content byte, which must be CR)
        let e = n + 1 + value_len(&rest[n + 1..]);
        match rest.get(e..) { Some([CR, LF, ..]) => {}, Some([] | [CR]) => return More(st), _ => return Err(Malformed) }
        if !name_ok { return Err(Malformed) }
        // 4. emit span + facts (name dispatch by length + folded-word compare, §6.1, shared module)
        st.emit(line_start = st.pos, colon = st.pos + n, value_end = st.pos + e)?;   // field count, CL/TE/Connection
        st.pos += e + 2;
    }
}
```
- **Equivalence with G (proof sketch):** STOP ⊂ not-tchar, so "first byte outside tchar is `:` and the name is
  non-empty" ⇔ "first STOP byte is `:`, the name is non-empty and every byte before it is tchar". On partial input
  G rejects a delimiter such as `(` as soon as it arrives while P-B would wait for a STOP byte, so P-B validates the
  newly scanned name bytes before returning `More` (incrementally, `validated_to`, so 1-byte feeding stays linear)
  — whole and split parsing still agree byte for byte (parser note §6.3 invariant). The scalar `value_len` is kept
  as is.
- **Phase machine only at the edges:** `step()` enters `scan_lines` with locals and writes `phase/pos/line_start/
  colon` back only on `More`; the status line keeps its 12-byte prefix check (a single `u64` + `u32` compare is
  equivalent for the complete prefix and is httparse's trick). `checked_add` chains inside the loop are replaced by
  offsets bounded once per call (`window.len() ≤ 65,536`, so `usize` arithmetic on offsets < window length cannot
  overflow — one assert at entry instead of ≈ 8 per field; the lint `arithmetic_side_effects` then needs
  `wrapping_add` with that bound as the documented reason, the same pattern as `value_len`).
- **Padded window (uWS fence, safe form):** if the connection's pooled read buffer keeps ≥ 8 initialised slack
  bytes past the data with a sentinel CR, the SWAR loops run over `as_chunks::<8>()` with no scalar tail, and
  results are clamped to the received length. This changes the parser API (`parse(buf, padded_len)`); measured
  separately.
- **Time:** O(head bytes), each byte loaded once by a locate scan and once by the validate fold (both in L1).
  **Space:** + `validated_to` (2 B). **Branches per byte:** names ≈ 1 per 8 bytes (+ 1 exit), values ≈ 1 per 8
  bytes (+ 1 exit), ≈ 5 per line fixed → ≈ 0.25–0.3 per byte (from ≈ 0.6). **Data-dependent exits per line:** 2
  (colon search, value search) as today, but each loop has 1–3 trips instead of 6–27.
- **Better than httparse because:** same one-loop-per-line shape, but resumable, facts fused, exact `tzcnt` stops,
  and on x86 no per-call runtime dispatch (SWAR inlines). On arm64 its NEON name LUT is 16 B per step against our
  8 B STOP search + table fold: P-B alone may tie it on the 20-field ring (unverified).
- **Risks:** (1) security: STOP must contain every byte outside tchar that could end a line or hide a separator
  (CR, LF, SP, HTAB, NUL, CTLs, DEL, ≥ 0x80) — it does by construction (≤ 0x20 ∪ ≥ 0x7F), and the table fold is the
  final word; exhaustive 256 × lane tests and the differential fuzz target (parser note §9.7) against G; (2) no
  `unsafe`; (3) partial input as above; (4) clippy rule 9 (function ≤ 60 lines): `scan_lines` splits into
  `locate_name`, `scan_value`, `emit` with `#[inline]`, measured.

### 5.4 Candidate P-C — stage-1 bitmaps + per-line `tzcnt` walk (P1a, `engine-simd`)

The SIMD note's lead candidate (§3.6, §6.1), now with spans:
- **Stage 1** (`engine-simd`, `unsafe` inside, safe API): `classify_block(&[u8; 64]) -> BlockMasks { not_tchar,
  not_field_content, cr, lf, colon, ows }` — NEON `tbl` nibble LUT for tchar + range compares, `vpaddq` ×3 mask
  reduction; x86 SSE2 (baseline) for the range classes and SSSE3/AVX2 `pshufb` for tchar, dispatched once per call.
- **Stage 2** (safe, `engine-core`): per line `name_end = next_set(not_tchar, line_start)`, check `colon` bit,
  `value_start = next_clear(ows, colon + 1)`, `value_end = next_set(not_field_content, colon + 1)`, check `cr` and
  `lf` bits, span, facts. `next_set(m, p) = p + (m >> (p % 64)).trailing_zeros()` with a block refill.
- **Time:** stage 1 ≈ 90–100 vector ops per 64 B (≈ 1.5 per byte, throughput-bound); stage 2 ≈ 15 scalar ops and 3
  dependent `tzcnt` per line. **Branches per byte:** ≈ 0.13 (per line ≈ 5, per block ≈ 1). **Space:** six `u64`
  for the current block (lazy) or per window.
- **Measured precedent (dev, SIMD note §4):** 0.42× the scalar chain on varied heads, 1.04× on one repeated head
  (whole head); lazy per block 0.88× repeated.
- **Risks:** a dependent `tzcnt` chain per line remains (latency ≈ 3–4 × 3 cycles per line); dispatch must be per
  call, not per span (SIMD note §6.1); trickle: classify only complete 64-byte blocks, scalar for the tail (keeps
  1-byte feeds linear and cheap).

### 5.5 Candidate P-D — bit-stream line validator (P1a; our own design on Parabix ScanThru)

Stage 1 as P-C (five classes: LF, CR, `:`, tchar, field-content; OWS optional). Stage 2 replaces the per-line walk
with **block-parallel validation** (§4.1, verified by the differential check) and **index extraction**:

```text
carry: lf_in, cr_in, name_c, colon_c, value_c          // 5 bits, the whole resumable state of the validator
for each complete 64-B block (base):
    S  = (LF << 1) | lf_in                              // line starts
    A  = (CR << 1) | cr_in                              // byte after a CR
    (s1, name_c)  = (S & T).carrying_add(T, name_c);   N = s1 & !T        // ScanThru names → byte after the name
    P  = (N << 1) | colon_c                             // byte after that (the colon's successor)
    (s2, value_c) = P.carrying_add(V, value_c);        E = s2 & !V        // ScanThru values → byte after the value
    err = (N & !COLON) | (E & !CR) | (A ^ LF) | (S & !(T | CR))
    H  = S & CR                                         // an empty line: end of head (limit masks bits after it)
    err &= mask_up_to(H) & received
    if err != 0 { hand the block to the scalar parser (it reports the first error exactly) }
    flatten(S & !CR) → name_start[]; flatten(N) → colon[]; flatten(P) → value_from[]; flatten(E) → value_end[]
    // four independent index streams (simdjson bit_indexer: fixed unconditional writes, popcount advance)
    lf_in = LF >> 63; cr_in = CR >> 63; colon_c = N >> 63
spans[k] = (name_start[k], colon[k], skip_ows(value_from[k]), trim_ows(value_end[k]));   facts per field (scalar)
```
- **What it validates:** exactly §5.2's field-line grammar: token names ending at `:`, field-content values ending
  at CR, CR always followed by LF and LF always preceded by CR (`A ⊕ LF`), no line starting with a non-tchar (obs-fold,
  leading SP, `:`), end of head at `CRLF CRLF`. The status line stays scalar (12-byte prefix + reason as one
  field-content run).
- **Time:** stage 1 ≈ 90–100 vector ops per 64 B; validator ≈ 32 `u64` ops per 64 B; flatten ≈ 4 × (popcount +
  8 unconditional writes) per block; per field ≈ 6 scalar ops for OWS and the span; facts by length switch. ≈ 2.5–3
  ops per byte, almost all independent → estimated ≈ 0.4–0.6 cycles per byte (unverified). **Branches per byte:**
  ≈ 0.05 (per block: error test, flatten overflow, loop; per field: the facts length switch). **No dependent chain
  between lines**: lines in a block are validated simultaneously. **Space:** 5 carry bits + four ≤ 16-entry index
  buffers on the stack.
- **Better than httparse because:** httparse is a serial per-span chain (find name end → find value end → next
  line), 16 B per step, restart from 0. P-D has no per-span dependency at all, and its cost does not depend on the
  branch predictor learning the head (the SIMD note's varied-head finding).
- **Our own additions** beyond Parabix/simdjson: (1) error handling by scalar re-run (§4.3), so the fast path is
  only an acceptance filter and the scalar parser keeps all error semantics; (2) causal checks → resume by carry
  checkpoint at the last complete block, re-classify ≤ 63 bytes; (3) spans for JS as `u16` offsets straight from the
  four index streams.
- **Risks:** (1) security — the acceptance filter must never accept what the scalar parser rejects: the five
  classes come from the `[bool; 256]` tables (single source of truth), exhaustive 256 × lane × alignment tests per
  `classify_block`, the Python reference ported as a Rust test, and the fuzz target runs SIMD vs scalar vs httparse;
  (2) `unsafe` only in `classify_block` (fixed-size `&[u8; 64]` loads: in-bounds by type; feature detection in the
  dispatcher); the validator and flatten are safe `u64` code in `engine-core`; (3) partial input: blocks are
  processed only when complete (or when the head ends inside the block); the tail and trickles go through the
  scalar parser — the switch point is a block boundary, so state is the 5 carry bits plus the scalar parser's state
  at that offset; (4) body bytes: a block straddling the head end loads up to 63 body bytes into registers (masked
  out; never interpreted) — documented change to the parser note's "body bytes are never examined", which was a
  semantic rule; (5) facts must still see every field: the per-field scalar dispatch runs on the extracted spans
  (O(fields)).

### 5.6 Rejected

- **Per-span 16-byte SIMD (httparse shape):** slower in our dependent chain (SIMD note §4.2).
- **SHENG / data-parallel FSM:** dependent shuffle per byte (§4.4).
- **Safe-Rust bitmaps in `engine-core`:** no auto-vectorisation (§5.0).
- **Name hashing during the scan (nginx):** a serial multiply per byte; a length switch is cheaper for 3 names.

### 5.7 Measurement plan (parser)

- **Equal work:** every row produces status, version, head length, per-field name/value spans with OWS trimmed,
  framing facts and keep-alive (the bench README's `+fields` rows). Once P-A lands, `blazehttp+fields` is retired:
  our row emits spans itself; the plain row stays as "facts only" for Zig.
- **Workloads:** `1k` (3 fields, 109 B), `1k-h20` (20 fields, 779 B), `ring64` (both, 64 digit variants),
  `mixed` (64 heads over the three fixtures), `trickle` (feeds (a)–(d), 1 byte per call, ≤ 1 ms each), **plus a
  `varied-256` ring** (names shuffled, value lengths 0.5–1.5×; the SIMD note's ring) because `ring64` varies only
  digits and lets the predictor learn the lengths.
- **Arches:** arm64 (reference Mac, Docker, nightly window, generic Armv8-A) and x64 (VM; baseline decided with the
  x64 host, every rival built for the same baseline).
- **Tools:** `crates/engine-core/examples/h1_parse_bench.rs` for same-container A/B of P-A vs P-B variants
  (alternated binaries, ≥ 4 rounds, as the journal does); `bench/parsers` for the cross-language race (+ fasthttp,
  nginx rows, §2.13); P1a criterion for P-C vs P-D. Secondary metrics on the x64 VM: `perf stat -e
  instructions,branches,branch-misses` per head (validates the branches-per-byte claims); `--emit asm` audit of each
  kernel (no panic paths, §8.5 of the parser note).
- **Gates:** with fields, faster than every rival on every workload and both arches (design §16.4 statistics on the
  reference host); P1a ≤ 0.85× httparse on `1k-h20` (parser note §8.4).

## 6. Our writer: candidates

Target: `post1k` from ≈ 75–91 ns to ≤ 2.3× the ≈ 36 ns floor (≤ 83 ns) with margin, and every rival@EW1 beaten
(§3.12). The ablation (§3 intro) says where: name classification ≈ 15 ns, delta scan ≈ 14–19 ns, other checks
≈ 8–10 ns.

### 6.1 Building block: one known-name classifier, shared by parser and writer

`h1/field_name.rs` (a shared module, design §13.1 rule 2): `enum KnownName { ContentLength, TransferEncoding,
Connection, Host, Te, Expect, Upgrade, KeepAlive, ProxyConnection, ProxyAuthorization, Authorization, Cookie,
ContentType, UserAgent }` and `fn classify(name: &[u8], has_underscore: bool) -> Option<(KnownName, Spelling)>`.
- **Algorithm:** `match name.len()` (jump table) → for lengths with several candidates, one discriminating byte
  (the last byte, h2o: at length 10 `n`/`e`/`t`; at 6 `e`/`t`) → **compare 1–3 overlapping little-endian `u64`
  words OR-ed with `0x2020…20`** against `const` canonical words (`u64::from_le_bytes(name[..8].try_into()?)` and
  `name[len-8..]`; names under 8 bytes zero-extended).
- **Exactness:** names are validated tokens before classification; canonical names contain only `a–z` and `-`;
  among tchar bytes, `|0x20` maps only `A–Z`/`a–z` into `a–z` (`^`→`~` and `_`→DEL land outside the canonical
  bytes), so the folded compare ⇔ ASCII case-insensitive equality. Underscore spellings (writer note W10/X9) go
  through the slow path only when the name scan saw a `_` (a flag OR-ed during the scan, as nginx hashes during its
  scan), so the common path never pays for them.
- **Cost:** a switch, ≤ 1 byte test, 1–3 word compares — ≈ 1–2 ns per name instead of a per-byte
  `eq_ignore_ascii_case` per candidate. The parser uses it for its three framing names (replacing `facts.rs`'s
  length switch + `eq_ignore_ascii_case`).

### 6.2 Candidate W-A — scalar, safe Rust (Task 5b, P0)

1. **Names:** the parser's STOP search + table fold (§5.3) — the same primitive in `grammar.rs` for both (DRY),
   with the `_` flag folded into a `[u8; 256]` flags table (tchar bit, underscore bit) so the fold yields both.
2. **Classification:** §6.1.
3. **Values:** today's SWAR `value_len`, then `": "` before and CRLF after checked with slice patterns.
4. **Other checks:** the authority binding by an integer id the connection receives at checkout instead of a
   10–30-byte compare per request (Task 6 hand-off; −2–4 ns, unverified); limits checked once on the summed
   length.
- **Time:** O(target + delta) validation + O(head + body) copy. **Space:** 0 extra. **Branches per delta byte:**
  ≈ 0.25 (from ≈ 0.6). **Allocations:** 0.
- **Expected:** −8 to −12 ns (classification) and −3 to −5 ns (names) → `post1k` ≈ 60–70 ns ≈ 1.7–1.9× floor
  (estimate, unverified).
- **Better than hyper/Go because:** the same validation as EW1 plus our EW2 checks with no allocation, no map, no
  `fmt`, no second pass; classification costs about one word compare per name where hyper pays a map entry and
  Go a map hash.
- **Risks:** classification must be exhaustively tested against the current `classify` (every special name in
  every case and underscore spelling, every length 0–64, random tokens); the line-count check (X3) stays.

### 6.3 Candidate W-B — validate against the writer's own layout (scalar; our design from the hint idea)

The JS layer already knows each line's name and value length. It passes them as **untrusted hints**
(`[u16; 2]` per line, alongside `header_lines`). Rust then needs no search at all:
```text
expect_cr_lf = 0; pos = 0
for (name_len, value_len) in hints:                     // O(lines)
    check block[pos + name_len .. +2] == ": "  and  block[pos + name_len + 2 + value_len .. +2] == "\r\n"
    classify(block[pos .. pos + name_len])               // §6.1, names validated by the fold below
    pos += name_len + 2 + value_len + 2
check pos == block.len()
one branch-free fold over the whole block:              // auto-vectorises (arithmetic compares, no table)
    bad |= !(b ≥ 0x20 && b ≠ 0x7F || b == HTAB || b == CR || b == LF);  cr += (b == CR);  lf += (b == LF)
accept iff bad == 0 && cr == lines && lf == lines        // CR/LF exist only at the hinted line ends
names: tchar fold over exactly name_len bytes per line   // table, branch-free
```
- **Injection safety (from the papers survey, verified by reasoning):** grammar validation alone is not enough — a
  value carrying `\r\nx-forwarded-for: 1.2.3.4` is well-formed line by line. Here every CR and LF must sit exactly
  where the layout says (counts equal and each line end checked), and every `: ` where the layout says; a smuggled
  line changes `cr` and fails. This is the line-count rule (X3) made exact.
- **Time:** O(delta) with no data-dependent loop exits: one vectorisable fold (the papers survey and the writers
  survey both note http's `try_from_generic` is written this way "so the loop vectorizes") + O(lines) constant-time
  checks. **Branches per byte:** ≈ 0 in the fold; ≈ 6 per line. **Space:** the hints (4 B per line, ≤ 100 lines).
- **Expected:** removes most of the 14–19 ns delta scan → `post1k` ≈ 55–60 ns ≈ 1.5–1.7× floor with W-A's
  classification (estimate).
- **Risks:** a wire-format change (P2 contract: hints next to the delta); hints are untrusted, so every check above
  is mandatory (fuzz target: random hints × random blocks vs the current `scan_fields` oracle — must agree on accept
  and on reject).

### 6.4 Candidate W-C — validate-once interned names (cross-layer)

As undici's lowercase record, h2o's tokens and hyper's `StandardHeader`: the JS layer sends a small integer id for
each **known** name (the §6.1 set plus common ones such as `accept`, `content-encoding`); Rust writes the canonical
bytes from its own static table and takes the class from it — no name scan or classification for those lines.
Unknown names still go through W-A/W-B. Constant per-client header *sets* already ride in the template (validated
once); this extends "validate once" to names in the per-request delta. **Risk:** the id table is part of the
generated spec (`policy/spec.json`) so JS and Rust cannot drift; a JS bug sending `content-length` as raw bytes is
still caught by classification.

### 6.5 Candidate W-D — SIMD delta validation (P1a, `engine-simd`)

The delta is contiguous: classify it with P-D's `classify_block` and validate all lines of a block with the ScanThru
validator in the writer's stricter form (separator exactly `": "`: `N` must be a colon **and** `N << 1` a SP; values
from `N << 2`). With W-B's layout, the check becomes bitmap equality: `cr == expected_line_ends`, `colon ==
expected_colons` — the papers survey's injection rule in its cheapest form. Optionally **fuse the copy**: the
`engine-simd` API `copy_classified(dst: &mut Vec<u8>, src) -> BlockMasks` loads each 64-byte block once, stores it
into `dst` and classifies it (safe API; the uninitialised-capacity write is `unsafe` inside `engine-simd` only — in
safe Rust it would need `set_len`, which is `unsafe`, or a per-word `extend_from_slice` with a capacity check). At
≈ 100–400 B the delta is in L1 after the copy anyway, so fusion is measured, not assumed. **Expected:** validation
≈ 5–10 ns for a 100–400 B delta (estimate).

### 6.6 Rejected

- Fused copy + validate in safe `engine-core` (needs `unsafe` `set_len` or per-word capacity checks).
- Hashing names (a multiply chain per byte costs more than 1–3 word compares).
- A per-client cache of validated delta blocks keyed by content (hashing ≈ the validation it saves; YAGNI).
- `writev` of head + small body (journal: equal up to 4 KiB, worse under TLS; writer note §8.3).

### 6.7 Measurement plan (writer)

- **Workloads:** `get`, `post1k`, `varied` (256-ring), plus `h20` (20 per-request fields) where validation
  dominates; arm64 and x64.
- **A/B:** `crates/engine-core/examples/h1_write_bench.rs` (writer vs copy floor, same container, alternated
  binaries, ≥ 4 rounds); ablations per check as in the journal (classification off, delta scan off, authority off).
- **Race:** Task 5c (§3.13): ours@EW2 vs every rival@EW1 and @EW2, EW0 vs EW0, allocations per op.
- **Gates:** `post1k` ≤ 2.3× floor (provisional, writer note §8.4); 0 allocations (`tests/h1_writer_alloc.rs`);
  faster than every rival@EW1 on every workload.

## 7. Measurement plan (summary)

| What | Where | Workloads | Arches | Method |
|---|---|---|---|---|
| Parser candidates P-A/P-B | `h1_parse_bench` example (A/B), then `bench/parsers` | 1k, 1k-h20, ring64, mixed, trickle, + varied-256 | arm64 ref (nightly window), x64 VM | §5.7 |
| Parser P-C/P-D | P1a criterion, then `bench/parsers` | same | both | §5.7 |
| Writer W-A/W-B/W-C | `h1_write_bench` example (A/B + ablations) | get, post1k, varied, + h20 | both | §6.7 |
| Writer W-D | P1a criterion | same | both | §6.7 |
| Cross-language writer race | Task 5c harness (§3.13) | get, post1k, varied, h20 | both | EW0/EW1/EW2 |
| Branch claims | `perf stat` on the x64 VM | 1k-h20, varied-256, post1k | x64 | instructions, branches, branch-misses per op |

Every kept change goes to `docs/perf/journal.md` with numbers; anything not measured stays in `ideas.md`. No number
in this note is a measurement made for it (the only runs were the compile-only asm check and the Python
differential check).

## 8. Decision table

Ranked within each component by expected gain per risk (all gains are estimates until §7 runs).

| Rank | Candidate | Component | Expected effect (unverified) | Risk | Build in |
|---|---|---|---|---|---|
| 1 | **P-A spans in G** | parser | removes the second pass: ratio vs httparse with fields from 1.16–2.02 to ≈ 0.74–1.13 | none new | **Task 4c** (first step) |
| 2 | **P-B line kernel** (locals, STOP search + table fold, `u64` prefix, shared classifier) | parser | ≈ −15–25% scalar; target: beat httparse on the 20-field ring too | medium (equivalence on partial input; fuzz) | **Task 4c** (second step, kept only if measured faster) |
| 3 | P-B padded window (fence) | parser | removes tails; small | API change (`padded_len`) | Task 4c option, measured; else P1a |
| 4 | **P-D bit-stream validator + flatten** | parser | ≈ 0.4–0.6 cycles per byte; ≤ 0.85× httparse with margin; predictor-independent | `unsafe` only in `classify_block`; acceptance-only fast path | **P1a** (lead) |
| 5 | P-C stage-1 + `tzcnt` walk | parser | measured 0.42× on varied heads (dev) | lower complexity than P-D | P1a (fallback / A/B baseline for P-D) |
| 1 | **W-A known-name classifier + shared STOP search** | writer | `post1k` ≈ 1.7–1.9× floor | low | **Task 5b** |
| 2 | **W-B layout hints + one vectorisable fold** | writer | `post1k` ≈ 1.5–1.7× floor | wire-format change (P2), hints untrusted | **Task 5b** if the P2 contract change is accepted; else P2 |
| 3 | W-C interned name ids | writer | known names cost ≈ 0 | spec-generated table; JS/Rust drift | P2 |
| 4 | **W-D SIMD delta validation (+ optional fused copy)** | writer | validation ≈ 5–10 ns | `unsafe` only in `engine-simd` | **P1a** |
| — | Task 5c cross-language writer race | writer | proves "every rival" | none | **Task 5c** (after 5b) |
| — | Add fasthttp + nginx rows to `bench/parsers` | parser race | closes "every Go / C server parser" | none | with Task 4c |

**What each task builds:**
- **Task 4c (parser, scalar, P0):** P-A first (spans in the scan, `FieldSpan` u16 ×4, OWS trimmed), then P-B
  (one loop body per line on locals, STOP search + branch-free tchar fold, `u64` status-prefix compare, shared
  `field_name.rs` classifier), each kept only if measured faster on the §5.7 workloads; add the `varied-256` ring and
  the fasthttp/nginx rows to `bench/parsers`.
- **Task 5b (writer, P0):** W-A (shared classifier, STOP search, authority by id with Task 6), and W-B if the P2
  contract change (per-line hints) is accepted by the controller; `post1k` ≤ 2.3× floor.
- **Task 5c:** the cross-language writer race (§3.13).
- **P1a (SIMD):** `engine-simd::classify_block` (NEON; SSE2 + SSSE3/AVX2), then P-D for the parser (P-C as its
  A/B baseline) and W-D for the writer, both with the scalar path as error oracle and tail handler.

## 9. Sources

All accessed 2026-11-08. Source code: verified at the commits of §1 (line numbers in §2–§3). Rust: verified by
compile on 1.99.1.

| # | Source | Link | Status |
|---|---|---|---|
| 1 | Cameron, Shermer, Shriraman, Herdy, Lin, Hull, Lin, *Bitwise Data Parallelism in Regular Expression Matching*, PACT 2014 | https://www2.cs.sfu.ca/~ashriram/papers/2014_PACT_GREP.pdf | verified (primary text: MatchStar, ScanThru, long-stream addition, 1–4 cycles/byte) |
| 2 | Mytkowicz, Musuvathi, Schulte, *Data-Parallel Finite-State Machines*, ASPLOS 2014, pp. 529–542 | (PDF fetched by the papers survey; cited in #1) | verified (primary text, abstract and shuffle mechanism) |
| 3 | Langdale, Lemire, *Parsing Gigabytes of JSON per Second*, VLDB J. 2019 | https://arxiv.org/abs/1902.08318 | verified (SIMD note) |
| 4 | simdjson `v5.1.1` `bitmask.h` (prefix_xor via clmul), `json_structural_indexer.h` (`bit_indexer`) | https://github.com/simdjson/simdjson/tree/40a686ab89dc5ee58ec10d0514dd5c99abbc472b | verified (source) |
| 5 | simdutf `v9.3.3` `src/generic/ascii_validation.h` (validate, then locate on error) | https://github.com/simdutf/simdutf/blob/59589cd3df6823cbfdbd253cde3377e7c3e1fe9b/src/generic/ascii_validation.h | verified (source) |
| 6 | Vectorscan `5.4.13` `src/nfa/sheng_impl.h` (SHENG pshufb DFA) | https://github.com/VectorCamp/vectorscan/blob/acd7363aadea43da9c5246542d9969db843dd132/src/nfa/sheng_impl.h | verified (source) |
| 7 | Wang et al., *Hyperscan*, NSDI 2019 | https://www.usenix.org/system/files/nsdi19-wang-xiang.pdf | verified (SIMD note) |
| 8 | Langdale, simdcsv README | https://github.com/geofflangdale/simdcsv/blob/a9494a85c83525bb42ba368d8f9bcf3acf53a8d1/README.md | verified (text) |
| 9 | Krasnov (Cloudflare), *Improving PicoHTTPParser further with AVX2*, 2014 | https://blog.cloudflare.com/improving-picohttpparser-further-with-avx2/ | verified (SIMD note); numbers unverified for us |
| 10 | uops.info `PCMPESTRI`, `PSHUFB`, `PMOVMSKB` | https://uops.info/ | verified (SIMD note) |
| 11 | Rust std 1.99.1: `u64::carrying_add`, `slice::as_chunks`, `Vec::set_len` (unsafe), `Vec::spare_capacity_mut` | https://doc.rust-lang.org/std/ | `carrying_add` and `as_chunks` verified by compile under `forbid(unsafe_code)`; `set_len` unsafe and `spare_capacity_mut` returning `MaybeUninit` from the std page index (verified) |
| 12 | Rust Reference, `target_feature` (no inlining into callers without the feature) | https://doc.rust-lang.org/reference/attributes/codegen.html | verified (SIMD note) |
| 13 | Compile-only asm check (scratch crate `asmcheck`, rustc 1.99.1, aarch64 generic, x86-64 baseline and v3) | session scratchpad `h1-research/asmcheck` | verified (run 2026-11-08) |
| 14 | ScanThru differential check `tools/scanthru_check.py` (200,000 mutated heads + boundary sweep) | session scratchpad | verified (run 2026-11-08) |
| 15 | Prior art: cursed-http, simdhttp, xocomil, simdzone (SIMD parsers); QUICHE Balsa | reported by the papers survey | unverified → `ideas.md` |
| 16 | Every parser and writer of §1 | §1 link bases | verified (source at the pins) |
