# `ResponseParser`: research note (Task 4)

Design §5.1 (component), §5.2 (rules), §6 (learning from proven code), §7 (speed), §11.5 (O(head bytes), offsets
only). Access date for every link: **2026-11-06**. "Verified" means checked against the pinned source, an official
doc page, or a measurement run for this note; "unverified" ideas are listed only in `docs/perf/ideas.md`.

## 1. Summary and decision

**Decision: a line-at-a-time incremental scanner (Design C below).** `memchr` finds each LF in bytes not searched
before; each completed line is validated exactly once with `const [bool; 256]` tables (a safe-Rust SWAR fast path for
field values), and its framing facts (`Content-Length`, `Transfer-Encoding`, `Connection`) are folded into a few
scalar fields of `ResponseParser` as soon as the line completes. Resumption state is two offsets (`line_start`,
`scan_from`) plus those facts. No allocation, no header map, no rescanning: every head byte is searched once by
`memchr` and validated once.

Why: it was the fastest resumable design measured (49 ns for the 3-header head, 319 ns for a 20-header ~1 KB head,
**0.75× and 0.71× of httparse 1.10.1 with NEON** in the same run, §7), it is linear under 1-byte feeding (httparse,
which re-parses from the start, is quadratic: 67 ms vs 0.18 ms for a 60 KB head fed byte by byte), and its three scan
primitives (`find_lf`, `token_len`, `is_field_value`) are exactly the functions `engine-simd` replaces in P1a, with no
change to the parser.

Interface: the brief's interface is kept, with four clarifications (§6.6): `Keep-Alive` is not extracted in P0
(no field and no consumer until the P1a pool); derives on the public types; contract misuse returns
`Engine(Internal)` instead of panicking; the tables and scan primitives live in `h1/grammar.rs` from the start.

## 2. References studied

All at their latest published release on 2026-11-06 (tags resolved with `git ls-remote` / the GitHub releases API;
picohttpparser publishes no releases, so its `master` HEAD is pinned, as libh2o is in §2).

| Project | Release | Commit | Files studied |
|---|---|---|---|
| picohttpparser (C) | none (HEAD) | `dd573d8fa9264c646a400e96ce9f07dea6f156e4` | [`picohttpparser.c`](https://github.com/h2o/picohttpparser/blob/dd573d8fa9264c646a400e96ce9f07dea6f156e4/picohttpparser.c), [`test.c`](https://github.com/h2o/picohttpparser/blob/dd573d8fa9264c646a400e96ce9f07dea6f156e4/test.c) |
| httparse (Rust) | `v1.10.1` (crates.io 1.10.1) | `9f29e79f9832dbd0ae5220acb17c1866745bdecd` | [`src/lib.rs`](https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/src/lib.rs), [`src/simd/swar.rs`](https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/src/simd/swar.rs), [`src/simd/neon.rs`](https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/src/simd/neon.rs), [`src/simd/avx2.rs`](https://github.com/seanmonstar/httparse/blob/9f29e79f9832dbd0ae5220acb17c1866745bdecd/src/simd/avx2.rs) |
| llhttp (C via llparse) | `v9.4.3` (generated C: `release/v9.4.3`) | `45c8699d0ca8431ab366c8706e613b2e2ac62c04` | [`src/llhttp/http.ts`](https://github.com/nodejs/llhttp/blob/45c8699d0ca8431ab366c8706e613b2e2ac62c04/src/llhttp/http.ts), [`src/native/http.c`](https://github.com/nodejs/llhttp/blob/45c8699d0ca8431ab366c8706e613b2e2ac62c04/src/native/http.c), generated [`src/llhttp.c`](https://github.com/nodejs/llhttp/blob/release/v9.4.3/src/llhttp.c) |
| hyper `proto/h1` (Rust) | `v1.12.0` | `7f67699ce32d717081deb7018b4da7d679277ac0` | [`src/proto/h1/role.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/proto/h1/role.rs), [`io.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/proto/h1/io.rs), [`decode.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/proto/h1/decode.rs), [`src/headers.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/headers.rs) |
| h11 (Python) | `v0.16.0` | `1c5b07581f058886c8bdd87adababd7d959dc7ca` | [`h11/_receivebuffer.py`](https://github.com/python-hyper/h11/blob/1c5b07581f058886c8bdd87adababd7d959dc7ca/h11/_receivebuffer.py), [`_readers.py`](https://github.com/python-hyper/h11/blob/1c5b07581f058886c8bdd87adababd7d959dc7ca/h11/_readers.py), [`_abnf.py`](https://github.com/python-hyper/h11/blob/1c5b07581f058886c8bdd87adababd7d959dc7ca/h11/_abnf.py) |
| Go `net/http` | `go1.27.3` | `2aae1000ea1d4b28923b96db28acf4e3b99d6aff` | [`net/http/response.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/response.go), [`transfer.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/transfer.go), [`net/textproto/reader.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/textproto/reader.go) |
| Bun HTTP client | `bun-v1.4.4` | `38afee6c84fcf48c0edb2466aa4b27d287a59c30` | [`src/http/lib.rs`](https://github.com/oven-sh/bun/blob/38afee6c84fcf48c0edb2466aa4b27d287a59c30/src/http/lib.rs), [`src/picohttp/lib.rs`](https://github.com/oven-sh/bun/blob/38afee6c84fcf48c0edb2466aa4b27d287a59c30/src/picohttp/lib.rs), [`scripts/build/deps/picohttpparser.ts`](https://github.com/oven-sh/bun/blob/38afee6c84fcf48c0edb2466aa4b27d287a59c30/scripts/build/deps/picohttpparser.ts) |
| curl | `curl-8_23_0` | `3163ec065643ba20709f190111f42c3e1fe95273` | [`lib/http.c`](https://github.com/curl/curl/blob/3163ec065643ba20709f190111f42c3e1fe95273/lib/http.c) (`http_parse_headers`, `http_header_c`, `http_header_t`) |

**Note on Bun:** at `bun-v1.4.4` Bun's source is Rust (0 `.zig` files in the tree); its client still parses heads
with picohttpparser (C, pinned to commit `f4d94b48…` with a chunked-decoder patch) through an FFI wrapper. The §6.2
"Bun's HTTP client (Zig)" row is therefore studied as "Bun's client, Rust + picohttpparser". (verified: tree listing
and files above)

### What each does (verified against the pinned files)

| Project | Strategy | Partial input | Strictness gaps vs §5.2 | Notes / bugs |
|---|---|---|---|---|
| picohttpparser | Scan-then-validate per token: `findchar_fast` (SSE4.2 `pcmpestri` range match) for runs, 8× unrolled scalar `IS_PRINTABLE_ASCII` otherwise; status digits parsed inline | Not resumable: re-parses from byte 0; `last_len` makes `is_complete` look for the blank line only in new bytes (anti-slowloris), so total ≤ 2 passes | Accepts bare LF (`get_token_to_eol`), any `HTTP/1.x` minor digit, multiple SP after version, obs-fold (returns name `NULL`), status `1xx`–`5xx` by first digit only | Issue [#89](https://github.com/h2o/picohttpparser/issues/89) (NULL buffer SEGV, 2025). `pcmpestri` is a microcoded, high-latency instruction (httparse issue [#85](https://github.com/seanmonstar/httparse/issues/85) discusses SSE4.2 benchmark fairness) |
| httparse | Same shape as pico in Rust; `Bytes` cursor with `unsafe` `advance`; SWAR 8-byte blocks for URI/value, NEON/SSE4.2/AVX2 16/32-byte blocks; `u64` compare for `HTTP/1.x` | Not resumable: `Status::Partial` → caller re-parses all; hyper adds `is_complete_fast` (scan new bytes for the blank line first) | Accepts bare LF and status codes 000–999; obs-fold, spaces before colon and invalid lines are opt-in configs | GHSA-f67m-9j94-qv9j (`mem::uninitialized` unsound `Header`), issue [#29](https://github.com/seanmonstar/httparse/issues/29) (status without SP — now accepted, matching our §21 decision) |
| llhttp | Generated byte-at-a-time state machine (llparse), resumable at any byte, callback spans; SSE4.2/NEON/wasm-simd only for header-value spans | Fully resumable | Strict by default; leniency flags exist; only 100/101/102/103/204/304 skip the body (`llhttp__after_headers_complete`), so another 1xx such as 150 is parsed as having a body | CVEs below; open issues [#876](https://github.com/nodejs/llhttp/issues/876) (2026-08-23: `Transfer-Encoding: chunked<HTAB>` not recognised as chunked in responses → close-delimited, **response smuggling**) and [#877](https://github.com/nodejs/llhttp/issues/877) (bare LF after request line in strict mode) |
| hyper h1 client | httparse into a `SmallVec` of indices, then a `HeaderMap` (allocations), then `Client::decoder` | As httparse + `is_complete_fast`; capped by `max_buf_size` | Equal duplicate `Content-Length`s accepted (`content_length_parse_all_values`); TE wins over CL; non-`chunked` last coding → close-delimited; `Connection` scanned per field with sticky `close` | RUSTSEC-2021-0078/0079, RUSTSEC-2021-0020 (below) |
| h11 | Regex over whole lines once the blank line is found; `_multiple_lines_search` resumes the blank-line search | Terminator search resumable; validation once | Accepts bare LF (`\n\r?\n`), obs-fold (joined) | CVE-2025-43859 (lenient chunked bodies); issue [#16](https://github.com/python-hyper/h11/issues/16) (no big-O tests for `ReceiveBuffer`) |
| Go `net/http` | `bufio` + `textproto` line reader, `strings.Cut` on the status line, `fixLength`/`parseTransferEncoding` after the map is built | Blocking reader | Bare LF, obs-fold, any 3-digit status, equal duplicate CLs accepted; TE ignored on HTTP/1.0; TE+CL → TE wins | Errors quote the raw line (`badStringError("malformed HTTP response", line)`), which §5.2 forbids for us |
| Bun | picohttpparser + rejects obs-fold in the wrapper; header dispatch by name hash; strict CL (`parse_content_length_strict`), conflicting CLs rejected | `last_len` offset | Inherits pico's bare-LF acceptance; 1 MiB head cap | Sticky `Connection: close`; 1xx/204/304 → no body |
| curl | Line-at-a-time: `memchr(buf, '\n')`, line copied into a dynbuf, per-line handler dispatched on first letter | Resumable per line (copies) | Bare LF, obs-fold (unfolded), comma lists of equal CLs accepted; non-chunked TE → close-delimited, CL ignored | Copies each line; we keep offsets in the caller's buffer instead |

## 3. What we take, and what we do differently

**Take:** curl's and h11's *line* as the unit of work, found with `memchr(b'\n')` (curl `http_parse_headers`);
pico/httparse's table-driven token and value classes and the idea of validating a whole run before branching;
httparse's SWAR range check (re-derived, §7); hyper/Bun's sticky `Connection: close` across several `Connection`
fields; Go's "one `Transfer-Encoding` field, value exactly `chunked`" rule (`parseTransferEncoding`); Bun's
"1xx/204/304 and HEAD never have a body"; picohttpparser's and hyper's insight that the end-of-head search must only
look at new bytes (`is_complete(last_len)`, `is_complete_fast`).

**Do differently:**
- One strict mode, no leniency flags (llhttp's 10 lenient modes caused #876/#877-style gaps between modes).
- Resumable without re-parsing (httparse/pico/hyper re-parse the whole head on every partial read: measured
  quadratic, §7) and without copying lines (curl).
- Reject bare LF and bare CR everywhere (every reference except llhttp-strict accepts bare LF somewhere).
- Every 1xx except 101 is interim and bodiless (llhttp treats e.g. 150 as having a body).
- Duplicate `Content-Length` rejected even when equal (hyper, Go, curl, Bun accept equal duplicates).
- `Transfer-Encoding` + `Content-Length` rejected (hyper and Go let TE win).
- Framing grammar checked in every head, interim heads included, independent of status (one code path).
- No header map, no per-field records: framing facts are folded during the scan; the JS side re-reads the validated
  block (`headers` range) only on access (§11.5).
- Errors carry only a code (`ErrorCode` has no payload), never bytes (Go echoes lines).

## 4. Security history → §5.2 rule

| Attack class / advisory | Mechanism | Blocking §5.2 rule | Vector (§9) |
|---|---|---|---|
| CVE-2022-32214 (llhttp, GHSA-q5vx-44v4-gch4); PortSwigger "bare LF header termination" | LF alone ends a line | Line endings: CRLF only | `bare-lf-*` |
| CVE-2023-30589 (llhttp, GHSA-cggh-pq45-6h9x) | CR alone ends a line | Line endings: bare CR rejected | `bare-cr-*` |
| CVE-2022-35256 (llhttp, GHSA-rc2m-q589-vpqx) | field not terminated with CRLF | Line endings | `bare-cr-in-value` |
| CVE-2022-32215 (llhttp, GHSA-5492-mr68-4m2h) | multi-line (folded) `Transfer-Encoding` | Header fields: obs-fold rejected | `obs-fold-te` |
| CVE-2024-27982 (Node, GHSA-hg6j-8h7m-3w3j) | space before `content-length` name hides it | Header fields: line starting with SP/HTAB rejected (**clarification needed**, §10) and no whitespace before the colon | `leading-sp-first-field`, `sp-before-colon` |
| CVE-2022-32213 (llhttp, GHSA-5689-v88g-g6rv); RUSTSEC-2021-0020 (hyper, CVE-2021-21299); CVE-2020-8287 (Node, GHSA-f33f-hhx9-6j4m) | TE value parsed loosely; two TE fields, first/last wins | TE: only `chunked`; > 1 TE field rejected | `te-twice`, `te-list`, `te-gzip-chunked` |
| llhttp #876 (open, 2026) | `chunked<HTAB>` not recognised → close-delimited | TE: compare after OWS (SP **and** HTAB) trimming | `te-trailing-htab` (accept), `te-vt` (reject) |
| RUSTSEC-2021-0078 (hyper, CVE-2021-32715) | `Content-Length: +3` accepted | CL: `1*DIGIT` | `cl-plus` |
| CL lists / differing duplicates (RFC 9110 §8.6) | two framings disagree | CL: no lists, no duplicates | `cl-dup-equal`, `cl-list-equal` |
| CL overflow (cf. RUSTSEC-2021-0079 for chunk sizes) | wrap to a small length | CL: must fit `u64` (checked arithmetic) | `cl-u64-max`, `cl-u64-max-plus-1` |
| CL.TE / TE.CL (Kettle; T-Reqs; HDiff) | CL and TE both present | TE with CL rejected | `cl-then-te`, `te-then-cl` |
| TE on HTTP/1.0 (RFC 9112 §6.1) | 1.0 peers ignore TE | TE on HTTP/1.0 rejected | `te-http10` |
| HEAD/204/304 bodies; response queue poisoning (Kettle 2025) | client reads a body the server did not send, or leaves bytes | Bodies: none for HEAD, 1xx, 204, 304; §5.1: bytes left after the response → close | existing llhttp 204/304 cases |
| Expect-based 0.CL / multiple header blocks (Kettle 2025) | interim heads misframed | ≤ 10 interim heads, all bodiless; we never send `Expect` (§5.1) | `interim-10`, `interim-11` |
| Client-side desync (Kettle 2022) | pooled connection reused after misframing | §5.1 reuse rules (close on any leftover byte, any error) | connection tests (Task 6) |
| HTTP/0.9 / garbage before status line | body parsed as a response | Status line: exactly `HTTP/1.0`/`HTTP/1.1` at byte 0 | existing |
| `101` without a requested upgrade | rest of stream treated as another protocol | Status line: `101` rejected | existing |
| Header-size DoS / slowloris | unbounded buffering, re-parsing | 64 KiB head incl. 1xx, 8 KiB status line, 100 fields; linear scan | limit vectors |
| Funky chunks (chunk-ext bare LF; Netty CVE-2025-58056, Kestrel CVE-2025-55315) | chunk-size line ends at LF | **Not in the head parser.** §5.2 Chunked rule must state CRLF-only for chunk-size lines and extensions — **gap**, §10 | P1a chunked vectors |

## 5. Options compared

Common to all: `buf` holds the current message from byte 0 and only grows; a resumable parser keeps state between
calls. n = head bytes, k = bytes per read.

| | A. Byte-at-a-time state machine (llhttp) | B. Find blank line, then validate whole head (pico/httparse + `is_complete_fast`, h11) | **C. Line-at-a-time (chosen)** |
|---|---|---|---|
| Time, whole head in one read | O(n), one branchy step per byte: **73 / 631 ns** (3 / 20 headers) | O(2n): terminator scan + validation; **65 / 447 ns** for httparse (NEON) | O(n): one `memchr` + one validation per byte: **49 / 319 ns** |
| Time, incremental | O(n) total, resumes at any byte | O(n) for the terminator scan + one O(n) validation | O(n) total: new bytes searched once, each line validated once when complete |
| State | state enum + 3–4 offsets + facts | one offset | 2 offsets + facts |
| Allocations | 0 | 0 (httparse needs a caller header array) | 0 |
| Branches | per byte | per byte in validation | per line + per 8-byte word |
| §5.2 enforcement | per byte; errors at the first bad byte | only after the blank line arrives; a bare-LF head never "completes" (needs extra rule to fail early) | per complete line; bare LF detected at every LF; errors at the first bad line |
| Error ordering / split invariance | natural | natural | holds if each line's size check precedes its content check (§6.3) |
| SIMD swap-in (P1a) | hard: per-byte states must be restructured into span skips (llparse does this only for values) | easy | easy: 3 primitives, signatures unchanged |
| Risk | many states → resumption bugs at every byte boundary | double scan; head-bytes rule needs care | partial lines are not validated until their LF arrives (garbage waits for LF, the limit or the timeout) |

Design F (a fused, picohttpparser-shaped single pass, 51 / 399 ns) is faster than plain C but cannot resume without
re-validating the partial line, and C with the SWAR value check beats it; F is not chosen.

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
- `fn find_lf(hay: &[u8]) -> Option<usize>`: `memchr::memchr(b'\n', hay)`.
- `fn token_len(s: &[u8]) -> usize`: length of the leading `TCHAR` run (`s.iter().position(|&b| !TCHAR[usize::from(b)])
  .unwrap_or(s.len())`).
- `fn is_field_value(s: &[u8]) -> bool`: SWAR over `s.chunks_exact(8)`: word `x = u64::from_le_bytes(chunk)`;
  `lt20 = x.wrapping_sub(0x2020…20) & !x & 0x8080…80` (some byte < 0x20, exact for bytes < 0x80);
  `eq7f`: `d = x ^ 0x7F7F…7F; d.wrapping_sub(0x0101…01) & !d & 0x8080…80`. If `lt20 | eq7f == 0` the word is clean;
  otherwise (HTAB, obs-text or a real CTL) re-check that word with the table. The remainder uses the table. The table
  is the only source of truth: a unit test checks `is_field_value` against the table for every byte value at every
  lane and in the remainder. Both formulas may report false positives (falling back to the table) but never miss a
  byte < 0x20 or 0x7F (classic "has less than"/"has zero byte" SWAR, re-derived; verified by the exhaustive test in
  the scratch prototype).
- P1a: `engine-simd` exports the same three signatures (safe API, `unsafe` inside); `grammar.rs` forwards to it.
  Nothing in `parser.rs` changes.

### 6.3 State and algorithm (`parser.rs`)

```rust
pub struct ResponseParser {
    head_request: bool,              // method == Method::Head
    phase: Phase,                    // StatusLine | Fields | Done   (private enum)
    line_start: usize,               // start of the first incomplete line in `buf`
    scan_from: usize,                // [line_start, scan_from) is known to contain no LF
    // per-message facts, reset after every interim head:
    status: u16, version: HttpVersion, headers_start: usize, fields: usize,
    content_length: Option<u64>, chunked: bool, saw_te: bool, conn_close: bool, conn_keep_alive: bool,
    // per-exchange accounting, reset only by `new`/`reset`:
    interim_count: usize, interim_bytes: usize,
}
```

`parse(buf)`:
1. `phase == Done` → `Err(Engine(Internal))` (called again without `reset`).
2. `budget = H1_LIMITS.max_head_bytes - interim_bytes`; `window = &buf[..buf.len().min(budget)]` (via `get`);
   `window.get(scan_from..)` is `None` → `Err(Engine(Internal))` (buffer shrank: contract violation, never a panic).
3. Loop: `find_lf(&window[scan_from..])`.
   - **Found** at absolute `lf`: `line = &window[line_start..lf]`; `line.split_last()` must give `(&b'\r', content)`,
     else `Malformed` (bare LF, including an LF at `line_start`). Then by phase:
     - `StatusLine`: if `lf + 1 - line_start > max_status_line_bytes` → `TooLarge(Headers)` (**size before content**);
       `parse_status(content)` (§6.4); if the status is 1xx and `interim_count == max_interim_responses` →
       `Malformed`; `headers_start = lf + 1`; `phase = Fields`.
     - `Fields` and `content` empty → head ends at `head_len = lf + 1` → step 5.
     - `Fields` otherwise → `field(content)` (§6.5).
     - `line_start = scan_from = lf + 1`; continue.
   - **Not found**: `scan_from = window.len()`; if `buf.len() >= budget` → `TooLarge(Headers)` (the head cannot end
     within the limit); if `phase == StatusLine && window.len() - line_start >= max_status_line_bytes` →
     `TooLarge(Headers)` (the status line needs at least its LF more); else `Partial`.
4. Body bytes are never scanned: the loop stops at the blank line.
5. End of head: if the status is 1xx → `interim_count += 1; interim_bytes += head_len`; reset the per-message facts,
   `line_start = scan_from = 0`, `phase = StatusLine`; return `Interim { consumed: head_len }`. Otherwise compute
   framing and keep-alive (§6.5), `phase = Done`, return `Complete(ResponseHead { status, version, head_len,
   headers: headers_start..head_len - 2, framing, keep_alive })`.

Limits count every byte including CR and LF: a head is accepted iff `head_len ≤ 65,536 − interim_bytes`; a status line
iff its length including CRLF is ≤ 8,192. This is what makes whole and split parsing agree: lines are processed in
order as they complete inside the window; a line that completes is size-checked before content-checked; a line that
crosses the window never completes, in either mode, so it yields `TooLarge` in both.

`headers` covers every field line with its CRLF and excludes the blank line, i.e. the `"name: value\r\n"*` shape of
Task 5's `RequestParts::headers`. With no fields it is empty (`start == end`).

`new(method)`/`reset(method)` set every field to its initial value (`reset` is `*self = Self::new(method)`).

### 6.4 Status line (`content` = line without CRLF)
`content.split_at_checked(8)` → version `b"HTTP/1.1"` | `b"HTTP/1.0"` (case-sensitive, RFC 9112 §2.3), else
`Malformed`. Rest must be `[b' ', d1, d2, d3]` (empty reason, owner decision §21) or `[b' ', d1, d2, d3, b' ',
reason @ ..]` with every `reason` byte in `FIELD_VCHAR` (so a reason may start with SP, as the imported llhttp case
requires). Digits ASCII, value 100–599, `101` → `Malformed`. Anything else (HTAB separator, 2 or 4 digits, missing
SP) → `Malformed`.

### 6.5 Field lines, framing facts, framing and keep-alive
Per completed field line `content` (non-empty), in this order:
1. `fields += 1`; `fields > max_header_fields` → `Malformed`.
2. `n = token_len(content)`; `n == 0` (covers a leading SP/HTAB — obs-fold or a hidden first field — and a leading
   colon) or `content[n] != b':'` (covers whitespace before the colon and a missing colon) → `Malformed`.
3. `raw = &content[n + 1..]`; `!is_field_value(raw)` → `Malformed` (bare CR, NUL, CTL, DEL).
4. `value = raw.trim_ascii()`. After step 3 the only ASCII whitespace that can remain is SP and HTAB, so this is
   exactly OWS trimming (RFC 9110 §5.5). Keep the order: never trim before validating.
5. Dispatch on `name.len()` then `eq_ignore_ascii_case` (field names are case-insensitive, RFC 9110 §5.1):
   - 14 `content-length`: `content_length.is_some() || saw_te` → `SmugglingRisk`; then `value` must be `1*DIGIT`
     folded with `checked_mul(10)`/`checked_add`, else (empty, sign, inner space, comma list, overflow) →
     `Malformed`; store.
   - 17 `transfer-encoding`: `saw_te || content_length.is_some() || version == Http10` → `SmugglingRisk`;
     `saw_te = true`; `value.eq_ignore_ascii_case(b"chunked")` else `SmugglingRisk`; `chunked = true`.
   - 10 `connection`: for each `value.split(|&b| b == b',')` element, `trim_ascii`, then `close` → `conn_close`,
     `keep-alive` → `conn_keep_alive` (case-insensitive); empty and other elements are ignored.
   - anything else (including `Keep-Alive`, `Proxy-Connection`, `Upgrade`): ignored.

These checks run in every head, 1xx included; in a 1xx head the facts are then discarded.

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
  return `Err(Engine(Internal))`; no code path panics (`get`, `split_last`, `split_at_checked`, checked arithmetic).
- EOF is not a parser input. The caller (and the test helper) maps "EOF while `Partial`" to
  `Network(ClosedBeforeResponse)` when no byte of the head has arrived and to `Protocol(MalformedResponse)` otherwise
  (the imported case `invalid / Invalid space after start line` already expects this). Task 6 must use the same
  mapping (§10).
- Reason phrase: not exposed; it stays in the head bytes (`0..head_len`) the completion carries (Task 6
  `ResponseData::head`), from which the JS side can read `statusText`.

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

The first failing line wins; within a line the order is §6.3 step 3 then §6.5 steps 1–5. Both are byte-order
deterministic, so every split of an input gives the same result.

## 8. Performance plan and budget

**Measured (verified, scratch prototype, not product code):** `scratchpad/parser-bench` (prototypes of A, B
(httparse), C, C + SWAR, F; each asserted equal to httparse on both heads and byte-by-byte split-invariant), Apple M2,
macOS, rustc 1.99.1, `lto = "fat"`, `codegen-units = 1`, `std::hint::black_box`, 9 runs × 2 M iterations, median.
Heads: 3-header (`content-type`, `content-length: 847`, `date`; 109 B) and a 20-header head (the 3 + 17
`x-bench-header-NN` fields, 993 B; the real `/1k-h20` cell is defined by Task 9).

| Design | 3 headers | 20 headers | 1-byte feed of a 60 KB head |
|---|---|---|---|
| C line-at-a-time, table values | 58.4 ns | 454 ns | 184 µs total |
| **C + SWAR values (chosen)** | **49.4 ns** | **319 ns** | linear (same loop) |
| C with `memchr_iter` | 59.3 ns | 457 ns | — |
| A byte state machine | 73.1 ns | 631 ns | 141 µs total |
| F fused single pass (not resumable) | 51.4 ns | 399 ns | — |
| httparse 1.10.1 (NEON) + same facts, uninit headers | 65.2 ns | 447 ns | 67,200 µs (re-parse per call) |

**Budget (P0):** ≤ 60 ns for the 3-header head and ≤ 0.35 ns per head byte for the 20-header head on the M2
reference, and **≤ 0.85× httparse + facts in the same run** on every host (the portable gate; hyper's head cost is
httparse plus a `HeaderMap`, so this keeps the parser well inside the 5–25% headroom of §1-F). At ≈ 5 µs CPU per
request (§1-F) the parser is ≈ 1% of a request; the win that matters is not allocating (hyper ≈ 9 allocations per
request).

**How it is measured:** P0 has no `criterion` (§16.2 per-component budgets start in P1a). In P0: (1) the
implementation is checked against the numbers above by re-running the scratch harness recipe (heads and method
recorded here) against the real `ResponseParser`, reported in the PR; (2) the G0 harness (Task 7) reports CPU µs per
request for the `/1k` and `/1k-h20` cells. In P1a: `crates/engine-core/benches/h1_parse.rs` (`criterion`) with the
two heads, a 1-byte feed and an httparse comparison, gating the budget; results in `docs/perf/journal.md`.

## 9. Additional test cases (beyond the brief)

Own vectors in `blazehttp-rules.json`; each is a full input followed by EOF (§`vectors.md`). "→" gives the expected
native result.

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

Parser-level unit tests (not vectors): contract violations return `Engine(Internal)`; `is_field_value` equals the table
for every byte at every lane; `reset` after `Complete`; a 64 KiB head fed 1 byte at a time completes (linearity is
shown by the P1a criterion bench, not by a wall-clock assertion, which would be flaky).

Fuzz (`h1_response`): the brief's split-equality property, plus "accept ⇒ httparse (default strict config) accepts with
the same status, version, field list and head length" as a differential oracle (httparse as a fuzz-crate dependency
only; the fuzz crate is outside the workspace and never shipped).

### Research still to do (§9)
- Port the HTTP Garden response-side and T-Reqs/HDiff differential cases as vectors (not yet read beyond abstracts).
- Port Go `response_test.go`/`transfer_test.go`, hyper `role.rs` tests, httparse response tests and curl response
  tests as vector files (studied for behaviour only so far).

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

### Research still to do (§8)
- Linux/Docker arm64 and x64 numbers (only macOS M2 measured); confirm C + SWAR stays ahead of A and httparse there.
- Assembly audit (e.g. `cargo-show-asm`) that table lookups and the SWAR loop have no bounds checks in the real code.

## 11. Sources

Official Rust documentation (§6.3):
- `slice::split_last`, `get`, `trim_ascii` (since 1.80; trims `u8::is_ascii_whitespace`), `eq_ignore_ascii_case`,
  `split_at_checked`, `chunks_exact`: https://doc.rust-lang.org/std/primitive.slice.html — verified (page read for
  `split_last`, `get`, `trim_ascii`, `eq_ignore_ascii_case`; `split_at_checked`/`chunks_exact` verified by compiling the
  prototype on 1.99.1).
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
- `memchr` 2.8.3 (version to lock): https://docs.rs/memchr/2.8.3/memchr/ ("only x86_64, wasm32 and aarch64 targets have
  vector accelerated implementations"; std used only for runtime CPU detection) — verified.

RFCs: RFC 9110 §5.1, §5.5, §5.6.1–5.6.3, §7.6.1, §8.6, §15.2 (https://www.rfc-editor.org/rfc/rfc9110); RFC 9112 §2.3,
§4, §5, §6.1–6.3, §7, §9.6 (https://www.rfc-editor.org/rfc/rfc9112) — verified against the rules quoted.

Advisories (GitHub Advisory Database via `gh api /advisories`, RustSec advisory-db files) — verified:
- RUSTSEC-2021-0078 / CVE-2021-32715 / GHSA-f3pg-qwvg-p99c: https://rustsec.org/advisories/RUSTSEC-2021-0078.html
- RUSTSEC-2021-0079 / CVE-2021-32714 / GHSA-5h46-h7hh-c6x9: https://rustsec.org/advisories/RUSTSEC-2021-0079.html
- RUSTSEC-2021-0020 / CVE-2021-21299 / GHSA-6hfq-h8hq-87mf: https://rustsec.org/advisories/RUSTSEC-2021-0020.html
- httparse GHSA-f67m-9j94-qv9j: https://github.com/advisories/GHSA-f67m-9j94-qv9j
- CVE-2022-32213 GHSA-5689-v88g-g6rv; CVE-2022-32214 GHSA-q5vx-44v4-gch4; CVE-2022-32215 GHSA-5492-mr68-4m2h;
  CVE-2022-35256 GHSA-rc2m-q589-vpqx; CVE-2023-30589 GHSA-cggh-pq45-6h9x; CVE-2024-27982 GHSA-hg6j-8h7m-3w3j;
  CVE-2020-8287 GHSA-f33f-hhx9-6j4m; CVE-2025-43859 GHSA-vqfr-h8mv-ghfj:
  https://github.com/advisories/<GHSA id>
- llhttp issues #876 and #877 (open): https://github.com/nodejs/llhttp/issues/876,
  https://github.com/nodejs/llhttp/issues/877 — verified (issue text read).

Research (security):
- J. Kettle, "HTTP/1.1 must die: the desync endgame" (2025): https://portswigger.net/research/http1-must-die —
  verified (page read: 0.CL, CL.0, Expect-based desync, V-H/H-V, bare-LF termination, response queue poisoning).
- J. Kettle, "Browser-Powered Desync Attacks" (2022): https://portswigger.net/research/browser-powered-desync-attacks
  — unverified (search summary only).
- Kallus et al., "The HTTP Garden" (arXiv 2405.17737): https://arxiv.org/abs/2405.17737, code
  https://github.com/narfindustries/http-garden — unverified (abstract via search).
- Jabiyev et al., "T-Reqs" (CCS 2021): https://github.com/bahruzjabiyev/t-reqs — unverified.
- Shen et al., "HDiff" (DSN 2022), DOI 10.1109/DSN53405.2022.00014 — unverified.
- Funky chunks (Netty CVE-2025-58056, Kestrel CVE-2025-55315):
  https://community.f5.com/t/http-request-smuggling-using-chunk-extensions-cve-2025-55315/76631 — unverified
  (secondary source); relevant to P1a chunked decoding only.

Upstream issues (read on 2026-11-06): httparse [#85](https://github.com/seanmonstar/httparse/issues/85),
[#29](https://github.com/seanmonstar/httparse/issues/29); picohttpparser
[#89](https://github.com/h2o/picohttpparser/issues/89); h11 [#16](https://github.com/python-hyper/h11/issues/16) —
verified (titles/bodies read).

Measurement: scratch prototype `parser-bench` (C, C+SWAR, A, F, httparse 1.10.1 baseline), run 2026-11-06 on the
owner's M2 — verified; numbers in §8. Not committed (scratch, §6.1: no prototype code in the repo).

### Research still to do (§4, §11)
- Read the primary texts of Kettle 2022 (client-side desync), HTTP Garden, T-Reqs and HDiff (marked unverified above)
  and map any response-side finding not yet in §4.
- The funky-chunks sources are secondary; confirm against the Netty/Kestrel advisories before the P1a chunked note.
- Rust Performance Book (bounds checks) and engineering blogs (Cloudflare, Bun, Discord) were not surveyed for this
  note; nothing found so far suggests they would change the decision (the decision is final unless the x64
  measurement above reverses the ordering).
