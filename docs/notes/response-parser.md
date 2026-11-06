# `ResponseParser`: research note (Task 4)

Design §5.1 (component), §5.2 (rules), §6 (learning from proven code), §7 (speed), §11.5 (O(head bytes), offsets
only). Access date for every link: **2026-11-06**. "Verified" means checked against the pinned source, an official
doc page, or a measurement run for this note; "unverified" ideas are listed only in `docs/perf/ideas.md`.

## 1. Summary and decision

**Decision (final, no longer provisional): a fused span scanner that resumes at any byte (Design G below).** The status
line is found with `memchr` and validated whole. Field lines are scanned in one pass: a `TCHAR` run for the name, the
colon, then a `FIELD_VCHAR` run for the value (safe-Rust SWAR, 8 bytes per step) that stops at the first byte outside
the class, which must be the CR of a CRLF. A partial line is resumed where the scan stopped, using a line-level
sub-state (`LineStart | Name | Value`) and three offsets (`pos`, `line_start`, `colon`); framing facts
(`Content-Length`, `Transfer-Encoding`, `Connection`) are folded into scalar fields when each line completes. No
allocation, no header map, no rescanning: every head byte is examined once.

**What changed since the first draft (commit `5b04853`):** that draft chose Design C (find each LF with `memchr`,
then validate the line). Its ranking came from macOS runs built with rustc's default target CPU for
`aarch64-apple-darwin` (`apple-m1`), which makes httparse ≈ 1.6× slower than the same code built for a generic
`aarch64` CPU (§8.2). On Linux arm64 (the reference host's container, generic target CPU) C + SWAR is 1.05–1.20×
**slower** than httparse on 20-header heads, which breaks the draft's own budget. Design G, written and measured for
this revision, is 0.92–0.96× httparse on 20-header heads and 0.86× on the 3-header head, while staying linear under
1-byte feeding (httparse is quadratic: 0.26 ms vs 67 ms for a 60 KB head fed byte by byte). G is
faster than C on every head on every build measured, so the change does not depend on the target-CPU question.

**What did not change:** the §5.2 rule set, the error mapping (§7), the interface clarifications (§6.6), the files
(`h1/grammar.rs` primitives swapped by `engine-simd` in P1a), and `memchr` as the only new dependency. The primitives
become `find_lf` (status line), `token_len` and `value_len` (span lengths, the shape httparse's NEON/AVX2 code and
Cloudflare's AVX2 picohttpparser use).

Security research and upstream suites (§4, §9.2) found no response-side case that the §5.2 rules miss for the
**head**; they add 46 vectors and three proposed design changes (§10, items 8–10: chunk-extension validation for
P1a, the benchmark target-CPU rule, and HTTP Garden's GPL-3.0 licence). x64 measurement is deferred until the owner's
x64 VM exists (§8.2).

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
| Netty (chunked, P1a) | `netty-4.1.125.Final` / `4.2.5.Final` | fix commit `edb55fd8e0a3bcbd85881e423464f585183d1284` | [`HttpObjectDecoder.java` diff](https://github.com/netty/netty/commit/edb55fd8e0a3bcbd85881e423464f585183d1284) |
| ASP.NET Core Kestrel (chunked, P1a) | 8.0.21 / 9.0.10 | fix commit `b50ab865ec4dcd3d6ac30221288cc36bf189d300` | [`Http1ChunkedEncodingMessageBody.cs` diff](https://github.com/dotnet/aspnetcore/commit/b50ab865ec4dcd3d6ac30221288cc36bf189d300) |

**Note on Bun:** at `bun-v1.4.4` Bun's source is Rust (0 `.zig` files in the tree); its client still parses heads
with picohttpparser (C, pinned to commit `f4d94b48…` with a chunked-decoder patch) through an FFI wrapper. The §6.2
"Bun's HTTP client (Zig)" row is therefore studied as "Bun's client, Rust + picohttpparser". (verified: tree listing
and files above)

### What each does (verified against the pinned files)

| Project | Strategy | Partial input | Strictness gaps vs §5.2 | Notes / bugs |
|---|---|---|---|---|
| picohttpparser | Scan-then-validate per token: `findchar_fast` (SSE4.2 `pcmpestri` range match) for runs, 8× unrolled scalar `IS_PRINTABLE_ASCII` otherwise; status digits parsed inline | Not resumable: re-parses from byte 0; `last_len` makes `is_complete` look for the blank line only in new bytes (anti-slowloris), so total ≤ 2 passes | Accepts bare LF (`get_token_to_eol`), any `HTTP/1.x` minor digit, multiple SP after version, obs-fold (returns name `NULL`), status `1xx`–`5xx` by first digit only | Issue [#89](https://github.com/h2o/picohttpparser/issues/89) (NULL buffer SEGV, 2025). `pcmpestri` is a microcoded, high-latency instruction (httparse issue [#85](https://github.com/seanmonstar/httparse/issues/85); Cloudflare 2014, §12) |
| httparse | Same shape as pico in Rust; `Bytes` cursor with `unsafe` `advance`; the value scan validates and stops at the first non-value byte (the CR) in one pass; SWAR 8-byte blocks, NEON/SSE4.2/AVX2 16/32-byte blocks; `u64` compare for `HTTP/1.x` | Not resumable: `Status::Partial` → caller re-parses all; hyper adds `is_complete_fast` (scan new bytes for the blank line first) | Accepts bare LF and status codes 000–999; obs-fold, spaces before colon and invalid lines are opt-in configs | GHSA-f67m-9j94-qv9j (`mem::uninitialized` unsound `Header`), issue [#29](https://github.com/seanmonstar/httparse/issues/29) (status without SP — now accepted, matching our §21 decision) |
| llhttp | Generated byte-at-a-time state machine (llparse), resumable at any byte, callback spans; SSE4.2/NEON/wasm-simd only for header-value spans | Fully resumable | Strict by default; leniency flags exist; only 100/101/102/103/204/304 skip the body (`llhttp__after_headers_complete`), so another 1xx such as 150 is parsed as having a body | CVEs below; open issues [#876](https://github.com/nodejs/llhttp/issues/876) (2026-08-23: `Transfer-Encoding: chunked<HTAB>` not recognised as chunked in responses → close-delimited, **response smuggling**) and [#877](https://github.com/nodejs/llhttp/issues/877) (bare LF after request line in strict mode) |
| hyper h1 client | httparse into a `SmallVec` of indices, then a `HeaderMap` (allocations), then `Client::decoder` | As httparse + `is_complete_fast`; capped by `max_buf_size` | Equal duplicate `Content-Length`s accepted (`content_length_parse_all_values`); TE wins over CL; non-`chunked` last coding → close-delimited; `Connection` scanned per field with sticky `close`; mixed `\n\r\n` terminator accepted (`test_parse_accepts_lf_crlf_terminator`) | RUSTSEC-2021-0078/0079, RUSTSEC-2021-0020 (below) |
| h11 | Regex over whole lines once the blank line is found; `_multiple_lines_search` resumes the blank-line search | Terminator search resumable; validation once | Accepts bare LF (`\n\r?\n`), obs-fold (joined) | CVE-2025-43859 (lenient chunked bodies); issue [#16](https://github.com/python-hyper/h11/issues/16) (no big-O tests for `ReceiveBuffer`) |
| Go `net/http` | `bufio` + `textproto` line reader, `strings.Cut` on the status line, `fixLength`/`parseTransferEncoding` after the map is built | Blocking reader | Bare LF, obs-fold, any 3-digit status (incl. `000`, `999`), `HTTP/1.2` and `HTTP/2.0` status lines, equal duplicate CLs accepted; TE ignored on HTTP/1.0; TE+CL → TE wins; CL capped at `int64` | Errors quote the raw line (`badStringError("malformed HTTP response", line)`), which §5.2 forbids for us |
| Bun | picohttpparser + rejects obs-fold in the wrapper; header dispatch by name hash; strict CL (`parse_content_length_strict`), conflicting CLs rejected | `last_len` offset | Inherits pico's bare-LF acceptance; 1 MiB head cap | Sticky `Connection: close`; 1xx/204/304 → no body |
| curl | Line-at-a-time: `memchr(buf, '\n')`, line copied into a dynbuf, per-line handler dispatched on first letter | Resumable per line (copies) | Bare LF, obs-fold (unfolded), comma lists of equal CLs accepted; `identity, chunked` accepted as chunked; non-chunked TE → CL ignored, close-delimited | Rejects CR and NUL inside header lines (tests 2105, 262); copies each line |

## 3. What we take, and what we do differently

**Take:** httparse's and picohttpparser's fused value scan — validate the value class and stop at the first byte
outside it, which must be the CR — re-derived in safe Rust as SWAR over `as_chunks::<8>()` (§6.2); llhttp's
resumability at any byte, but with one sub-state per *line part* instead of per byte; pico/httparse's table-driven
token and value classes; hyper/Bun's sticky `Connection: close` across several `Connection` fields; Go's "one
`Transfer-Encoding` field, value exactly `chunked`" rule (`parseTransferEncoding`); Bun's "1xx/204/304 and HEAD never
have a body"; Netty's and Kestrel's 2025 strict-CRLF fixes for chunk lines (P1a, §10 item 8); Cloudflare's
bitmask-and-`tzcnt` structure as the starting point for `engine-simd` (P1a, unverified, `ideas.md`).

**Do differently:**
- One strict mode, no leniency flags (llhttp's 10 lenient modes caused #876/#877-style gaps between modes; Netty
  and Kestrel both kept opt-out switches for their 2025 fixes).
- Resumable without re-parsing (httparse/pico/hyper re-parse the whole head on every partial read: measured
  quadratic, §8) and without copying lines (curl).
- Reject bare LF and bare CR everywhere (every reference except llhttp-strict and post-2025 Netty accepts bare LF
  somewhere).
- Every 1xx except 101 is interim and bodiless (llhttp treats e.g. 150 as having a body).
- Duplicate `Content-Length` rejected even when equal (hyper, Go, curl, Bun accept equal duplicates).
- `Transfer-Encoding` + `Content-Length` rejected (hyper and Go let TE win; curl lets TE win for any value).
- Framing grammar checked in every head, interim heads included, independent of status (one code path).
- No header map, no per-field records: framing facts are folded during the scan; the JS side re-reads the validated
  block (`headers` range) only on access (§11.5).
- Errors carry only a code (`ErrorCode` has no payload), never bytes (Go echoes lines).

## 4. Security history → §5.2 rule

### 4.1 Mapping

| Attack class / advisory / research | Mechanism (response side) | Blocking §5.2 rule | Vector (§9) |
|---|---|---|---|
| CVE-2022-32214 (llhttp, GHSA-q5vx-44v4-gch4); Klein 2006 §1 (LF-only terminators in browsers and proxies); PortSwigger 2025 bare-LF | LF alone ends a line | Line endings: CRLF only | `bare-lf-*` |
| CVE-2023-30589 (llhttp, GHSA-cggh-pq45-6h9x); Klein 2006 §3 (CR-only in IE and Sun proxy); HTTP Garden "`\r` treated as a line terminator", "`\r\n\rX` ends the head" | CR alone ends a line or the head | Line endings: bare CR rejected | `bare-cr-*`, `cr-x-terminator`, `status-bare-cr-separator` |
| CVE-2022-35256 (llhttp, GHSA-rc2m-q589-vpqx) | field not terminated with CRLF | Line endings | `bare-cr-in-value` |
| CVE-2022-32215 (llhttp, GHSA-5492-mr68-4m2h); HTTP Garden "`Transfer-\r\nEncoding`" | multi-line (folded or split) `Transfer-Encoding` | Header fields: obs-fold rejected; token name then `:` | `obs-fold-*`, `name-split-across-lines` |
| CVE-2024-27982 (Node, GHSA-hg6j-8h7m-3w3j); HTTP Garden "`\tContent-Length`" | whitespace before the first field name hides it | Header fields: line starting with SP/HTAB rejected (§10 item 1) and no whitespace before the colon | `leading-sp-first-field`, `sp-before-colon` |
| HTTP Garden "`Content-Length\x85`", "`\xefoo`", "`\x00` in names" | non-token bytes in names, stripped or ignored by some parsers | Header fields: token names | `name-obs-text-*`, `ctl-only-name`, `bare-cr-in-name` |
| HTTP Garden "`\x00`/`\r`/`\n` permitted in values", "values truncated at `\x00`", "`\x00` stripped from value ends" | control bytes hide or split values | Header fields: values without CTL except HTAB | `nul-at-value-end`, §9.1 NUL/VT/FF/DEL cases |
| CVE-2022-32213 (llhttp, GHSA-5689-v88g-g6rv); RUSTSEC-2021-0020 (hyper, CVE-2021-21299); CVE-2020-8287 (Node, GHSA-f33f-hhx9-6j4m); HTTP Garden "multiple TE chunked", "`,chunked` distinct from `chunked`", "unknown codings treated as chunked", "`\xa0chunked\xa0`" | TE value parsed loosely; two TE fields | TE: only `chunked`; > 1 TE field rejected | `te-twice`, `te-list`, `te-leading-comma`, `te-nbsp`, `te-obs-text-wrapped`, `te-identity`, `te-identity-chunked` |
| llhttp #876 (open, 2026) | `chunked<HTAB>` not recognised → close-delimited | TE: compare after OWS (SP **and** HTAB) trimming | `te-trailing-htab` (accept), `te-vt` (reject) |
| RUSTSEC-2021-0078 (hyper, CVE-2021-32715); Go issue 39017; HTTP Garden "Python `int`" / "`strtoll(,,0)`" / "longest valid prefix" / "empty CL is 0" / "negative CL" | `Content-Length: +3`, `1_0`, `0x10`, `1Z`, empty, `-48` accepted or misread | CL: `1*DIGIT` after ASCII OWS | `cl-plus`, `cl-underscore`, `cl-hex-suffix`, `cl-empty-then-cl`, `cl-nbsp` |
| CL lists / differing duplicates (RFC 9110 §8.6); Klein 2006 §2 (IE/Apache take the first CL, Firefox/Sun/DeleGate the last); HTTP Garden "conflicting CLs, first/last wins" | two framings disagree | CL: no lists, no duplicates | `cl-dup-equal`, `cl-dup-differ`, `cl-list-equal` |
| CL overflow (cf. RUSTSEC-2021-0079 for chunk sizes) | wrap to a small length | CL: must fit `u64` (checked arithmetic) | `cl-u64-max`, `cl-u64-max-plus-1` |
| CL.TE / TE.CL (Kettle; T-Reqs; HDiff); curl test 2918 (TE wins over CL) | CL and TE both present | TE with CL rejected | `cl-then-te`, `te-then-cl`, `cl-then-te-unknown` |
| TE on HTTP/1.0 (RFC 9112 §6.1); Pingora CVE-2026-2835 class (HTTP/1.0 + TE, request side) | 1.0 peers ignore TE | TE on HTTP/1.0 rejected | `te-http10` |
| Klein 2006 §4–5 (lenient versions: `HTTP/01.1`, `HTTP/ 1.0`, `HTTP /1.0`, `HTTP/foobar`, `HTTP/2.0`; HTTP/0.9 fallback) | garbage parsed as a status line or as a 0.9 body | Status line: exactly `HTTP/1.0`/`HTTP/1.1` at byte 0 | `status-version-variants`, existing |
| Kettle 2022 client-side desync: Akamai "stacked HEAD" (404 with `Content-Length: 432837` and no body, then a spliced response) | client reads a body the server did not send for HEAD, or keeps the spliced bytes | Bodies: none for HEAD, 1xx, 204, 304; §5.1: bytes left after the response → close | imported `content-length / Content-Length reset when no body is received`; Task 6 |
| Kettle 2022 Verisign (chunked response to a bodiless request; CL-framed response to HEAD) | framing disagrees with the request method | Bodies: HEAD never has a body | existing "HEAD with TE chunked" |
| Kettle 2022 pause-based desync (server answers a partial request and keeps the connection) | early response, then stale request bytes | §5.1: a final response before the request body is written → stop writing, close | connection tests (Task 6) |
| Kettle 2025 response queue poisoning; HTTP Garden #47 (invalid chunk, connection kept) | misframing leaves bytes that become the next response | §5.1 reuse rules (close on any leftover byte, any error) | connection tests (Task 6) |
| Expect-based 0.CL / multiple header blocks (Kettle 2025) | interim heads misframed | ≤ 10 interim heads, all bodiless; we never send `Expect` (§5.1) | `interim-10`, `interim-11` |
| `101` without a requested upgrade; Pingora CVE-2026-2833 class (bytes passed through before `101`) | rest of stream treated as another protocol | Status line: `101` rejected | existing |
| Header-size DoS / slowloris | unbounded buffering, re-parsing | 64 KiB head incl. 1xx, 8 KiB status line, 100 fields; linear scan | limit vectors |
| Funky chunks (Netty CVE-2025-58056, Kestrel CVE-2025-55315, Go CVE-2025-22871, h11 CVE-2025-43859) | chunk-size line ends at bare LF or `\rX`; extensions skipped unvalidated; spill | **Not in the head parser.** §5.2 Chunked: §10 items 6 and 8; §11 | P1a chunked vectors |

### 4.2 What the primary texts say about clients parsing responses (verified, read on 2026-11-06)

- **Kettle 2022, "Browser-Powered Desync Attacks".** Response-side material is confined to client-side desync: the
  attacker desyncs the *browser's* connection so a later request receives a response meant for an injected request.
  Concrete client-relevant mechanics: HEAD responses carrying a large `Content-Length` and no body (Akamai), a
  server that answers before reading a body that stays on the socket (CL.0), pause-based desync (Varnish/Apache answer
  a partial request and keep the connection), and the observation that browsers discard a connection that delivers
  more data than expected ("stacked-response problem"). The paper gives no response-header parsing flaw in clients.
  All mechanics are blocked by §5.1 (no pipelining, close on leftover bytes, close on an early final response) plus
  §5.2 Bodies.
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

**Gaps found:** none in the head rules. One in the chunked rules (chunk extensions are "ignored" in §5.2; Kestrel's
CVE-2025-55315 was exactly an ignored extension that hid a bare LF): §10 item 8.

## 5. Options compared

Common to all: `buf` holds the current message from byte 0 and only grows; a resumable parser keeps state between
calls. n = head bytes. Times: Linux arm64 container, 1 CPU, median of 3 runs (§8.2) for the 3-header / synthetic
20-header / proposed `/1k-h20` heads.

| | A. Byte-at-a-time state machine (llhttp) | B. Find blank line, then validate whole head (httparse + `is_complete_fast`) | C. Line-at-a-time: `memchr` LF, then validate the line | **G. Fused span scan, resumable at any byte (chosen)** |
|---|---|---|---|---|
| Time, whole head in one read | O(n), one branchy step per byte: 77 / 666 / 577 ns | O(n) validation (+ O(n) terminator scan when partial): 54 / 292 / 293 ns (httparse NEON) | O(2n): `memchr` + validation per byte: 53 / 352 / 307 ns (C + SWAR) | O(n): each byte examined once, 8 per step in values: **47 / 281 / 271 ns** |
| Time, incremental | O(n) total | O(n²) worst case (re-parse per read; 67 ms for 60 KB fed 1 B at a time) | O(n) total | O(n) total (0.26 ms for 60 KB fed 1 B at a time; more per-call overhead than C's 0.19 ms) |
| State | state enum + 3–4 offsets + facts | one offset | 2 offsets + facts | 4-value sub-state + 3 offsets + facts |
| Allocations | 0 | 0 (httparse needs a caller header array) | 0 | 0 |
| Branches | per byte | per byte in names, per block in values | per line + per 8-byte word | per name byte + per 8-byte value word |
| §5.2 enforcement | errors at the first bad byte | only after the blank line arrives (a bare-LF head never "completes") | at line completion | errors at the first bad byte (status line: at its LF) |
| Split invariance | natural | natural | holds if each line's size check precedes its content check | natural: each byte's verdict depends only on earlier bytes and one byte of CR lookahead |
| SIMD swap-in (P1a) | hard: per-byte states must become span skips | easy | easy: 3 primitives | easy: `token_len`, `value_len` (span lengths, the httparse/Cloudflare shape) |
| Risk | many states → resumption bugs at every byte boundary | double scan; quadratic | slower than httparse on generic `aarch64` (1.05–1.20× on 20 headers) | 4 sub-states (each resumption point is tested by the 1-byte split test) |

Design F (the fused scan without resumption, table-per-byte values, as picohttpparser) is 53 / 412 / 348 ns; G is F made resumable
plus the SWAR value run, and is faster than F everywhere.

## 6. Chosen design (precise)

### 6.1 Files and dependency
- `crates/engine-core/src/h1/mod.rs` (`pub mod parser; pub(crate) mod grammar;` and re-exports).
- `crates/engine-core/src/h1/grammar.rs`: `TCHAR` and `FIELD_VCHAR` tables (`static [bool; 256]` built by a
  `const fn`) and the scan primitives below. Task 5 reuses it (so Task 5 does not have to move tables).
- `crates/engine-core/src/h1/parser.rs`: the types and `ResponseParser`.
- `engine-core` `[dependencies] memchr = "2.8.3"` (allowed infrastructure crate, §14; its `unsafe` stays in the crate,
  `engine-core` keeps `#![forbid(unsafe_code)]`).

### 6.2 Grammar tables and primitives (`grammar.rs`)
- `TCHAR[b]` ⇔ `b` is RFC 9110 §5.6.2 `tchar` (`!#$%&'*+-.^_`|~`, DIGIT, ALPHA).
- `FIELD_VCHAR[b]` ⇔ `b == 0x09 || (0x20..=0x7E).contains(&b) || b >= 0x80` (RFC 9110 §5.5 `field-vchar`/SP/HTAB;
  `reason-phrase` in RFC 9112 §4 uses the same class). CR, LF, NUL, other CTLs and DEL are excluded.
- `fn find_lf(hay: &[u8]) -> Option<usize>`: `memchr::memchr(b'\n', hay)`. Used for the status line only.
- `fn token_len(s: &[u8]) -> usize`: length of the leading `TCHAR` run (`s.iter().position(|&b| !TCHAR[usize::from(b)])
  .unwrap_or(s.len())`).
- `fn value_len(s: &[u8]) -> usize`: length of the leading `FIELD_VCHAR` run. `let (words, tail) =
  s.as_chunks::<8>()`; per word `x = u64::from_le_bytes(*w)`; `lt20 = x.wrapping_sub(0x2020…20) & !x & 0x8080…80`
  (some byte < 0x20, exact for bytes < 0x80); `eq7f`: `d = x ^ 0x7F7F…7F; d.wrapping_sub(0x0101…01) & !d &
  0x8080…80`. If `lt20 | eq7f == 0` the word is clean (advance 8); otherwise find the first byte of that word outside
  the table (`w.iter().position(..)`), and if there is none (HTAB or obs-text false positive) advance 8. The `tail`
  uses the table. The table is the only source of truth: a unit test checks `value_len` against the table-based
  `position` for every byte value at every lane and in the tail. Both formulas may report false positives (falling
  back to the table) but never miss a byte < 0x20 or 0x7F (classic "has less than"/"has zero byte" SWAR, re-derived;
  verified by the scratch prototype's asserts).
- P1a: `engine-simd` exports the same signatures (safe API, `unsafe` inside); `grammar.rs` forwards to it.
  Nothing in `parser.rs` changes.

### 6.3 State and algorithm (`parser.rs`)

```rust
pub struct ResponseParser {
    head_request: bool,              // method == Method::Head
    phase: Phase,                    // StatusLine | LineStart | Name | Value | Done   (private enum)
    pos: usize,                      // [0, pos) of the current message is fully examined
    line_start: usize,               // start of the current line
    colon: usize,                    // the current field line's ':' (meaningful in phase Value)
    // per-message facts, reset after every interim head:
    status: u16, version: HttpVersion, headers_start: usize, fields: usize,
    content_length: Option<u64>, chunked: bool, saw_te: bool, conn_close: bool, conn_keep_alive: bool,
    // per-exchange accounting, reset only by `new`/`reset`:
    interim_count: usize, interim_bytes: usize,
}
```

`parse(buf)`:
1. `phase == Done` → `Err(Engine(Internal))` (called again without `reset`).
2. `budget = H1_LIMITS.max_head_bytes - interim_bytes`; `window = buf.get(..buf.len().min(budget))`;
   `window.get(pos..)` is `None` → `Err(Engine(Internal))` (buffer shrank: contract violation, never a panic).
3. Loop on `phase`, with `rest = &window[pos..]` (via `get`):
   - `StatusLine`: `find_lf(rest)`.
     - Found at absolute `lf`: if `lf + 1 - line_start > max_status_line_bytes` → `TooLarge(Headers)` (**size before
       content**); `window.get(line_start..lf)` must match `[content @ .., b'\r']`, else `Malformed`;
       `parse_status(content)` (§6.4); if the status is 1xx and `interim_count == max_interim_responses` →
       `Malformed`; `headers_start = pos = line_start = lf + 1`; `phase = LineStart`.
     - Not found: `pos = window.len()`; if `window.len() - line_start >= max_status_line_bytes` → `TooLarge(Headers)`;
       else **more** (below).
   - `LineStart`: `match rest`: `[] | [b'\r']` → **more**; `[b'\r', b'\n', ..]` → end of head, `head_len = pos + 2`
     → step 5; `[b'\r', ..]` → `Malformed`; otherwise `fields += 1`, `fields > max_header_fields` → `Malformed`,
     `phase = Name`.
   - `Name`: `n = token_len(rest)`; `n == rest.len()` → `pos = window.len()`, **more**; otherwise the byte after the
     run must be `:` and `pos + n > line_start` (non-empty name), else `Malformed` (covers a leading SP/HTAB —
     obs-fold or a hidden first field — whitespace before the colon, a missing colon, a bare CR/LF in a name, a
     non-token byte); `colon = pos + n`; `pos = colon + 1`; `phase = Value`.
   - `Value`: `e = pos + value_len(rest)`; `match window.get(e..)`: `[] | [b'\r']` → `pos = e`, **more**;
     `[b'\r', b'\n', ..]` → `field(name = line_start..colon, raw = colon + 1..e)` (§6.5), then
     `pos = line_start = e + 2`, `phase = LineStart`; anything else (bare LF, CR not followed by LF, NUL, other CTL,
     DEL) → `Malformed`.
   - **more**: `buf.len() >= budget` → `TooLarge(Headers)` (the head cannot end within the limit); else `Partial`.
4. Body bytes are never examined: the loop stops at the blank line.
5. End of head: if the status is 1xx → `interim_count += 1; interim_bytes += head_len`; reset the per-message facts,
   `pos = line_start = 0`, `phase = StatusLine`; return `Interim { consumed: head_len }`. Otherwise compute framing
   and keep-alive (§6.5), `phase = Done`, return `Complete(ResponseHead { status, version, head_len, headers:
   headers_start..head_len - 2, framing, keep_alive })`.

Limits count every byte including CR and LF: a head is accepted iff `head_len ≤ 65,536 − interim_bytes`; a status line
iff its length including CRLF is ≤ 8,192. Whole and split parsing agree because every verdict depends only on bytes
already in the window (plus one byte of lookahead after a CR, which waits for more input in both modes), bytes are
examined in order, bytes beyond the window are never examined, and the status line — the only part validated as a
unit — is size-checked before content-checked.

`headers` covers every field line with its CRLF and excludes the blank line, i.e. the `"name: value\r\n"*` shape of
Task 5's `RequestParts::headers`. With no fields it is empty (`start == end`).

`new(method)`/`reset(method)` set every field to its initial value (`reset` is `*self = Self::new(method)`).

### 6.4 Status line (`content` = line without CRLF)
`content.split_at_checked(8)` → version `b"HTTP/1.1"` | `b"HTTP/1.0"` (case-sensitive, RFC 9112 §2.3), else
`Malformed`. Rest must be `[b' ', d1, d2, d3]` (empty reason, owner decision §21) or `[b' ', d1, d2, d3, b' ',
reason @ ..]` with `value_len(reason) == reason.len()` (so a reason may start with SP, as the imported llhttp case
requires, and may contain HTAB and obs-text). Digits ASCII, value 100–599, `101` → `Malformed`. Anything else (HTAB
separator, 2 or 4 digits, missing SP) → `Malformed`.

### 6.5 Field lines, framing facts, framing and keep-alive
When a field line completes (§6.3 `Value`, CRLF seen), with `name = window[line_start..colon]` and
`raw = window[colon + 1..e]` (both via `get`; `raw` is already validated):
1. `value = raw.trim_ascii()`. After validation the only ASCII whitespace that can remain is SP and HTAB, so this is
   exactly OWS trimming (RFC 9110 §5.5). Never trim before validating. Non-ASCII bytes (`\x85`, `\xa0`) are not
   whitespace and are never trimmed.
2. Dispatch on `name.len()` then `eq_ignore_ascii_case` (field names are case-insensitive, RFC 9110 §5.1):
   - 14 `content-length`: `content_length.is_some() || saw_te` → `SmugglingRisk`; then `value` must be `1*DIGIT`
     folded with `checked_mul(10)`/`checked_add`, else (empty, sign, inner space, comma list, `_`, `0x`, NBSP,
     overflow) → `Malformed`; store.
   - 17 `transfer-encoding`: `saw_te || content_length.is_some() || version == Http10` → `SmugglingRisk`;
     `saw_te = true`; `value.eq_ignore_ascii_case(b"chunked")` else `SmugglingRisk`; `chunked = true`.
   - 10 `connection`: for each `value.split(|&b| b == b',')` element, `trim_ascii`, then `close` → `conn_close`,
     `keep-alive` → `conn_keep_alive` (case-insensitive); empty and other elements are ignored.
   - anything else (including `Keep-Alive`, `Proxy-Connection`, `Upgrade`, `Trailer`): ignored.

The field count (step `LineStart`) and these checks run in every head, 1xx included; in a 1xx head the facts are then
discarded.

Final head:
- framing: `head_request || status == 204 || status == 304` → `Empty`; else `chunked` → `Chunked`; else
  `Some(n)` → `ContentLength(n)` (also `0`); else `CloseDelimited`.
- keep-alive: `Http11 → !conn_close`; `Http10 → conn_keep_alive && !conn_close`; then `false` if `CloseDelimited`.

### 6.6 Interface (kept, with clarifications)
- Types exactly as in the brief. Derives: `HttpVersion`, `Framing`: `Clone, Copy, Debug, PartialEq, Eq`;
  `ResponseHead`, `ParseStatus`: `Clone, Debug, PartialEq, Eq` (`Range` is not `Copy`). Needed by the split-invariance
  test's `assert_eq!`.
- **`Keep-Alive` is not extracted in P0.** `ResponseHead` has no field for it and the only consumer, the pool's
  idle timeout (§5.1), arrives in P1a; extracting it now would be unused code (§14.2). P1a adds
  `keep_alive_timeout` with its consumer. The brief's sentence listing `Keep-Alive` should read "`Content-Length`,
  `Transfer-Encoding` and `Connection`".
- Contract violations (`parse` after `Complete`/error without `reset`; a `buf` shorter than the previous one)
  return `Err(Engine(Internal))`; no code path panics (§6.7).
- EOF is not a parser input. The caller (and the test helper) maps "EOF while `Partial`" to
  `Network(ClosedBeforeResponse)` when no byte of the head has arrived and to `Protocol(MalformedResponse)` otherwise
  (the imported case `invalid / Invalid space after start line` already expects this). Task 6 must use the same
  mapping (§10).
- Reason phrase: not exposed; it stays in the head bytes (`0..head_len`) the completion carries (Task 6
  `ResponseData::head`), from which the JS side can read `statusText`.

### 6.7 No panics, no bounds checks (verified by an assembly audit, §8.5)
- Every slice by a stored offset (`pos`, `line_start`, `colon`, `e`) uses `get(..)` (→ `Engine(Internal)` on `None`)
  or a slice pattern; never `buf[a..b]`. In the audit this was the **only** construct that left a panic path
  (`core::slice::index::slice_index_fail`) in optimised code, on both aarch64 and x86_64: LLVM cannot relate offsets
  stored in a struct to `buf.len()`. A `get` costs the same compare as the check it replaces, without the panic.
- Scans use iterators (`position`, `all`) and `as_chunks::<8>()` + `u64::from_le_bytes(*w)` (an array, no element
  indexing); the tail comes from `as_chunks`, not `v[n..]`. `chunks_exact(8)` + `c[0..8]` was also check-free after
  optimisation, but only because LLVM proved it; `as_chunks` makes it true by type, and Clippy 1.99's default lint
  `chunks_exact_to_as_chunks` flags the `chunks_exact(8)` form, so the CI's `-D warnings` would reject it anyway.
- Tables are `[bool; 256]` indexed by `usize::from(u8)`: no check is emitted (the index is provably < 256).
- Lookahead uses slice patterns (`[b'\r', b'\n', ..]`) instead of indexing.
- Enforcement: `#![deny(clippy::indexing_slicing)]` in `grammar.rs` and `parser.rs` (a restriction lint; it fired on
  every `[]` in the prototype), with the table lookup in one `#[inline]` helper carrying the only `#[allow]` and a
  comment naming the `u8` bound.

## 7. Error mapping (decided)

| Condition | Error |
|---|---|
| `head_len > 65,536 − interim_bytes`, i.e. no blank line within the window and `buf.len() ≥` budget | `TooLarge(Headers)` |
| status line > 8,192 bytes including CRLF (complete or partial) | `TooLarge(Headers)` |
| second `Content-Length` field (even equal) | `Protocol(SmugglingRisk)` |
| `Content-Length` and `Transfer-Encoding` in one head (either order) | `Protocol(SmugglingRisk)` |
| second `Transfer-Encoding` field | `Protocol(SmugglingRisk)` |
| `Transfer-Encoding` on an HTTP/1.0 response | `Protocol(SmugglingRisk)` |
| `Transfer-Encoding` value other than `chunked` (ASCII case-insensitive, after OWS trimming) | `Protocol(SmugglingRisk)` |
| bare LF, bare CR, CTL/DEL/NUL anywhere in the head | `Protocol(MalformedResponse)` |
| bad version, status digits, separators, status outside 100–599, `101` | `Protocol(MalformedResponse)` |
| non-token name, empty name, whitespace before colon, missing colon, line starting with SP/HTAB | `Protocol(MalformedResponse)` |
| `Content-Length` value not `1*DIGIT` within `u64` after OWS trimming | `Protocol(MalformedResponse)` |
| more than 100 fields in one head; an 11th interim head (at its status line) | `Protocol(MalformedResponse)` |
| EOF with ≥ 1 head byte (caller) / with 0 bytes (caller) | `Protocol(MalformedResponse)` / `Network(ClosedBeforeResponse)` |
| contract violation | `Engine(Internal)` |

The first offending byte wins (for the status line: the first offending complete line, size before content); a
field line's framing check runs when its CRLF arrives. Both are byte-order deterministic, so every split of an input
gives the same result. Note the consequence for CTL bytes in `Transfer-Encoding`/`Content-Length` values: they are
`Malformed` (the value check runs first), not `SmugglingRisk`.

## 8. Performance plan and budget

### 8.1 Prototype
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
`x-bench-header-NN: value-NN-abcdefghijklmnopqrstuv`; 993 B), and the proposed `/1k-h20` layout (§8.3; 794 B).

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
httparse is within 10% of G and faster than C. Which way the shipped
`darwin-arm64` build goes depends on the target CPU we ship; §10 item 9 makes the rule explicit. httparse with NEON and
with SWAR only differ by ≤ 4% on these heads (values are short), so httparse's advantage over C came from its fused
single pass, not from NEON — which is what G adopts.

**x64: deferred** until the owner's x64 VM exists (Task 4 is not blocked: the decision holds on both arm64 builds
measured, and G has no arch-specific code). When the VM exists, re-run `parser-bench` there (same recipe, `--cpus=1`
container) and record the result here and in `docs/perf/journal.md`.

### 8.3 Proposed `/1k-h20` layout (for Task 9)

The 20 fields of a realistic JSON API response behind a CDN, in this order (status line `HTTP/1.1 200 OK`; total head
794 B: names 275 B, values 420 B; the body is the same 847-byte payload as `/1k`):

| Name | Len | Value | Len |
|---|---|---|---|
| `date` | 4 | `Fri, 06 Nov 2026 12:00:00 GMT` | 29 |
| `content-type` | 12 | `application/json; charset=utf-8` | 31 |
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

**P0 (safe scalar Rust, checked against the real `ResponseParser`):** on the Linux arm64 reference container, in the
same run as httparse 1.10.1 (default features) + the same facts:
- 3-header head: ≤ 55 ns and ≤ 0.95× httparse;
- `/1k-h20` head (§8.3): ≤ 0.40 ns per head byte (≤ 318 ns for 794 B) and ≤ 1.05× httparse;
- 60 KB head fed 1 byte at a time: ≤ 1 ms (linear; httparse ≈ 67 ms).

Margins over G's prototype numbers cover what the product adds (window and limit checks, field count, `Interim`
handling). Parity with httparse is enough for P0: hyper's head cost is httparse **plus** a `HeaderMap` and ≈ 9
allocations per request, which we do not have, and at ≈ 5 µs CPU per request (§1-F) the parser is ≈ 1–5% of a
request.

**P1a (with `engine-simd`):** ≤ 0.85× httparse on the `/1k-h20` head on arm64 and x64 (criterion gate), via NEON/AVX2
`token_len`/`value_len`.

**How it is measured:** P0 has no `criterion` (§16.2 per-component budgets start in P1a). In P0: (1) the
implementer re-runs the `parser-bench` recipe of §8.1–8.2 (heads above, same container command) against the real
`ResponseParser` and reports the table in the PR; (2) the G0 harness (Task 7) reports CPU µs per request for the
`/1k` and `/1k-h20` cells. In P1a: `crates/engine-core/benches/h1_parse.rs` (`criterion`) with the three heads, a
1-byte feed and an httparse comparison, gating the budget, built with the shipped target CPU (§10 item 9); results in
`docs/perf/journal.md`.

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
patterns (Reference, "Slice patterns"); Clippy lints `indexing_slicing` and `chunks_exact_to_as_chunks` (§13).

## 9. Additional test cases (beyond the brief)

Own vectors in `blazehttp-rules.json`; each is a full input followed by EOF (`docs/notes/vectors.md`). "→" gives the
expected native result. Accepting cases list the header values that must come back byte-exact.

### 9.1 From the §5.2 rules (first draft)

Line endings and whitespace:
- `bare-cr-in-value`: `Foo: a\rContent-Length: 5\r\n` (CVE-2023-30589) → malformed.
- `bare-lf-in-fields`: one field line ending in `\n` among CRLF lines → malformed; `bare-lf-blank-line`: head ending
  `\r\n\n` → malformed.
- `leading-sp-first-field`: `HTTP/1.1 200 OK\r\n Content-Length: 5\r\n…` (CVE-2024-27982 shape) → malformed;
  same with HTAB.
- `obs-fold-te`: `Transfer-Encoding: chunked\r\n , identity\r\n` (CVE-2022-32215) → malformed.
- `sp-before-colon` / `htab-before-colon` → malformed; `empty-name` (`: x`) and `no-colon` (`Foo bar`) → malformed.
- Bytes: NUL in a name and in a value, DEL in name/value/reason, VT and FF in a value → malformed; obs-text
  (`\x80`, `\xff`) in a value and in the reason → accepted; `\x80` in a name → malformed.

Status line:
- `HTTP/1.1 200OK`, `HTTP/1.1 2000 OK`, `HTTP/1.1 20 OK`, `HTTP/1.1 099`, `HTTP/1.1 600`, `HTTP/1.1 -20`,
  `http/1.1 200 OK`, `HTTP/1.1 200\tOK`, `HTTP/2.0 200 OK`, ` HTTP/1.1 200 OK` → malformed.
- `HTTP/1.1 200  OK` (reason starts with SP) → accepted. `HTTP/1.1 101` with no `Upgrade` → malformed.
- `HTTP/1.1 150 X` then `200` → status 200 (every 1xx except 101 is interim).

Framing (each with its accepting neighbour):
- `cl-plus` `+3` (RUSTSEC-2021-0078), `-1`, `0x10`, `1e3`, `4 2`, empty → malformed; `cl-leading-zeros` `007` →
  7 bytes; OWS with HTAB around the value → accepted; `cl-u64-max` (`18446744073709551615`, no body sent → the
  helper reports incomplete-body) vs `…616` → malformed; `cl-list-equal` `5, 5` → malformed.
- `cl-dup-equal`, `cl-dup-differ` → smuggling-risk.
- `cl-then-te`, `te-then-cl`, `te-twice` (both `chunked`; CVE-2020-8287/RUSTSEC-2021-0020 class), `te-list`
  (`chunked, chunked`), `te-gzip-chunked`, `te-identity`, `te-chunkedx`, `te-chunked-param` (`chunked;q=1`),
  `te-empty`, `te-http10` → smuggling-risk.
- `te-trailing-htab` (`chunked\t`, llhttp #876) and `te-uppercase` (`CHUNKED`) → accepted as chunked (skipped by the
  body helper until P1a like the imported chunked cases, or asserted on `ResponseHead` directly in a unit test);
  `te-vt` (`chunked\x0b`) → malformed.
- `te-on-304` → accepted, no body; `cl-and-te-on-304`, `cl-dup-in-100`, `te-and-cl-in-103` → smuggling-risk (checked
  in every head).
- HEAD with `Transfer-Encoding: chunked` → no body, keep-alive true.

Connection:
- `Connection: Close` (case) → keep-alive false; `Connection: keep-alive, close` on 1.0 → false; a second
  `Connection: close` field after `keep-alive` → false; `Connection: ,close,` → false; `Connection: closed` on 1.1 →
  true; `Connection: Keep-Alive` on 1.0 → true; `Keep-Alive: timeout=5` alone on 1.0 → false.
- `103` with `Connection: close`, then `200` with `Content-Length` → keep-alive true (interim facts discarded).

Limits (exact boundaries, all bytes counted including CRLF):
- status line of 8,192 vs 8,193 bytes; 100 vs 101 fields; head of 65,536 vs 65,537 bytes; 1xx heads plus a final head
  totalling 65,536 vs 65,537; a final status line that fits 8 KiB but not the budget left after interim heads →
  too-large; 10 vs 11 interim heads.

### 9.2 From security research (new: 20 vectors)

Only cases not already in §9.1 or the imported llhttp files. Inputs are written with escapes; unless shown, the
status line is `HTTP/1.1 200 OK\r\n`, other lines end in CRLF, and the head ends with `\r\n`. Source in brackets.
Already covered and therefore not repeated: Kettle 2022's stacked HEAD (imported `content-length / Content-Length
reset when no body is received`), CR as a field terminator (imported `invalid / Headers separated by CR`), `HTTP/01.1`
(imported), unknown codings (`te-identity`, `te-chunkedx`), negative and `0x` lengths, differing duplicate lengths
(`cl-dup-differ`, Klein 2006 §2).

- `te-nbsp` `Transfer-Encoding: \xa0chunked\xa0` and `te-obs-text-wrapped` `Transfer-Encoding: \xffchunked\xff`
  [HTTP Garden: non-ASCII stripped as whitespace] → smuggling-risk (only ASCII SP/HTAB are OWS) (2).
- `te-leading-comma` `Transfer-Encoding: ,chunked` [HTTP Garden; Garden paper §6 "field value parsing"] →
  smuggling-risk.
- `name-obs-text-suffix` `Content-Length\x85: 10` and `name-obs-text-prefix` `\xefoo: bar` [HTTP Garden] →
  malformed (2).
- `cr-x-terminator` `Foo: a\r\n\rZX: y\r\n\r\n` [HTTP Garden "`\r\n\rX` ends the head"] → malformed.
- `name-split-across-lines` `Transfer-\r\nEncoding: chunked` + `Content-Length: 5` [HTTP Garden] → malformed (at the
  first line: no colon).
- `ctl-only-name` `\x00\x01…\x1f "(),/;<=>?@[]{}: whatever` [HTTP Garden] → malformed.
- `cl-underscore` `1_0` and `cl-hex-suffix` `1Z` [HTTP Garden: Python `int`, longest valid prefix] → malformed (2).
- `cl-nbsp` `Content-Length: \xa05` [HTTP Garden "`\xa0`/`\x85` stripped"] → malformed.
- `cl-empty-then-cl` `Content-Length: \r\nContent-Length: 43` [HTTP Garden "empty CL prioritised"] → malformed (the
  first line fails first).
- `cl-octal-like` `Content-Length: 010` with 10 body bytes [HTTP Garden `strtoll(,,0)`] → accepted, 10-byte body
  (unlike `007`, distinguishes decimal from octal).
- `nul-at-value-end` `Evil: evil\x00` [HTTP Garden "`\x00` stripped from value ends"] → malformed (the NUL is the last
  value byte, where trim-before-validate bugs hide it).
- `obs-text-value-preserved` `User: \x85admin\xa0` [HTTP Garden "`\xa0`/`\x85` stripped"] → accepted, value exactly
  `\x85admin\xa0` (non-ASCII is never trimmed).
- `status-version-variants` [Klein 2006 §4]: `HTTP/ 1.1 200 OK`, `HTTP /1.1 200 OK`, `HTTP/foobar 200 OK`,
  `HTTP/1.2 200 OK` → malformed (4).
- `cl-then-te-unknown` `Content-Length: 4400\r\nTransfer-Encoding: gobbledigook` [curl test2918: TE wins] →
  smuggling-risk.

### 9.3 From upstream suites (behaviour only; new: 26 vectors)

Only cases not already in §9.1, §9.2 or the imported files.

Go `net/http` (go1.27.3):
- `eof-mid-fields` `HTTP/1.1 301 Moved Permanently\r\nFoo: bar` + EOF and `eof-after-version` `HTTP/1.1` + EOF
  [`TestReadResponseErrors`] → malformed (EOF rule, §6.6) (2).
- Status codes [`TestReadResponseErrors`]: `20X`, `abcd`, `c8`, `0x12d`, `二百` (UTF-8), ` Unknown` (no code) →
  malformed (6). (Go also accepts `000`, `001`, `999`; §9.1's `099`/`600` already cover our 100–599 bounds.)
- `cl-dup-in-204` (`7`, `8`) and `cl-dup-equal-in-304` (`961`, `961`) → smuggling-risk (Go checks 204/304 too but
  accepts equal values) (2).
- `cl-i64-max-plus-1` `9223372036854775808` [`TestParseContentLength`] → accepted (**deliberate difference**: Go caps
  at `int64`, §5.2 says `u64`); the helper reports incomplete-body.
- `trailer-field-ignored` `Trailer: Content-MD5, Content-Sources` + `Content-Length: 10` → accepted, 10-byte body.
- `connection-keep-alive-close-11` `Connection: keep-alive, close` on 1.1 (Go issue 8840) → keepAlive false.

hyper (v1.12.0, `role.rs` tests):
- `htab-inside-value` `server: hello\tworld` [`parse_header_htabs`] → accepted, value `hello\tworld`.
  (hyper's other client cases — TE `yolo` close-delimited, CL+TE chunked, equal duplicate CLs, `a: b\n\r\n` — are
  behaviours we reject, already covered by `te-identity`, `cl-then-te`, `cl-dup-equal`, `bare-lf-in-fields`.)

httparse (v1.10.1, `mod tests`):
- `reason-trailing-htab` `HTTP/1.1 200 OK\t\r\n` → accepted; `reason-nul` `HTTP/1.1 200 \x00\r\n` → malformed (2).
- `status-bare-cr-separator` `HTTP/1.1 200\rOK\r\n` and `status-two-codes` `HTTP/1.1 99 200 OK` → malformed (2).
- `obs-fold-whitespace-only-line` `Line-Folded-Header: \r\n   \r\n hello there` and `obs-fold-in-empty-field`
  `Line-Folded-Header:   \r\n \r\n \r\n` (`RESPONSE_WITH_OBSOLETE_LINE_FOLDING_AT_START`, `…_IN_EMPTY_HEADER`) →
  malformed (2).
- `bare-cr-in-name` `Access-Control-Allow-Cred\rentials: hello` → malformed.
- `no-ows-after-colon` `foo:bar` → accepted, value `bar`.

curl (curl-8_23_0, `tests/data`, rewritten with CRLF where curl's file uses LF):
- `reason-digits-only` `HTTP/1.1 405 405` [test1466] → accepted.
- `te-identity-chunked` `identity, chunked` [test1493: curl accepts as chunked] and `te-chunked-identity`
  `chunked, identity` [test1495] → smuggling-risk (2).
- `obs-fold-htab-continuation` a non-first field followed by a line starting with HTAB [test1274] → malformed.

Total new: 20 (§9.2) + 26 (§9.3) = **46 vectors**. Not parser vectors: the HTTP/1.1-then-HTTP/0.9 response on one
connection (curl test1479) and the pipelined-bytes cases are connection-layer tests for Task 6.

Parser-level unit tests (not vectors): contract violations return `Engine(Internal)`; `value_len` equals the table
for every byte at every lane and in the tail; `reset` after `Complete`; a 64 KiB head fed 1 byte at a time
completes; every sub-state boundary (a split after each of `\r`, `:`, the last name byte, the first value byte) for
one head of each kind (linearity is shown by the P1a criterion bench, not by a wall-clock assertion, which would be
flaky).

Fuzz (`h1_response`): the brief's split-equality property, plus "accept ⇒ httparse (default strict config) accepts with
the same status, version, field list and head length" as a differential oracle (httparse as a fuzz-crate dependency
only; the fuzz crate is outside the workspace and never shipped).

### 9.4 Port in P1b (design §15 "ported corpora"; not ported now)

Behaviour was read for the cases above; the full suites become vector files in P1b, re-imported at the latest release
then:

| Suite | Tag | Commit | Files | Licence |
|---|---|---|---|---|
| Go `net/http` | `go1.27.3` | `2aae1000ea1d4b28923b96db28acf4e3b99d6aff` | [`response_test.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/response_test.go) (`respTests`, `TestReadResponseErrors`), [`transfer_test.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/transfer_test.go) (`TestParseTransferEncoding`, `TestParseContentLength`), [`internal/chunked_test.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/internal/chunked_test.go) (chunked: P1a) | BSD-3-Clause |
| hyper | `v1.12.0` | `7f67699ce32d717081deb7018b4da7d679277ac0` | [`src/proto/h1/role.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/proto/h1/role.rs) tests: `test_parse_response*`, `test_decoder_response`, `test_parse_response_multiple_connection_headers`, `parse_header_htabs`, `test_client_obs_fold_line`, `test_parse_accepts_lf_crlf_terminator`, `parse_too_large_headers`, `test_h1_client_max_header_size` | MIT |
| httparse | `v1.10.1` | `9f29e79f9832dbd0ae5220acb17c1866745bdecd` | [`src/lib.rs`](https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/src/lib.rs) `mod tests`: `test_response_*`, `RESPONSE_*` statics and their `test_forbid_*`/`test_allow_*` pairs (strict expectations only) | MIT OR Apache-2.0 |
| curl | `curl-8_23_0` | `3163ec065643ba20709f190111f42c3e1fe95273` | [`tests/data`](https://github.com/curl/curl/tree/3163ec065643ba20709f190111f42c3e1fe95273/tests/data) `test{178,249,262,282,1033,1115,1144,1154,1266,1274,1432,1433,1466,1473,1479,1482,1483,1493,1494,1495,1546,1665,1722,1723,2105,2107,2119,2918}`; chunked (P1a): `test{207,266,1416,1417,1499,2106}` | curl (MIT-style) |
| HTTP Garden | none | `b417e806c1b15e8ea0b9312f81a91fbdcbc7a83e` | **Do not port** (GPL-3.0): classes re-derived in §9.2 | GPL-3.0 |

## 10. Proposed design changes (for the owner; the design doc is unchanged)

1. **§5.2 Header fields:** say "a field line starting with SP or HTAB is rejected (obs-fold, or a hidden first field)".
   Today "obs-fold rejected" does not literally cover the first field line, which is the CVE-2024-27982 shape.
2. **§5.2 Transfer-Encoding:** "only the value `chunked`, compared ASCII case-insensitively after OWS (SP and HTAB)
   trimming (RFC 9112 §7)". "Exact value" read literally would reject `Chunked`, which every reference accepts, and the
   trimming rule is where llhttp #876 went wrong.
3. **§5.2 limits:** state that the 64 KiB head and 8 KiB status-line limits count every byte including CRLF
   (`head_len ≤ 65,536`).
4. **§5.2 framing in interim heads:** state that `Content-Length`/`Transfer-Encoding` grammar and conflict rules apply
   to every head, 1xx included (their framing is then ignored).
5. **§8 / Task 6:** define EOF during a head: `NETWORK/closed-before-response` with 0 bytes received,
   `PROTOCOL/malformed-response` with ≥ 1 byte.
6. **§5.2 Chunked (P1a):** extend "CRLF only" explicitly to chunk-size lines, chunk extensions and trailers (funky-chunks
   class: Netty CVE-2025-58056, Kestrel CVE-2025-55315).
7. **Task 4 brief:** drop `Keep-Alive` from the extracted fields until P1a (§6.6).
8. **§5.2 Chunked (P1a): chunk extensions are validated, not just skipped.** Replace "chunk extensions bounded
   (≤ 4 KiB per message) and ignored" with "chunk extensions bounded (≤ 4 KiB per message), validated as RFC 9112
   §7.1.1 `chunk-ext` (no CTL except HTAB; any CR must be followed by LF and end the line) and then ignored; exactly
   CRLF after chunk data (no spill)". Kestrel's CVE-2025-55315 was an extension parser that searched only for CR
   and skipped everything else, so a bare LF inside an "ignored" extension ended the line for the other hop; the fix
   (`b50ab865`) rejects any unpaired CR or LF there. Netty's fix (`edb55fd`) rejects a bare LF in chunk lines and
   anything but CRLF after chunk data. Go's CVE-2025-22871 fix covers the client's chunked reader too.
9. **§16.2 / §7 benchmark rule:** "Per-component micro-benchmarks and their reference comparisons (httparse etc.) are
   built with the exact rustc target and target CPU of the shipped binary for that host (e.g. generic `aarch64` for
   `linux-arm64-gnu`, rustc's default `apple-m1` for `darwin-arm64` unless we set another), both sides in the same
   build." Measured here: the target CPU changed httparse's time by ≈ 1.6× and reversed the ranking against our
   first-draft design.
10. **§6 licences:** §6 says all references are MIT, Apache-2.0, BSD or curl-licence, but HTTP Garden is GPL-3.0.
    State: "HTTP Garden (GPL-3.0) is studied for bug classes only; its files and payloads are not copied; vectors for
    its classes are written as our own." (T-Reqs is MIT; HDiff has no public corpus.)

## 11. For P1a chunked decoding (funky chunks, confirmed against primary sources)

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

## 12. Engineering and research survey (blogs, talks, write-ups)

| Source | Idea / finding | Status |
|---|---|---|
| Rust Performance Book, "Bounds Checks" (N. Nethercote) — https://nnethercote.github.io/perf-book/bounds-checks.html | iterate instead of index; slice before loops; assert ranges; bounds checks "can affect performance … though less often than you might expect" | **verified** by the asm audit (§8.5); rules in §6.7 |
| Cloudflare, "Improving PicoHTTPParser further with AVX2" (V. Krasnov, 2014-12-18) — https://blog.cloudflare.com/improving-picohttpparser-further-with-avx2/ | replace `pcmpestri` (~11-cycle latency, ≈ 1.45 B/cycle cap) with AVX2 range compares → bitmask (`pmovmskb`) for 128 B at a time, walk with `tzcnt`; 1.68–1.79× on Haswell | **unverified** for us (x64 VM pending) → `ideas.md`, P1a `engine-simd` |
| Cloudflare, "Fixing request smuggling vulnerabilities in Pingora OSS deployments" (2026-03-09) — https://blog.cloudflare.com/pingora-oss-smuggling-vulnerabilities/ | CVE-2026-2833/2835/2836: bytes passed on before `101`; HTTP/1.0 + TE; multiple TE headers; a permissive own parser "to handle non-RFC traffic" became the risk | **verified** (advisory text); confirms §5.2 TE-on-1.0, single-TE and `101` rules |
| Cloudflare, "How we built Pingora" (2022) — https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/ | own HTTP library for leniency; 70% less CPU than the NGINX/Lua service (whole proxy, not the parser) | not applicable to the parser (whole-system numbers) |
| httparse source (S. McArthur et al.), `src/simd/*` | the value scan validates and stops at CR in one pass; NEON/AVX2 blocks; SWAR fallback | **verified** (source read; G adopts the fused pass; NEON vs SWAR measured within 4% on our heads) |
| Bun blog / release notes — https://bun.sh/blog | search found no post on Bun's client head parsing (only e.g. v1.0.10 moving `node:http` wrapper code to native, seen as a search result); the technique is picohttpparser (verified in Bun's source, §2) | not applicable |
| Discord engineering | no post on HTTP/1 parsing found (search 2026-11-06); the Go→Rust post is about GC latency | not applicable |
| tokio / hyper authors | no dedicated parsing post found (search); httparse's changelog, seen as a search result, credits ~10% on short requests to a faster version parser | the `u64` version compare is **verified** in source; we compare the 8-byte version slice to two literals, same cost class; the 10% figure is unverified |
| Kettle 2022 / 2025, Klein 2006, HTTP Garden, T-Reqs, HDiff | §4.2 | verified (primary texts) except HDiff (description via the HTTP Garden paper) |
| PortSwigger research on `Expect`/0.CL (2025) | interim heads as a desync vector | verified (first draft); §5.2 interim limits |

## 13. Sources

Official Rust documentation (§6.3):
- `slice::split_last`, `get`, `trim_ascii` (since 1.80; trims `u8::is_ascii_whitespace`), `eq_ignore_ascii_case`,
  `split_at_checked` (stable since 1.80.0, returns `None` if `mid > len`), `as_chunks` (stable since 1.88.0; "a
  remainder slice with length strictly less than `N`"; panics only for `N == 0`, a compile-time constant):
  https://doc.rust-lang.org/std/primitive.slice.html — verified (page read; all compiled in the prototype on 1.99.1).
- Slice patterns: https://doc.rust-lang.org/reference/patterns.html#slice-patterns — verified (compiled).
- `u64::from_le_bytes`, `checked_mul`, `checked_add`, `wrapping_sub`: https://doc.rust-lang.org/std/primitive.u64.html
  — verified (compiled).
- `u8::is_ascii_whitespace`, `is_ascii_digit`: https://doc.rust-lang.org/std/primitive.u8.html — verified.
- `Iterator::position`: https://doc.rust-lang.org/std/iter/trait.Iterator.html#method.position — verified.
- `std::hint::black_box`: https://doc.rust-lang.org/std/hint/fn.black_box.html — verified (used in the measurement).
- `core::ops::Range`: https://doc.rust-lang.org/std/ops/struct.Range.html — verified.
- Const functions: https://doc.rust-lang.org/reference/const_eval.html#const-functions — verified (table builder
  compiled as `const fn`).
- Indexing panics: https://doc.rust-lang.org/reference/expressions/array-expr.html#array-and-slice-indexing-expressions
  — verified.
- `unsafe_code` lint: https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html#unsafe-code — verified.
- rustc `--emit asm`, `-C target-cpu`: https://doc.rust-lang.org/rustc/command-line-arguments.html#--emit-specifies-the-types-of-output-files-to-generate,
  https://doc.rust-lang.org/rustc/codegen-options/index.html#target-cpu — verified (used).
- Clippy lints `indexing_slicing` and `chunks_exact_to_as_chunks`:
  https://rust-lang.github.io/rust-clippy/rust-1.99.0/index.html#indexing_slicing,
  https://rust-lang.github.io/rust-clippy/rust-1.99.0/index.html#chunks_exact_to_as_chunks — verified (both fired on
  the prototype with Clippy 1.99; the second without opting in).
- Rust Performance Book, "Bounds Checks": https://nnethercote.github.io/perf-book/bounds-checks.html — verified.
- `memchr` 2.8.3 (version to lock): https://docs.rs/memchr/2.8.3/memchr/ ("only x86_64, wasm32 and aarch64 targets have
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
  https://w4ke.info/2025/06/18/funky-chunks.html — verified (primary; §11).

Upstream issues (read on 2026-11-06): httparse [#85](https://github.com/seanmonstar/httparse/issues/85),
[#29](https://github.com/seanmonstar/httparse/issues/29); picohttpparser
[#89](https://github.com/h2o/picohttpparser/issues/89); h11 [#16](https://github.com/python-hyper/h11/issues/16) —
verified (titles/bodies read).

Measurement: scratch prototypes `parser-bench` (A, C, CS, C2, F, G, httparse 1.10.1) and `asm-audit`, run
2026-11-06 on the owner's M2 (macOS and Linux arm64 container) — verified; numbers in §8. Not committed (scratch).

## 14. Deferred (with phase)

- **x64 numbers** (G vs C vs httparse on AVX2 hosts): when the owner's x64 VM exists (P0 if it arrives before the
  Task 4 PR, else P1a); §8.2 gives the recipe.
- **NEON/AVX2 `token_len`/`value_len`** in `engine-simd`, bitmask walk, SWAR name fast path: P1a (`ideas.md`).
- **Chunked decoding rules and vectors:** P1a (§11, §10 items 6 and 8).
- **Porting the upstream suites as vector files:** P1b (§9.4).
