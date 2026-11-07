# `ResponseParser`: research note (Task 4)

Design §5.1 (component), §5.2 (rules), §6 (learning from proven code), §7 (speed), §11.5 (O(head bytes), offsets
only). Access date for every link: **2026-11-06**. "Verified" means checked against the pinned source, an official
doc page, or a measurement run for this note; "unverified" ideas are listed only in `docs/perf/ideas.md`.

## 1. Summary and decision

**Decision (final): a fused span scanner that resumes at any byte (Design G below).** The status line's fixed
12-byte prefix (`HTTP/1.x SP DDD`) is checked byte by byte as it arrives, then the reason is scanned as a
`FIELD_CONTENT` run. Field lines are scanned in one pass: a `TCHAR` run for the name, the colon, then a
`FIELD_CONTENT` run for the value (safe-Rust SWAR, 8 bytes per step) that stops at the first byte outside the class,
which must be the CR of a CRLF. A partial line is resumed where the scan stopped, using a sub-state per line part and
three offsets (`pos`, `line_start`, `colon`); framing facts are folded into two small enums when each line completes.
No allocation, no header map, no new dependency; each head byte is examined O(1) times (once by the span scans; the
≤ 12-byte status prefix is re-checked on each call while it is incomplete).

**Why G (changed from the first draft, commit `5b04853`):** that draft chose Design C (find each LF with `memchr`,
then validate the line). Its ranking came from macOS runs built with rustc's default target CPU for
`aarch64-apple-darwin` (`apple-m1`), which makes httparse ≈ 1.6× slower than the same code built for a generic
`aarch64` CPU (§8.2). On Linux arm64 (the reference host's container, generic target CPU) C + SWAR is 1.05–1.20×
**slower** than httparse on 20-header heads. Design G is **at parity** with httparse on 20-header heads (0.92–0.96×)
and 0.86× on the 3-header head, while staying linear under 1-byte feeding (httparse is quadratic: 0.26 ms vs 67 ms for
a 60 KB head fed byte by byte). G is faster than C on every head in every build measured.

**Review round 1 (2026-11-06) changed the design detail, not the decision:** `memchr` is dropped (the status line is
scanned like a field line, which also rejects non-HTTP bytes at their first byte, §6.3); framing and `Connection`
facts are enums, not five bools; a terminal `Failed` phase makes "parse after an error" a contract violation; a 204/304
carrying framing headers is not reused; a 1xx carrying `Transfer-Encoding` or a non-zero `Content-Length` is
`SmugglingRisk`; `Connection: close` in a 1xx head is sticky for the exchange; EOF is classified by bytes received
for the whole exchange. The accepted rule changes are now in the design doc, marked **[author's call]** (§10).

**Unchanged:** the §5.2 rule set as amended, the interface (§6.6), the files (`h1/grammar.rs` primitives swapped by
`engine-simd` in P1a: `token_len` and `value_len`, the span-length shape httparse's NEON/AVX2 code uses).

Security research and upstream suites (§4, §9) found no response-side case the amended rules miss for the **head**.
§11 records the undici transport's divergences (enforced in P4) and §12 the hand-off to Task 6. x64 measurement is
deferred until the owner's x64 VM exists (§16).

## 2. References studied

All at their latest published release on 2026-11-06 (tags resolved with `git ls-remote` / the GitHub releases API;
picohttpparser and HTTP Garden publish no releases, so their `HEAD` is pinned).

| Project | Release | Commit | Files studied |
|---|---|---|---|
| picohttpparser (C) | none (HEAD) | `dd573d8fa9264c646a400e96ce9f07dea6f156e4` | [`picohttpparser.c`](https://github.com/h2o/picohttpparser/blob/dd573d8fa9264c646a400e96ce9f07dea6f156e4/picohttpparser.c), [`test.c`](https://github.com/h2o/picohttpparser/blob/dd573d8fa9264c646a400e96ce9f07dea6f156e4/test.c) |
| httparse (Rust) | `v1.10.1` (crates.io 1.10.1) | `9f29e79f9832dbd0ae5220acb17c1866745bdecd` | [`src/lib.rs`](https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/src/lib.rs) (incl. `mod tests`), [`src/simd/swar.rs`](https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/src/simd/swar.rs), [`src/simd/neon.rs`](https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/src/simd/neon.rs), [`src/simd/avx2.rs`](https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/src/simd/avx2.rs), [`build.rs`](https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/build.rs) |
| llhttp (C via llparse) | `v9.4.3` (generated C: `release/v9.4.3`) | `45c8699d0ca8431ab366c8706e613b2e2ac62c04` | [`src/llhttp/http.ts`](https://github.com/nodejs/llhttp/blob/45c8699d0ca8431ab366c8706e613b2e2ac62c04/src/llhttp/http.ts), [`src/native/http.c`](https://github.com/nodejs/llhttp/blob/45c8699d0ca8431ab366c8706e613b2e2ac62c04/src/native/http.c), generated [`src/llhttp.c`](https://github.com/nodejs/llhttp/blob/release/v9.4.3/src/llhttp.c) |
| hyper `proto/h1` (Rust) | `v1.12.0` | `7f67699ce32d717081deb7018b4da7d679277ac0` | [`src/proto/h1/role.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/proto/h1/role.rs) (incl. tests), [`io.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/proto/h1/io.rs), [`decode.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/proto/h1/decode.rs), [`src/headers.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/headers.rs) |
| h11 (Python) | `v0.16.0` | `1c5b07581f058886c8bdd87adababd7d959dc7ca` | [`h11/_receivebuffer.py`](https://github.com/python-hyper/h11/blob/1c5b07581f058886c8bdd87adababd7d959dc7ca/h11/_receivebuffer.py), [`_readers.py`](https://github.com/python-hyper/h11/blob/1c5b07581f058886c8bdd87adababd7d959dc7ca/h11/_readers.py), [`_abnf.py`](https://github.com/python-hyper/h11/blob/1c5b07581f058886c8bdd87adababd7d959dc7ca/h11/_abnf.py) |
| Go `net/http` | `go1.27.3` | `2aae1000ea1d4b28923b96db28acf4e3b99d6aff` | [`response.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/response.go), [`transfer.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/transfer.go), [`textproto/reader.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/textproto/reader.go), [`response_test.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/response_test.go), [`transfer_test.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/transfer_test.go), [`internal/chunked_test.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/internal/chunked_test.go) |
| Bun HTTP client | `bun-v1.4.4` | `38afee6c84fcf48c0edb2466aa4b27d287a59c30` | [`src/http/lib.rs`](https://github.com/oven-sh/bun/blob/38afee6c84fcf48c0edb2466aa4b27d287a59c30/src/http/lib.rs), [`src/picohttp/lib.rs`](https://github.com/oven-sh/bun/blob/38afee6c84fcf48c0edb2466aa4b27d287a59c30/src/picohttp/lib.rs), [`scripts/build/deps/picohttpparser.ts`](https://github.com/oven-sh/bun/blob/38afee6c84fcf48c0edb2466aa4b27d287a59c30/scripts/build/deps/picohttpparser.ts) |
| curl | `curl-8_23_0` | `3163ec065643ba20709f190111f42c3e1fe95273` | [`lib/http.c`](https://github.com/curl/curl/blob/3163ec065643ba20709f190111f42c3e1fe95273/lib/http.c), [`tests/data/`](https://github.com/curl/curl/tree/3163ec065643ba20709f190111f42c3e1fe95273/tests/data) (cases listed in §9.3) |
| HTTP Garden | none (HEAD) | `b417e806c1b15e8ea0b9312f81a91fbdcbc7a83e` | [`TROPHIES.md`](https://github.com/narfindustries/http-garden/blob/b417e806c1b15e8ea0b9312f81a91fbdcbc7a83e/TROPHIES.md), [README](https://github.com/narfindustries/http-garden/tree/b417e806c1b15e8ea0b9312f81a91fbdcbc7a83e). **Licence GPL-3.0** (§10 item 10) |
| T-Reqs | none (HEAD) | `b90f97e4f39afbe39c66ecfe99fae1b832b20f62` | [README / grammar](https://github.com/bahruzjabiyev/t-reqs/tree/b90f97e4f39afbe39c66ecfe99fae1b832b20f62) (MIT) |
| Netty (chunked, P1a) | fix versions `4.1.125.Final` / `4.2.5.Final` | fix commit `edb55fd8e0a3bcbd85881e423464f585183d1284` | [`HttpObjectDecoder.java` diff](https://github.com/netty/netty/commit/edb55fd8e0a3bcbd85881e423464f585183d1284) |
| ASP.NET Core Kestrel (chunked, P1a) | fix versions 8.0.21 / 9.0.10 | fix commit `b50ab865ec4dcd3d6ac30221288cc36bf189d300` | [`Http1ChunkedEncodingMessageBody.cs` diff](https://github.com/dotnet/aspnetcore/commit/b50ab865ec4dcd3d6ac30221288cc36bf189d300) |

**Note on Bun:** at `bun-v1.4.4` Bun's source is Rust (0 `.zig` files in the tree); its client still parses heads
with picohttpparser (C, pinned to commit `f4d94b48b31e0abae029ebeafcfd9ca0680ede58` with a chunked-decoder patch) through an FFI wrapper. The design's
§6.2 "Bun's HTTP client (Zig)" row is therefore studied as "Bun's client, Rust + picohttpparser", and the design
wording is corrected (§10 item 11). Bun's published security advisories (3 on 2026-11-06: QPACK amplification, bundled
brotli, emSign roots) contain no HTTP/1 head-parsing issue. (verified: tree listing, files above, `gh api
repos/oven-sh/bun/security-advisories`)

### What each does (verified against the pinned files)

| Project | Strategy | Partial input | Strictness gaps vs §5.2 | Notes / bugs |
|---|---|---|---|---|
| picohttpparser | Scan-then-validate per token: `findchar_fast` (SSE4.2 `pcmpestri` range match) for runs, 8× unrolled scalar `IS_PRINTABLE_ASCII` otherwise; status digits parsed inline | Not resumable: re-parses from byte 0; `last_len` makes `is_complete` look for the blank line only in new bytes (anti-slowloris), so total ≤ 2 passes | Accepts bare LF (`get_token_to_eol`), any `HTTP/1.x` minor digit, multiple SP after version, obs-fold (returns name `NULL`), status `1xx`–`5xx` by first digit only | Issue [#89](https://github.com/h2o/picohttpparser/issues/89) (NULL buffer SEGV, 2025). `pcmpestri` is a microcoded, high-latency instruction (httparse issue [#85](https://github.com/seanmonstar/httparse/issues/85); Cloudflare 2014, §12) |
| httparse | Same shape as pico in Rust; `Bytes` cursor with `unsafe` `advance`; the value scan validates and stops at the first non-value byte (the CR) in one pass; SWAR 8-byte blocks, NEON/SSE4.2/AVX2 16/32-byte blocks; `u64` compare for `HTTP/1.x` | Not resumable: `Status::Partial` → caller re-parses all; hyper adds `is_complete_fast` (scan new bytes for the blank line first) | Accepts bare LF and status codes 000–999; obs-fold, spaces before colon and invalid lines are opt-in configs | GHSA-f67m-9j94-qv9j (`mem::uninitialized` unsound `Header`), issue [#29](https://github.com/seanmonstar/httparse/issues/29) (status without SP — now accepted, matching our §21 decision) |
| llhttp | Generated byte-at-a-time state machine (llparse), resumable at any byte, callback spans; SSE4.2/NEON/wasm-simd only for header-value spans | Fully resumable | Strict by default; leniency flags exist; `llhttp_message_needs_eof` is 0 for every 1xx, so whether a 1xx body is read is the embedder's choice (undici never reads one; a raw llhttp user can) | CVEs below; open issues [#876](https://github.com/nodejs/llhttp/issues/876) (2026-08-23: `Transfer-Encoding: chunked<HTAB>` not recognised as chunked in responses → close-delimited, **response smuggling**) and [#877](https://github.com/nodejs/llhttp/issues/877) (bare LF after request line in strict mode) |
| hyper h1 client | httparse into a `SmallVec` of indices, then a `HeaderMap` (allocations), then `Client::decoder` | As httparse + `is_complete_fast`; capped by `max_buf_size` | Equal duplicate `Content-Length`s accepted (`content_length_parse_all_values`); TE wins over CL; non-`chunked` last coding → close-delimited; `Connection` scanned per field with sticky `close`; mixed `\n\r\n` terminator accepted (`test_parse_accepts_lf_crlf_terminator`) | RUSTSEC-2021-0078/0079, RUSTSEC-2021-0020 (below) |
| h11 | Regex over whole lines once the blank line is found; `_multiple_lines_search` resumes the blank-line search | Terminator search resumable; validation once | Accepts bare LF (`\n\r?\n`), obs-fold (joined) | CVE-2025-43859 (lenient chunked bodies); issue [#16](https://github.com/python-hyper/h11/issues/16) (no big-O tests for `ReceiveBuffer`) |
| Go `net/http` | `bufio` + `textproto` line reader, `strings.Cut` on the status line, `fixLength`/`parseTransferEncoding` after the map is built | Blocking reader | Bare LF, obs-fold, any 3-digit status (incl. `000`, `999`), `HTTP/1.2` and `HTTP/2.0` status lines, equal duplicate CLs accepted; TE ignored on HTTP/1.0; TE+CL → TE wins; CL capped at `int64` | Errors quote the raw line (`badStringError("malformed HTTP response", line)`), which §5.2 forbids for us |
| Bun | picohttpparser + rejects obs-fold in the wrapper; header dispatch by name hash; strict CL (`parse_content_length_strict`), conflicting CLs rejected | `last_len` offset | Inherits pico's bare-LF acceptance; 1 MiB head cap | Sticky `Connection: close`; 1xx/204/304 → no body |
| curl | Line-at-a-time: `memchr(buf, '\n')`, line copied into a dynbuf, per-line handler dispatched on first letter | Resumable per line (copies) | Bare LF, obs-fold (unfolded), comma lists of equal CLs accepted; `identity, chunked` accepted as chunked; non-chunked TE → CL ignored, close-delimited | Rejects CR and NUL inside header lines (tests 2105, 262); copies each line; [CVE-2023-38039](https://curl.se/docs/CVE-2023-38039.html) "HTTP headers eat all memory" (no cap on stored headers, 7.84.0–8.2.1; fixed with a 300 KB total cap) — our 64 KiB cumulative cap and no header storage avoid the class |

## 3. What we take, and what we do differently

**Take:** httparse's and picohttpparser's fused value scan — validate the value class and stop at the first byte
outside it, which must be the CR — re-derived in safe Rust as SWAR over `as_chunks::<8>()` (§6.2); llhttp's
resumability at any byte, but with one sub-state per *line part* instead of per byte; pico/httparse's table-driven
token and value classes; hyper/Bun's sticky `Connection: close` across several `Connection` fields; Go's "one
`Transfer-Encoding` field, value exactly `chunked`" rule (`parseTransferEncoding`); Bun's "1xx/204/304 and HEAD never
have a body"; Netty's and Kestrel's 2025 strict-CRLF fixes for chunk lines (P1a, §10 item 8). (Unverified SIMD ideas for
`engine-simd` are in `docs/perf/ideas.md` only.)

**Do differently:**
- One strict mode, no leniency flags (llhttp's 10 lenient modes caused #876/#877-style gaps between modes; Netty
  and Kestrel both kept opt-out switches for their 2025 fixes).
- Resumable without re-parsing (httparse/pico/hyper re-parse the whole head on every partial read: measured
  quadratic, §8) and without copying lines (curl).
- Reject bare LF and bare CR everywhere (every reference except llhttp-strict and post-2025 Netty accepts bare LF
  somewhere).
- Every 1xx except 101 is interim and bodiless, and may not carry `Transfer-Encoding` or a non-zero `Content-Length`
  (raw llhttp leaves 1xx bodies to the embedder).
- Duplicate `Content-Length` rejected even when equal (hyper, Go, curl, Bun accept equal duplicates).
- `Transfer-Encoding` + `Content-Length` rejected (hyper and Go let TE win; curl lets TE win for any value).
- Framing grammar checked in every head, interim heads included, independent of status (one code path); a 204/304
  that announces a body is accepted without one but not reused.
- No header map, no per-field records: framing facts are folded during the scan; the JS side re-reads the validated
  block (`headers` range) only on access (design §11.5), decoded as latin1 (design §9.3).
- Errors carry only a code (`ErrorCode` has no payload), never bytes (Go echoes lines).

## 4. Security history → §5.2 rule

### 4.1 Mapping

| Attack class / advisory / research | Mechanism (response side) | Blocking §5.2 rule | Vector (§9) |
|---|---|---|---|
| CVE-2022-32214 (llhttp, GHSA-q5vx-44v4-gch4); Klein 2006 §1 (LF-only terminators in browsers and proxies); [PortSwigger 2025](https://portswigger.net/research/http1-must-die) bare-LF termination | LF alone ends a line | Line endings: CRLF only | `bare-lf-*` |
| CVE-2023-30589 (llhttp, GHSA-cggh-pq45-6h9x); Klein 2006 §3 (CR-only in IE and Sun proxy); HTTP Garden "`\r` treated as a line terminator", "`\r\n\rX` ends the head" | CR alone ends a line or the head | Line endings: bare CR rejected | `bare-cr-*`, `cr-x-terminator`, `status-bare-cr-separator` |
| CVE-2022-35256 (llhttp, GHSA-rc2m-q589-vpqx) | field not terminated with CRLF | Line endings | `bare-cr-in-value` |
| CVE-2022-32215 (llhttp, GHSA-5492-mr68-4m2h); HTTP Garden "`Transfer-\r\nEncoding`" | multi-line (folded or split) `Transfer-Encoding` | Header fields: obs-fold rejected; token name then `:` | `obs-fold-*`, `name-split-across-lines` |
| CVE-2024-27982 (Node, GHSA-hg6j-8h7m-3w3j: "a space placed before a content-length header"); HTTP Garden "`\tContent-Length`" | whitespace before the first field name hides it | Header fields: line starting with SP/HTAB rejected (§10 item 1) and no whitespace before the colon | `leading-sp-first-field`, `sp-before-colon` |
| HTTP Garden "`Content-Length\x85`", "`\xefoo`", "`\x00` in names" | non-token bytes in names, stripped or ignored by some parsers | Header fields: token names | `name-obs-text-*`, `ctl-only-name`, `bare-cr-in-name` |
| HTTP Garden "`\x00`/`\r`/`\n` permitted in values", "values truncated at `\x00`", "`\x00` stripped from value ends" | control bytes hide or split values | Header fields: values without CTL except HTAB | `nul-at-value-end`, §9.1 NUL/VT/FF/DEL cases |
| CVE-2022-32213 (llhttp, GHSA-5689-v88g-g6rv); RUSTSEC-2021-0020 (hyper, CVE-2021-21299); CVE-2020-8287 (Node, GHSA-f33f-hhx9-6j4m); HTTP Garden "multiple TE chunked", "`,chunked` distinct from `chunked`", "unknown codings treated as chunked", "`\xa0chunked\xa0`" | TE value parsed loosely; two TE fields | TE: only `chunked`; > 1 TE field rejected | `te-twice`, `te-list`, `te-leading-comma`, `te-nbsp`, `te-obs-text-wrapped`, `te-identity`, `te-identity-chunked` |
| llhttp #876 (open, 2026) | `chunked<HTAB>` not recognised → close-delimited | TE: compare after OWS (SP **and** HTAB) trimming | `te-trailing-htab` (accept), `te-vt` (reject) |
| RUSTSEC-2021-0078 (hyper, CVE-2021-32715); Go issue [39017](https://github.com/golang/go/issues/39017); HTTP Garden "Python `int`" / "`strtoll(,,0)`" / "longest valid prefix" / "empty CL is 0" / "negative CL" | `Content-Length: +3`, `1_0`, `0x10`, `1Z`, empty, `-48` accepted or misread | CL: `1*DIGIT` after ASCII OWS | `cl-plus`, `cl-underscore`, `cl-hex-suffix`, `cl-empty-then-cl`, `cl-nbsp` |
| CL lists / differing duplicates (RFC 9110 §8.6); Klein 2006 §2 (IE/Apache take the first CL, Firefox/Sun/DeleGate the last); HTTP Garden "conflicting CLs, first/last wins" | two framings disagree | CL: no lists, no duplicates | `cl-dup-equal`, `cl-dup-differ`, `cl-list-equal` |
| CL overflow (cf. RUSTSEC-2021-0079 for chunk sizes) | wrap to a small length | CL: must fit `u64` (checked arithmetic) | `cl-u64-max`, `cl-u64-max-plus-1` |
| CL.TE / TE.CL (Kettle; T-Reqs; HDiff); curl test 2918 (TE wins over CL) | CL and TE both present | TE with CL rejected | `cl-then-te`, `te-then-cl`, `cl-then-te-unknown` |
| TE on HTTP/1.0 (RFC 9112 §6.1); Pingora CVE-2026-2835 (HTTP/1.0 + TE and multiple TE, request side) | 1.0 peers ignore TE | TE on HTTP/1.0 rejected | `te-http10` |
| Klein 2006 §4–5 (lenient versions: `HTTP/01.1`, `HTTP/ 1.0`, `HTTP /1.0`, `HTTP/foobar`, `HTTP/2.0`; HTTP/0.9 fallback) | garbage parsed as a status line or as a 0.9 body | Status line: exactly `HTTP/1.0`/`HTTP/1.1` at byte 0 | `status-version-variants`, existing |
| Kettle 2022 client-side desync (Akamai "stacked HEAD": the attacker's own HEAD request leaves a spliced response queued on the victim browser's connection) | an extra response sits on a pooled connection and answers the next request | **§5.1 leftover/idle close** (bytes after a response, or any byte while idle → close) plus the 204/304 no-reuse rule. Not "no body for HEAD": the victim never sent HEAD. **Residual:** an extra response that arrives after checkout races the next request (design §18); idle detection is specified in Task 6 (§12) | Task 6 vectors `two-responses-one-read`, `extra-response-while-idle` (§12) |
| Kettle 2022 Verisign (chunked response to a bodiless request; CL-framed response to HEAD) | framing disagrees with the request method | Bodies: HEAD never has a body; §5.1 leftover close for the bytes it leaves | existing "HEAD with TE chunked"; imported `content-length / Content-Length reset when no body is received` |
| Kettle 2022 pause-based desync (server answers a partial request and keeps the connection) | early response, then stale request bytes | §5.1: a final response before the request body is written → stop writing, close | `early-final-response` (Task 6, §12) |
| Kettle 2025 response queue poisoning; HTTP Garden #47 (invalid chunk, connection kept) | misframing leaves bytes that become the next response | §5.1 reuse rules (close on any leftover byte, any error) | connection tests (Task 6) |
| Expect-based 0.CL / multiple header blocks (Kettle 2025) | interim heads misframed | ≤ 10 interim heads, all bodiless, no framing headers in a 1xx (S16); headers deadline never extended by 1xx (design §3.2); we never send `Expect` (§5.1) | `interim-10`, `interim-11`, `cl-on-150`, `interim-flood-timer` (Task 6) |
| `101` without a requested upgrade; Pingora CVE-2026-2833 class (bytes passed through before `101`) | rest of stream treated as another protocol | Status line: `101` rejected | existing |
| Header-size DoS / slowloris | unbounded buffering, re-parsing | 64 KiB head incl. 1xx, 8 KiB status line, 100 fields; linear scan | limit vectors |
| Funky chunks (Netty CVE-2025-58056, Kestrel CVE-2025-55315, Go CVE-2025-22871, h11 CVE-2025-43859) | chunk-size line ends at bare LF or `\rX`; extensions skipped unvalidated; spill | **Not in the head parser.** §5.2 Chunked: §10 items 6 and 8; §13 | P1a chunked vectors |

### 4.2 What the primary texts say about clients parsing responses (verified, read on 2026-11-06)

- **Kettle 2022, "Browser-Powered Desync Attacks".** Response-side material is confined to client-side desync: the
  attacker desyncs the *browser's* connection so a later request receives a response meant for an injected request.
  Concrete client-relevant mechanics: HEAD responses carrying a large `Content-Length` and no body (Akamai), a
  server that answers before reading a body that stays on the socket (CL.0), pause-based desync (Varnish/Apache answer
  a partial request and keep the connection), and the observation that browsers discard a connection that delivers
  more data than expected ("stacked-response problem"). The paper gives no response-header parsing flaw in clients.
  The defence is §5.1 (no pipelining, close on leftover bytes or on any byte while idle, close on an early final
  response) plus the 204/304 no-reuse rule — **not** "no body for HEAD" (the victim never sent HEAD). One race
  remains: an extra response that arrives after a connection is checked out answers the next request; it is in
  design §18, and Task 6's research note decides how idle connections are watched (§12).
- **HTTP Garden (Kallus, Anantharaman, Locasto, Smith; arXiv 2405.17737v1, 2024) and `TROPHIES.md`.** Request streams
  only: "examines both origin servers' interpretations and gateway servers' transformations of HTTP requests";
  response-direction parsing is listed as future work. No clients are targets. Its bug *classes* (integer parsing,
  chunk lines, field lines, field values) apply to any HTTP/1.1 field grammar, so they are re-expressed as response
  vectors in §9.2. The repository is **GPL-3.0**: payloads are re-derived as our own vectors, citing the class, and
  no file is copied (§10 item 10).
- **T-Reqs (Jabiyev, Sprecher, Onarlioglu, Kirda; CCS 2021).** A grammar-based differential fuzzer of request lines
  and request headers against servers/proxies; no response-side cases, no published discrepancy corpus. Nothing to
  port; its mutation operators (insert/delete/replace a byte or sub-tree) are what our split-and-mutate fuzz target
  covers.
- **HDiff (Shen et al.; DSN 2022, DOI 10.1109/DSN53405.2022.00014).** Described by HTTP Garden as a differential
  fuzzer for transducers and origin servers over requests (grammar from RFC 7230 documentation analysis). No public
  response corpus found. Nothing to port. (Paper text not freely available; summary verified only through the HTTP
  Garden paper's description.)
- **Klein 2006, "HTTP Response Smuggling"** (WebAppSec list, 2006-02-20): the one primary text on parsing
  discrepancies between proxies and *clients* for responses: LF-only and CR-only terminators, first-vs-last duplicate
  `Content-Length`, lenient status-line versions, HTTP/0.9 fallback. Every class is already rejected by §5.2;
  vectors added in §9.2.
- **Kettle 2025, "HTTP/1.1 must die"** (read for the first draft): 0.CL, CL.0, Expect-based desync, response queue
  poisoning, bare-LF termination; mapped above.

**Gaps found:** none in the byte-level head grammar. Review round 1 added reuse and accounting rules (204/304
no-reuse, 1xx framing headers, EOF after an interim head, `Connection: close` in a 1xx; §10). One gap in the chunked rules (chunk extensions are "ignored" in §5.2; Kestrel's
CVE-2025-55315 was exactly an ignored extension that hid a bare LF): §10 item 8.

## 5. Options compared

Common to all: `buf` holds the current message from byte 0 and only grows; a resumable parser keeps state between
calls. n = head bytes. Times: Linux arm64 container, 1 CPU, median of 3 runs (§8.2) for the 3-header / synthetic
20-header / proposed `/1k-h20` heads.

| | A. Byte-at-a-time state machine (llhttp) | B. Find blank line, then validate whole head (httparse + `is_complete_fast`) | C. Line-at-a-time: `memchr` LF, then validate the line | **G. Fused span scan, resumable at any byte (chosen)** |
|---|---|---|---|---|
| Time, whole head in one read | O(n), one branchy step per byte: 77 / 666 / 577 ns | O(n) validation (+ O(n) terminator scan when partial): 54 / 292 / 293 ns (httparse NEON) | O(2n): `memchr` + validation per byte: 53 / 352 / 307 ns (C + SWAR) | O(n): each byte examined O(1) times, 8 per step in values: **47 / 281 / 271 ns** (prototype with a `memchr` status line) |
| Time, incremental | O(n) total | O(n²) worst case (re-parse per read; 67 ms for 60 KB fed 1 B at a time) | O(n) total | O(n) total (0.26 ms for 60 KB fed 1 B at a time; more per-call overhead than C's 0.19 ms) |
| State | state enum + 3–4 offsets + facts | one offset | 2 offsets + facts | 7-variant phase (5 resumable sub-states + `Done`/`Failed`) + 3 offsets + 2 enums |
| Allocations | 0 | 0 (httparse needs a caller header array) | 0 | 0 |
| Branches | per byte | per byte in names, per block in values | per line + per 8-byte word | per name byte + per 8-byte value word |
| §5.2 enforcement | errors at the first bad byte | only after the blank line arrives (a bare-LF head never "completes") | at line completion | errors at the first bad byte, status line included |
| Split invariance | natural | natural | holds if each line's size check precedes its content check | natural: each byte's verdict depends only on earlier bytes and one byte of CR lookahead |
| SIMD swap-in (P1a) | hard: per-byte states must become span skips | easy | easy: 3 primitives | easy: `token_len`, `value_len` (span lengths, the httparse/Cloudflare shape) |
| Risk | many states → resumption bugs at every byte boundary | double scan; quadratic | slower than httparse on generic `aarch64` (1.05–1.20× on 20 headers) | 5 resumable sub-states (each resumption point is tested by the 1-byte split test and the multi-message fuzz) |

Design F (the fused scan without resumption, table-per-byte values, as picohttpparser) is 53 / 412 / 348 ns; G is F made resumable
plus the SWAR value run, and is faster than F everywhere.

## 6. Chosen design (precise)

This section supersedes the Task 4 brief's Step 5 hints ("`memchr` for CR", "`const [bool; 256]`"): there is no
`memchr`, and the tables are `static [bool; 256]` built by a `const fn`.

### 6.1 Files (Task 4 scope)
- `crates/engine-core/src/h1/mod.rs` (`mod parser; pub(crate) mod grammar;` and `pub use` of the public types: one
  public path, design §13.1 rule 3).
- `crates/engine-core/src/h1/grammar.rs`: the `TCHAR` and `FIELD_CONTENT` tables and the scan primitives (§6.2).
  Task 5 reuses it.
- `crates/engine-core/src/h1/parser/mod.rs`: the public types and `ResponseParser` (the resumable scan, §6.3), and
  `crates/engine-core/src/h1/parser/facts.rs`: what the fields mean — the framing and `Connection` facts and the final
  framing and keep-alive (§6.5). One responsibility per file (design §13.1 "Code structure").
- `crates/engine-core/examples/h1_parse_bench.rs` and `[dev-dependencies] httparse = "=1.10.1"` (benchmark only;
  `httparse` is not on policy-check's HTTP-engine list, and dev-dependencies never ship) — spec in §8.1.
- `tests/fixtures/h1-heads/{1k,1k-h20,synthetic-h20}.http`: raw head bytes shared by the bench, the unit tests and
  Task 9's `bench-server` (§8.3).
- Vectors: `tests/vectors/h1-response/blazehttp-rules.json` and `{go,httparse,hyper,curl}-selected.json` (§9.5),
  registered in `tests/vectors/index.json`; `THIRD_PARTY_NOTICES` regenerated; the 204/304 reuse rule in the llhttp
  importer's derivation, which flips the two imported 304 cases (§9.4).
- Vector format: `inputEncoding: "latin1"` (each code point U+0000–U+00FF is one byte; expected header names, values
  and the body are decoded the same way) in `tools/vectors/schema.ts`, `crates/engine-core/tests/support/vectors.rs`
  and `docs/notes/vectors.md`, so single obs-text bytes (`\x80`, `\xa0`) can be written; and the optional
  provenance field `derivedFrom: "<file>:<test> @ <tag>"` for cases re-derived from an upstream test (§9.5).
- `crates/engine-core/tests/h1_response.rs`; fuzz crate `crates/engine-core/fuzz` (outside the workspace, never
  shipped) with `httparse` as its dependency for the differential oracle (§9.7).
- No runtime dependency is added to `engine-core`.

### 6.2 Grammar tables and primitives (`grammar.rs`)
- `TCHAR[b]` ⇔ `b` is RFC 9110 §5.6.2 `tchar` (`!#$%&'*+-.^_`|~`, DIGIT, ALPHA).
- `FIELD_CONTENT[b]` ⇔ `b == 0x09 || (0x20..=0x7E).contains(&b) || b >= 0x80` (RFC 9110 §5.5 `field-vchar` plus SP
  and HTAB; RFC 9112 §4 `reason-phrase` uses the same class). CR, LF, NUL, other CTLs and DEL are excluded.
- `fn class(table: &[bool; 256], b: u8) -> bool`: `table.get(usize::from(b)).copied().unwrap_or(false)` — a `u8`
  index into 256 entries never misses, so no runtime indexing and no lint exception. The tables are built from
  inclusive byte ranges by a `const fn` (`table[b as usize] = true`; `get_mut` is not const on 1.99.1 and const fns
  cannot call function pointers), which carries the only exception:
  `#[expect(clippy::indexing_slicing, reason = "…")]`. Policy-check forbids `allow`/`expect` only for `deprecated`,
  `dead_code` and `unused*` (`tools/policy-check/rules.ts`, `FORBIDDEN_LINTS`), so it passes.
- `fn token_len(s: &[u8]) -> usize`: length of the leading `TCHAR` run (`s.iter().position(|&b| !class(&TCHAR, b))
  .unwrap_or(s.len())`).
- `fn value_len(s: &[u8]) -> usize`: length of the leading `FIELD_CONTENT` run. `let (words, tail) =
  s.as_chunks::<8>()`; per word `x = u64::from_le_bytes(*w)`; `lt20 = x.wrapping_sub(0x2020…20) & !x & 0x8080…80`
  (some byte < 0x20, exact for bytes < 0x80); `eq7f`: `d = x ^ 0x7F7F…7F; d.wrapping_sub(0x0101…01) & !d &
  0x8080…80`. If `lt20 | eq7f == 0` the word is clean (advance 8). **As built (fix round 1, measured):** borrows only
  propagate upward from a truly flagged byte, so the lowest flag is exact: `lane = flags.trailing_zeros() / 8` jumps
  straight to the stop byte; only when that byte is HTAB (field content below 0x20) does the rest of the word go
  through the table (flags above it may be borrow-propagation false positives). Obs-text never sets a flag (`!x`
  clears its high bit). The `tail` uses the table. The table is the only source of truth (exhaustive unit tests in
  §9.6). Measured: ≈ 18% less time on a varied-head ring, neutral on `1k`/`1k-h20` (§8.2, journal).
- P1a: `engine-simd` exports the same signatures (safe API, `unsafe` inside); `grammar.rs` forwards to it. The
  `(window, pos) -> end` seam (`token_end`/`value_end`, `docs/notes/simd-scanning-research.md` §7 item 3) was tried in
  fix round 1 and **dropped**: ≈ 3% slower on `1k-h20` in a same-container A/B; P1a adds it with the bitmaps if their
  shape needs it.

### 6.3 State and algorithm (`parser.rs`)

```rust
enum Phase { StatusPrefix, StatusReason, LineStart, Name, Value, Done, Failed }   // private
enum FramingFact { None, Length(u64), Chunked }                                     // private
enum ConnectionOption { Default, KeepAlive, Close }                                 // private

pub struct ResponseParser {
    head_request: bool,              // method == Method::Head
    phase: Phase,
    pos: usize,                      // [0, pos) of the current message is fully examined
    line_start: usize,               // start of the current line
    colon: usize,                    // the current field line's ':' (meaningful in phase Value)
    // per message, reset after every interim head:
    line: StatusLine { status: u16, version: HttpVersion }, headers_start: usize, fields: usize,
    // per exchange, reset only by `new`/`reset`:
    facts: Facts { framing_fact: FramingFact,         // reset after every interim head
                   connection: ConnectionOption },     // `Close` is sticky across interim heads; `KeepAlive` is reset
    interim_count: usize, interim_bytes: usize,
}
```

As built, `FramingFact`, `ConnectionOption` and `Facts` live in `parser/facts.rs` (§6.1); the scan holds a `Facts`
and calls `field`, `after_interim` and `conclude` on it.

One bool remains (`clippy::struct_excessive_bools`, denied through `pedantic`, fires on the first draft's five).
`saw_te` is gone: a non-`chunked` TE fails immediately, so "TE seen" ≡ `framing == Chunked`.

Invariants within a call: `line_start ≤ pos ≤ window.len()`; **in phase `Value` only**, also `line_start ≤ colon <
pos` (`colon` is stale in every other phase); `interim_bytes ≤ max_head_bytes`.

`parse(buf)` is `let r = self.step(buf); if r.is_err() { self.phase = Phase::Failed; } r` — the single exit point.
`step(buf)`:
1. `phase` is `Done` or `Failed` → `Err(Engine(Internal))` (called again after `Complete` or an error without
   `reset`).
2. `budget = H1_LIMITS.max_head_bytes.checked_sub(interim_bytes).ok_or(Engine(Internal))?`;
   `window = buf.get(..buf.len().min(budget))`; `window.get(pos..)` is `None` → `Err(Engine(Internal))` (contract:
   `buf` shorter than the prefix already examined, `pos`).
3. Loop on `phase`, with `rest = window.get(pos..)`:
   - `StatusPrefix` (`line_start == pos == 0`; `rest_of_line` = `window.get(line_start..)`, i.e. the whole window):
     `n = rest_of_line.len().min(12)`; each of the first `n` bytes must match the
     prefix grammar at its position (0–6 `HTTP/1.`, 7 `0`|`1`, 8 SP, 9–11 DIGIT), else `Malformed` — a non-HTTP
     reply (a TLS record `\x16\x03…`, HTML, HTTP/0.9) fails at its first wrong byte, never waiting for an LF. This
     re-checks ≤ 12 bytes per call (O(1)). Fewer than 12 bytes → **more**. Otherwise: version from byte 7; status
     from the digits (`wrapping_sub(b'0')` after the DIGIT check); outside 100–599 or `101` → `Malformed`; 1xx and
     `interim_count == max_interim_responses` → `Malformed`; `pos = 12`; `phase = StatusReason`. As built, the
     per-position grammar is one function (`status_prefix_byte`) used for both the partial prefix and the complete
     12-byte prefix (checked as a fixed-size array, which the compiler unrolls); decoding reads the validated array.
   - `StatusReason`: the line must end with CRLF within `limit = line_start + max_status_line_bytes` (8,192, CRLF
     included); bytes at or after `limit` are never examined. At `pos == 12` (a `buf` shorter than 12 bytes here is a
     contract violation → `Engine(Internal)`): `[b' ', ..]` → `pos = 13`;
     `[b'\r', ..]` → empty reason, go to the CRLF check; `[]` → **more**; anything else → `Malformed`. Then
     `e = pos + value_len(window.get(pos..limit.min(len))?)`; `match window.get(e..limit.min(len))`: `[b'\r', b'\n', ..]` →
     `headers_start = pos = line_start = e + 2`, `phase = LineStart`; `[] | [b'\r']` → if `window.len() >= limit`
     → `TooLarge(Headers)` (the line cannot end within 8 KiB), else `pos = e`, **more**; anything else (bare LF, CR
     not followed by LF, CTL, DEL) → `Malformed`.
   - `LineStart`: `match rest`: `[] | [b'\r']` → **more**; `[b'\r', b'\n', ..]` → end of head, `head_len = pos + 2`
     → step 5; `[b'\r', ..]` → `Malformed`; otherwise `fields += 1`, `fields > max_header_fields` → `Malformed`,
     `phase = Name`.
   - `Name`: `n = token_len(rest)`; `n == rest.len()` → `pos = window.len()`, **more**; otherwise the byte after the
     run must be `:` and the run non-empty, else `Malformed` (covers a leading SP/HTAB — obs-fold or a hidden first
     field — whitespace before the colon, a missing colon, a bare CR/LF in a name, a non-token byte); `colon = pos +
     n`; `pos = colon + 1`; `phase = Value`.
   - `Value`: `e = pos + value_len(rest)`; `match window.get(e..)`: `[] | [b'\r']` → `pos = e`, **more**;
     `[b'\r', b'\n', ..]` → `field(name = line_start..colon, raw = colon + 1..e)` (§6.5), then
     `pos = line_start = e + 2`, `phase = LineStart`; anything else → `Malformed`.
   - **more**: `buf.len() >= budget` → `TooLarge(Headers)` (the head cannot end within the limit); else `Partial`.
4. Body bytes are never examined: the loop stops at the blank line.
5. End of head: if the status is 1xx → `interim_count += 1; interim_bytes += head_len`; reset the per-message facts;
   `connection = Close` if it was `Close`, else `Default`; `pos = line_start = 0`; `phase = StatusPrefix`; return
   `Interim { consumed: head_len }`. Otherwise compute framing and keep-alive (§6.5), `phase = Done`, return
   `Complete(ResponseHead { status, version, head_len, headers: headers_start..head_len - 2, framing, keep_alive })`.

Limits count every byte including CR and LF: **fields per head** (≤ 100 in each head), **bytes cumulative** (a head
is accepted iff `head_len ≤ 65,536 − interim_bytes`); a status line iff its length including CRLF is ≤ 8,192. Whole
and split parsing agree because every verdict depends only on bytes already in the window (plus one byte of
lookahead after a CR, which waits for more input in both modes), bytes are examined in order, and bytes beyond the
window or beyond the status line's 8 KiB are never examined.

`headers` covers every field line with its CRLF and excludes the blank line, i.e. the `"name: value\r\n"*` shape of
Task 5's `RequestParts::headers`. With no fields it is empty (`start == end`).

`new(method)`/`reset(method)` set every field to its initial value (`reset` is `*self = Self::new(method)`).

### 6.4 Status line
Version: `HTTP/1.1` | `HTTP/1.0` (case-sensitive, RFC 9112 §2.3). Exactly one SP, three DIGITs with value 100–599,
`101` rejected (no upgrade is ever requested). Then CRLF (empty reason, owner decision, design §21) or SP and a reason whose
bytes are all `FIELD_CONTENT` (so a reason may start with SP, as the imported llhttp case requires, and may contain
HTAB and obs-text). Anything else (HTAB separator, 2 or 4 digits, missing SP) → `Malformed`.

### 6.5 Field lines, framing facts, framing and keep-alive
When a field line completes (§6.3 `Value`, CRLF seen), with `name = window[line_start..colon]` and
`raw = window[colon + 1..e]` (both via `get`; `raw` is already validated), dispatch on `name.len()` then
`eq_ignore_ascii_case` (field names are case-insensitive, RFC 9110 §5.1); only the three framing names trim:
`value = raw.trim_ascii()` — after validation the only ASCII whitespace left is SP/HTAB, so this is exactly OWS
trimming (RFC 9110 §5.5); non-ASCII bytes (`\x85`, `\xa0`) are never trimmed.
- 14 `content-length`: `framing_fact != None` → `SmugglingRisk`; `value` must be `1*DIGIT` folded with
  `checked_mul(10)`/`checked_add`, else (empty, sign, inner space, comma list, `_`, `0x`, NBSP, overflow) →
  `Malformed`; in a 1xx head a value other than 0 → `SmugglingRisk` (RFC 9110 §8.6, RFC 9112 §6.1); `framing_fact =
  Length(n)`.
- 17 `transfer-encoding`: `framing_fact != None`, `version == Http10` or a 1xx status → `SmugglingRisk`; `value` must
  be `chunked` (ASCII case-insensitive) else `SmugglingRisk`; `framing_fact = Chunked`.
- 10 `connection`: for each `value.split(|&b| b == b',')` element, `trim_ascii`, then `close` → `connection =
  Close`; `keep-alive` → `connection = KeepAlive` unless it is `Close`; empty and other elements are ignored.
- anything else (including `Keep-Alive`, `Proxy-Connection`, `Upgrade`, `Trailer`): ignored.

The field count and these checks run in every head, 1xx included; in a 1xx head the per-message facts are then
discarded and only a `Close` survives (RFC 9112 §9.6: the connection closes after the exchange).

Final head:
- `framing` (output `Framing`): `head_request || status == 204 || status == 304` → `Empty`; else `framing_fact`
  `Chunked` → `Chunked`; `Length(n)` → `ContentLength(n)` (also `0`); `None` → `CloseDelimited`.
- keep-alive: `Http11 → connection != Close`; `Http10 → connection == KeepAlive`; then `false` if
  `CloseDelimited`; then `false` if `status ∈ {204, 304} && !head_request && (framing_fact == Chunked ||
  framing_fact == Length(n > 0))` — such a response is accepted without a body, but a server that announces a body it does not send
  may still send one, so the connection is not reused (design §5.2 Bodies, **[author's call]**).

  **304 with `Content-Length` > 0 (review ruling, fix round 1):** RFC 9110 §8.6 permits `Content-Length` on a 304 (the
  size the selected representation would have had in a 200), so a compliant server can send it with nothing following.
  We still do not reuse that connection: undici does not reuse it either (§11 D7, parity), and until Task 6 adds
  idle-byte detection (§12) a server that does send the announced bytes would leave them to answer the next request
  (client-side desync). The cost is one reconnect per such revalidation. Revisit with Task 6 data (idle detection in
  place, frequency of 304 + CL > 0 in practice).

### 6.6 Interface (kept, with clarifications)
- Types exactly as in the brief. Derives: `HttpVersion`, `Framing`: `Clone, Copy, Debug, PartialEq, Eq`;
  `ResponseHead`, `ParseStatus`: `Clone, Debug, PartialEq, Eq` (`Range` is not `Copy`).
- **`Keep-Alive` is not extracted in P0.** The only consumer, the pool's idle timeout (§5.1), arrives in P1a. The
  brief's sentence listing `Keep-Alive` should read "`Content-Length`, `Transfer-Encoding` and `Connection`".
- Contract violations — `parse` after `Complete` or after an error without `reset`; a `buf` shorter than the prefix
  already examined (`pos`) — return `Err(Engine(Internal))`; no code path panics (§6.7). During `StatusPrefix`, `pos`
  stays 0 until all 12 prefix bytes are present, so a shorter buffer there is harmless: the prefix is re-validated
  from byte 0 on every call.
- **EOF is not a parser input.** The caller (the connection, and the test helper) tracks the bytes received for the
  exchange: EOF before the final head is `Network(ClosedBeforeResponse)` **only when zero bytes of this exchange's
  response arrived, interim heads included**, otherwise `Protocol(MalformedResponse)`. The parser exposes nothing new
  for this (after an `Interim` the caller already knows bytes arrived). This also keeps an exchange that saw a
  `100 Continue` out of the stale-connection re-send (§5.1 "before any response byte, interim heads included").
- Reason phrase: not exposed; it stays in the head bytes the completion carries (Task 6 `ResponseData::head`), from
  which the JS side reads `statusText`.
- `ResponseParser` is not used for proxy CONNECT replies (the shared bounded CONNECT parser, design §9.2).

### 6.7 No panics, no bounds checks (verified by an assembly audit, §8.5)
- Every slice by a stored offset (`pos`, `line_start`, `colon`, `e`, `limit`) uses `get(..)` (→ `Engine(Internal)`
  on `None`) or a slice pattern; never `buf[a..b]`. In the audit this was the **only** construct that left a panic
  path (`core::slice::index::slice_index_fail`) in optimised code, on both aarch64 and x86_64.
- Scans use iterators (`position`, `all`) and `as_chunks::<8>()` + `u64::from_le_bytes(*w)`; the tail comes from
  `as_chunks`, not `v[n..]`. Clippy 1.99's default lint `chunks_exact_to_as_chunks` rejects the `chunks_exact(8)`
  form anyway.
- Lookahead uses slice patterns (`[b'\r', b'\n', ..]`); the version and digits use `split_first_chunk`/slice
  patterns, never indexing; the only `split_at` form allowed is `split_at_checked`.
- Arithmetic (as built; every form is explicit because `clippy::arithmetic_side_effects` is denied):
  - offsets and counters (`pos`, `line_start`, `colon + 1`, `e + 2`, `fields`, `interim_count`, `interim_bytes`):
    `checked_add(..).ok_or(Engine(Internal))` — all are bounded by the 64 KiB window and the limits, so these never
    fire, and an impossible overflow becomes a typed error rather than a wrap;
  - `budget`: `checked_sub` (same mapping);
  - SWAR word arithmetic and the scanned-length counter in `value_len`: `wrapping_*` (the counter is ≤ `s.len()` ≤
    `isize::MAX`, so it cannot wrap; the mask arithmetic is meant to wrap);
  - status code: each digit is `u16::from(d.wrapping_sub(b'0'))` after the DIGIT check, combined with
    `wrapping_mul`/`wrapping_add` — bounded by 999, so it cannot wrap;
  - const table builder: `wrapping_add` on loop counters bounded by the loop condition;
  - `Content-Length`: `checked_mul`/`checked_add` (overflow is a `Malformed` value).
- Enforcement in `grammar.rs` and `parser.rs`: `#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects,
  clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unreachable)]` (also in `parser/facts.rs`), with
  one exception: the `#[expect(clippy::indexing_slicing)]` on the const table builder (§6.2). The fuzz target runs with
  debug assertions and overflow checks on.

## 7. Error mapping (decided)

| Condition | Error |
|---|---|
| `head_len > 65,536 − interim_bytes`, i.e. no blank line within the window and `buf.len() ≥` budget | `TooLarge(Headers)` |
| status line > 8,192 bytes including CRLF (no CRLF within its first 8,192 bytes) | `TooLarge(Headers)` |
| second `Content-Length` field (even equal, any name case) | `Protocol(SmugglingRisk)` |
| `Content-Length` and `Transfer-Encoding` in one head (either order) | `Protocol(SmugglingRisk)` |
| second `Transfer-Encoding` field | `Protocol(SmugglingRisk)` |
| `Transfer-Encoding` on an HTTP/1.0 response | `Protocol(SmugglingRisk)` |
| `Transfer-Encoding` value other than `chunked` (ASCII case-insensitive, after OWS trimming) | `Protocol(SmugglingRisk)` |
| `Transfer-Encoding`, or `Content-Length` other than `0`, in a 1xx head | `Protocol(SmugglingRisk)` |
| bare LF, bare CR, CTL/DEL/NUL anywhere in the head | `Protocol(MalformedResponse)` |
| first bytes not a prefix of `HTTP/1.0 DDD`/`HTTP/1.1 DDD`; bad separators; status outside 100–599; `101` | `Protocol(MalformedResponse)` |
| non-token name, empty name, whitespace before colon, missing colon, line starting with SP/HTAB | `Protocol(MalformedResponse)` |
| `Content-Length` value not `1*DIGIT` within `u64` after OWS trimming | `Protocol(MalformedResponse)` |
| more than 100 fields in one head; an 11th interim head (at its status code) | `Protocol(MalformedResponse)` |
| EOF before the final head: ≥ 1 byte of this exchange's response received (interim heads included) / 0 bytes (caller) | `Protocol(MalformedResponse)` / `Network(ClosedBeforeResponse)` |
| contract violation (parse after `Complete` or an error; `buf` shorter than `pos`) | `Engine(Internal)` |

Not errors: a 204/304 (not HEAD) with `Transfer-Encoding` or `Content-Length > 0` completes with `Empty` framing
and `keep_alive = false`.

**Ordering:** the first offending byte wins, everywhere (the status prefix is checked byte by byte, and bytes beyond a
limit are never examined); a field line's framing checks run when its CRLF arrives. Both are byte-order
deterministic, so every split of an input gives the same result. Consequence: CTL bytes in a
`Transfer-Encoding`/`Content-Length` value are `Malformed` (the value check runs first), not `SmugglingRisk`.

## 8. Performance plan and budget

### 8.1 Prototype and the committed benchmark
Scratch prototype `parser-bench` (not committed, §6.1: no prototype code in the repo): designs A, C (table values),
C + SWAR ("CS"), C with `memchr_iter`, F, **G**, and httparse 1.10.1 (`parse_response_with_uninit_headers`, 100
uninitialised headers) followed by the same framing-fact extraction. Every design is asserted equal to httparse on
every head, and A, C and G are asserted byte-by-byte split-invariant (G also asserted equal at completion for every
split). A differential run over 1,000,000 randomly mutated heads (1–3 byte replacements, insertions or deletions
drawn from CR, LF, SP, HTAB, `:`, NUL, DEL, `\x80`, VT, `A`, `0`, `,`, `-`, `;`) found G and C + SWAR accepting
exactly the same inputs with identical facts (380,542 accepted), and G's verdict with a random split equal to its
whole-input verdict, errors included. rustc 1.99.1, `lto = "fat"`, `codegen-units = 1`, `std::hint::black_box`,
9 runs × 2 M iterations, median.
Heads: 3 headers (`content-type`, `content-length: 847`, `date`; 109 B), synthetic 20 headers (the 3 + 17
`x-bench-header-NN: value-NN-abcdefghijklmnopqrstuv`; 993 B), and the proposed `/1k-h20` layout (794 B: measured
with `content-type: application/json; charset=utf-8`; the fixture in §8.3 uses `application/json`, 779 B). The
prototype's G finds the status line's LF with `memchr`; the product's fused status scan (§6.3) is measured by the
committed benchmark below. Limits of these numbers: each design parsed one identical head 2 M times (the branch
predictor learns it, which flatters loops with data-dependent branches such as G's name loop), measured outside the
nightly window with host load 2.5–4.

**Committed benchmark (Task 4): `crates/engine-core/examples/h1_parse_bench.rs`** (run with `cargo run --release -p
engine-core --example h1_parse_bench`; replaced by the criterion bench `benches/h1_parse.rs` in P1a):
- Compares `ResponseParser` with httparse 1.10.1 (`[dev-dependencies]`, default features,
  `parse_response_with_uninit_headers`, 100 headers) followed by the same fact extraction, in the same process.
- Inputs: the committed fixtures `tests/fixtures/h1-heads/1k.http` (109 B: `HTTP/1.1 200 OK`, `content-type:
  application/json`, `content-length: 847`, `date: Fri, 06 Nov 2026 12:00:00 GMT`), `1k-h20.http` (779 B, §8.3) and
  `synthetic-h20.http` (993 B, the 17 `x-bench-header-NN` fields above).
- Rotation: 64 variants per fixture, generated deterministically at start-up with the same length (date seconds,
  `x-request-id` and `traceparent` hex digits, `etag` hex, three-digit `content-length` values), cycled per
  iteration so no single head trains the predictor; 100 ms warm-up per design before timing. Fix round 1 added a
  **varied ring** (`docs/notes/simd-scanning-research.md` §7 item 1): 256 heads from `1k-h20.http` with shuffled
  field order and value lengths scaled 0.5–1.5× (fixed xorshift seed).
- Statistic: N ≥ 5 runs; each run times both parsers interleaved and records the ratio ours/httparse; the budget is
  checked on the **median ratio**; absolute ns are advisory. Output: one table (median ns, median ratio, min/max).
- 1-byte feeds (each ≤ 1 ms): (a) one 60,000-byte value; (b) 99 fields × 600 B; (c) one 60,000-byte name
  (`xxx…x: 1`); (d) an 8,192-byte status line (`HTTP/1.1 200 ` + reason). httparse is reported for (a).
- P0 runs are **non-blocking dev measurements** reported in the PR, unless run in the owner's nightly window on the
  reference host (design §16.1).

### 8.2 Measurements (verified)

**Linux arm64 (reference for this note):** Docker Desktop on the owner's M2, `rust:1-bookworm` image (rustc 1.99.1),
one container, `--cpus=1` (`cpu.max = 100000 100000`), kernel 6.11.11-linuxkit, generic `aarch64` target CPU. No
other container was running; the macOS host had load average ≈ 2.5–4 from other processes, so this is a dev
measurement, not a gate run. Median of the medians of 3 runs:

| Design | 3 headers (109 B) | synthetic 20 (993 B) | `/1k-h20` proposal (794 B) | 60 KB fed 1 B at a time |
|---|---|---|---|---|
| **G fused span scan, resumable (chosen)** | **46.6 ns** | **280.7 ns** | **270.9 ns (0.341 ns/B)** | **0.26 ms** |
| C + SWAR (first draft's choice) | 53.3 | 351.9 | 306.9 | 0.19 ms |
| C, table values | 61.3 | 491.6 | 399.5 | 0.22 ms |
| A byte state machine | 77.2 | 666.4 | 577.0 | 0.18 ms |
| F fused, not resumable, table values | 52.8 | 411.7 | 348.4 | — |
| httparse 1.10.1 (NEON) + facts | 53.9 | 292.1 | 292.9 | 67.3 ms (re-parse per call) |
| httparse 1.10.1 SWAR only (`--cfg httparse_disable_simd`) + facts (2 runs) | 54.2 | 299.5 | 282.5 | — |
| **G / httparse NEON** | **0.86×** | **0.96×** | **0.92×** | — |

**macOS, same M2, same source** (one run each, for comparison with the first draft; the `apple-m1` row ran while a
container run was active, so it is the noisier of the two):

| Build | G | C + SWAR | httparse NEON + facts |
|---|---|---|---|
| default target CPU (`apple-m1`) | 46.2 / 270.0 / 267.6 ns | 53.1 / 329.2 / 295.5 ns | 68.5 / 458.4 / 370.1 ns |
| `-C target-cpu=generic` | 47.2 / 281.4 / 271.4 ns | 52.4 / 350.5 / 312.6 ns | 55.0 / 284.2 / 268.6 ns |

(3 headers / synthetic 20 / `/1k-h20` proposal.) The first draft's "0.71× httparse" came from the `apple-m1` build:
that target CPU slows httparse by ≈ 1.6× on the 20-header head (439–458 ns in two `apple-m1` runs vs 282–284 ns
generic) while moving our designs by under 7%. On generic `aarch64` — what the Linux arm64 npm package is built for —
G and httparse are at parity on 20-header heads (0.92–0.96×) and httparse is faster than C. Which way the shipped
`darwin-arm64` build goes depends on the target CPU we ship; §10 item 9 makes the rule explicit. httparse with NEON and
with SWAR only differ by ≤ 4% on these heads (values are short), so httparse's advantage over C came from its fused
single pass, not from NEON — which is what G adopts.

**x64: deferred** until the owner's x64 VM exists (Task 4 is not blocked: the decision holds on both arm64 builds
measured, and G has no arch-specific code). When the VM exists, run the committed benchmark there (same recipe, `--cpus=1`
container) and record the result here and in `docs/perf/journal.md`.

**Committed benchmark results (Task 4, dev measurements; `docs/perf/journal.md`).** Linux arm64 container, same setup
as above, 3 runs of the binary, median per-run ratio ours/httparse + facts: `1k.http` 0.830–0.841× (43–45 ns),
`1k-h20.http` 0.963–0.976× (229–231 ns), `synthetic-h20.http` 0.929–0.939×; 1-byte feeds (a)–(d) 0.03–0.21 ms
(httparse (a) 68–70 ms). The P0 budget (§8.4) holds. The fused byte-wise status prefix did not regress against the
prototype's `memchr` status line (43–45 ns vs 46.6 ns on the 3-header head). macOS: 0.64–0.66× with the default
`apple-m1` target CPU, 0.88× / 0.97× with `-C target-cpu=generic`.

**After fix round 1 (2026-11-07, same method, plus the varied ring):** Linux arm64 `1k.http` 0.79–0.82×,
`1k-h20.http` 0.95–0.96×, `synthetic-h20.http` 0.95–0.97×, varied ring 0.65–0.67× (the `trailing_zeros` value scan,
§6.2); feeds 0.03–0.38 ms. Budget (§8.4) still met.

### 8.3 Proposed `/1k-h20` layout (for Task 9)

The 20 fields of a realistic JSON API response behind a CDN, in this order (status line `HTTP/1.1 200 OK`; total head
779 B: names 275 B, values 405 B; the body is the same 847-byte payload as `/1k`; committed as
`tests/fixtures/h1-heads/1k-h20.http`, read by Task 4's tests and bench and by Task 9's `bench-server`, so the parser
is measured on the exact bytes the G0 cell serves):

| Name | Len | Value | Len |
|---|---|---|---|
| `date` | 4 | `Fri, 06 Nov 2026 12:00:00 GMT` | 29 |
| `content-type` | 12 | `application/json` (same as `/1k`) | 16 |
| `content-length` | 14 | `847` | 3 |
| `connection` | 10 | `keep-alive` | 10 |
| `server` | 6 | `bench-server` | 12 |
| `cache-control` | 13 | `private, no-cache, max-age=0` | 28 |
| `etag` | 4 | `W/"34f-9c1e4a7b2d8f6e03"` | 24 |
| `last-modified` | 13 | `Fri, 06 Nov 2026 11:59:00 GMT` | 29 |
| `vary` | 4 | `Accept-Encoding, Origin` | 23 |
| `x-request-id` | 12 | `0b7c1f0e-5d2a-4c8e-9f3b-7a6d1e2c4b59` | 36 |
| `traceparent` | 11 | `00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01` | 55 |
| `strict-transport-security` | 25 | `max-age=31536000; includeSubDomains` | 35 |
| `x-content-type-options` | 22 | `nosniff` | 7 |
| `x-frame-options` | 15 | `DENY` | 4 |
| `referrer-policy` | 15 | `strict-origin-when-cross-origin` | 31 |
| `access-control-allow-origin` | 27 | `*` | 1 |
| `x-ratelimit-limit` | 17 | `1000` | 4 |
| `x-ratelimit-remaining` | 21 | `999` | 3 |
| `x-ratelimit-reset` | 17 | `1793966400` | 10 |
| `server-timing` | 13 | `cdn-cache;desc=MISS, edge;dur=1, origin;dur=3` | 45 |

Rationale: the fields seen on typical API responses (framing, caching, tracing, security, CORS, rate limiting), all
lower case as HTTP/2-era servers emit them, a `date` that the server pre-builds (fixed bytes, as `/1k`), no
`content-encoding` (that is `/1k-gzip`), no `set-cookie` (cookie handling is not engine work). Names are 35% of the
head bytes, so name scanning (`token_len`, table per byte in P0) matters as much as value scanning here — the
synthetic head (long uniform values) understates it. `connection: keep-alive` exercises the `Connection` fact on every
request.

### 8.4 Budget

**P0 (safe scalar Rust, the real `ResponseParser`, committed benchmark §8.1):** on the Linux arm64 reference
container, median over ≥ 5 runs of the per-run ratio to httparse 1.10.1 + the same facts:
- `1k.http` (3 headers): ratio ≤ 0.95× (advisory: ≤ 55 ns);
- `1k-h20.http`: ratio ≤ 1.05×, i.e. **parity** (advisory: ≤ 0.40 ns per head byte, ≤ 312 ns for 779 B);
- 1-byte feeds (a)–(d): each ≤ 1 ms (linear; httparse ≈ 67 ms on (a)).

The prototype's raw runs show why the gate is a median ratio: one 3-header run measured 56.9 ns against httparse's
58.9 ns (0.97×) while the median of three was 0.86×. Margins over G's prototype numbers cover what the product adds
(window and limit checks, field count, `Interim` handling, the fused status scan). Parity with httparse is enough for
P0: hyper's head cost is httparse **plus** a `HeaderMap` and ≈ 9 allocations per request, which we do not have, and
at ≈ 5 µs CPU per request (§1-F) the parser is ≈ 1–5% of a request.

**P1a (with `engine-simd`):** target ≤ 0.85× httparse on `1k-h20.http` on arm64 and x64 (criterion gate). **Risk:**
httparse with NEON and with SWAR only measured within 4% of each other on these heads (values are short), so 16-byte
vectors may not buy the remaining 8–13%; if the target is missed, the bitmask/name-run ideas in `ideas.md` are next,
and the result goes to the owner with data rather than being relaxed silently.

**G0:** the harness (Task 7) reports CPU µs per request for the `/1k` and `/1k-h20` cells. P1a's criterion bench is
built with the shipped target CPU (design §16.2); results in `docs/perf/journal.md`.

### 8.5 Bounds-check audit (verified)
A scratch crate `asm-audit` holds each prototype scan form (`p_*`) next to its rewritten form (`f_*`), each
`#[inline(never)]`, built `--release` (`codegen-units = 1`, fat LTO) with `--emit asm` for `aarch64-apple-darwin`
and, cross-compiled, `x86_64-unknown-linux-gnu`. Every call target in the output was listed; the only panic call in
either file is `core::slice::index::slice_index_fail`, in `p_line` (`buf[pos..lf]` after a `memchr`).

| Form | aarch64 | x86_64 |
|---|---|---|
| `token_len` (`iter().position` over a `[bool; 256]` table) | no panic path | no panic path |
| SWAR check with `chunks_exact(8)` + `c[0]..c[7]` | no panic path (LLVM-proved) | no panic path |
| SWAR check / span with `as_chunks::<8>()` | no panic path (by type) | no panic path |
| span with `v[n..]` tail | no panic path (LLVM-proved) | no panic path |
| field split with `line[colon]`, `&line[colon + 1..]` after `position` | no panic path (LLVM-proved) | no panic path |
| field split with `split_at` + `[b':', value @ ..]` | no panic path | no panic path |
| line by stored offsets `buf[pos..lf]` | **`slice_index_fail`** | **`slice_index_fail`** |
| line by `buf.get(pos..lf)` + `[content @ .., b'\r']` | no panic path | no panic path |
| status line (`split_at_checked` + slice patterns) | no panic path | no panic path |

Rules derived: §6.7. Sources: Rust Performance Book, "Bounds Checks" (iterate instead of indexing; slice before
loops; assert ranges; `get_unchecked` only as a last resort — not available to us under `forbid(unsafe_code)`);
`slice::as_chunks` (stable since 1.88.0), `slice::split_at_checked` (stable since 1.80.0), `slice::get`, slice
patterns (Reference, "Slice patterns"); Clippy lints `indexing_slicing` and `chunks_exact_to_as_chunks` (§15).

### 8.6 Cross-language comparison (dev)

`bench/parsers/` (method and fairness rules in its README) parses the shared fixtures with our parser and the
fastest head parsers in each language, each in its own `--cpus=1` container built for the same generic Armv8-A
baseline, after a differential gate (same status, head length, Content-Length and field bytes on every head). Dev
run of 2026-11-07 on the owner's M2 (Linux arm64 Docker, host load 3.7–16.7, so ranges are wide; full table and
versions in `docs/perf/journal.md` and `bench/results/2026-11-07-arm64-dev-parsers.jsonl`). Median ns/head and the
median per-round ratio ours/theirs (< 1: ours faster):

| Parser | 3 headers | 20 headers | rotated 64 | Work per head |
|---|---|---|---|---|
| **blazehttp** `ResponseParser` | **55** | **269** | 252 | validates every byte, offsets + facts, 0 allocations |
| httparse 1.10.1 | 62 (0.82×) | 300 (0.96×) | **244** (0.95×) | validates, slices into a caller array, + facts pass |
| picohttpparser `dd573d8` | 61 (0.86×) | 391 (0.65×) | 326 (0.67×) | validates, pointers into a caller array, + facts pass |
| llhttp 9.4.3 (strict) | 84 (0.62×) | 391 (0.66×) | 339 (0.70×) | state machine, facts built in |
| Beast 1.92 `basic_parser` | 117 (0.50×) | 615 (0.43×) | 505 (0.45×) | virtual call per field, facts built in |
| Beast 1.92 `response_parser` | 187 (0.28×) | 1,323 (0.19×) | 1,970 (0.11×) | builds `http::fields` (4 / 21 allocations) |
| Zig 0.17.1 `std.http` | 358 (0.16×) | 2,329 (0.12×) | 1,976 (0.11×) | finds the end, splits lines; little validation |
| Go 1.27.3 `net/http` | 909 (0.06×) | 4,082 (0.07×) | 3,415 (0.07×) | `*Response` + header map (10 / 40 allocations) |

1-byte feeds (a)–(d): we stay linear (0.08–0.56 ms), as do llhttp, picohttpparser (its `last_len` scan, then one
parse), Zig and Go (1.4–2.6 ms); httparse re-parses from byte 0 (118–344 ms) and Beast re-parses the current partial
line (25 ms–2.1 s). Conclusion for P1a: P0 leads every parser on short heads and every non-Rust parser on 20-field
heads, but is only **at parity with httparse** on 20-field and mixed heads; P1a's SIMD target (§8.4: ≤ 0.85×
httparse on `1k-h20.http`) is what turns that into a lead. x64 (where picohttpparser enables SSE4.2) is deferred to
the owner's x64 VM.

## 9. Additional test cases (beyond the brief)

Each vector is a full input followed by EOF (`docs/notes/vectors.md`). "→" gives the expected native result.
Accepting cases list the header values that must come back byte-exact. Inputs containing a single byte ≥ 0x80 use
`inputEncoding: "latin1"` (§6.1); all others stay `utf8`. Unless shown, the status line is `HTTP/1.1 200 OK\r\n`,
other lines end in CRLF and the head ends with `\r\n`. `expect.undici` is given only where it differs and is
enforced from P4 (§11).

### 9.1 From the §5.2 rules (`blazehttp-rules.json`)

Line endings and whitespace:
- `bare-cr-in-value`: `Foo: a\rContent-Length: 5\r\n` (CVE-2023-30589) → malformed.
- `bare-lf-in-fields`: one field line ending in `\n` among CRLF lines → malformed; `bare-lf-blank-line`: head ending
  `\r\n\n` → malformed.
- `leading-sp-first-field`: `HTTP/1.1 200 OK\r\n Content-Length: 5\r\n…` (CVE-2024-27982 shape) → malformed;
  same with HTAB.
- `obs-fold-te`: `Transfer-Encoding: chunked\r\n , identity\r\n` (CVE-2022-32215) → malformed.
- `sp-before-colon` / `htab-before-colon` → malformed; `empty-name` (`: x`) and `no-colon` (`Foo bar`) → malformed.
- Bytes: NUL in a name and in a value, DEL in name/value/reason, VT and FF in a value → malformed; obs-text
  (`\x80`, `\xff`, latin1) in a value and in the reason → accepted; `\x80` in a name → malformed.

Status line:
- `HTTP/1.1 200OK`, `HTTP/1.1 2000 OK`, `HTTP/1.1 20 OK`, `HTTP/1.1 099`, `HTTP/1.1 600`, `HTTP/1.1 -20`,
  `http/1.1 200 OK`, `HTTP/1.1 200\tOK`, `HTTP/2.0 200 OK`, ` HTTP/1.1 200 OK` → malformed.
- Accepting boundaries: `status-599` `HTTP/1.1 599 X` → 599; `status-100-then-200` and `status-199-then-200` (a
  `100 Continue` / `199 X` head, then `200` with `Content-Length: 2` and `ok`) → 200.
- `HTTP/1.1 200  OK` (reason starts with SP) → accepted. `HTTP/1.1 101` with no `Upgrade` → malformed.
- `HTTP/1.1 150 X` then `200` → status 200 (every 1xx except 101 is interim).
- `garbage-no-lf` `\x16\x03\x03\x00\x02\x02\x28` (a TLS alert record, no LF; `utf8`: every byte is < 0x80) → malformed at the first byte,
  without waiting for more input (unit test: `parse` returns the error on the first call).

Framing (each with its accepting neighbour):
- `cl-plus` `+3` (RUSTSEC-2021-0078), `-1`, `0x10`, `1e3`, `4 2`, empty → malformed; `cl-leading-zeros` `007` →
  7 bytes; OWS with HTAB around the value → accepted; `cl-u64-max` (`18446744073709551615`, no body sent → the
  helper reports incomplete-body) vs `…616` → malformed; `cl-list-equal` `5, 5` → malformed.
- `cl-dup-equal`, `cl-dup-differ` → smuggling-risk; `cl-dup-case` `Content-Length: 5` + `content-length: 5` →
  smuggling-risk.
- `cl-then-te`, `te-then-cl`, `te-twice` (both `chunked`; CVE-2020-8287/RUSTSEC-2021-0020 class), `te-list`
  (`chunked, chunked`), `te-gzip-chunked`, `te-identity`, `te-chunkedx`, `te-chunked-param` (`chunked;q=1`),
  `te-empty`, `te-http10` → smuggling-risk. Two-field shapes (RUSTSEC-2021-0020): `te-fields-chunked-then-identity`
  and `te-fields-gzip-then-chunked` → smuggling-risk. `te-name-mixed-case` `tRaNsFeR-eNcOdInG: chunked` +
  `content-length: 5` → smuggling-risk.
- TE accepted as chunked, asserted with request `HEAD` (framing `Empty`, TE grammar still checked; the `GET` twins
  move to the P1a chunked file): `te-trailing-htab` `Transfer-Encoding: chunked\t` (llhttp #876) and
  `te-leading-htab` `Transfer-Encoding:\tchunked` → ok, status 200, TE echoed, keepAlive true; `te-trailing-htab`
  carries `expect.undici: "PROTOCOL/smuggling-risk"` (D1, P4; `te-leading-htab` waits for the P4 check in §11);
  `te-uppercase` `CHUNKED` → ok. `te-vt` (`chunked\x0b`) → malformed.
- 1xx heads (fields per head, bytes cumulative; facts reset; framing headers forbidden):
  `cl-on-150` `HTTP/1.1 150 X\r\nContent-Length: 39\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nevil` →
  smuggling-risk; `te-and-cl-in-103`, `cl-dup-in-100` → smuggling-risk (now at the first framing field, by the 1xx
  rule; `cl-dup-in-100` is `HTTP/1.1 100 Continue\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\n` and fails at
  its first field); `cl0-in-103-then-cl-in-200` (`Content-Length: 0` in a 103, then `200` with `Content-Length: 2`,
  `ok`) → 200, body `ok`; `fields-100-in-103-and-100-in-200` (a 103 with 100 `x-fill-NNNN: a` fields, then a 200 with
  99 of them plus `Content-Length: 0`) → accepted.
- 204/304 (design §5.2 Bodies, [author's call]): `te-on-304` → accepted, no body, keepAlive **false**;
  `204-cl-5-no-reuse` (`Content-Length: 5`, no body) → keepAlive false; `304-cl-0` → keepAlive true; `head-cl-123`
  (request `HEAD`, `HTTP/1.1 304 Not Modified\r\nContent-Length: 123\r\n\r\n`) → keepAlive true (S1's HEAD
  exclusion); `cl-and-te-on-304` → smuggling-risk. (`304-te-no-reuse` duplicated `te-on-304` and was dropped.)
- HEAD with `Transfer-Encoding: chunked` → no body, keep-alive true.

Connection:
- `Connection: Close` (case) → keep-alive false; `connection-name-upper` `CONNECTION: close` → false;
  `Connection: keep-alive, close` on 1.0 → false; a second `Connection: close` field after `keep-alive` → false;
  `Connection: ,close,` → false; `Connection: closed` on 1.1 → true; `Connection: Keep-Alive` on 1.0 → true;
  `Keep-Alive: timeout=5` alone on 1.0 → false.
- `103` with `Connection: close`, then `200` with `Content-Length` → keep-alive **false** (`Close` is sticky for
  the exchange, RFC 9112 §9.6).

EOF (design §5.2, [author's call]; the helper applies the §6.6 rule):
- `eof-empty` `""` → `NETWORK/closed-before-response`; `eof-after-interim` `HTTP/1.1 100 Continue\r\n\r\n` and
  `interim-then-eof` `HTTP/1.1 103 Early Hints\r\n\r\n` → `PROTOCOL/malformed-response`; `eof-after-h` `H` and
  `eof-mid-status` `HTTP/1.1 200 OK\r` + EOF → `PROTOCOL/malformed-response`. (`eof-lone-cr` was replaced: `\r`
  fails the status prefix at byte 0 and never reached EOF.)

Limits (exact boundaries, all bytes counted including CRLF), built by one deterministic rule so they can be
regenerated: filler names `x-fill-NNNN` (4 digits), values of `a`, each line 1,000 bytes except one last field sized
to hit the total exactly; the status-line cases use a reason of `r` bytes:
- status line of 8,192 vs 8,193 bytes; 100 vs 101 fields (`x-fill-NNNN: a`); head of 65,536 vs 65,537 bytes; 1xx
  heads plus a final head totalling 65,536 vs 65,537; a final status line that fits 8 KiB but not the budget left
  after interim heads → too-large; 10 vs 11 interim heads.

### 9.2 From security research (`blazehttp-rules.json`)

Only cases not already in §9.1 or the imported llhttp files; payloads are re-derived with our own names, filler and
lengths (HTTP Garden is GPL-3.0; §10 item 10). Already covered and not repeated: Kettle 2022's stacked HEAD
(imported `content-length / Content-Length reset when no body is received`), CR as a field terminator (imported
`invalid / Headers separated by CR`), `HTTP/01.1` (imported), unknown codings (`te-identity`, `te-chunkedx`),
negative and `0x` lengths, differing duplicate lengths (`cl-dup-differ`, Klein 2006 §2).

- `te-nbsp` `Transfer-Encoding: \xa0chunked\xa0` and `te-obs-text-wrapped` `Transfer-Encoding: \xffchunked\xff`
  (latin1) [class: non-ASCII stripped as whitespace] → smuggling-risk (only ASCII SP/HTAB are OWS).
- `te-leading-comma` `Transfer-Encoding: ,chunked` [class: `,chunked`] → smuggling-risk.
- `name-obs-text-suffix` `Content-Length\x80: 3` + body `abc` and `name-obs-text-prefix` `\xf0x-ours: 1` (latin1)
  [class: obs-text in names] → malformed.
- `cr-x-terminator` `X-A: 1\r\n\rQX-B: 2\r\n\r\n` [class: `\r\n\rX` ends the head] → malformed.
- `name-split-across-lines` `Transfer-\r\nEncoding: chunked\r\nContent-Length: 3\r\n\r\nabc` [class: split TE
  name] → malformed (at the first line: no colon).
- `ctl-and-separators-in-name` `\x05(x)\x1d: v` [class: CTLs and separators in names] → malformed.
- `cl-underscore` `1_0` and `cl-hex-suffix` `3Q` [class: Python `int` / longest valid prefix] → malformed.
- `cl-nbsp` `Content-Length: \xa03` (latin1) [class: `\xa0` stripped] → malformed.
- `cl-empty-then-cl` `Content-Length: \r\nContent-Length: 3\r\n\r\nabc` [class: empty CL prioritised] → malformed
  (the first line fails first).
- `cl-octal-like` `Content-Length: 010` with 10 body bytes [class: `strtoll(,,0)`] → accepted, 10-byte body (unlike
  `007`, distinguishes decimal from octal).
- `nul-at-value-end` `X-Ours: v\x00` [class: `\x00` stripped from value ends] → malformed.
- `obs-text-value-preserved` `X-Ours: \x85v\xa0` (latin1) [class: `\xa0`/`\x85` stripped] → accepted, value exactly
  `\x85v\xa0`; also run through the JS header surface on both transports (P2 native, P4 undici; design §9.3).
- `status-version-variants` [Klein 2006 §4]: `HTTP/ 1.1 200 OK`, `HTTP /1.1 200 OK`, `HTTP/foobar 200 OK`,
  `HTTP/1.2 200 OK` → malformed (4).
- `cl-then-te-unknown` `Content-Length: 12\r\nTransfer-Encoding: x-ours` [class: curl test2918, TE wins] →
  smuggling-risk.

### 9.3 From upstream suites (behaviour only; per-source files)

Cases re-derived from an upstream test (one behaviour each, often with that test's bytes) live in a file per source
project, so licence and provenance are recorded once (§9.5); each case also carries `derivedFrom: "<file>:<test> @
<tag>"` (`reference` stays for what reference implementations did with an input). Only cases not already in §9.1, §9.2 or the imported files.

`go-selected.json` (go1.27.3):
- `eof-mid-fields` `HTTP/1.1 301 Moved Permanently\r\nFoo: bar` + EOF and `eof-after-version` `HTTP/1.1` + EOF
  [`response_test.go:TestReadResponseErrors`] → malformed (EOF rule, §6.6).
- Status codes [`TestReadResponseErrors`]: `20X`, `abcd`, `c8`, `0x12d`, `二百` (UTF-8), ` Unknown` (no code) →
  malformed (6). (Go also accepts `000`, `001`, `999`; §9.1's `099`/`600` cover our bounds.)
- `cl-dup-in-204` (`7`, `8`) and `cl-dup-equal-in-304` (`961`, `961`) → smuggling-risk (Go checks 204/304 too but
  accepts equal values).
- `cl-i64-max-plus-1` `9223372036854775808` [`transfer_test.go:TestParseContentLength`, Go issue
  [39017](https://github.com/golang/go/issues/39017)] → accepted (**deliberate difference**: Go caps at `int64`,
  §5.2 says `u64`); the helper reports incomplete-body.
- `trailer-field-ignored` `Trailer: Content-MD5, Content-Sources` + `Content-Length: 10` → accepted, 10-byte body.
- `connection-keep-alive-close-11` `Connection: keep-alive, close` on 1.1 (Go issue
  [8840](https://github.com/golang/go/issues/8840)) → keepAlive false.

`hyper-selected.json` (v1.12.0, `src/proto/h1/role.rs`):
- `htab-inside-value` `server: hello\tworld` [`parse_header_htabs`] → accepted, value `hello\tworld`. (hyper's other
  client behaviours — TE `yolo` close-delimited, CL+TE chunked, equal duplicate CLs, `a: b\n\r\n` — are rejected by
  us and already covered by `te-identity`, `cl-then-te`, `cl-dup-equal`, `bare-lf-in-fields`.)

`httparse-selected.json` (v1.10.1, `src/lib.rs` `mod tests`):
- `reason-trailing-htab` `HTTP/1.1 200 OK\t\r\n` → accepted; `reason-nul` `HTTP/1.1 200 \x00\r\n` → malformed.
- `status-bare-cr-separator` `HTTP/1.1 200\rOK\r\n` and `status-two-codes` `HTTP/1.1 99 200 OK` → malformed.
- `obs-fold-whitespace-only-line` `Line-Folded-Header: \r\n   \r\n hello there` and `obs-fold-in-empty-field`
  `Line-Folded-Header:   \r\n \r\n \r\n` (`RESPONSE_WITH_OBSOLETE_LINE_FOLDING_AT_START`, `…_IN_EMPTY_HEADER`) →
  malformed.
- `bare-cr-in-name` `Access-Control-Allow-Cred\rentials: hello` → malformed.
- `no-ows-after-colon` `foo:bar` → accepted, value `bar`.

`curl-selected.json` (curl-8_23_0, `tests/data`, CRLF where curl's file uses LF):
- `reason-digits-only` `HTTP/1.1 405 405` [test1466] → accepted.
- `te-identity-chunked` `identity, chunked` [test1493: curl accepts as chunked] and `te-chunked-identity`
  `chunked, identity` [test1495] → smuggling-risk.
- `obs-fold-htab-continuation` a non-first field followed by a line starting with HTAB [test1274] → malformed.

### 9.4 Imported cases changed by the 204/304 rule
`connection / HTTP 304 with Content-Length` and `connection / HTTP 304 with Transfer-Encoding` flip to keepAlive
**false** (design §5.2 Bodies, 204/304 rule). **As built** this is not an override: the rule is ours, not a llhttp
divergence, so it lives in the importer's derivation (`finalOutcome` in `tools/vectors/import-llhttp.ts`: a 204/304
answering a non-`HEAD` request with `Transfer-Encoding` or `Content-Length` > 0 → keepAlive false). The cases keep
their span check, and any future imported 204/304 case gets the rule. The importer now also stops for an override when
an interim head (other than `101`) carries `Content-Length`, `Transfer-Encoding` or `Connection` (§5.2 Interim heads;
none does in llhttp v9.4.3).

### 9.5 Per-source vector files created by Task 4

| File | Project | Tag | Commit | Licence | Copyright (verbatim, licence file at the commit) |
|---|---|---|---|---|---|
| `go-selected.json` | Go | `go1.27.3` | `2aae1000ea1d4b28923b96db28acf4e3b99d6aff` | BSD-3-Clause ([LICENSE](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/LICENSE)) | `Copyright 2009 The Go Authors.` |
| `hyper-selected.json` | hyper | `v1.12.0` | `7f67699ce32d717081deb7018b4da7d679277ac0` | MIT ([LICENSE](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/LICENSE)) | `Copyright (c) 2014-2026 Sean McArthur` |
| `httparse-selected.json` | httparse | `v1.10.1` | `9f29e79f9832dbd0ae5220acb17c1866745bdecd` | MIT (upstream is MIT OR Apache-2.0; the file declares MIT, [LICENSE-MIT](https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/LICENSE-MIT)) | `Copyright (c) 2015-2025 Sean McArthur` |
| `curl-selected.json` | curl | `curl-8_23_0` | `3163ec065643ba20709f190111f42c3e1fe95273` | curl ([COPYING](https://github.com/curl/curl/blob/3163ec065643ba20709f190111f42c3e1fe95273/COPYING)) | `Copyright (c) 1996 - 2026, Daniel Stenberg, <daniel@haxx.se>, and many contributors, see the THANKS file.` (first line verified; the implementer copies the full sentence from the file) |

Each file is registered in `tests/vectors/index.json` for `engine-core:h1_response`, and `pnpm vectors:notices`
regenerates `THIRD_PARTY_NOTICES`. Tags are the latest published releases on 2026-11-06; P1b's full port (design §15)
extends the same files after `--refresh`-style re-resolution of the latest release.

**Counts as built (Task 4):** `blazehttp-rules.json` 144 (every §9.1 rule with its accepting neighbours, the §9.2
classes and the limit boundaries; fix round 1 added `connection-close-then-keep-alive-11`,
`connection-close-keep-alive-10`, `te-on-103`, `cl-5-on-100`, `connection-htab-ows`, `name-every-tchar-special`,
`fields-101-in-103`, `cl-not-digits-in-103`), `go-selected.json` 13, `hyper-selected.json` 1,
`httparse-selected.json` 8, `curl-selected.json` 4: **170** new vectors, plus the two imported 304 cases flipped
(§9.4). Each fix-round vector was shown failing against an injected mutant before it was kept.

**Counts planned in the note.** First completion: 46 new vectors (§9.2: 20; §9.3: 26 = go 13, hyper 1, httparse 8, curl 4). Review
round 1 adds **21** vectors to §9.1 (`garbage-no-lf`; 3 status boundaries; `cl-dup-case`; 2 two-field TE;
`te-name-mixed-case`; `te-leading-htab`; `cl-on-150`; `cl0-in-103-then-cl-in-200`;
`fields-100-in-103-and-100-in-200`; 4 for 204/304; `connection-name-upper`; 4 EOF) and 2 overrides (§9.4): **67**
new vectors in total, beyond the first draft's §9.1 rule groups.

### 9.6 Unit tests (not vectors)
- Contract violations return `Engine(Internal)`: `parse` after `Complete`; `parse` after an error (the `Failed`
  phase); a `buf` shorter than `pos`: feed `HTTP/1.1 200 OK\r\nFoo` (20 bytes → phase `Name`, `pos == 20`), then only
  its first 15 bytes; and in `StatusReason`: `HTTP/1.1 200` (`Partial`, `pos == 12`) then `HTTP/` (fix round 1: this
  used to blame the peer with `Malformed`). (A shorter buffer during `StatusPrefix` is re-validated, not an error:
  §6.6.)
- Framing as built (the vector format drops `framing`): a `GET` with `Transfer-Encoding: chunked` → `Chunked`,
  keep-alive (`te_on_get_framing`; the test helper reports a chunked body as an explicit outcome until P1a decodes
  it); 204, 304 and `HEAD` → `Empty`, with keep-alive per §6.5.
- `value_len` equals the table-based `position` for every byte value at every lane and in the tail, and for every
  adjacent pair (256 × 256) at lanes 0/1, 6/7 and 7/tail.
- `reset` after `Complete`; a 64 KiB head fed 1 byte at a time completes; every sub-state boundary (a split after
  each of `\r`, `:`, the last name byte, the first value byte, each status-prefix byte) for one head of each kind.
- **Error payloads:** the brief's `errors_never_contain_header_bytes` is replaced by "every error is a payload-free
  code: `ErrorCode::ALL_SAMPLES.contains(&e)`" for every error the corpus produces (the substring form fails: `Debug`
  of `Protocol(MalformedResponse)` shares "Protocol" with the imported "Switching Protocols", and limit filler would
  match `TOO_LARGE/headers`). It holds by construction today (no `ErrorCode` variant carries bytes) and is kept as a
  tripwire for the day a variant gains a payload.
- **Split test cost:** every split point of every vector stays (the brief's property); the workspace sets
  `[profile.test] opt-level = 1` so the O(n²) sweep over the 64 KiB vectors takes ≈ 6 s instead of ≈ 57 s, with debug
  assertions and overflow checks still on.

### 9.7 Fuzz (`h1_response`)
- The brief's split-equality property, over **multi-message inputs**: 0–12 interim heads plus a final head, random
  `head_request`, whole input = arbitrary reads = 1-byte reads, splits anywhere including across interim boundaries.
- **Structure-aware inputs (fix round 1, F1/F4/M2):** field names from the framing-relevant set in several cases
  (`Content-Length`, `cOnTeNt-LeNgTh`, `TRANSFER-ENCODING`, `CONNECTION`, `Proxy-Connection`, …); values are
  `Chunked { case mask, OWS }`, `Conn` (comma lists of `close`/`keep-alive`/`upgrade`/`closed`/empty with OWS),
  `Digits { leading zeros, n }`, `u64::MAX + 1` or raw bytes; status lines with a phrase, an empty reason or a reason
  glued to the code; `Filler { count, len }` lines up to 70,000 bytes; `Boundary` exchanges a few bytes either side of
  each limit (64 KiB head, interim + final 64 KiB, 8 KiB status line, 100 fields, 10 interim heads) — the
  structure-aware form of seeding with the `head-65537`, `interim-plus-final-65537` and `status-line-8193` vectors;
  optional byte mutations; raw bytes. Dictionary `fuzz/h1_response.dict`; runs use `-use_value_profile=1`.
- Differential oracle with httparse (fuzz-crate dependency only): accept ⇒ httparse (default strict config) accepts
  with the same status, version, field list and head length; httparse completes a head ⇒ we accept or reject it.
- **Framing/keep-alive model** (reference model of RFC 9112 §6.3 plus our §5.2 over httparse's field list, in field
  order, limits from `H1_LIMITS`, returning the error class): accept ⇒ same `(framing, keep_alive)`; reject ⇒ our
  error equals the model's class or is `Malformed` (a byte rule httparse does not check), `TooLarge` only past a limit,
  and `SmugglingRisk` never where the model accepts.
- **Over-strictness:** an unmutated structured exchange whose bytes are all valid, which the model accepts and which
  fits the limits, must be accepted.
- Built with debug assertions and overflow checks.
- **Mutation check (fix round 1, from an empty corpus, dictionary + value profile, macOS arm64):**
  `FramingFact::Chunked => Framing::CloseDelimited` (the llhttp #876 class) crashed the target after 19,298 runs
  (≈ 36 s); dropping `&& self.connection != ConnectionOption::Close` after 111,927 runs (≈ 125 s). Both had survived
  the first harness (114k and 611k runs).

### 9.8 Port in P1b (design §15 "ported corpora")

The full suites become vector files in P1b, extending §9.5's files:

| Suite | Tag | Commit | Files | Licence |
|---|---|---|---|---|
| Go `net/http` | `go1.27.3` | `2aae1000ea1d4b28923b96db28acf4e3b99d6aff` | [`response_test.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/response_test.go) (`respTests`, `TestReadResponseErrors`), [`transfer_test.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/transfer_test.go) (`TestParseTransferEncoding`, `TestParseContentLength`), [`internal/chunked_test.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/internal/chunked_test.go) (chunked: P1a) | BSD-3-Clause |
| hyper | `v1.12.0` | `7f67699ce32d717081deb7018b4da7d679277ac0` | [`src/proto/h1/role.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/proto/h1/role.rs) tests: `test_parse_response*`, `test_decoder_response`, `test_parse_response_multiple_connection_headers`, `parse_header_htabs`, `test_client_obs_fold_line`, `test_parse_accepts_lf_crlf_terminator`, `parse_too_large_headers`, `test_h1_client_max_header_size` | MIT |
| httparse | `v1.10.1` | `9f29e79f9832dbd0ae5220acb17c1866745bdecd` | [`src/lib.rs`](https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/src/lib.rs) `mod tests`: `test_response_*`, `RESPONSE_*` statics and their `test_forbid_*`/`test_allow_*` pairs (strict expectations only) | MIT (chosen from upstream MIT OR Apache-2.0, as `httparse-selected.json` declares) |
| curl | `curl-8_23_0` | `3163ec065643ba20709f190111f42c3e1fe95273` | [`tests/data`](https://github.com/curl/curl/tree/3163ec065643ba20709f190111f42c3e1fe95273/tests/data) `test{178,249,262,282,1033,1115,1144,1154,1266,1274,1432,1433,1466,1473,1479,1482,1483,1493,1494,1495,1546,1665,1722,1723,2105,2107,2119,2918}`; chunked (P1a): `test{207,266,1416,1417,1499,2106}` | curl (MIT-style) |
| HTTP Garden | none | `b417e806c1b15e8ea0b9312f81a91fbdcbc7a83e` | **Do not port** (GPL-3.0): classes re-derived in §9.2 | GPL-3.0 |

## 10. Design changes (accepted; applied to the design doc as **[author's call]** on 2026-11-06)

The controller accepted items 1–6 and 8–10 in review round 1 and authorised the design-doc edits; item 7 changes
the Task 4 brief only. Where each landed:

1. Field line starting with SP or HTAB rejected, the first field line included (CVE-2024-27982 shape) → design §5.2
   Header fields.
2. `Transfer-Encoding`: only `chunked`, ASCII case-insensitive after OWS (SP, HTAB) trimming (llhttp #876) → design §5.2.
3. 64 KiB head and 8 KiB status-line limits count every byte including CRLF; fields per head, bytes cumulative →
   design §5.2.
4. CL/TE grammar and conflict rules apply to every head, 1xx included; a 1xx with TE or `Content-Length` ≠ 0 →
   `PROTOCOL/smuggling-risk` (S16); `Connection: close` in a 1xx is sticky for the exchange → design §5.2.
5. EOF before the final head: `NETWORK/closed-before-response` only with zero bytes of the exchange's response
   (interim heads included), else `PROTOCOL/malformed-response` → design §5.2; design §5.1 re-send wording "before any response
   byte (interim heads included)".
6. P1a chunked: CRLF only on chunk-size lines, chunk extensions and trailer lines → design §5.2 Chunked.
7. Task 4 brief: drop `Keep-Alive` from the extracted fields until P1a (§6.6). (Brief, not design.)
8. P1a chunked: chunk extensions validated (no CTL except HTAB, no unpaired CR/LF) then ignored; exactly CRLF after
   chunk data → design §5.2 Chunked.
9. Benchmarks: every engine and reference built for the same rustc target and target CPU as the shipped binary for
   that host, in the same run → design §16.2.
10. HTTP Garden (GPL-3.0) studied for bug classes only; no file or payload copied → design §6.1.
11. Factual correction: design §6.2 "Bun's HTTP client (Zig)" → "Bun's HTTP client (Rust since bun-v1.4,
    picohttpparser for heads)" → design §6.2.

Also applied from the security review: S1 (204/304 with TE or CL > 0 not reused, design §5.2 Bodies), S2 (residual
client-side-desync risk, design §18; idle detection deferred to the Task 6 note, design §5.1), S4/S5/D9/S18 (undici
`DispatchHook` rules, design §9.2, enforced in P4), S5 (headers deadline not extended by interim heads, design §3.2),
S8 (`Content-Length` never sizes an allocation, design §11.3), S14 (JS header decoding, design §9.3), S17 (CONNECT
scope, design §5.2). Design §21 lists them in one line.

## 11. Divergences for the undici transport (P4)

The undici transport is built in P4; these rows seed its published divergence table (design §9.2). "Hook" means the
undici `DispatchHook` re-check, **enforced in P4**; "table" means listed as a divergence. Source:
`.superpowers/sdd/2026-11-06-p0-foundations/task-4-note-findings-security.md` (undici v8.12.4, llhttp 9.4.3 at
`45c8699`).

| Id | Input | Native | undici (llhttp) | P4 resolution |
|---|---|---|---|---|
| D1 | `Transfer-Encoding: chunked\t` (and `:\tchunked`) | chunked | close-delimited (llhttp #876: only SP accepted after `chunked`) — spliced body | hook: TE accepted only if ASCII-case-insensitive `chunked` followed by SP*; anything else → `PROTOCOL/smuggling-risk`, socket destroyed before body bytes; vectors carry `expect.undici` |
| D2 | HTTP/1.0 + `Transfer-Encoding: chunked` | smuggling-risk | chunked (the hook cannot see the version) | table |
| D3 | `HTTP/2.0`, `HTTP/0.9` status lines | malformed | accepted | table |
| D4 | status 000–099, 600–999 | malformed | accepted (< 200 treated as interim) | hook rejects outside 100–599 |
| D5 | unsolicited `100 Continue` then 200 | 100 skipped | socket destroyed ("bad response") | decide one behaviour in P4; table until then |
| D6 | endless 1xx stream | malformed after 10 heads / 64 KiB | unbounded; timer refreshed per 1xx | hook counts interim heads ≤ 10 and cumulative head bytes ≤ 64 KiB; headers deadline not refreshed (design §3.2, §9.2) |
| D7 | 204/304 with CL > 0 or TE | keepAlive false | not reused (`onHeadersComplete`) | parity after S1 |
| D8 | leading CRLF before a status line (incl. after a 100) | malformed | accepted | table |
| D9 | reason with CTL/NUL/DEL/ESC | malformed | accepted; `statusText` carries the bytes | hook validates `statusMessage` (`FIELD_CONTENT` only) |
| D10 | TE then CL | smuggling-risk | `HPE_INVALID_CONTENT_LENGTH` → malformed | map that code by its reason string (design §9.2); Task 4 adds a `TODO(#7)` "P1a importer: map `HPE_INVALID_CONTENT_LENGTH` by reason before importing `transfer-encoding.md`" to `docs/notes/vectors.md` |
| D11 | 1xx heads totalling > 64 KiB, each < 64 KiB | `TOO_LARGE/headers` | accepted | hook tracks cumulative head bytes |
| D12 | two TE fields | smuggling-risk | last field decides | hook rejects > 1 TE before the body |
| D13 | `Proxy-Connection: close` | keepAlive true (ignored) | honoured | table |
| D14 | any 1xx except 101 with TE or CL ≠ 0 | smuggling-risk (S16) | no body read (undici never reads a 1xx body) | table |
| D15 | trailing OWS / obs-text values | byte-exact, ASCII OWS only | — | verify parity through the JS surface (design §9.3, `obs-text-value-preserved`) |
| D16 | head between 16 and 64 KiB | accepted (64 KiB limit) | undici `maxHeaderSize` defaults to 16 KiB | **verify in P4:** pass `maxHeaderSize: 65536`, map `UND_ERR_HEADERS_OVERFLOW` to `TOO_LARGE/headers`, check how undici counts (status line, CRLF, interim heads) |
| D17 | more than 100 fields | `PROTOCOL/malformed-response` | no field-count limit | **verify in P4:** the hook counts fields per head |
| D18 | reason / status line over 8 KiB | `TOO_LARGE/headers` | bounded only by `maxHeaderSize` | **verify in P4:** whether `statusText` counts toward undici's limit; the hook bounds `statusMessage` |
| D19 | HEAD/204/304 with an unknown `Transfer-Encoding`, or HTTP/1.0 HEAD with TE | `PROTOCOL/smuggling-risk` (TE grammar checked in every head) | succeeds (no body read) | **verify in P4:** the hook applies the TE value rule to bodiless responses too |
| D20 | `Connection: close` in a 1xx head | sticky: final keepAlive false | only the final head's `Connection` counts | **verify in P4:** whether the hook sees 1xx heads (see item (b) below) |
| D21 | DEL (0x7F) in field values | `PROTOCOL/malformed-response` | llhttp strict mode: to check | **verify in P4:** llhttp's value-byte table in strict mode |

Vectors with an undici expectation now: `te-trailing-htab` only (`expect.undici: "PROTOCOL/smuggling-risk"`). P4
adds `interim-11` and `interim-flood-timer` on undici.

**To verify in P4** (not defects; decide before asserting an undici expectation):
- (a) whether llhttp strips leading OWS from a `Transfer-Encoding` value, so `te-leading-htab` (`:\tchunked`) is
  chunked and accepted on undici; `expect.undici` is added to it only once verified;
- (b) whether a `DispatchHook` sees 1xx heads and can stop undici's `headersTimeout` refresh; if not, D6/D11 need
  connector/socket-level byte accounting and blazehttp's own deadline timer;
- (c) llhttp fails duplicate `Content-Length` and `Content-Length` + `Transfer-Encoding` before headers complete, so
  hook parity there comes only from the `HPE_*` mapping (design §9.2), not from the hook.

## 12. Hand-off to Task 6 (connection)

- **EOF accounting is the connection's:** it tracks bytes received for the exchange (interim heads included) and
  applies §6.6's mapping; the parser exposes nothing new. The re-send rule (design §5.1) uses the same fact.
- **Idle-byte detection** (any byte or EOF on an idle pooled connection → discard it) is decided in the Task 6
  research note; options: keep read interest on idle connections, or a non-blocking readiness check / `try_read` at
  checkout. The residual race (an extra response arriving after checkout) is in design §18.
- **Connection vectors** (connection-layer tests, not parser vectors):
  - `two-responses-one-read`: a complete response plus the start of another in one read → first delivered,
    connection closed (leftover bytes).
  - `extra-response-while-idle`: a second response arrives while the connection is idle → discarded, next request
    uses a fresh connection.
  - `early-final-response`: a final response before the request body is fully written → stop writing, deliver,
    close.
  - `interim-flood-timer`: a `103` every 100 ms → `TIMEOUT/headers` at the deadline (interim heads never extend it;
    P1a with timers).
  - `cl-1tib-no-body`: `Content-Length: 1099511627776`, EOF after the head → `PROTOCOL/incomplete-body` with no
    allocation sized from the header (parser level: accepted as `ContentLength`).
  - `tls-close-delimited-no-close-notify` → `PROTOCOL/truncated` and `tls-close-delimited-with-close-notify` → ok
    (P1a, with TLS).
  - `http11-then-http09` (curl test1479): the leftover after the first response is never parsed as a response.
- **Allocation rule (S8, design §11.3):** `Content-Length` never sizes an allocation; read buffers grow by received
  bytes through `try_reserve`; `u64` → `usize` only via `try_from`, never `as`. Fail-fast on a known
  `Content-Length` above `maxContentLength` (without `Content-Encoding`) is the body path's decision (design §11.1).

## 13. For P1a chunked decoding (funky chunks, confirmed against primary sources)

Verified on 2026-11-06 against the advisories and fix commits:
- **Netty CVE-2025-58056** (GHSA-fghv-69vj-qj49; `io.netty:netty-codec-http` < 4.1.125.Final and 4.2.0.Alpha1 to
  < 4.2.5.Final): a bare LF ended the chunk-size line inside a chunk extension. Fix commit
  [`edb55fd`](https://github.com/netty/netty/commit/edb55fd8e0a3bcbd85881e423464f585183d1284) ("Prevent HTTP
  request/response smuggling via chunk encoding") is in `HttpObjectDecoder`, the base of **both** `HttpRequestDecoder`
  and the client's `HttpResponseDecoder`: chunk-size lines must end in CRLF (`InvalidChunkExtensionException`), the
  delimiter after chunk data must be exactly CRLF (`InvalidChunkTerminationException`: spill rejected), and initial and
  header lines are strict CRLF by default (`io.netty.handler.codec.http.defaultStrictLineParsing`, default `true`).
- **Kestrel CVE-2025-55315** (GHSA-5rrx-jjjq-q2r5; ASP.NET Core ≤ 8.0.20, ≤ 9.0.9, 10.0 RC1; CVSS 9.9): the advisory
  gives no bytes; the fix commit
  [`b50ab865`](https://github.com/dotnet/aspnetcore/commit/b50ab865ec4dcd3d6ac30221288cc36bf189d300) ("Fix chunked
  request parsing") shows it: `ParseExtension` searched only for CR (`PositionOf(ByteCR)`) and treated any `\rX` as
  extension content; it now searches CR or LF and throws `BadChunkExtension` for an unpaired `\r` or `\n`. Opt-out
  switch: `Microsoft.AspNetCore.Server.Kestrel.EnableInsecureChunkedRequestParsing`. Server (request) side only.
- **Go CVE-2025-22871** (GO-2025-3563; fixed in go1.23.8/go1.24.2): `net/http/internal` `chunkedReader` accepted a
  bare LF ending a chunk-size line; the same reader decodes client response bodies. go1.27.3's
  `TestChunkInvalidInputs` covers bare LF in the size line, in data, in the extension, and `\r\r\n`.
- **Funky chunks** (J. B. Weikop, 2025-06-18, w4ke.info — the primary write-up, previously cited only through a
  secondary source): classes TERM.EXT / EXT.TERM (bare LF in an extension: terminator for one hop, content for the
  other), `\rX` and bare `\r` variants, and SPILL (bytes after the declared size before CRLF), with TERM.SPILL and
  SPILL.TERM combinations; affected implementations include the above plus AIOHTTP (CVE-2024-52304), h11
  (CVE-2025-43859), Apache Traffic Server (CVE-2024-53868), Google Classic ALB (CVE-2025-4600), Ktor
  (CVE-2025-29904). Recommendation: only CRLF ends a chunk line; reject CTLs in extensions; reject oversized chunks.
  The write-up targets request smuggling; a client is exposed the same way when a proxy between it and the origin
  frames a response differently.
- HTTP Garden chunk classes to cover in P1a vectors: chunk sizes with `0x`, `_`, `+`, `-` prefixes or a longest-valid
  prefix (`0_2e`, `Z`); empty chunk size; `\r\r` in extension whitespace; `\rX` terminating a chunk line; trailer
  section ended by `\r\nXX`.

P1a chunked rules therefore: §5.2 Chunked plus §10 items 6 and 8; vectors from Go `chunked_test.go`, curl tests
207/266/1416/1417/1499/2106, the classes above, and llhttp's chunked suite.

## 14. Engineering and research survey (blogs, talks, write-ups)

| Source | Idea / finding | Status |
|---|---|---|
| Rust Performance Book, "Bounds Checks" (N. Nethercote) — https://nnethercote.github.io/perf-book/bounds-checks.html | iterate instead of index; slice before loops; assert ranges; bounds checks "can affect performance … though less often than you might expect" | **verified** by the asm audit (§8.5); rules in §6.7 |
| Cloudflare, "Improving PicoHTTPParser further with AVX2" (V. Krasnov, 2014-12-18) — https://blog.cloudflare.com/improving-picohttpparser-further-with-avx2/ | AVX2 replacement for `pcmpestri` | **unverified** for us; the idea and its numbers are recorded in `ideas.md` only (P1a `engine-simd`) |
| Cloudflare, "Fixing request smuggling vulnerabilities in Pingora OSS deployments" (2026-03-09) — https://blog.cloudflare.com/pingora-oss-smuggling-vulnerabilities/ | CVE-2026-2833: bytes passed to the backend before its `101`; CVE-2026-2835: HTTP/1.0 + TE and multiple-TE request framing; CVE-2026-2836: cache poisoning through an insecure-by-default cache key. All request-side; a permissive own parser "to handle non-RFC traffic" became the risk | **verified** (post text); consistent with our TE-on-1.0, single-TE and `101` rules |
| Cloudflare, "How we built Pingora" (2022) — https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/ | own HTTP library for leniency; 70% less CPU than the NGINX/Lua service (whole proxy, not the parser) | not applicable to the parser (whole-system numbers) |
| httparse source (S. McArthur et al.), `src/simd/*` | the value scan validates and stops at CR in one pass; NEON/AVX2 blocks; SWAR fallback | **verified** (source read; G adopts the fused pass; NEON vs SWAR measured within 4% on our heads) |
| Bun blog / release notes — https://bun.sh/blog | search found no post on Bun's client head parsing (only e.g. v1.0.10 moving `node:http` wrapper code to native, seen as a search result); the technique is picohttpparser (verified in Bun's source, §2) | not applicable |
| Discord engineering | no post on HTTP/1 parsing found (search 2026-11-06); the Go→Rust post is about GC latency | not applicable |
| tokio / hyper authors | no dedicated parsing post found (search) | the `u64` version compare in httparse is **verified** in source; our status prefix check is the same cost class |
| Kettle 2022 / 2025, Klein 2006, HTTP Garden, T-Reqs, HDiff | §4.2 | verified (primary texts) except HDiff (description via the HTTP Garden paper) |
| PortSwigger research on `Expect`/0.CL (2025) | interim heads as a desync vector | verified (first draft); §5.2 interim limits |

## 15. Sources

Official Rust documentation (§6.3):
- `slice::split_last`, `get`, `trim_ascii` (since 1.80; trims `u8::is_ascii_whitespace`), `eq_ignore_ascii_case`,
  `split_at_checked` (stable since 1.80.0, returns `None` if `mid > len`), `as_chunks` (stable since 1.88.0; "a
  remainder slice with length strictly less than `N`"; panics only for `N == 0`, a compile-time constant):
  https://doc.rust-lang.org/1.99.1/std/primitive.slice.html — verified (page read; all compiled in the prototype on 1.99.1).
- Slice patterns: https://doc.rust-lang.org/1.99.1/reference/patterns.html#slice-patterns — verified (compiled).
- `u64::from_le_bytes`, `checked_mul`, `checked_add`, `wrapping_sub`: https://doc.rust-lang.org/1.99.1/std/primitive.u64.html
  — verified (compiled).
- `u8::is_ascii_whitespace`, `is_ascii_digit`: https://doc.rust-lang.org/1.99.1/std/primitive.u8.html — verified.
- `Iterator::position`: https://doc.rust-lang.org/1.99.1/std/iter/trait.Iterator.html#method.position — verified.
- `std::hint::black_box`: https://doc.rust-lang.org/1.99.1/std/hint/fn.black_box.html — verified (used in the measurement).
- `core::ops::Range`: https://doc.rust-lang.org/1.99.1/std/ops/struct.Range.html — verified.
- Const functions: https://doc.rust-lang.org/1.99.1/reference/const_eval.html#const-functions — verified (table builder
  compiled as `const fn`).
- Indexing panics: https://doc.rust-lang.org/1.99.1/reference/expressions/array-expr.html#array-and-slice-indexing-expressions
  — verified.
- `unsafe_code` lint: https://doc.rust-lang.org/1.99.1/rustc/lints/listing/allowed-by-default.html#unsafe-code — verified.
- rustc `--emit asm`, `-C target-cpu`: https://doc.rust-lang.org/1.99.1/rustc/command-line-arguments.html#--emit-specifies-the-types-of-output-files-to-generate,
  https://doc.rust-lang.org/1.99.1/rustc/codegen-options/index.html#target-cpu — verified (used).
- Clippy lints `indexing_slicing` and `chunks_exact_to_as_chunks`:
  https://rust-lang.github.io/rust-clippy/rust-1.99.0/index.html#indexing_slicing,
  https://rust-lang.github.io/rust-clippy/rust-1.99.0/index.html#chunks_exact_to_as_chunks — verified (both fired on
  the prototype with Clippy 1.99; the second without opting in).
- Rust Performance Book, "Bounds Checks": https://nnethercote.github.io/perf-book/bounds-checks.html — verified.
- `memchr` 2.8.3 (used by the scratch prototype only; not an `engine-core` dependency after review round 1): https://docs.rs/memchr/2.8.3/memchr/ ("only x86_64, wasm32 and aarch64 targets have
  vector accelerated implementations"; std used only for runtime CPU detection) — verified.

RFCs: RFC 9110 §5.1, §5.5, §5.6.1–5.6.3, §7.6.1, §8.6, §15.2 (https://www.rfc-editor.org/rfc/rfc9110); RFC 9112 §2.3,
§4, §5, §6.1–6.3, §7, §7.1.1, §9.6 (https://www.rfc-editor.org/rfc/rfc9112) — verified against the rules quoted.

Advisories (GitHub Advisory Database via `gh api /advisories`, RustSec advisory-db files, vendor pages) — verified:
- RUSTSEC-2021-0078 / CVE-2021-32715 / GHSA-f3pg-qwvg-p99c: https://rustsec.org/advisories/RUSTSEC-2021-0078.html
- RUSTSEC-2021-0079 / CVE-2021-32714 / GHSA-5h46-h7hh-c6x9: https://rustsec.org/advisories/RUSTSEC-2021-0079.html
- RUSTSEC-2021-0020 / CVE-2021-21299 / GHSA-6hfq-h8hq-87mf: https://rustsec.org/advisories/RUSTSEC-2021-0020.html
- httparse GHSA-f67m-9j94-qv9j: https://github.com/advisories/GHSA-f67m-9j94-qv9j
- CVE-2022-32213 GHSA-5689-v88g-g6rv; CVE-2022-32214 GHSA-q5vx-44v4-gch4; CVE-2022-32215 GHSA-5492-mr68-4m2h;
  CVE-2022-35256 GHSA-rc2m-q589-vpqx; CVE-2023-30589 GHSA-cggh-pq45-6h9x; CVE-2024-27982 GHSA-hg6j-8h7m-3w3j;
  CVE-2020-8287 GHSA-f33f-hhx9-6j4m; CVE-2025-43859 GHSA-vqfr-h8mv-ghfj:
  https://github.com/advisories/<GHSA id>
- Netty CVE-2025-58056: https://github.com/advisories/GHSA-fghv-69vj-qj49, fix
  https://github.com/netty/netty/commit/edb55fd8e0a3bcbd85881e423464f585183d1284
- ASP.NET Core CVE-2025-55315: https://github.com/dotnet/aspnetcore/security/advisories/GHSA-5rrx-jjjq-q2r5,
  https://github.com/dotnet/announcements/issues/371, https://github.com/dotnet/aspnetcore/issues/64033, fix
  https://github.com/dotnet/aspnetcore/commit/b50ab865ec4dcd3d6ac30221288cc36bf189d300
- Go CVE-2025-22871: https://pkg.go.dev/vuln/GO-2025-3563
- curl CVE-2023-38039 ("HTTP headers eat all memory"): https://curl.se/docs/CVE-2023-38039.html
- Go issues 39017 (`Content-Length: +3`) and 8840 (`keep-alive, close`): https://github.com/golang/go/issues/39017,
  https://github.com/golang/go/issues/8840
- Bun security advisories (none on head parsing): https://github.com/oven-sh/bun/security/advisories
- Pingora CVE-2026-2833/2835/2836: https://blog.cloudflare.com/pingora-oss-smuggling-vulnerabilities/
- llhttp issues #876 and #877 (open): https://github.com/nodejs/llhttp/issues/876,
  https://github.com/nodejs/llhttp/issues/877 — verified (issue text read).

Research (security) — read 2026-11-06:
- J. Kettle, "HTTP/1.1 must die: the desync endgame" (2025): https://portswigger.net/research/http1-must-die —
  verified.
- J. Kettle, "Browser-Powered Desync Attacks" (2022): https://portswigger.net/research/browser-powered-desync-attacks
  — verified (full text read; §4.2).
- B. Kallus, P. Anantharaman, M. Locasto, S. W. Smith, "The HTTP Garden: Discovering Parsing Vulnerabilities in
  HTTP/1.1 Implementations by Differential Fuzzing of Request Streams" (arXiv 2405.17737v1, 2024):
  https://arxiv.org/html/2405.17737 — verified (full text read); code and `TROPHIES.md`
  https://github.com/narfindustries/http-garden (GPL-3.0) — verified.
- B. Jabiyev, S. Sprecher, K. Onarlioglu, E. Kirda, "T-Reqs: HTTP Request Smuggling with Differential Fuzzing"
  (CCS 2021, pp. 1805–1820): https://github.com/bahruzjabiyev/t-reqs — verified (repository and grammar read; the
  paper's scope confirmed through the HTTP Garden paper).
- K. Shen et al., "HDiff: A Semi-automatic Framework for Discovering Semantic Gap Attack in HTTP Implementations"
  (DSN 2022), DOI 10.1109/DSN53405.2022.00014 — paper text not accessible; scope taken from the HTTP Garden paper.
- A. Klein, "HTTP Response Smuggling" (2006-02-20): https://seclists.org/webappsec/2006/q1/240 — verified.
- J. B. Weikop, "Funky chunks: abusing ambiguous chunk line terminators for request smuggling" (2025-06-18):
  https://w4ke.info/2025/06/18/funky-chunks.html — verified (primary; §13).

Upstream issues (read on 2026-11-06): httparse [#85](https://github.com/seanmonstar/httparse/issues/85),
[#29](https://github.com/seanmonstar/httparse/issues/29); picohttpparser
[#89](https://github.com/h2o/picohttpparser/issues/89); h11 [#16](https://github.com/python-hyper/h11/issues/16) —
verified (titles/bodies read).

Measurement: scratch prototypes `parser-bench` (A, C, CS, C2, F, G, httparse 1.10.1) and `asm-audit`, run
2026-11-06 on the owner's M2 (macOS and Linux arm64 container) — verified; numbers in §8. Not committed (scratch).

## 16. Deferred (with phase)

- **x64 numbers** (G vs httparse on AVX2 hosts): when the owner's x64 VM exists (P0 if it arrives before the Task 4
  PR, else P1a), with the committed benchmark (§8.1).
- **NEON/AVX2 `token_len`/`value_len`** in `engine-simd`, bitmask walk, SWAR name fast path: P1a (`ideas.md`); the
  ≤ 0.85× target is a risk (§8.4).
- **Chunked decoding rules and vectors:** P1a (§13, §10 items 6 and 8).
- **Timer and TLS connection vectors** (`interim-flood-timer`, TLS close-delimited): P1a (§12).
- **undici divergences D1–D15:** P4 (§11).
- **Porting the upstream suites as vector files:** P1b (§9.8).
