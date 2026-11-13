# `RequestWriter` and header templates: research note (Task 5)

Design §4 (request flow: per-request bytes in pooled buffers, header templates), §5.1 (`RequestWriter`: pre-encoded
template bytes, one syscall per request where possible), §5.2 (outgoing request-target), §5.3 (head + body in one
buffer for small requests, vectored writes only for large plaintext bodies), §10 (latin1 header deltas, values
≤ 0xFF, Rust re-validates), §11.2 (templates), §12 (Injection row), §13.1, §16.2. Access date for every link:
**2026-11-07**. **Verified** = checked against the pinned source, an official document, or a measurement run for this
note; **unverified** = everything else (unverified ideas are listed only in `docs/perf/ideas.md`).

## 1. Summary and decision

**Decision: Design B — a pre-encoded, pre-validated header template plus a validated per-request delta, appended
with `extend_from_slice` into one reused `Vec<u8>` sized once with `try_reserve_exact`; `Content-Length` formatted with the
standard library's `format_into` (stable since 1.98.0); head and body in one buffer for one write. The writer returns
`Reuse` from the one framing predicate, and a template only serves the connection authority it was built for.**

- **Template** (built once per client and origin on the control plane): `host: <authority>\r\n` followed by the
  client's default fields, stored as one immutable `Box<[u8]>`. It is validated once when built and records its
  authority, its singleton names and a sensitive flag (§6.2). `write_request` rejects it on a connection to any other
  authority (§6.6).
- **Per request:** `METHOD SP target SP "HTTP/1.1" CRLF` + template + delta + `content-length: N CRLF` (when due) +
  `connection: close CRLF` (only for a body on `GET`/`DELETE`/`OPTIONS`) + `CRLF` + body; returns
  `Reuse::{Allowed, CloseAfterExchange}` (§6.5). Zero allocations once `out` has grown to the connection's working
  size (`Vec::clear` keeps the capacity; a single `try_reserve_exact` per request is a no-op then; the writer never
  shrinks `out`; Task 6 does after the write, only above `MAX_P0_REQUEST_BYTES`, §6.7).
- **Re-validation in Rust on every request** for the bytes that change per request: the target (origin form, only
  bytes WHATWG emits in the path or query it sits in, ≤ 16 KiB, §6.3) and the delta (`token ": " field-content CRLF`
  per line, forbidden names in any case, singleton names at most once across template and delta, and exactly
  `header_lines` lines — the count the JS layer serialised, so a well-formed line smuggled inside a value is a
  mismatch, X3). Every outgoing head passes through this one check — the JS layer (P2), redirects built in Rust (P1b)
  and any future source. undici's history (§4) is five CVEs from the same class, each a new sink that skipped
  validation. **What the check closes:** malformed bytes (bare CR or LF, NUL, other controls, obs-fold) are closed
  in Rust whatever their source. A *well-formed* injected line (`x: a\r\nx-evil: 1`) is closed only when the
  caller's `header_lines` is counted independently of the bytes (one per entry it serialised); a caller that counts
  CRLFs in its own output disables that check, and the writer cannot tell. That count is a contract on
  `RequestParts` (§6.6), and each producer states where its count comes from (P2 §10.4, P1b §10.3).
- **Measured (committed benches, dev, Linux arm64 container on a quiet machine, `docs/perf/journal.md` 2026-11-13
  fix round 2):** the writer costs **≈ 21–23 ns** for a template-only `GET` and **≈ 92–100 ns** for a `POST` with a
  1 KiB JSON body and 3 extra headers — **0.17–0.21× (GET) and 0.29–0.30× (POST)** an emulation of hyper 1.12's
  encoder fed the same `(name, value)` pairs (ours + join / construct+encode; no hyper code runs; target parsed and
  every per-request field built per call; 0.26–0.42× against its clone+encode, the conservative figure), and
  ≈ 0.64–0.69 µs / 1.0–1.2 µs with 5 / 8 allocations for Go 1.27.3 `(*Request).Write`. In one binary, `get` and
  `post1k` are 1.16–1.21× the pre-review writer since fix round 1's safety checks (open, Task 5b, §10.5); `varied`
  is at fix round 1's time. Against the ≈ 5 µs CPU budget of a whole request (design §1-F) the writer is ≈ 0.4–2%.

**Interface changes to the brief** (§6.6), each justified there:
- `HeaderTemplate::new(host, fields, field_lines, sensitive_names)` takes **wire bytes**, in the same
  `("name: value\r\n")*` format as `RequestParts::headers`, and the field count JS serialised (the template's
  counterpart of `header_lines`). Latin1 values cannot round-trip through `&str`, and one format means one validator.
- `write_request(out, parts, template, conn_authority) -> Result<Reuse, ErrorCode>`.
- `RequestParts` gains `header_lines: u16`, the number of fields JS serialised into `headers` (§6.4, X3).
- `pub const MAX_P0_REQUEST_BYTES`: the largest request P0 can write, derived from the spec limits (§6.7, X5).

**Rule changes (§6.5):**
- `content-length: 0` is also sent for an empty `QUERY` (RFC 10008 defines content for it; RFC 9110 §8.6).
- A body on `GET`/`DELETE`/`OPTIONS` gets `connection: close` and is not reused.
- `HEAD` with a body is rejected.

**Error mapping refined (§7):** limits map to `TooLarge`, allocation failure to `Engine(ResourceExhausted)`, and
grammar violations to `Engine(Internal)` as the brief says, with a fixed precedence.

**Deferred with numbers, not opinions (§8.3):** the copy-vs-`writev` threshold for large plaintext bodies.
- On loopback the single copied buffer costs the same as `writev` up to 4 KiB (within noise in 3 runs). `writev`
  wins from 16 KiB in 2 of 3 runs and clearly from 64 KiB (1.6–3.3×). Two separate writes are the worst at 1–4 KiB
  (1.6–2.7×).
- Under TLS, rustls 0.23.46's `write_vectored` allocates a `Vec` per call, so TLS always uses the single buffer.
- In P0, bodies > 64 KiB never reach the writer: Task 6's `try_submit` rejects them with `TooLarge(RequestBody)` (`maxCopiedBodyBytes`, §6.7). So `write_request` always copies.
- The large-body path (`write_head` + `writev`, threshold between 4 and 16 KiB, measured on the reference host) is
  owned by P1b.

## 2. References studied

All at their latest published release on 2026-11-07 (tags resolved with the GitHub API; h2o publishes no releases,
so `master` is pinned). Line ranges are those read for this note.

| Project | Release | Commit | Functions and lines studied |
|---|---|---|---|
| hyper (Rust) | `v1.12.0` | `7f67699ce32d717081deb7018b4da7d679277ac0` | [`proto/h1/role.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/proto/h1/role.rs): `Client::encode` L1215–1262, `Client::set_length` L1343–1471, `set_content_length` L1540–1571, `write_headers` L1629–1636, `FastWrite` L1676–1690, `extend` L1693–1696, `AVERAGE_HEADER_SIZE` L32; [`proto/h1/io.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/proto/h1/io.rs): `Buffered::new` (strategy from `is_write_vectored`) L59–64, `poll_flush` (writev of ≤ 64 slices) L278–312, `poll_flush_flattened` L318–335, `WriteBuf::buffer` L549–584, `WriteStrategy` L649–652; [`proto/h1/encode.rs`](https://github.com/hyperium/hyper/blob/7f67699ce32d717081deb7018b4da7d679277ac0/src/proto/h1/encode.rs): `Encoder::encode` L128–166, `encode_and_end` L224–286 |
| http (Rust, hyper's types) | `v1.5.0` | `16fc9a7b840c2181e7f8b37397c107b0ffcd050d` | [`header/value.rs`](https://github.com/hyperium/http/blob/16fc9a7b840c2181e7f8b37397c107b0ffcd050d/src/header/value.rs): `is_valid` L563–565, integer `From` impls via `itoa` L405–406 |
| reqwest (Rust) | `v0.13.5` | `de55373434f07f42926599dbb5a88550d8e55112` | [`Cargo.toml`](https://github.com/seanmonstar/reqwest/blob/de55373434f07f42926599dbb5a88550d8e55112/Cargo.toml) L120–121: delegates HTTP/1 serialisation to hyper + hyper-util (no own writer) |
| ureq (Rust) | `3.5.1` → `ureq-proto 0.7.1` | `33ca9d35e97b6a10e767fb0fa3139f01d315e89c` / `acb5cc3ee1261709316acd6538be08e82f84f3c6` | [`ureq-proto src/client/sendreq.rs`](https://github.com/algesten/ureq-proto/blob/acb5cc3ee1261709316acd6538be08e82f84f3c6/src/client/sendreq.rs): `try_write_prelude` L190–210, `do_write_send_line` L241–248 (`write!` with `core::fmt`), `do_write_headers` L250–268 |
| httparse | `v1.10.1` | `9f29e79f9832dbd0ae5220acb17c1866745bdecd` | Parser only — no request writer exists. Used here as the request-parse **oracle** in tests and fuzzing (`Request::parse`) |
| curl (C) | `curl-8_23_0` | `3163ec065643ba20709f190111f42c3e1fe95273` | [`lib/http.c`](https://github.com/curl/curl/blob/3163ec065643ba20709f190111f42c3e1fe95273/lib/http.c): `Curl_add_custom_headers` L1808–1942 (value cut at the first newline, `curlx_str_untilnl`), `http_target` L2161+, `http_add_content_hds` L2514–2590 (`Content-Length: %" FMT_OFF_T` via `curlx_dyn_addf`), `http_add_hd` L2927+, `Curl_http` L3082–3165; [`lib/request.c`](https://github.com/curl/curl/blob/3163ec065643ba20709f190111f42c3e1fe95273/lib/request.c): `Curl_req_send` L380–425 (head sent directly if no body, else copied into the 64 KiB `sendbuf` together with body bytes), `req_flush` L298–350; [`lib/curlx/dynbuf.c`](https://github.com/curl/curl/blob/3163ec065643ba20709f190111f42c3e1fe95273/lib/curlx/dynbuf.c): `dyn_nappend` L67–100 (`MIN_FIRST_ALLOC 32`, doubling), `curlx_dyn_addf` L220+; `DYN_HTTP_REQUEST` = 1 MiB cap (`dynbuf.h` L69); [`lib/cf-socket.c`](https://github.com/curl/curl/blob/3163ec065643ba20709f190111f42c3e1fe95273/lib/cf-socket.c) `tcpnodelay` L152–165 |
| Go `net/http` | `go1.27.3` | `2aae1000ea1d4b28923b96db28acf4e3b99d6aff` | [`request.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/request.go): `(*Request).write` L592–784 (Host validation and blanking L611–654, `stringContainsCTLByte(ruri)` L666–668, `fmt.Fprintf` request line and `Host`, bufio wrap L673–689); [`transfer.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/transfer.go): `shouldSendContentLength` L262–284, `writeHeader` L286–343 (`strconv.FormatInt`), `writeBody` L346+; [`header.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/header.go): `writeSubset` L190–225 (sorted keys, `headerNewlineToSpace`, `TrimString`); [`transport.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/transport.go): `writeBufferSize` (4 KiB) L322–327, `validateHeaders` L594–608 (called L628/L634), `bufio.NewWriterSize` L2121, `writeLoop` L2830–2866 (one `Flush` per request); [`bufio/bufio.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/bufio/bufio.go) `(*Writer).Write` L676–698 (large write with an empty buffer bypasses the copy); [`httpguts/httplex.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/vendor/golang.org/x/net/http/httpguts/httplex.go) `ValidHeaderFieldName` L196, `ValidHostHeader` L209+, `ValidHeaderFieldValue` L303–311; tests [`requestwrite_test.go`](https://github.com/golang/go/blob/2aae1000ea1d4b28923b96db28acf4e3b99d6aff/src/net/http/requestwrite_test.go) cases 25 (CTL in URL) and 26 (#40978, `PATCH` → `Content-Length: 0`), trailer cases (#78775) |
| undici (Node) | `v8.12.4` | `779ee3e12cb8c1f365f6e69ee34b088159402d2f` | [`lib/dispatcher/client-h1.js`](https://github.com/nodejs/undici/blob/779ee3e12cb8c1f365f6e69ee34b088159402d2f/lib/dispatcher/client-h1.js): `shouldSendContentLength` L1168–1170, `writeH1` L1203–1397 (`expectsPayload` incl. `QUERY` L1217–1223; head built by string concatenation L1343–1371; `HEAD` → no reuse L1314–1320), `writeBuffer` L1536–1564 (`cork`, head `write(..., 'latin1')`, body `write`, `uncork`); [`lib/core/request.js`](https://github.com/nodejs/undici/blob/779ee3e12cb8c1f365f6e69ee34b088159402d2f/lib/core/request.js): `invalidPathRegex` (rejects code points outside U+0021–U+00FF) L29 + L124, `processHeader` L472–570; [`lib/core/util.js`](https://github.com/nodejs/undici/blob/779ee3e12cb8c1f365f6e69ee34b088159402d2f/lib/core/util.js): `isValidHTTPToken` L738+, `headerCharRegex = /[^\t\x20-\x7e\x80-\xff]/` L759–767; tests `test/headers-crlf.js`, `test/content-length.js` |
| Bun (`fetch` client) | `bun-v1.4.4` | `38afee6c84fcf48c0edb2466aa4b27d287a59c30` | [`src/http/lib.rs`](https://github.com/oven-sh/bun/blob/38afee6c84fcf48c0edb2466aa4b27d287a59c30/src/http/lib.rs): `validate_request_target` L1314–1319 (rejects ≤ 0x20 and 0x7F, **accepts ≥ 0x80**), `write_request` L1413–1429, `write_to_socket` L1488–1507, `get_request_body_send_buffer` L2367–2378 (a `Vec` per request, TLS capped at one record), `build_request` L2513–2680 (header-name hashing, `int_as_bytes` for Content-Length, user `Content-Length`/`Transfer-Encoding` always dropped), `send_initial_request_payload` L3105–3203 (head and body copied into one buffer, one write) |
| h2o client (C) | none (`master`) | `f35c735ebe0657235702c2b91912542052cc05db` | [`lib/common/http1client.c`](https://github.com/h2o/h2o/blob/f35c735ebe0657235702c2b91912542052cc05db/lib/common/http1client.c): `build_request` L747–837 (pool-allocated buffer, `memcpy` appends, no validation), `start_request` L839–895 (`h2o_socket_write` with head + body iovecs) |
| Boost.Beast (C++) | `boost-1.92.0` | `7c1e061f91e2ef542217b76286c314d006c0c8fc` | [`http/impl/fields.hpp`](https://github.com/boostorg/beast/blob/7c1e061f91e2ef542217b76286c314d006c0c8fc/include/boost/beast/http/impl/fields.hpp): `writer` L30–260 (request line `buf_[13]`, one `const_buffer` per field, `buffers_cat_view`), `element` layout L271–335 (each field stored pre-formatted as `name: value\r\n`), `try_create_new_element` L968–994 (only size checks + trim; **no CR/LF check**) |
| itoa (Rust) | `1.0.18` | `af77385d0daf4d0e949e81f2588be2e44f69f086` | [`src/lib.rs`](https://github.com/dtolnay/itoa/blob/af77385d0daf4d0e949e81f2588be2e44f69f086/src/lib.rs) L1–56 (docs: "comes straight from libcore"), `impl_Unsigned` L259–330 (4 digits per loop from a 200-byte pair table, `unsafe` writes) |
| Rust `core::fmt::num` | `1.99.1` | `58cf8f9c35b355199727627d07ae34a4e2846d67` | [`library/core/src/fmt/num.rs`](https://github.com/rust-lang/rust/blob/58cf8f9c35b355199727627d07ae34a4e2846d67/library/core/src/fmt/num.rs): `DECIMAL_PAIRS` L100, `impl_Display` L121–260 (`_fmt_inner`: same 4-digit loop), `format_into` L276/L320 (`#[stable(feature = "int_format_into", since = "1.98.0")]`) |
| tokio | `1.54.1` (not yet in `Cargo.lock`; Task 6 adds it) | `b53b2001ed2ccfd9b87e32a0c6da1a1eebdc80b8` | [`net/tcp/stream.rs`](https://github.com/tokio-rs/tokio/blob/b53b2001ed2ccfd9b87e32a0c6da1a1eebdc80b8/tokio/src/net/tcp/stream.rs): `try_write` L935+, `try_write_vectored` L997+, `is_write_vectored` → `true` L1518–1520 |
| rustls | `v/0.23.46` | `f04804fa13341a9b1dc02b5d1dd8a746f2c4b7fb` | [`rustls/src/conn.rs`](https://github.com/rustls/rustls/blob/f04804fa13341a9b1dc02b5d1dd8a746f2c4b7fb/rustls/src/conn.rs): `PlaintextSink::write_vectored` L392–407 — for > 1 slice it **collects a `Vec<&[u8]>` per call** |

### What each does (verified against the pinned files)

| Project | Head construction | `Content-Length` | Validation at write time | Buffering and syscalls | Body | Notes / bugs |
|---|---|---|---|---|---|---|
| hyper 1.12 | Per request: `extend_from_slice` per piece; target through `Display` (`write!` into `FastWrite`, with a `TODO: … encode without std::fmt`); header loop over a `HeaderMap`; `reserve(30 + n × 30)` | Inserted into the `HeaderMap` as `HeaderValue::from(u64)` (`itoa` inside `http`), then serialised with the others | None — `http` types validated at construction (`is_valid`: `b >= 32 && b != 127 \|\| b == '\t'`) | `TcpStream::is_write_vectored()` is `true`, so `Queue` strategy: `writev` of head + body slices (≤ 64) | Exact / chunked / limited encoders | Clears the request's `HeaderMap` after encoding (L1259): every request carries its own map. User TE without `chunked` is "repaired" by appending `chunked` |
| ureq-proto 0.7.1 | `write!` per line into a caller slice (`core::fmt`) | `write!` | Header values checked by `http` types | Caller's buffer, caller writes | — | — |
| curl 8.23.0 | Per request into a `dynbuf` (`malloc`, doubling from 32 B, 1 MiB cap): request line, ~20 `http_add_hd` steps, custom headers | `curlx_dyn_addf("%" FMT_OFF_T)` (curl's own printf) | Custom header value cut at the first CR/LF (`curlx_str_untilnl`); `Host` from the user list skipped when curl sets one | No body: one `send`. With body: head copied into the 64 KiB `sendbuf`, filled with body bytes, one `send` per flush | Upload reader fills `sendbuf` | `TCP_NODELAY` on by default (`tcpnodelay`). CVE-2014-8150 (CR/LF in a URL sent to a proxy) |
| Go 1.27.3 | `fmt.Fprintf` for request line and `Host`; `User-Agent` default; transfer headers; `writeSubset` sorts keys (pooled sorter) | `strconv.FormatInt` → string concat (allocates) | `validateHeaders` in `roundTrip` (names token, values no CTL except SP/HTAB); `ValidHostHeader` (blank Host if invalid and not via proxy); `stringContainsCTLByte(ruri)`; `headerNewlineToSpace` as a second line of defence | `bufio.Writer` 4 KiB per connection; one `Flush` per request → head + small body in one `write`; a body larger than the free space is written directly | `io.Copy` | CVE-2019-9741 (CRLF via URL), CVE-2023-29406 (Host injection), #78775 (trailer injection) |
| undici 8.12.4 | String concatenation per request: `${method} ${path} HTTP/1.1\r\n`, `host:` (or cached `kHostHeader`), `connection: keep-alive`, user headers | Template literal | At request construction (`processHeader`): token names, `headerCharRegex` values, path limited to U+0021–U+00FF (**allows DEL, `#` and 0x80–0xFF in the path**), forbids `transfer-encoding`, `keep-alive`, `upgrade`, `expect`; one `content-length` | `socket.cork()`, `write(head, 'latin1')`, `write(body)`, `uncork()` → one `writev` | Buffer, stream, iterable, blob | Five CRLF-class CVEs in sinks that skipped validation (§4) |
| Bun 1.4.4 | `Vec` per request sized from an estimate; `extend_from_slice` per piece; default headers chosen by name hash | `int_as_bytes` into a field of the client | `validate_request_target` only (bytes ≤ 0x20 and 0x7F rejected); values validated earlier by `Headers` | Head and as much body as fits in one buffer, one `write` | Bytes or stream | User `Content-Length` and `Transfer-Encoding` always replaced by its own framing |
| h2o (master) | Pool-allocated buffer, `memcpy` per piece | Supplied by caller | None in `http1client.c` | `h2o_socket_write` with head + body iovecs (`writev`) | iovec | — |
| Boost.Beast 1.92 | **Pre-encoded per field**: every field stored as `name: value\r\n` at insertion; serialiser emits one buffer per field | Caller sets the field | Only name/value size checks; values trimmed; **no CR/LF check** | Buffer sequence → `writev` of (method, target, version, every field, CRLF) | Body buffers appended to the sequence | GHSA-8c6g-xf48-2fvx (trailers merged into headers; parser side) |

## 3. What we take, and what we do differently

**Take:**
- Beast's insight — encode once, at insertion, not per request — applied to the **whole default set** as one
  contiguous template (Beast keeps one allocation and one iovec per field).
- Go's and curl's coalescing: head and a small body leave in **one** write (Go's 4 KiB `bufio`, curl's `sendbuf`,
  Bun's spare-capacity copy). hyper's and h2o's `writev` of head + body is kept as the large-body path (§8.3).
- hyper's and Bun's `extend_from_slice` appends into a reused `Vec` (no `core::fmt` on the hot path; hyper's own
  `TODO` asks for the same for the target).
- Go's and undici's content-length rule (`POST`/`PUT`/`PATCH` always; undici adds `QUERY`), Go's Host validation
  (`ValidHostHeader` byte class), Go's `ValidHeaderFieldValue` value class and undici's forbidden-header set.
- curl's and Go's default `TCP_NODELAY` (Task 6).

**Do differently:**
- **Validate in the writer, on every request, every per-request byte** — Go validates in `roundTrip` and again in
  `Request.write` (two places, one missed Host until CVE-2023-29406); undici validates at construction and each new
  sink skipped it; hyper, h2o and Beast do not validate at write time at all.
- **Reject, never repair:** Go replaces CR/LF with SP and blanks an invalid Host; hyper appends `chunked` to a
  user TE; curl truncates at a newline. Silent repair sends something the user did not write (a differential); we
  return an error.
- **No user framing headers at all** (`content-length`, `transfer-encoding`, `te`, …): the writer generates the only
  `content-length` (Bun does the same by dropping them; undici allowed duplicates until CVE-2026-1525).
- **Stricter target than undici and Bun:** only bytes WHATWG emits in that component (no control, SP, DEL,
  0x80–0xFF, `#`, `"`, `<`, `>` anywhere; no `\`, `^`, `` ` ``, `{`, `}` in the path; no `'` in the query), and
  origin-form only until P1b adds absolute-form for proxies (§6.3).
- **Templates bound to one authority and singleton fields enforced** (§6.2, §6.4): no reference checks either at
  write time.
- **Integer formatting from `std`** (`u64::format_into`, the libcore algorithm itoa copies) — no dependency and no
  `unsafe` of our own (§5.3).
- **No per-request allocation:** curl (`dynbuf`), Go (≈ 5–10 allocations, §8.2), Bun (`Vec` per request), hyper
  (`HeaderMap` per request) all allocate; we reuse the connection's buffer.

## 4. Security history → writer rule

### 4.1 Client-side advisories (verified: advisory texts read via the GitHub API, vuln.go.dev JSON, NVD, curl's `vuln.json`)

"Layer" says where the rule is enforced: **JS** = P2's `INVALID_CONFIG` (user-facing), **Rust** = this writer
(`Engine(Internal)`, defence in depth and the only check for bytes produced in Rust).

| Advisory | Mechanism | Writer rule | Layer | Test (§9) |
|---|---|---|---|---|
| undici GHSA-3cvr-822r-rqcc / CVE-2022-31150 (< 5.8.0) | CRLF in header values, path and method | values: `FIELD_CONTENT` only; a CRLF that forms a well-formed extra line breaks the `header_lines` count (X3); target: only bytes WHATWG emits in its component (§6.3); method from a closed enum | JS + Rust | `rejects_header_value_ctl_except_htab`, `rejects_injected_line_by_count`, `rejects_target_with_space_ctl_or_non_ascii` |
| undici GHSA-f772-66g8-q5h3 / CVE-2022-35948 | CRLF via `content-type` (a sink that skipped validation) | every delta byte re-validated in Rust whatever its source closes the malformed case; the well-formed case is closed by the line count only when the sink's caller counts entries, not bytes (X3, §1) | Rust + caller's count | `injection_vectors`, `rejects_injected_line_by_count` |
| undici GHSA-5r9g-qh6m-jxff / CVE-2023-23936 | CRLF via `host` | Host only from the template; `HOST` byte class at template build; user `host` forbidden | JS + Rust | `template_rejects_invalid_host`, `rejects_forbidden_user_headers` |
| undici GHSA-4992-7rv2-5pvq / CVE-2026-1527 | CRLF via the `upgrade` option | `upgrade`/`connection` forbidden; no upgrade path in 1.0 | JS + Rust | `rejects_forbidden_user_headers` |
| undici GHSA-m8rv-5g2x-5cg5 / CVE-2026-15157 (fixed 8.9.0) | CRLF via a blob-like body's `.type` pushed into headers unvalidated | same as CVE-2022-35948: Rust validates the assembled delta, not individual sinks; the line count closes the well-formed case only with an independent count (§1) | Rust + caller's count | `injection_vectors`, `rejects_injected_line_by_count` |
| undici GHSA-2mjp-6q6p-2qxm / CVE-2026-1525 | Two `Content-Length` fields via case variants in a header array | user `content-length` forbidden in any case; exactly one generated | JS + Rust | `rejects_forbidden_user_headers` (case variants), fuzz property "exactly one CL" |
| undici GHSA-8qr4-xgw6-wmr3 / CVE-2022-35949 | `pathname` given as an absolute URL changed the request's origin (SSRF) | target must start with `/`; `//x` is valid origin-form and cannot change Host (§6.3); URL joining is P2's | JS + Rust | `target_must_be_origin_form` (`INVALID_TARGETS`, `VALID_TARGETS`) |
| undici GHSA-3787-6prv-h9w3 / CVE-2024-24758, GHSA-wqq4-5wpv-mx2g / CVE-2023-45143; Go GO-2024-2600 / CVE-2023-45289; curl CVE-2018-1000007 | `Proxy-Authorization`, `Cookie`, `Authorization` or custom credentials kept on a cross-origin redirect | template bound to `conn_authority` (§6.6); P1b rebuilds the template and filters the delta without `sensitiveHeaders` on a cross-origin hop (§10.3) | Rust (now + P1b) | `rejects_template_for_other_authority`; P1b `cross_origin_redirect_strips_template_and_delta_sensitive_fields` |
| undici GHSA-35p6-xmwp-9g52 / CVE-2026-6733 | Unsolicited response on an idle keep-alive socket attached to the next request | not a writer rule → Task 6 (`any byte while idle → close`, design §5.1) | Rust (Task 6) | Task 6 |
| Node CVE-2018-12116 (< 6.15 / 8.14) | Unicode path written as latin1: U+010D U+010A → CR LF | JS writes deltas with `buf.write(str, off, 'latin1')`: code points > 0xFF must be rejected **before** the write (design §10); Rust sees only bytes and rejects CR/LF either way | JS + Rust | P2 test; Rust `injection_vectors` (CR/LF bytes) |
| Go CVE-2019-9741 (GO-2019 net/http) | CRLF in URL query via `NewRequest` | target: no control or SP in any component (§6.3) | Rust | `rejects_target_with_space_ctl_or_non_ascii` |
| Go CVE-2023-29406 / GO-2023-1878 | Host header not fully validated → header/request injection | `HOST` byte class (Go's `validHostByte` set), reject (Go blanks) | Rust | `template_rejects_invalid_host` |
| Go #78775 (go1.27.x) | CR/LF in trailer names/values | no trailers are sent in 1.0; a future chunked writer reuses the field validator | — | deferred (§14) |
| Python CVE-2019-9740 / -9947 / -18348 | CRLF in URL query, path, host (`urllib`) | target: no control or SP in any component; Host byte class | Rust | as above |
| curl CVE-2014-8150 | CR/LF in a URL copied into an absolute-form target sent to a proxy | absolute-form (P1b) gets the same byte check (§10.3) | Rust (P1b) | P1b proxy tests |
| hyper RUSTSEC-2021-0020 / CVE-2021-21299, RUSTSEC-2021-0078 / CVE-2021-32715 | Parser side (TE lists, `+` in CL) | the writer never emits TE or a non-`1*DIGIT` CL (generated from a `u64`) | Rust | fuzz property |
| Boost.Beast GHSA-8c6g-xf48-2fvx | Trailers merged into headers (parser) | n/a for the writer; Beast's serializer has no CR/LF check (§2), a reminder that pre-encoding without validation is unsafe | — | — |

Bun's three advisories (QPACK, brotli, emSign roots) and reqwest/ureq/http (none published) contain no
request-writer issue (verified: GitHub security-advisories API).

### 4.2 Research papers and what they imply for what a client emits (verified where stated)

- **T-Reqs** (Jabiyev, Sprecher, Onarlioglu, Kirda, CCS 2021, pp. 1805–1820; [PDF](https://seclab.nu/static/publications/ccs2021treqs.pdf),
  verified from the PDF text): grammar-based differential fuzzing of 10 servers/proxies/CDNs. Request-line mutation
  classes that caused body-parsing discrepancies (Table 6): **mangled method** (`hEAD`), **distorted protocol**
  (`hTTP/1.1`), **invalid version** (`HTTP/1.19`; ATS forwarded GET bodies "with any decimal version number"),
  **manipulated termination** (SP/HTAB/CRLF before the request line's CRLF), **embedded request lines**. Header
  classes (Table 7): **distorted values** of `Transfer-Encoding`/`Content-Length` (VT, FF, SP, `+`, `,`),
  **manipulated termination** (SP/HTAB after a field's CRLF — an obs-fold), **`Expect`**, **`identity`
  encoding**, **chunked on HTTP/1.0**. Their base request is a **GET with a `Content-Length` body**.
  → Rules: method only from the closed `Method` enum (exact upper case); version only the literal `HTTP/1.1`;
  exactly one SP on each side of the target; no byte after the target but ` HTTP/1.1\r\n`; every field line starts
  with a token byte (never SP/HTAB); `content-length` digits only, generated; no TE, no `Expect`, ever.
- **The HTTP Garden** (Kallus, Anantharaman, Locasto, Smith; [arXiv 2405.17737](https://arxiv.org/abs/2405.17737)):
  coverage-guided differential fuzzing; 39 exploitable discrepancies (abstract verified; detail in
  `docs/notes/response-parser.md`). → The writer's output is checked against a request parser oracle (§9.5).
- **HDiff** (Shen et al., DSN 2022, DOI 10.1109/DSN53405.2022.00014): semantic-gap differential testing from RFC
  grammar. Only the description via the HTTP Garden paper was readable — **unverified** beyond that.
- **Host of Troubles** (Chen, Jiang, Duan, Weaver, Wan, Paxson, CCS 2016): multiple-Host, whitespace-wrapped Host and
  absolute-URI-vs-Host ambiguities enable cache poisoning and filter bypass (abstract-level, via search summaries —
  **unverified** in detail). RFC 9112 §3.2.2 (verified): an origin server receiving absolute-form "MUST ignore the
  received Host header field".
  → Exactly one Host, first, from the template.
  → **Origin-form only** on a direct connection: an attacker-shaped absolute-form target would silently override
  Host at the origin.
  → The same ambiguity applies to other single-valued fields: `authorization`, `cookie`, `content-type` and
  `user-agent` at most once (§6.4).
  → No `#` in the target: some components truncate at a fragment and some do not (§6.3).
- **Kettle, "HTTP Desync Attacks: Request Smuggling Reborn"** (PortSwigger, 2019,
  [link](https://portswigger.net/research/http-desync-attacks-request-smuggling-reborn); background, not re-read —
  unverified here); **"Browser-Powered
  Desync Attacks"** (10 Aug 2022, [link](https://portswigger.net/research/browser-powered-desync-attacks), verified):
  client-side desync needs a server that "must ignore the request's Content-Length"; the client's connection pool is
  then poisoned. **"HTTP/1.1 must die"** (6 Aug 2025, [link](https://portswigger.net/research/http1-must-die),
  verified): 0.CL desyncs from bodies an intermediary ignores, `Expect`-based desyncs ("Simply sending a valid Expect
  header causes a 0.CL desync on numerous different servers"), and the advice "Reject requests that have a body, if the
  method doesn't require one to be present (GET/HEAD/OPTIONS)".
  → Never send `Expect` (design §5.1 already).
  → A body on `GET`/`DELETE`/`OPTIONS` is framed with `content-length`, and the writer adds `connection: close` and
  returns `Reuse::CloseAfterExchange` (§6.5). `HEAD` with a body is rejected. With a forward proxy, P1b rejects bodies
  on `GET`/`HEAD`/`DELETE` (§10.3).
  → **undici, for comparison:** it does not reuse such a connection either, but it still sends
  `connection: keep-alive`, setting `kReset` only after the head is built. It sends `connection: close` only for
  `HEAD`, where `kReset` is set first. Detail in §6.5.
- **RFC 9110 / 9112** (verified, rfc-editor.org text): §5.5 field values with CR, LF or NUL are "invalid and
  dangerous", other CTLs "also invalid"; §7.2 "A user agent that sends Host SHOULD send it as the first field";
  §8.6 Content-Length rule (§6.5 below); §9.3.1 a client "SHOULD NOT generate content in a GET request" unless the
  origin supports it; RFC 9112 §3 `request-line = method SP request-target SP HTTP-version`, §3.2 "No whitespace is
  allowed in the request-target" and Host "identical to that authority component, excluding any userinfo",
  recommended ≥ 8000-octet request lines; §5.1 "No whitespace is allowed between the field name and colon".

## 5. Options compared

### 5.1 Head construction

| | A. Format per request (`core::fmt`) | **B. Template + validated delta, slice appends (chosen)** | C. Per-field pre-encoded buffers + `writev` (Beast) |
|---|---|---|---|
| What | `write!` request line, every default field and `content-length` each time (ureq, Go, hyper's target) | defaults pre-encoded once; per request: 6–9 `extend_from_slice` + one integer format | each field its own pre-encoded allocation; one iovec per field |
| Time | O(head) + per-argument `fmt` dispatch (≈ 20–60 ns each) | O(head) memcpy + O(target + delta) validation | O(fields) iovec setup; kernel copies |
| Space | 1 buffer | 1 buffer (+ template once per client/origin) | iovec array; allocation per field |
| Allocations steady state | 0 (reused `Vec`) | **0** | 0 per request, but 1 per field at insertion; iovec count bounded by `IOV_MAX` (1024) |
| Branches | many (formatting) | few, data-independent except per-line validation | many iovecs → long kernel loop |
| TLS | fine | fine (one contiguous plaintext) | rustls `write_vectored` collects a `Vec` per call (verified) → allocation per request |
| Measured (§8.2, `POST` 1 KiB + 3 headers, runs 2 / 3) | 729 / 385 ns (3.3–3.5×) | **220 / 111 ns** | not measured (rejected on TLS and syscall grounds) |
| Risk | slow; `fmt` cannot carry latin1 bytes as `&str` | template must be validated at build time and must not repeat a delta name (P2 rule, §10.4) | many small segments without `TCP_CORK` |

### 5.2 Rust-side validation of per-request bytes

| | V0. None (trust JS) | V1. Per-byte table (`FIELD_CONTENT`, `TCHAR`) | **V2. `grammar.rs` primitives: `token_len` (table) + `value_len` (SWAR, 8 B/step) (chosen)** | V3. `memchr` for CR + branch-free fold over the value |
|---|---|---|---|---|
| Safety | relies on every JS sink (undici's history says no) | exact | exact (SWAR proven equal to the table, exhaustive tests in `grammar.rs`) | exact (differential-tested equal, §8.1) |
| Cost, `POST` 1 KiB + 3 headers (runs 2 / 3) | floor 112 / 72 ns | 325 / 166 ns | **220 / 111 ns** | 257 / 119 ns |
| Cost, `varied` ring (runs 2 / 3) | floor 99 / 52 ns | 494 / 269 ns | **247 / 110 ns** | 278 / 130 ns |
| Code | — | new loop | **reuses the parser's shared primitives (DRY, design §13.1 rule 2)**; P1a's SIMD replaces them for both | adds `memchr` to engine-core (allowed, §14) and a second value scanner to keep equal to the table |
| Decision | rejected (§12 requires re-validation) | rejected (1.5–2.4× slower than V2) | **chosen** | rejected (7–18% slower than V2; recorded in `ideas.md`) |

### 5.3 Integer → decimal for `Content-Length`

Literature: libcore's 4-digits-per-step pair table (verified in `num.rs` 1.99.1, the algorithm `itoa` "comes straight
from"); James Anhalt's (jeaiii) multiply-shift algorithm ([repo](https://github.com/jeaiii/itoa); explained by
Junekey Jeon, [blog](https://jk-jeon.github.io/posts/2022/02/jeaiii-algorithm)); Lemire's AVX-512 conversion (2022
blog) and Champagne Gareau & Lemire, "Converting an Integer to a Decimal String in Under Two Nanoseconds" (SPE,
[arXiv 2604.26019](https://arxiv.org/abs/2604.26019)). Both target long numbers. Their reported numbers are
**unverified** (read from search summaries only), are listed in `ideas.md`, and are not used for the decision.
Content-Length values are 1–7 digits in practice. Since the body length is a `usize`, the implementation calls
`usize::format_into` (the same macro-generated code).

| | `core::fmt` (`write!`) | **`u64::format_into(&mut NumBuffer)` (chosen)** | `itoa` 1.0.18 | Safe hand-rolled pair table |
|---|---|---|---|---|
| Algorithm | libcore pairs + `Formatter` | libcore pairs, no `Formatter` | libcore copy | pairs, safe indexing |
| Dependency | std | **std (stable since 1.98.0, verified)** | crate not on the engine-core allow-list (design §14) → design change | none, but our own code to keep correct |
| `unsafe` in our crate | no | **no** | no (inside the crate) | no |
| Measured, 0–10⁴ (runs 2 / 3, §8.2) | 27.6 / 26.4 ns | **12.8 / 12.7 ns** | 13.5 / 13.4 ns | 12.7 / 13.0 ns |
| Measured, 0–10¹³ log-uniform | 45.1 / 29.8 ns | **32.9 / 22.1 ns** | 29.7 / 20.7 ns | 29.6 / 19.3 ns |

### 5.4 Body placement and syscalls

| | **S1. Copy head + body into one buffer, one `write` (chosen for small bodies and all TLS)** | S2. `writev([head, body])` | S3. Two writes |
|---|---|---|---|
| Who | Go (`bufio` ≤ 4 KiB), curl (`sendbuf`), Bun | hyper (`Queue`), h2o, undici (`cork` → `_writev`) | naïve |
| Packets with `TCP_NODELAY` | 1 send → as few segments as MSS allows | 1 send | 2 sends → ≥ 2 segments (`tcp(7)`: segments "always sent as soon as possible") |
| Without `TCP_NODELAY` | same | same | second write waits for the ACK of the first (Nagle × delayed ACK; Brooker 2024, unverified for our hosts) |
| TLS (rustls 0.23.46) | one `write` → minimal records | `write_vectored` allocates a `Vec<&[u8]>` per call (verified, `conn.rs` L392–407) | 2 `write`s → 2 records |
| Measured (loopback, §8.3) | equal to S2 up to 4 KiB | cheaper from 16 KiB (2 of 3 runs), 1.6–3.3× at 64 KiB | 1.6–2.7× worse at 1–4 KiB |

`TCP_CORK`/`MSG_MORE` (Linux-only, `tcp(7)`: "should not be used in code intended to be portable", 200 ms ceiling)
are not needed: one `write`/`writev` per request already carries the whole request (verified, man-pages 6.19).

## 6. Chosen design (precise)

### 6.1 Files (Task 5 scope)
- `crates/engine-core/src/h1/writer/` (split in fix rounds 1 and 2, design §13.1 rule 2): `mod.rs` serialises a
  request (`RequestParts`, `Reuse`, `write_request`, the framing predicate, `MAX_P0_REQUEST_BYTES`, and the `CRLF`
  and `RESOURCE_EXHAUSTED` constants the submodules share); `target.rs` checks the request-target (§6.3);
  `fields.rs` validates field blocks for template and delta (line grammar, the name classifier with forbidden and
  singleton names, the line count, sensitive names); `template.rs` holds `HeaderTemplate`; `host.rs` validates its
  host (§6.2).
- `crates/engine-core/src/h1/grammar.rs` (already on `milestone/p0`): reused as is (`TCHAR`, `FIELD_CONTENT`,
  `token_len`, `value_len`). **No new table there.** The target's byte table lives in `writer/target.rs` and the host
  rules in `writer/host.rs`, because only the writer uses them (design §13.1 rule 1).
- `crates/engine-core/src/h1/mod.rs`: re-exports `HeaderTemplate`, `RequestParts`, `Reuse` and `write_request`.
  The `#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::unwrap_used, clippy::expect_used,
  clippy::panic, clippy::unreachable)]` block now at the top of `grammar.rs` is **hoisted once to `h1/mod.rs`**, so
  `grammar.rs`, `parser/` and `writer/` all inherit it (a lint attribute on a module applies to everything inside
  it, [rustc book: lint levels via attributes](https://doc.rust-lang.org/1.99.1/rustc/lints/levels.html#via-compiler-flag); DRY). The error codes
  shared by parser and writer (`INTERNAL`, `HEADERS_TOO_LARGE`) live once in `h1/mod.rs`.
  The now-duplicated blocks in `grammar.rs`, `parser/mod.rs` and `parser/facts.rs` are deleted (X11); behaviour is
  unchanged and the parser tests stay green.
- Tests and tools:
  - `crates/engine-core/tests/h1_writer.rs`: unit and vector tests, including the vector runner `injection_vectors`.
  - `crates/engine-core/tests/h1_writer_alloc.rs`: a separate test binary, because `allocation-counter` installs a
    global allocator (§8.4).
  - `crates/engine-core/examples/h1_write_bench.rs`.
  - `crates/engine-core/fuzz/fuzz_targets/h1_writer.rs` and `fuzz/h1_writer.dict` (§9.5).
  - `bench/writer/`: the hyper-encoder emulation and the Go `Request.Write` comparison (§8.4).
  - `tests/vectors/h1-request/*.json` (§9.3).
  - `tools/vectors/schema.ts`, `crates/engine-core/tests/support/vectors.rs`, `docs/notes/vectors.md`,
    `tests/vectors/index.json` and `THIRD_PARTY_NOTICES` (§9.3).
- `policy/spec.json` gains `limits.h1.maxRequestTargetBytes = 16384` (design §11.5 URL limit),
  `limits.h1.maxRequestFieldsBytes = 65536` and `limits.h1.maxCopiedBodyBytes = 65536`. Regenerate `generated/spec.rs`
  and the TS output with `pnpm specgen`, and update `tools/specgen` tests and `spec.test.ts` (`spec_wire.rs` if the
  limits struct is asserted there).
- Every public function and constructor gets a rustdoc `# Errors` section listing its `ErrorCode`s (§7).

### 6.2 Template
```rust
/// Pre-encoded `host: <authority>\r\n` + default fields, validated once on the control plane.
pub struct HeaderTemplate {
    bytes: Box<[u8]>,  // "host: " authority "\r\n" ("name: value\r\n")*
    authority_end: usize,  // authority = bytes[6..authority_end]
    singletons: u8,  // bit set of singleton names present (§6.4)
    sensitive: bool,  // contains a name from the client's sensitiveHeaders list
}
impl HeaderTemplate {
    pub fn new(host: &[u8], fields: &[u8], field_lines: u16, sensitive_names: &[&[u8]]) -> Result<Self, ErrorCode>;
    pub fn authority(&self) -> &[u8];
    pub fn is_sensitive(&self) -> bool;
    pub(super) fn bytes(&self) -> &[u8];      // for write_request (writer/mod.rs)
    pub(super) fn singletons(&self) -> u8;
}
```
- **`host`** is WHATWG `url.host` as JS produces it: punycoded, lowercase, IPv6 bracketed, default port omitted, no
  userinfo (design §12; P2 hand-off §10.4). The structure is checked, not just the bytes (W9, fix round 1):
  - either `[`, an IPv6 literal, `]`. The literal must parse as
    [`Ipv6Addr`](https://doc.rust-lang.org/1.99.1/core/net/struct.Ipv6Addr.html) (std; so no zone id and no
    malformed groups) **and** be spelled exactly as the URL Standard's
    [IPv6 serializer](https://url.spec.whatwg.org/#concept-ipv6-serializer) writes it (fix round 2, W3): lowercase
    hex, no leading zeros, the first longest run of two or more zero pieces compressed to `::`, and never a dotted
    IPv4 tail (`[::ffff:1.2.3.4]` is spelled `[::ffff:102:304]`). `Ipv6Addr`'s `Display` differs for IPv4-mapped
    addresses, so `host.rs` writes the eight pieces itself through a `fmt::Write` that compares against the literal
    as it goes: no allocation. One host therefore has one spelling, which the byte-equal authority binding (§6.6)
    and the registry's byte-equal deduplication rely on;
  - or a reg-name/IPv4 of **lowercase** ALPHA / DIGIT / `-._~!$&'()*+,;=` with **no `%`** (WHATWG has already
    lowercased, and decoded or rejected percent-encoding) and no empty label (one trailing dot, a fully qualified
    name, is allowed);
  - then optionally `:` and 1–5 DIGIT without a leading zero and a value 1–65535 (WHATWG omits default ports and
    never pads);
  - non-empty.

  WHATWG itself can emit two hosts this rule rejects: port 0 (`http://a:0/` has host `a:0`) and empty labels
  (`http://a..b/` has host `a..b`; verified in the review with Node 26.6.0, W5). Neither names a reachable origin
  over TCP, so the writer keeps rejecting them and P2 rejects both first with `INVALID_CONFIG` (§10.4).

  `,` stays allowed, because RFC 3986 sub-delims includes it and the WHATWG host parser can emit it. The Host of
  Troubles caveat: some intermediaries split Host on `,`. It is documented and not rejected, because WHATWG can
  produce it for a valid URL. Hosts outside this class (WHATWG accepts e.g. `a{b`) are rejected by P2 with
  `INVALID_CONFIG {field: 'url', reason: 'host'}` before Rust sees them (§10.4, X7); reaching Rust is a bypass.
- **`fields`**: same grammar, forbidden set and singleton rule as the per-request delta (§6.4), validated **once**
  here. The singleton bits are recorded for the per-request check.
- **`fields`'s line count (fix round 1, item 1):** `field_lines` is the number of default fields JS serialised;
  the template rejects a mismatch with `Engine(Internal)` exactly as the delta does (§6.4 line count), through the
  same `scan_fields`. A default value carrying `\r\nauthorization: Basic evil` would otherwise ride on every request
  to that origin.
- **`sensitive`**: true when any field name is a name in `sensitive_names` under the name policy's spelling rule
  (ASCII case folded, `_` read as `-` on both sides, fix round 1), so `x_api_key` counts as `x-api-key`. It is
  recorded in the same pass that validates the fields. That is the
  client's configured `sensitiveHeaders` list, passed with `RegisterTemplate`; the design §8 default is
  `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`, `x-auth-token`. The registry uses the
  flag to release sensitive templates immediately (design §11.2), and P1b's redirect stripping uses it (§10.3).
- **Size:** the host line plus `fields` ≤ `maxRequestFieldsBytes`, else `TooLarge(Headers)` (§7).
- **Allocation:** one `Vec::try_reserve_exact(total)` filled with the slices, then `into_boxed_slice`. The
  allocator may give more than asked ([`try_reserve_exact`](https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html#method.try_reserve_exact):
  capacity "can not be relied upon to be precisely minimal"), in which case `into_boxed_slice` moves the bytes into
  an exact allocation ([`into_boxed_slice`](https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html#method.into_boxed_slice)): at most one
  more allocation, control plane only. A failed reserve is `Engine(ResourceExhausted)`. The template is built on the control plane
  (dispatcher, design §11.3), moved to the shard thread and never mutated again.
- **No `Clone`.** Copying secret bytes implicitly is unwanted. **Ownership (X1):** Task 6's `ShardConfig` owns the
  `HeaderTemplate` **by value**. It is `Send + Sync` (auto traits: every field — `Box<[u8]>`, `usize`, `u8`,
  `bool` — is, [`Send`](https://doc.rust-lang.org/1.99.1/std/marker/trait.Send.html); pinned by the test `template_is_send_and_sync`), so it
  moves from the
  control plane to the shard thread; if workers on that thread share it, the shard wraps it in an `Rc` once, on its
  own thread.
- **Manual `Debug`** (W8): prints byte lengths, `authority().len()` and the `sensitive` flag, never the bytes. The
  head is never logged or traced. After `out.clear()` a pooled `out` still holds old credentials in memory past
  `len`, and a dropped `HeaderTemplate` frees its bytes without zeroing them (item 23). Both are off the wire and
  unreachable through safe code; this is documented and accepted.
- **Registry rules (W7; template registry task, recorded here):** a template is looked up by `(id, generation)`. A
  stale generation fails (`Engine(Internal)`) and never falls back to another template. Content-keyed deduplication
  compares the **full bytes, host included**, never a hash alone.

### 6.3 Request target (origin-form)
- **Non-empty**, first byte `/` (origin-form, RFC 9112 §3.2.1), and at most `maxRequestTargetBytes`.
- **One rule (fix round 2, W4): a byte WHATWG never emits in the component it sits in is rejected.** The target
  splits at its first `?` into path and query (RFC 9112 origin-form `absolute-path [ "?" query ]`). WHATWG's URL
  serializer percent-encodes, per the [URL Standard](https://url.spec.whatwg.org/#percent-encoded-bytes):
  - **everywhere:** C0 controls, SP, DEL and every byte ≥ 0x80, plus `#` (a fragment is never sent, and servers
    that truncate at `#` would read a different target — a differential), `"`, `<` and `>`;
  - **in the path** also `\`, `^`, `` ` ``, `{` and `}` (the path percent-encode set; WHATWG even rewrites `\` to `/`
    in a special URL's path, so a raw `\` cannot come from the URL layer);
  - **in the query** also `'` (the special-query percent-encode set; http and https are special schemes).

  Everything else in 0x21–0x7E is accepted where WHATWG leaves it raw, including `|`, `[`, `]` and `%` in the path,
  and `\`, `^`, `` ` ``, `{`, `}`, `|` and a second `?` in the query. That is stricter than undici (whose path regex
  rejects only code points outside U+0021–U+00FF) and Bun (accepts ≥ 0x80), and it costs nothing for a target the
  URL layer built (design §5.2). Deviation from RFC 3986, recorded: the query bytes above are outside its `query`
  rule and stay accepted because WHATWG sends them raw.
- **How it is checked:** one branch-free fold of comparisons joined with `&` over the target accepts the bytes
  allowed in every component (visible ASCII without `#`, `"`, `<`, `>` and without the six component-specific bytes
  `\`, `^`, `` ` ``, `{`, `}`, `'`); it vectorises (16 bytes per step, 8 for short targets; checked in the assembly).
  Only a target holding one of the six is split at its first `?` and each part checked against its own component,
  so the common target costs one pass and no search for `?`. A 256-entry flag table (scalar: one dependent load per
  byte) and comparison-and-select flags (byte-dependent branches on short targets) both measured slower (journal
  2026-11-13).
- **Accepted, the caller's responsibility (not normalised):** `//x` (an empty first segment), raw dot segments
  (`/../../etc`), `/@evil.example` and `%` not followed by two hex digits (`/%zz`). WHATWG removes dot segments and
  never produces the others from a URL whose path it parsed, so P2 cannot send them; P1b, which builds redirect
  targets in Rust, must build them through the same URL parser and not assume the writer normalises (§10.3).
- **`//evil.example/x` is accepted (decision, W11; accepted risk, fix round 2).** It is a valid origin-form:
  `absolute-path = 1*( "/" segment )` with an empty first segment (RFC 9112 §3.2.1 via RFC 9110 §4.1). On the wire it
  goes to the connection's own authority and cannot change the Host. The client-side danger class is the JS URL
  **join** (undici GHSA-8qr4-xgw6-wmr3 / CVE-2022-35949, a path that was an absolute URL redirected the request). P2
  owns that: `baseURL` + `url` joining through WHATWG `URL`, which never yields a path that changes the origin.
  **Accepted risk on the server side:** origin code that passes the request-target to a URL-reference parser reads
  `//evil.example/x` as a network-path reference with authority `evil.example` — the open-redirect class behind
  server advisories such as CPython `http.server` CVE-2021-28861 (cited from the review, not re-checked here). That
  is the server's bug and a client cannot fix it by refusing valid paths: rejecting `//` would break legitimate APIs
  whose paths start with an empty segment.
- **Absolute-form (P1b, W6)** will be a separate `Target::Absolute` variant (§10.3), never a byte pattern accepted
  here. Asterisk-form is never produced (the API has no server-wide `OPTIONS`).

### 6.4 Per-request delta (`RequestParts::headers`)
Grammar, checked in one pass with no allocation:
```
delta       = *( field-name ": " field-value CRLF )
field-name  = 1*TCHAR                        ; grammar::token_len
field-value = *FIELD_CONTENT                 ; grammar::value_len (HTAB, SP–'~', 0x80–0xFF)
```
- **Separator:** exactly `": "`. Our JS emits it; no other whitespace may appear before or after the colon (RFC 9112
  §5.1).
- **Values:** the scan stops at the first byte outside `FIELD_CONTENT`, and the next two bytes must be exactly CR LF.
  So a bare CR, a bare LF, NUL, any other CTL or DEL anywhere in a value is an error. A line can never start with SP
  or HTAB (T-Reqs "manipulated termination", obs-fold).
- **Other line errors:** an empty line inside the delta (`\r\n` where a name is expected: `token_len` = 0), a missing
  final CRLF, `a:b` without the SP, and an empty name. Each has a test (§9.1).
- **Allowed:** empty values (`x: \r\n` is trailing OWS and valid), and leading or trailing SP/HTAB inside a value as
  OWS. Nothing is trimmed; undici and axios do not trim either, while Go does. P2 decides the user-facing behaviour.
- **Name classification:** one private `fn classify(name: &[u8]) -> NameClass` with
  `enum NameClass { Forbidden, Singleton(u8), Other }`. It checks the `proxy-` prefix, matches on `name.len()`, then
  compares with at most three candidates through `same_name` (ASCII case folded, `_` read as `-`), so it is O(1) per
  line.
- **Forbidden names (W10; fix round 1 adds `trailer`, `http2-settings` and the `proxy-` prefix):** `host`,
  `transfer-encoding`, `connection`, `upgrade`, `expect`, `te`, `keep-alive`, `content-length`, `trailer` (Go drops a
  user `Trailer`, Fetch forbids it; no trailers are sent in 1.0), `http2-settings` (meaningless without an h2c
  `Upgrade`, which is forbidden), **every `proxy-` name** (Fetch's forbidden prefix; proxy credentials and hop-by-hop
  proxy fields come only from the proxy configuration, P1b), plus **the underscore spelling of every forbidden or
  singleton name** (X9). The names whose canonical spelling has a `-` are the ones affected: `transfer_encoding`,
  `keep_alive`, `content_length`, `http2_settings`, every `proxy_` name (`proxy_authorization`,
  `proxy_connection`, …), `content_type` and `user_agent`. Some gateways (CGI-style environments, nginx with
  `underscores_in_headers`) map `_` to `-`, which would turn them into framing or credential headers, or into a
  second copy of a singleton. The match compares ASCII case-insensitively and treats `_` in the name as `-` in the
  canonical spelling, so it stays O(1) (one length switch, at most three candidates); a match that used an
  underscore is always `Forbidden`. The same set
  applies to the template, whose `host` comes separately. Design §12 lists the user-facing set. `content-length` is
  added because the writer is its only producer: P2 strips or rejects a user value, and Rust never accepts one.
  `connection` is produced only by the writer (§6.5), and `proxy-authorization` only with the P1b proxy variant
  (§10.3).
- **Singleton names (W3):** `authorization`, `cookie`, `content-type`, `user-agent` may appear **at most once
  across template and delta together**. The template records a `u8` bit set. The delta pass starts from it and, per
  line, checks `seen & bit != 0` (error) then sets `seen |= bit`. That is O(1) per line with no allocation, and a
  repeat inside the template or inside the delta is caught the same way. Two `authorization` or two `content-type`
  values let an intermediary and the origin pick different ones (Host of Troubles-style ambiguity). Other duplicate
  names are not a smuggling vector once framing headers are forbidden. **Contract, not check (item 7):** RFC 9110
  §5.3 forbids repeating a non-list field; the writer enforces only the singleton subset and passes other repeats
  through (vectors `duplicates / …` document it). P2 guarantees one line per non-list name and never repeats a template
  name in the delta (§10.4), with its own tests. `sec-` (Fetch's other forbidden prefix) is browser-only and not
  forbidden here.
- **OPTIONS content (item 2):** RFC 9110 §9.3.7 — a client that sends content in OPTIONS MUST send a valid
  `Content-Type`. `OPTIONS` with a non-empty body and no `content-type` in template or delta is `Engine(Internal)`
  (P2: `INVALID_CONFIG`). The singleton bits already say whether `content-type` is present, so the check is O(1).
  **Presence only (fix round 2, W6):** the writer does not parse the media type, so `content-type: ` (empty) or a
  malformed value is written; checking that the value is a valid `media-type` (RFC 9110 §8.3.1) is P2's (§10.4).
- **Residual differentials, accepted (fix round 2, W8):**
  - *Obs-text* (0x80–0xFF) is allowed in values by RFC 9110 §5.5 and by design (§10: latin1). Some receivers that
    decode header bytes as latin1 and then split on Unicode line breaks may treat a single byte such as NEL (0x85) as
    a line end (reported for older Python header parsing; from the review, not re-checked here). Go and undici send
    the same bytes, so this is parity, not a writer gap; P2 may restrict values to ASCII per client option later.
  - *Routing-relevant names* (`x-forwarded-host`, `forwarded`, `x-http-method-override`, `via`, `max-forwards`) are
    written. They are legitimate client fields; the forbidden list protects framing, credentials and connection
    management, and is not a server-side request forgery filter.
- **Size:** `maxRequestFieldsBytes` counts exactly the template bytes (**including the `host:` line**) plus the delta
  bytes. It excludes the request line, the generated `content-length` and `connection` lines, and the final CRLF.
- **Line count (X3, ruling).** `RequestParts::header_lines: u16` is the number of fields the JS layer serialised
  into `headers`. The delta pass already finds every CRLF-terminated line; it counts them and rejects a mismatch
  with `Engine(Internal)` (O(1) extra). This closes the well-formed injection that the byte classes cannot see: a
  value carrying `\r\nx-forwarded-for: 1.2.3.4` (or a `cookie` line when none was set) parses as one more valid
  line, so the count is off by one. 64 KiB of minimal lines (`a: \r\n`, 5 bytes) is 13,107 lines, so `u16` fits.
  The count is **the number of entries JS iterated, never derived from the serialised bytes** (item 9): deriving it
  from CRLFs would count the smuggled line too. The template takes the same count (`field_lines`, item 1).
  The writer cannot verify where a count came from (W1): `headers` = `x: a\r\nx-evil: 1\r\n` with `header_lines` = 2
  is written. The check is therefore only as good as each producer's count, and each producer's rule is stated
  where it is built: P2 counts the entries of the header map it serialises (§10.4); P1b, which builds redirect
  heads in Rust, counts the fields it pushes, one per push, and never counts CRLFs (§10.3). A type that owns bytes
  and count together (a `FieldBlock` built only by `push(name, value)`) would make the misuse unrepresentable; it is
  P1b's to add when the first Rust producer exists, because P0 has no Rust producer to build it for (§10.3).

### 6.5 Framing and reuse: one predicate
```rust
#[must_use]
pub enum Reuse { Allowed, CloseAfterExchange }

/// The single place that decides framing; `write_request` only follows it.
fn framing(method: Method, body_len: usize) -> Result<RequestFraming, ErrorCode>;
struct RequestFraming { content_length: bool, close: bool } // private; distinct from the parser's `Framing` (X8)
```

| Method | Empty body | Non-empty body |
|---|---|---|
| `POST`, `PUT`, `PATCH`, `QUERY` | `content-length: 0`, `Reuse::Allowed` | `content-length: N`, `Reuse::Allowed` |
| `GET`, `DELETE`, `OPTIONS` | no `content-length`, `Reuse::Allowed` | `content-length: N` + generated **`connection: close`**, `Reuse::CloseAfterExchange` |
| `HEAD` | no `content-length`, `Reuse::Allowed` | **`Engine(Internal)`** (P2: `INVALID_CONFIG`) |

- **Always-length set:** RFC 9110 §8.6 (verified) says a UA "SHOULD send Content-Length in a request when the method
  defines a meaning for enclosed content … even when the value is 0" and "SHOULD NOT" otherwise. RFC 10008 (QUERY,
  verified) defines the request content, so `QUERY` belongs with `POST`. Go (`shouldSendContentLength`, #40978) and
  undici (`expectsPayload`, which includes `QUERY`) agree.
- **Bodies on `GET`/`DELETE`/`OPTIONS`:** the body is framed (an unframed body would become the next request), and
  the writer announces the close itself. RFC 9112 §9.6 (verified): a client that sends `close` "MUST NOT send further
  requests on that connection … and MUST close the connection after reading the final response". This blunts
  Kettle's 0.CL and client-side desyncs, where an intermediary that ignores the body sees it as a new request on a
  connection we would otherwise reuse.
- **`HEAD` with a body** has no defined semantics (RFC 9110 §9.3.2), and its response never has one, so it is
  rejected.
- **undici parity, corrected:** undici sends `connection: keep-alive` in the head of a `GET`-with-body and then sets
  `kReset` after building the head (`writeBuffer` L1536–1564). So the server is not told, but undici does not reuse
  the connection. For `HEAD` it sets `kReset` *before* building the head, so `connection: close` is sent
  (`writeH1` L1314–1320, L1351–1357). We go one step further: the server is told in every close case.
- Never `transfer-encoding` in Task 5 (chunked request bodies are a later feature, §14). Never `Expect`.
- Method bytes come from `Method::as_str()`, which is generated; `CONNECT` and `TRACE` are not representable. A wire
  method id that does not decode (`Method::from_wire` returns `None`) fails in the codec with `Engine(Internal)`
  before the writer runs (test in §9.1).
- **Order:** request line, `host` (first, RFC 9110 §7.2), template defaults, delta, `content-length`, `connection:
  close`, CRLF, body.

### 6.6 Interface (changes to the brief, justified)
```rust
/// Borrowed parameter object, never mutated (design §13.2 rule 1 carve-out); one caller contract: `header_lines`.
pub struct RequestParts<'a> {
    pub method: Method,
    pub target: &'a [u8],
    pub headers: &'a [u8], // ("name: value\r\n")*
    pub header_lines: u16, // fields JS serialised into `headers` (§6.4, X3)
    pub body: &'a [u8],
}
pub fn write_request(
    out: &mut Vec<u8>,
    parts: &RequestParts<'_>,
    template: &HeaderTemplate,
    conn_authority: &[u8],
) -> Result<Reuse, ErrorCode>;
```
1. **`HeaderTemplate::new(host: &[u8], fields: &[u8], field_lines: u16, sensitive_names: &[&[u8]])`** replaces
   `(&str, &[(&str, &str)])`.
   - Header values are latin1 bytes (design §10: values ≤ 0xFF, written by JS with `'latin1'`, as undici does). A
     `&str` would carry `é` as UTF-8 `C3 A9` while undici sends `E9`, a transport divergence (G5).
   - One wire format for template and delta means one validator.
   - `field_lines` is the count JS serialised into `fields` (§6.2, fix round 1 item 1).
   - `sensitive_names` feeds the flag in §6.2.
2. **`conn_authority` (W1, High).** `write_request` returns `Engine(Internal)` unless `conn_authority` is byte-equal
   to `template.authority()`, so a template is never used on a connection to another **authority** (credentials in
   defaults would otherwise leak). The scheme is not bound in P0, which is plaintext only (item 4): a template built
   for `https://a.example` would pass on `http://a.example`. P1a's TLS task binds the whole origin (§10.5). Task 6 passes the authority its connection was opened for, from `ShardConfig`'s
   authority field (P0; the pool key from P1b), and `ShardConfig::new` checks that field is byte-equal to
   `template.authority()` (X10).
3. **Returns `Reuse`** from the one predicate in §6.5. Task 6 must honour it (§10.1).
4. **`RequestParts` keeps `pub` fields.** It is a borrowed parameter object with no state changes; design §13.2
   rule 1 lists it as passive data. It does carry one cross-field contract the writer cannot check: `header_lines`
   is counted while `headers` is built, never from its bytes (§1, §6.4 line count, W1). Its doc comment states it;
   making it unrepresentable (a `FieldBlock` that owns bytes and count) is P1b's, with the first Rust producer
   (§10.3). `HeaderTemplate`
   keeps private fields; `write_request` reads them through `pub(super)` accessors (`bytes()`, `singletons()`). **No `Debug` derive** on `RequestParts` (X4): a manual
   `Debug` prints the method, the byte lengths and `header_lines`, never the bytes (headers carry credentials).
5. **Buffer contract:** `write_request` **clears `out` at the start**, runs every check (§7 order), computes the total
   length with checked additions, **reserves exactly** with one `out.try_reserve_exact(total)` (a no-op when the
   capacity already suffices, i.e. in steady state), then appends. It **never shrinks** `out`. Shrinking is Task 6's,
   because only the connection knows when the write completed (§6.7, §10.1). On any error `out` is left **cleared**
   (capacity kept) and nothing has been sent.
6. Content-Length uses `usize::format_into(&mut NumBuffer::<usize>::new())` (stable since 1.98.0, verified by
   compiling on 1.99.1 under `forbid(unsafe_code)` and `-D warnings`), so no `u64` conversion is needed.

### 6.7 Memory (W5, F10, F12)
- **P0 body limit:** bodies larger than `maxCopiedBodyBytes` (64 KiB) are rejected **by Task 6's `try_submit` with
  `TooLarge(RequestBody)`** before any connection work, so they never reach `write_request`. The writer re-checks it
  as part of its contract and returns `Engine(Internal)` if a caller bypassed `try_submit`. This keeps the transient
  copy bounded at head + 64 KiB.
- **Shrink (owned by Task 6, not the writer; X5 ruling):** the writer exposes `pub const MAX_P0_REQUEST_BYTES`, the
  largest request P0 can write, derived from the spec limits: `maxRequestTargetBytes` (16 KiB) +
  `maxRequestFieldsBytes` (64 KiB) + `maxCopiedBodyBytes` (64 KiB) + the fixed overhead (the longest method, two SP,
  ` HTTP/1.1`, the `content-length` line with the digits of `maxCopiedBodyBytes`, the `connection: close` line and
  the CRLFs). After the write of a request completes, Task 6 does `out.clear();` and, only if `out.capacity()` >
  `MAX_P0_REQUEST_BYTES`, `out.shrink_to(MAX_P0_REQUEST_BYTES)`. Every P0 request fits, so steady state stays at zero
  allocations even for 63–64 KiB bodies, and a buffer can never pin more than one maximal request (in addition to
  the 30 s idle trim, design §11.1). The writer only clears at its own start, reserves exactly, and never shrinks
  (§6.6 item 5).
- **From P1a:** capacity of `out` above its base size is charged to the §11.1 byte budget *before*
  `try_reserve_exact`. If
  the charge fails, the request waits for the budget, then fails with `QUEUE_FULL/bytes` after `queue` (G6).
  **Budget hook (X6):** P1a adds `pub fn required_len(parts, template) -> Result<usize, ErrorCode>` to the writer —
  the same limit checks and precedence as `write_request` (§7), no writing — so the connection can charge the
  budget before calling `write_request`. It is not implemented in Task 5: it has no caller before P1a, and its
  precedence differs from `write_request` (no `conn_authority`), so it is not a trivial share of the reserve
  computation. Listed as the P1a interface.
- **Large bodies (owner P1b):** one copied buffer stays the rule for small bodies. For large plaintext bodies, P1b
  adds `write_head` (the same code without the body append) plus `writev([head, body])`, with the threshold measured
  between 4 and 16 KiB (§8.3). For large bodies **under TLS**, P1b measures head-then-body plain `write`s against a
  copy, and never uses `write_vectored` (rustls allocates per call, §5.4). Large-body rules (W11):
  - `content-length` comes from the declared length;
  - the bytes written equal it exactly;
  - a short or failed upload closes the connection;
  - the writer never pads or truncates.

### 6.8 No panics, no `unsafe`
- `engine-core` is `#![forbid(unsafe_code)]`.
- `extend_from_slice` "Panics if the new capacity exceeds isize::MAX bytes" (Vec docs 1.99.1, verified). That cannot
  happen after a successful `try_reserve_exact(total)`.
- The total is a `checked_add` chain; overflow is `Engine(ResourceExhausted)`. `try_reserve_exact` failure is the same code.
- Slicing uses `split_at_checked`, `strip_prefix` and `get`, under the hoisted lint block (§6.1).

## 7. Error mapping (decided)

**Precedence:** limits first (O(1), and they bound every later scan), then grammar in wire order, then allocation. The
first failing check wins, so every input has exactly one error.

| # | Check | Error |
|---|---|---|
| 1 | `body.len() > maxCopiedBodyBytes` (P0 contract; Task 6 `try_submit` already returns `TooLarge(RequestBody)`, so reaching here is a bypass) | `Engine(Internal)` |
| 2 | `target.len() > maxRequestTargetBytes` (16,384) | `TooLarge(Url)` |
| 3 | `template.bytes().len() + headers.len() > maxRequestFieldsBytes` (65,536; §6.4 counting) | `TooLarge(Headers)` |
| 4 | `conn_authority != template.authority()` | `Engine(Internal)` |
| 5 | `HEAD` with a non-empty body (§6.5) | `Engine(Internal)` |
| 6 | target empty, first byte not `/`, or a byte WHATWG never emits in its component (§6.3) | `Engine(Internal)` |
| 7 | delta line grammar, forbidden name in any spelling, repeated singleton, line count ≠ `header_lines`, OPTIONS content without `content-type` | `Engine(Internal)` |
| 8 | `checked_add` overflow of the total length | `Engine(ResourceExhausted)` |
| 9 | `out.try_reserve_exact(total)` fails | `Engine(ResourceExhausted)` |
| T1 | `HeaderTemplate::new`: host line + fields > 65,536 (template alone) | `TooLarge(Headers)` |
| T2 | `HeaderTemplate::new`: host structure or field grammar, forbidden or repeated singleton, line count ≠ `field_lines` | `Engine(Internal)` |
| T3 | `HeaderTemplate::new`: `try_reserve_exact` fails | `Engine(ResourceExhausted)` |

The template's length sum saturates, so an overflow is reported as T1 `TooLarge(Headers)` (a sum past `usize::MAX`
is past the limit too). Rows whose codes differ are pinned pairwise by `error_precedence` (fix round 1, item 25);
rows 4–7 share `Engine(Internal)` and are ordered for determinism only. Which rule fired is not visible in the code
(item 29): a private rule enum for diagnostics is a P1a item (§10.5).

- The brief's "re-validation failures are `Engine(Internal)`" is **confirmed** for grammar violations: P2 validates
  first with `INVALID_CONFIG`, so reaching Rust means that contract was bypassed.
- Size limits are `TooLarge`, which is the correct user-facing code for targets built in Rust (redirects, P1b;
  design §11.5).
- Allocation failure follows design §11.3 ("`try_reserve` → `ENGINE/resource-exhausted`").
- P1b's redirect layer validates a `Location`-derived target itself and maps a failure to
  `REDIRECT_REJECTED/invalid-location` before calling the writer, so `Engine(Internal)` keeps meaning "bug".
- Errors carry no bytes (design §5.2).
- Precedence cases have their own tests (§9.1): one pair per two rows whose codes differ (`PRECEDENCE`, nine
  pairs), e.g. an oversized **and** CR-containing target gives `TooLarge(Url)`, and fields over the limit **and** a
  bad target give `TooLarge(Headers)`. Rows 4–7 share `Engine(Internal)`, so their order is not observable and has
  no test.

## 8. Performance plan and budget

### 8.1 Prototype (scratch, not committed)
Scratch crate `writer-bench` (rustc 1.99.1, `lto = "fat"`, `codegen-units = 1`, `-C target-cpu=generic`,
`std::hint::black_box`): designs A, B with V1/V2/V3 validation, B without validation (copy floor), and an emulation
of hyper 1.12's encoder (per-request `HeaderMap` clone + `HeaderValue::from(u64)` + `Display` target + header loop,
from role.rs L1215–1262/L1540–1571/L1629–1636; and the same serialisation without the clone). Go 1.27.3
`(*Request).Write` into a reused 8 KiB `bufio.Writer` over `io.Discard` (pre-built request, body reader re-armed per
op). Inputs: `get` (template-only `GET /data`), `post1k` (1 KiB JSON, `content-type`, `x-request-id`,
`authorization`), `varied` (256 requests: mixed methods, targets 8–200 B, 0–6 extra headers of 4–80 B, bodies
0–2 KiB), all cycled so no single request trains the predictor.

**Baselines (exact definitions; fix round 2 renamed and added, M2/W2).** Each writes the writer's exact bytes into
the same reused `Vec` after `clear()`, with the **same framing result** as the writer (X8), taken once per request
when the ring is built; the bench checks the bytes before timing.
- **`memcpy`:** one `extend_from_slice` of the finished request. The lower bound for any writer that fills a buffer.
- **Unchecked appends** (called the **"copy floor"** in this note and the journal before 2026-11-13; the prototype
  rows of §8.2 keep that label): the pieces in wire order (request line, template, delta, `content-length` line via
  `format_into`, `connection: close` when due, CRLF, body), 7–11 `extend_from_slice` calls behind two framing
  branches, with no checks, no length sum and no reservation. It is **not** a lower bound and not the writer's
  sequence: the writer always appends 12 pieces (some empty) after a checked sum and one `try_reserve_exact`.
- **Writer shape:** `write_request`'s own 12-piece append list, checked length sum and one exact reservation, with
  every check removed.

So `writer − shape` is the cost of the checks (target, limits, authority, delta scan, framing predicate), and
`shape − appends` the cost of the writer's append shape. The earlier statement "writer − floor = validation +
bookkeeping" mixed the two.

**Correctness of the prototype (verified):** the brief's exact-bytes `GET` case; capacity and pointer unchanged on
the second write; `QUERY` with an empty body ends in `content-length: 0\r\n\r\n`; 15 injection vectors rejected by
V1, V2 and V3 alike; a differential run over **1,000,000** randomly mutated delta blocks (1–3 replacements,
insertions or deletions from CR, LF, HTAB, SP, `:`, NUL, DEL, 0x80, 0xFF, `a`, `Z`, `0`, `-`, `,`, `;`): V1, V2 and V3
accept exactly the same 342,409 blocks, and every accepted request parses with `httparse::Request` 1.10.1 as complete,
with exactly one `content-length`, no `transfer-encoding`, and the body bytes after the head.

### 8.2 Measurements (dev)
**Setup:** Docker Desktop on the owner's M2, `rust:1-bookworm` (rustc 1.99.1) and `golang:1.27.3-bookworm`, one
container per suite, `--cpus=1` (`GOMAXPROCS=1` for Go), kernel 6.11.11-linuxkit, generic `aarch64` target CPU.
**Not a gate run:** other agents' containers and builds were running; host load average was 31 → 10 (run 1),
23 → 15 (run 2) and 15 → 13 (run 3). Run 1 used an older prototype (no V3; HTAB in 1/26 of `varied` value bytes) and
is shown only where comparable. Each cell is the median of 9 runs × 2 M requests (int: 4 M values). Because absolute
numbers moved up to 2× between runs, **decisions use ratios within a run**.

| ns per request | `get` (head 162 B) run 2 / 3 | `post1k` (head 378 B + 1 KiB) run 2 / 3 | `varied` (mean head 435 B) run 2 / 3 |
|---|---|---|---|
| **B + V2 SWAR (chosen)** | **34.7 / 28.1** | **220.3 / 110.6** | **247.0 / 110.2** |
| B + V3 memchr + fold | 38.5 / 26.2 | 257.1 / 118.8 | 277.9 / 129.7 |
| B + V1 table | 33.4 / 24.1 | 324.7 / 166.2 | 494.2 / 268.9 |
| B without validation (copy floor) | 22.3 / 17.4 | 111.6 / 72.0 | 98.5 / 51.6 |
| A `core::fmt` per request | 241.8 / 186.4 | 729.2 / 384.5 | 839.3 / 466.9 |
| hyper 1.12 emulation: `HeaderMap` clone + CL insert + encode | 190.8 / 156.3 | 826.3 / 422.9 | 924.3 / 399.4 |
| hyper 1.12 emulation: encode only (map pre-built, no clone) | 93.2 / 76.4 | 351.0 / 147.4 | 348.8 / 166.7 |
| **chosen / hyper (clone + encode)** | **0.18 / 0.18** | **0.27 / 0.26** | **0.27 / 0.28** |
| chosen / copy floor | 1.56 / 1.61 | 1.97 / 1.54 | 2.51 / 2.14 |

Go 1.27.3 `(*Request).Write` (5 runs each, `-benchmem`): `get` 1.5–4.9 µs (medians 2.06 / 1.81 / 2.16 µs over runs
1–3), 224 B and **5 allocations** per op; `post1k` 2.5–10.6 µs (medians 3.72 / 2.65 / 5.58 µs), 304 B and **10
allocations** per op. Go formats with `fmt.Fprintf`, sorts header keys and builds the `Content-Length` string per
request (§2), and its numbers swung most with host load.

**Integer formatting** (ns per value including the append to a reused `Vec`, runs 2 / 3): see §5.3 table. All three
table-based variants (`format_into`, `itoa`, safe pairs) are within each other's run-to-run noise; `core::fmt` is
≈ 2× slower for Content-Length-sized values. The hand-rolled `count + resize` variant (digit count via `ilog10`,
then fill) is 20–30% slower than the pair loop for small values. **Decision: `format_into`** (measured on `u64`;
the product calls `usize::format_into`, the same generated code): same speed, std, no dependency, no code of ours to
keep correct. Stability was confirmed by the review by compiling and running it on 1.99.1 under
`forbid(unsafe_code)` and `-D warnings`.

**What the numbers say:**
- Pre-encoding (B vs A) removes ≈ 70–85% of the cost; the remaining cost is copying (the floor) plus validation.
- V2 (the parser's SWAR `value_len`) beats the per-byte table by 1.5–2.4× on headers and V3 by 7–18%. On the
  template-only `GET` (no delta, so no value scan runs) V1 was 4% and 14% faster than V2 and V3 7% faster in run 3,
  differences of the size that rejected V3 elsewhere; two loaded runs decide nothing there. V2 is chosen for the
  sets with fields and because it reuses the parser's primitives (DRY).
- *(prototype only, superseded by `writer − shape` in §8.4)* Validation costs 0.5–1.5× the copy floor.
- *(prototype)* hyper's serialisation alone (encode-only, map pre-built, no validation) measured 1.33–1.59× our full
  prototype writer on `post1k`/`varied`. That compares a floor with a validating writer; the committed bench compares
  the emulation's construct+encode with our writer fed the same pairs (ours + join, 0.17–0.30× in fix round 2;
  0.26–0.42× against clone+encode, the conservative figure) and floors with floors (our copy floor
  / emulated encode-only: 0.23–0.46×), §8.4.
- *(prototype only)* The measured prototype did not yet include the checks added in review rounds 1 and 2; the
  provisional gates were set with a 15% margin for them. The margin did not hold (§8.4).

### 8.3 Body placement: copy vs `writev` (dev, Linux)
Scratch binary `sys`: a blocking `std::net::TcpStream` over loopback with `TCP_NODELAY`, a reader thread draining
the peer, `--cpus=2` (writer and reader), a 380-byte head and bodies of 0–256 KiB. Metric: writer-thread CPU time
(`CLOCK_THREAD_CPUTIME_ID`, which on loopback includes the kernel's send path) per request, median of 5 runs ×
200,000 requests (10,000 for ≥ 64 KiB). Same loaded host as §8.2.

| body | copy + `write` (S1) runs 1 / 2 / 3 | `writev([head, body])` (S2) | two `write`s (S3) |
|---|---|---|---|
| 0 | 3580 / 1854 / 3014 ns | 3690 / 2166 / 2752 | 2779 / 1789 / 2464 |
| 1 KiB | 2015 / 1124 / 2781 | 3746 / 1156 / 2852 | 3214 / 3082 / 5140 |
| 4 KiB | 1867 / 1701 / 3341 | 1875 / 1754 / 2961 | 2945 / 2755 / 6657 |
| 16 KiB | 3197 / 5065 / 6157 | 2758 / 5863 / 4941 | 4305 / 5615 / 7190 |
| 64 KiB | 8074 / 39073 / 14657 | 4226 / 11878 / 9037 | 5598 / 15027 / 11622 |
| 256 KiB | 21818 / 55410 / 43396 | 12738 / 37267 / 28803 | 14200 / 36514 / 34210 |

(With an empty body S2 and S3 send the head alone, so that row only shows the noise floor: ≈ ±40%.)
**Reading:** the syscall dominates small requests (≈ 1–3 µs here vs ≈ 0.03–0.2 µs for the writer), so "one write per
request" matters far more than how the bytes were assembled; S3 (head and body in separate writes) costs 1.6–2.7× at
1–4 KiB. Up to 4 KiB S1 ≈ S2. From 16 KiB S2 wins in 2 of 3 runs; at 64 KiB S2 wins by 1.6–3.3×. **Decision for Task
5:** `write_request` always copies (all requests that reach the writer in P0 are small; TLS must copy anyway). The
threshold for a `write_head` + `writev` path is measured by the task that adds it, on the reference host in the
nightly window and on the x64 VM, between 4 and 16 KiB. The kernel side is as documented: one `write`/`writev` = one
atomic block (`writev(2)`), and with `TCP_NODELAY` a second write is sent "as soon as possible" as its own segment
(`tcp(7)`), which is what S3 pays for.

### 8.4 Gates, committed benchmarks and allocation proof

**Committed benchmark (Task 5): `crates/engine-core/examples/h1_write_bench.rs`.** Run it with
`cargo run --release -p engine-core --example h1_write_bench`; P1a replaces it with a criterion bench. It mirrors
`h1_parse_bench.rs`:
- **Inputs:** the three request sets of §8.1 (`get`, `post1k`, and the `varied` ring of 256), generated
  deterministically from a fixed xorshift seed.
- **What it compares:** `write_request` against the three baselines of §8.1 (writer shape, unchecked appends,
  `memcpy`), in one binary, each run made of rotated rounds (the starting variant changes every round, so drift
  inside a run does not always land on the same variant; fix round 2, M2). It reports 7 runs with the median ns per
  variant, `writer − shape`, `writer − appends` and `writer / appends` (with min/max). Warm-up is 100 ms.

**Committed comparison code** (reproducibility of §8.2; competitors are allowed only outside engine-core, design §14):
- `bench/writer/hyper-emul/`: a standalone Cargo package that depends on `http = "=1.5.0"` and our `engine-core`
  through a path dependency, and runs the §8.1 **emulation** of hyper 1.12's encoder against `write_request` on the
  same inputs. No hyper code runs: every "vs hyper" figure is "vs our emulation of hyper's encoder" until Task 5c
  runs the real `Client::encode`. Its headline pair is equal input shape: "ours + join" builds the delta wire bytes
  from the same `(name, value)` pairs the emulation's `HeaderMap` is built from, inside the timed region (M8). It is
  excluded from the workspace, like `crates/engine-core/fuzz`.
- `bench/writer/go/`: the Go `(*Request).Write` benchmark (`writer_test.go`, `go.mod` pinned to go1.27.3). Its
  `post1k` body is an `io.NopCloser` over a `*bytes.Reader`, the body `http.NewRequest` builds for bytes, so
  `net/http` takes its in-memory path (a test pins one write per request; fix round 2, R2-3).
- `bench/writer/run.sh build BIN_DIR` compiles all three in digest-pinned images (`rust:1-bookworm` with rustc
  1.99.1, `golang:1-bookworm` with Go 1.27.3) and records the commit; `bench/writer/run.sh measure BIN_DIR LOG_DIR
  [CPUSET]` runs the prebuilt binaries alone (it refuses while any other container runs), one CPU pinned with
  `--cpuset-cpus`, `--memory=2g`, no network, and writes the provenance file next to the logs (fix round 2, M9). Build
  and measure are separate so that no compile, test or download runs during a timing (owner rule, 2026-11-13). `bench/writer/README.md` explains the output. Results go to
  `docs/perf/journal.md`; the logs a journal entry cites are committed under `docs/perf/logs/`.

**Zero allocations (hard gate), not timed:**
- **Test:** `tests/h1_writer_alloc.rs` with the dev-dependency `allocation-counter = "=0.8.1"` (MIT/Apache-2.0).
  - The API was read from the 0.8.1 source: `measure<F: FnOnce()>(F) -> AllocationInfo` at `lib.rs` L165, with
    fields `count_total`, `count_current`, `count_max`, `bytes_total`, `bytes_current` and `bytes_max`.
  - It counts **only the calling thread** (L137–138), and its `#[global_allocator]` is in `allocator.rs` L66.
    That is why it gets its own test binary. The `unsafe` lives inside that crate, so our crates gain none.
  - Its last release is 2023-09 and it is not deprecated. `cargo deny check` (advisories, bans, licences, sources)
    passes locally for the workspace and for `bench/writer/hyper-emul` (fix round 1); CI gains `cargo deny` with
    Task 15, until then the check is manual.
- **Steps:**
  1. Warm up with one pass over the `get`, `post1k` and `varied` rings (the counter must see that growth).
  2. `measure(|| for r in ring { write_request(...) })` must report `count_total == 0`.
  3. A hard check, once after the measured pass, that `out.as_ptr()` and `out.capacity()` equal their values
     after the warm-up. Together with `count_total == 0` (no allocation, so no move or growth inside the pass) that
     covers every ring request.
  4. A check that `out.capacity()` ≤ `MAX_P0_REQUEST_BYTES` (§6.7). Growth policy (exact reservations reallocate on
     each new high, bounded by `MAX_P0_REQUEST_BYTES`): measured and decided by Task 5b (item 31).

**Measured (fix round 2, `docs/perf/journal.md` 2026-11-13 run 3; dev on a quiet machine, indicative):**
writer / unchecked appends `get` 2.29×, `post1k` 3.01× (above its gate: the accepted miss, owned by Task 5b),
`varied` 2.62×; `writer − shape` (the checks' cost) `get` 10.0, `post1k` 58.5, `varied` 64.9 ns; ours + join /
hyper-emulation construct+encode 0.17–0.30× (clone+encode, the conservative figure: 0.26–0.42×). One-binary A/B
against earlier revisions (same-launch ratios): `varied` 0.90–1.03× fix round 1; `get` 1.18–1.19× and `post1k`
1.17–1.21× pre-review, as since fix round 1 (1.16–1.18× / 1.16–1.17×): Task 5 merges at fix round 1's speed and
Task 5b wins the gap back ([controller's call], §10.5). Fix round 1's numbers: `get` 2.43–2.51×, `post1k`
3.01–3.07×, `varied` 2.58–2.63× the then "copy floor" (journal 2026-11-08).

**Ratio gates: PROVISIONAL.** They hold until 10 rounds have run on the reference host in the nightly window *and*
on the x64 VM. After that the thresholds are reset from the median and bootstrap 95% CI (design §16.4); journal and
note are updated in the same PR. Until then each gate is the worst prototype dev value + 15%:
- `post1k` writer/appends ≤ **2.3×** (worst dev 1.97×);
- `varied` writer/appends ≤ **2.9×** (worst dev 2.51×);
- advisory absolute values: `get` ≤ 60 ns, `post1k` ≤ 250 ns, `varied` ≤ 300 ns (dev 28–35 / 111–220 / 110–247 ns).

**Gate redefinition — proposed for the owner's decision in Task 5b (fix round 2, M2; not applied).** The ratio
gates divide by the unchecked appends, which is not a lower bound and whose time moved 13–21% with code layout while
its code stayed the same (journal, 2026-11-08 fix round 1). A ratio also contains the body copy, so the same check
cost gives a different ratio per set. Proposal: (a) gate the cost of the checks in ns, `writer − shape` per set, from
the one-binary rotated bench, with the design §16.4 bootstrap CI; (b) gate the absolute ns per set against the
fastest rival writer measured in the same harness (Task 5c), which is the owner's "#1 against every rival" rule; (c)
keep `memcpy` as the reported lower bound and drop the ratio to the unchecked appends as a gate. Until the owner
decides, the ratio gates above stay as written and are reported with the ns differences beside them.

**Comparison targets** (reported in the PR and `journal.md`, not gated in P0): ≤ 0.5× the hyper-encoder emulation
(no hyper code runs until Task 5c): reported as ours + join / construct+encode (equal input shape) and ours /
clone+encode (the conservative figure; dev 0.26–0.42× in fix round 2), with Go for context.
The owner's goal is to beat every writer; the cross-language writer race is Task 5c.

**Share of a request:** ≈ 0.03–0.22 µs of ≈ 5 µs CPU (design §1-F). The parser takes ≈ 0.05–0.27 µs
(`docs/notes/response-parser.md` §8.2) and the `write` syscall itself ≈ 1–3 µs (§8.3).

**P1a:** `value_len` and `token_len` become `engine-simd` block classification
(`docs/notes/simd-scanning-research.md` §6.1). The delta block is contiguous, so it can be classified in one call.
**Target (defined, fix round 2, W2):** P1a halves `writer − shape` on `post1k` and on `varied` (the cost of every
check, §8.1), measured with the one-binary rotated bench against the fix-round-2 value in the journal (run 3:
`post1k` 58.5 ns, `varied` 64.9 ns). The earlier
"validation overhead ≤ 50 ns" named no quantity: the delta scan alone was ≈ 30 ns (already met) and writer − floor
90–112 ns (missed by 2×). The fixed cost on `get` (no delta) is not a classification cost and is Task 5b's (§10.5).
**G0:** Task 7's harness reports CPU µs per request end to end; the
writer is not gated separately there.

## 9. Test plan (beyond the brief)

### 9.1 Unit and integration (`tests/h1_writer.rs`)
The brief's eight tests, adapted to the new signatures (`template_rejects_invalid_defaults` takes byte fields). Each
`rejects_*` test that names header names runs **three letter cases**: lower, UPPER and miXed.

**Framing and reuse (§6.5):**
- `post_put_patch_query_send_content_length_even_when_empty_and_stay_reusable`;
- `get_head_delete_options_empty_body_send_none`, `query_with_body_is_framed_and_reusable`;
- `get_delete_options_with_body_send_connection_close_and_not_reusable`;
- `head_with_body_rejected`;
- `content_length_digits`: 0, 9, 10, 99, 100, 847 and 65,536 through the writer;
- `options_with_body_requires_content_type` (item 2).

**Origin binding and templates:**
- `rejects_template_for_other_authority`: the authority differs by port, by case and by a trailing dot;
- `template_rejects_invalid_host` (`INVALID_HOSTS`, 42): empty; SP, `/`, `@`, `?`, `#`, `%`, CR, LF, NUL, DEL, 0x80;
  UTF-8 `é`; `[::1` and `::1]`; port `:`, `:0`, `:65536`, `:123456` and `:-1`; empty labels; IPv6 literals WHATWG
  never writes (fix round 2, W3): `[2001:DB8::A]`, `[0001:0db8::0001]`, `[0:0:0:0:0:0:0:1]`, `[::ffff:1.2.3.4]`,
  `[1::2:3:4:5:6:7]` (a single zero piece compressed), `[1::2:0:0:0:3]` (not the longest run), `[1:0:0:2:3::4]` (the
  second of two equal runs);
- `template_accepts_hosts` (`VALID_HOSTS`, 14): `example.com`, `127.0.0.1:3999`, `[::1]:8080`, `xn--bcher-kva.example`,
  `a,b.example` (`,` is kept, §6.2), and the canonical IPv6 forms `[::ffff:102:304]`, `[2001:db8::a]`,
  `[1:0:2:3:4:5:6:7]`, `[1:0:0:2::3]` (the longer of two runs), `[1::2:3:0:0:4]` (the first of two equal runs; fix
  round 2 mutant `>`→`>=` in `longest_zero_run`) and `[::]`;
- `template_rejects_forbidden_defaults`: every forbidden name, underscore variants included;
- `template_sensitive_flag`: set by `authorization`, `cookie`, `x-api-key` and a custom configured name; clear
  otherwise;
- `template_too_large`: a template alone over 64 KiB gives `TooLarge(Headers)`;
- `template_rejects_injected_line_by_count` (item 1) and `template_is_send_and_sync`;
- `template_debug_hides_bytes` and `parts_debug_hides_bytes` (X4): no field, target or body bytes appear in the
  `{:?}` output.

**Header names:**
- `rejects_forbidden_user_headers`: every forbidden name, underscore variants included (CVE-2026-1525 case variants);
- `rejects_duplicate_singleton_template_vs_delta` and `rejects_duplicate_singleton_within_delta`, for each of
  `authorization`, `cookie`, `content-type` and `user-agent`;
- `rejects_injected_line_by_count` (X3): an injected `\r\nx-forwarded-for: 1.2.3.4` inside a value, and a
  `cookie` line injected when the request has none, are well-formed lines; `header_lines` is one short, so the
  writer returns `Engine(Internal)`. `header_lines_must_match` covers one too many and one too few in both
  directions;
- `rejects_latin1_truncated_forbidden_names`: the bytes a JS string with U+0154 would produce after latin1
  truncation (`0x54` = `T`), so the name `Te` written from `Ŕe` arrives as `Te` and must be rejected (W11). The
  JS layer must reject code points > 0xFF first (§10.4).

**Line grammar (§6.4):**
- `rejects_malformed_delta_lines` (`BAD_LINES`, delta and template): an empty line inside the block, a missing
  final CRLF, a bare CR, `a:b` without the SP, an empty name, SP before the colon, an obs-fold;
- `accepts_obs_text_htab_ows_and_empty_values`.

**Target (§6.3):**
- `target_must_be_origin_form` (`INVALID_TARGETS`, 21: not `/`-first, empty, `#` at start/middle/end, `"`, `<`, `>`,
  and per component (fix round 2, W4) `\`, `^`, `` ` ``, `{`, `}` in the path and `'` in the query; `VALID_TARGETS`,
  7: `//evil.example/x` per the §6.3 decision, `/../a/%zz` (not normalised), percent-encoded data with `'`, `|`,
  `[`, `]` in the path, raw `\^`{|}` and a second `?` in the query);
- `byte_classes_follow_the_whatwg_percent_encode_sets` (unit test in `writer/target.rs`): every one of the 256 byte
  values is allowed everywhere, in the path or in the query exactly as the URL Standard's sets say;
- `rejects_target_with_space_ctl_or_non_ascii`;
- `target_limit`: 16,384 accepted, 16,385 → `TooLarge(Url)`.

**Limits and error precedence (§7):**
- `fields_limit`: 65,536 / 65,537 → `TooLarge(Headers)`, counted as §6.4 states, host line included;
- `body_over_copy_limit_rejected`: 65,537 → `Engine(Internal)`;
- `error_precedence` (`PRECEDENCE`, 9 pairs): each pair of §7 rows with different codes — body vs target or fields
  limit, target limit vs fields limit, authority or target grammar, fields limit vs authority, `HEAD` with a body,
  a bad target or a bad delta — gives the earlier row's code; a long target containing CR gives `TooLarge(Url)`;
- `max_p0_request_bytes_is_the_largest_request`.

**Codec and buffer:**
- `method_from_wire_rejects_unknown_id`: `Method::from_wire(99)` gives `None`; the codec's mapping of an unknown id to
  `Engine(Internal)` is tested with the codec (Task 6, §10.1);
- `reuses_output_capacity` across a failing request; every test helper asserts that an error leaves `out` empty.

**Vectors:** `injection_vectors` runs every file of suite `engine-core:h1_writer` (§9.3).

### 9.2 Injection vectors (`tests/vectors/h1-request/blazehttp-rules.json`)
- For **names, values and targets**, every position class (start, middle, end) × {CR, LF, CR LF, NUL, HTAB in a name
  or target, SP in a name or target, DEL, VT, FF, 0x80 and 0xFF in a name or target, `#` in a target}.
- A value carrying `\r\nc: d`: the flat delta then holds a well-formed extra line, so the vector states the JS
  count (`headerLines` one less than the lines present) and expects `ENGINE/internal` (X2, X3). `\r\n\r\nGET /
  HTTP/1.1` smuggled in a value and in the target.
- `%0d%0a`: accepted as plain visible ASCII. It is percent-encoded data, which the server decodes only after parsing.
- Latin1-truncation bytes: U+010D U+010A become `0x0D 0x0A` (CVE-2018-12116), and U+0154 becomes `T`.
- A name with a trailing SP before the colon; `Content-Length` with a leading SP; an obs-fold continuation line
  (`\r\n x`).
- Every forbidden name in three cases plus the underscore spellings; singleton repeats.
- Every §6.5 method × body cell with its expected bytes and `reuse`.

### 9.3 Request-vector format (F5)
**Case shape** (new; documented in `docs/notes/vectors.md`):
```json
{
  "source": { "project": "…", "url": "…", "tag": "…", "commit": "…", "licence": "…", "copyright": "…" },
  "cases": [
    {
      "name": "content-crlf / CRLF in header value",
      "inputEncoding": "utf8",
      "request": {
        "method": "POST",
        "target": "/x",
        "templateHost": "127.0.0.1:3999",
        "templateFields": "accept: */*\r\n",
        "headers": "host: x\r\nfoo: bar\r\n",
        "headerLines": 2,
        "body": "hello"
      },
      "expect": { "error": "ENGINE/internal" },
      "derivedFrom": "test/headers-crlf.js:CRLF Injection in Nodejs ‘undici’ via host @ v8.12.4"
    }
  ]
}
```
- **`expect`** is `{ "bytes": "<exact head + body>", "reuse": "allowed" | "closeAfterExchange" }` or
  `{ "error": "<CODE/detail as ErrorCode Display prints it>" }`, for example `ENGINE/internal` or `TOO_LARGE/url`.
- **`inputEncoding`** (the same field name as the response schema, X8) follows the existing rules: `latin1` only
  when a byte ≥ 0x80 occurs. It applies to every byte string in the case.
- **`headerLines`** is required: the field count the JS layer would pass with `headers` (§6.4, X3). An injection
  vector states the count JS serialised, not the number of CRLFs present.
- **`connAuthority`** is optional and defaults to `templateHost`. It exists for the binding cases.
- **Discriminator:** the **directory**. Every vector file sits exactly one level below `tests/vectors`, in a
  directory named after its case kind (item 22, tested); `tools/vectors/schema.ts` picks the case schema from it (`h1-response/` → response case, `h1-request/` → request case), so no new `kind` field is needed and
  unknown keys stay errors.
- **`templateFieldLines`** is required (fix round 1): the count JS serialised into `templateFields`.
- **Rust:** `load_suite::<RequestCase>(suite)` (`tests/support/request_vectors.rs`); the index walk in
  `tests/support/vectors.rs` is shared with `load_suite::<ResponseCase>`, so both get the same unregistered-file and
  empty-file panics.
- **Index:** `tests/vectors/index.json` entries map to suite `engine-core:h1_writer`.
- **Provenance:** ported cases carry `derivedFrom` (`"<file>:<test> @ <tag>"`) and the file's `source.tag` and
  `source.commit`. `THIRD_PARTY_NOTICES` is regenerated (`pnpm vectors:notices`) with:
  - Go (BSD-3-Clause, needs its `PROJECT_LICENCE_TEXTS` entry naming "The Go Authors");
  - golang.org/x/net (BSD-3-Clause, same text);
  - undici (MIT) and hyper (MIT).

**Planned files and counts:**

| File | Source | Cases (planned) |
|---|---|---|
| `h1-request/blazehttp-rules.json` | own (§9.2) | ≈ 160 (built: 294) |
| `h1-request/undici-selected.json` | undici v8.12.4 (§9.4) | ≈ 25 (built: 38) |
| `h1-request/go-selected.json` | go1.27.3 (§9.4) | ≈ 15 (built: 17) |
| `h1-request/httpguts-selected.json` | x/net v0.62.0 `TestValidHeaderFieldName`, `TestIsTokenRune`, `TestPunycodeHostPort` | ≈ 15 (built: 145; `TestValidHeaderFieldName` has only 6 cases, so the token-rune and host tests were added) |
| `h1-request/hyper-selected.json` | hyper v1.12.0 (§9.4) | 4 (built: 5) |

### 9.4 Ported from upstream (behaviour only, re-expressed as vectors, at pinned tags)
- **undici `v8.12.4` (`779ee3e…`, MIT):**
  - `test/request-crlf.js`, `test/headers-crlf.js` and `test/header-function-crlf.js`: CRLF in method, path, header
    values and `host`;
  - `test/invalid-headers.js`: invalid names and values, the `transfer-encoding`/`upgrade`/`expect`/`keep-alive`/
    `connection` rules;
  - `test/upgrade-crlf.js` (CVE-2026-1527);
  - `test/content-length.js`: a user `content-length` and `content-length: 0` on a bodiless `PUT`;
  - `test/headers-as-array.js`: duplicate and case-variant `content-length` (CVE-2026-1525) and `host` — these cases
    live here, not in `content-length.js` (corrected in Task 5);
  - `test/query.js`: `QUERY` with and without a body.
- **hyper `v1.12.0` (`7f67699…`, MIT), `src/proto/h1/role.rs`:**
  `test_client_request_encode_explicit_transfer_encoding_removes_content_length` (L2745) and
  `test_client_request_encode_automatic_transfer_encoding_removes_content_length` (L2805). Ported as
  **reject-not-repair** vectors: a user `transfer-encoding` or `content-length` is an error for us, where hyper
  rewrites it. `test_client_request_encode_title_case` and the `orig_case` tests (L2857–2925) do **not** apply:
  names are sent as given, with no case rewriting.
- **Go `go1.27.3` (`2aae100…`, BSD-3-Clause):**
  - `src/net/http/requestwrite_test.go`: case 25 (CTL in `RawQuery` → error), case 26 (#40978, `PATCH` without a
    body → `Content-Length: 0`); the trailer injection cases (#78775) are **not ported** (no trailers in 1.0; they
    come with the chunked writer);
  - `src/net/http/request_test.go`: `TestRequestBadHostHeader` (L774, CVE-2023-29406; Go blanks the Host, we
    reject) and `TestRequestInvalidMethod` (L577; for us an unknown method is unrepresentable or fails in the codec).
- **golang.org/x/net `v0.62.0` (`3de643db506517b3e652a13cc0b56034cea6b8e6`, BSD-3-Clause):**
  `http/httpguts/httplex_test.go` `TestValidHeaderFieldName` (L112) → name-grammar vectors.
- **curl:** no case applies before proxies exist. Its relevant class (CR/LF in URLs sent to a proxy, CVE-2014-8150)
  is selected from `tests/data` by P1b together with the absolute-form work.

### 9.5 Fuzz (`fuzz_targets/h1_writer.rs`, `cargo-fuzz` on the pinned nightly)
**Plumbing:**
- `[[bin]] name = "h1_writer"` in `crates/engine-core/fuzz/Cargo.toml`;
- dictionary `fuzz/h1_writer.dict`: forbidden names in mixed case, underscore spellings, `\r\n`, `: `, ` HTTP/1.1`,
  `#`, `%0d%0a`, `[::1]` and `:65535`; fix round 2 adds the per-component target bytes (`^`, `` ` ``, `{`, `}`, `'`,
  `?`) and non-canonical IPv6 hosts (`[::FFFF:1.2.3.4]`, `[0:0:0:0:0:0:0:1]`);
- run with `-use_value_profile=1`.

**Structure-aware input (`arbitrary`):**
- method id;
- target bytes;
- template host and field lines;
- delta lines;
- body length;
- a "mutation" choice: forbidden names in every case, singleton repeats, and sizes exactly at 16,384/16,385
  (target) and 65,536/65,537 (fields).

**Line bound:** the generator emits at most 200 lines, and the target sizes the oracle's header array from the line
count (`vec![EMPTY_HEADER; lines + 2]`), so the oracle never fails for lack of slots.

**Properties when `write_request` returns `Ok`:**
1. **Byte-exact reference.** A deliberately naive reference serializer in the fuzz target (`format!` plus
   `Vec::extend`, no validation) produces the same bytes from the same accepted input.
2. **Oracle parse.** `httparse::Request::parse` (fuzz crate only) returns `Complete(n)` with:
   - the same method, and path == target;
   - headers == template ++ delta ++ generated lines, comparing values **after OWS trimming on both sides**;
   - `out[n..] == body`.
3. **Exactly one of each.** One `host` (first); at most one `content-length` (== body length, `1*DIGIT`); zero
   `transfer-encoding` and `expect`; a `connection` field only as the generated `close`; singletons at most once.
4. **Strictness beyond httparse:** every head byte is in `VCHAR ∪ {SP, HTAB, CR, LF} ∪ [0x80, 0xFF]`; CR only before
   LF; LF only after CR; exactly two SP in the request line.
5. **Head boundary (W11).** The first `\r\n\r\n` in `out` is exactly the end of the head (`n - 4`).
6. **Return value:** `Reuse` equals the §6.5 table for (method, body length).
7. **Line count (X3):** `header_lines` and `field_lines` are the number of **generated** lines (never counted from
   the bytes, item 9), plus an optional skew; a raw value holding `\r\nx: y` therefore mismatches as the attack does.
8. **Sensitive flag:** a fuzzed subset of sensitive names; `is_sensitive` equals the model's.
9. **Forbidden set from the output:** no design §12 forbidden name in any spelling appears in the written head
   besides the generated `content-length`/`connection` (read from the bytes, independent of the model's lists).
   Fix round 2 (item 10): no underscore spelling of a singleton appears, and each singleton appears at most once
   counting every spelling; a writer and model that both admitted `content_type` were caught in 6,278 executions,
   where the previous oracle ran 2.37 M executions without a crash.

**On `Err`:** `out` is empty.

**Over-strictness property:** an independent, plainly written spec predicate (`spec_accepts`) re-implements §6.3–§6.5
and §7 from the RFC grammar with per-byte loops and string comparisons. `write_request` must return `Ok` **iff**
`spec_accepts` is true, so the writer is neither too lenient nor too strict.

**Corpus:** runs start from an empty corpus (the §9.2 vectors are not expressible as structure-aware inputs) with the
dictionary; the main run keeps its corpus in the task logs and the fuzz mutants are measured from an empty corpus.

**Main runs, fix round 2 (capped container, one core, no crash):** 1,201 s and 5,887,521 executions on the flag-table
target check; then 601 s and 2,681,933 executions on the final target check, continuing from the kept corpus
(1,569 inputs, 1,998 edges).

### 9.6 Mutation testing (F8)
- **`cargo mutants`** (27.1.0), in the capped container (`--memory=2g --cpus=1`, `--jobs 1`, 20 s timeout):
  `cargo mutants -p engine-core --file 'crates/engine-core/src/h1/writer/**' --jobs 1 --timeout 20 -- -- --test-threads 2`
  (the writer was split into `writer/` in fix round 1, so the original `--file src/h1/writer.rs` named no file;
  R2-1). Target: **0 survivors**; each survivor is killed by a new test or justified as an equivalent mutant
  (`docs/notes/response-parser.md`'s practice). The overflow branch of the total-length sum is not a mutant:
  `try_fold(.., checked_add)` + `ok_or` has no operator cargo-mutants replaces.
  - **Fix round 1** (`writer.rs`, `grammar.rs`, `mod.rs`): 188 mutants, 174 caught, 2 timeouts, 10 unviable, 2 missed.
  - **Fix round 2** (`writer/**`): 185 mutants in 22 min, 164 caught, 2 timeouts, 16 unviable, 3 missed. One was a
    test gap, now killed: `longest_zero_run` `>`→`>=` (compress the *last* of two equal zero runs) survived because
    the vector named "first of equal zero runs" had runs of 2 and 3; `[1::2:3:0:0:4]` (valid) and `[1:0:0:2:3::4]`
    (rejected) now fail three tests under that mutant (RED, then GREEN).
  - **Timeouts (detected):** `split_field_line` replaced by an empty name with a fixed one-byte rest returns a rest
    that never shrinks, so the field loop never ends; the test run is killed at the timeout, which counts as caught.
  - **Equivalent (justified):** `admit`'s `seen | bit` → `seen ^ bit` runs only when `seen & bit == 0`, where `|`
    and `^` agree; the flag table's `NEVER = NOT_IN_PATH | NOT_IN_QUERY` → `^` (that run's `target.rs`, since
    replaced) combined two disjoint bits, where `|` and `^` agree; `grammar::non_field_content_flags` `|` → `^`
    (fix round 1, outside `writer/` now) is equivalent for the same reason (the first task report).
  - **Final target check** (the fold, `target.rs` only; the other writer files are unchanged since the run above):
    52 mutants in 3 min, **52 caught**, 0 missed, 0 timeouts, 0 unviable.
- **Fuzz mutant check (X8):** four deliberately broken writers, each applied as a **temporary patch in a scratch
  checkout** (the parser's practice, `docs/notes/response-parser.md` §9.7), never as `cfg`-gated branches in
  engine-core. Each must be caught by §9.5 within **10 minutes or 10 M executions** on one core, from an empty corpus:
  1. drop one forbidden name;
  2. flip the content-length condition;
  3. accept a bare LF in values;
  4. accept DEL in the target.

  Report the time to first crash for each in the PR. **Fix round 1** (empty corpus, after the speed fix): m1 0 s,
  m2 0 s, m3 15 s, m4 8 s. **Fix round 2** rewrote only the target check (`target.rs`); the field, grammar and
  framing code m1–m3 patch is unchanged. m4 was re-applied to each version: to the flag table (DEL's flags set to 0),
  caught from an empty corpus after 424,029 executions (≈ 44 s at 9,637/s); to the final fold (`emitted` also admits
  `0x7F`), caught after 200,084 executions (≈ 21 s at 9,527/s). The item-10 oracle fix was checked the same way
  (§9.5 property 9).

## 10. Hand-offs

### 10.1 To Task 6 (connection)
- **Reuse contract.** (a) `Ok(Reuse::CloseAfterExchange)`: send, read the response, then close; never pool.
  (b) On a writer **error nothing was written**: the request fails with that code, the connection stays clean and
  reusable, and no bytes are sent.
- One `write_request` per request into the connection's own reused `Vec<u8>`, then one `write`/`try_write` loop over
  that buffer. No `writev` (bodies are ≤ 64 KiB in P0, and TLS must copy anyway).
- Pass `conn_authority` from `ShardConfig`'s authority field (P0; the pool key from P1b); `ShardConfig::new` checks
  it is byte-equal to `template.authority()` (§6.6 item 2, X10). **Derive the dial address and `conn_authority`
  from that one field** (item 4): two fields equal by construction make the writer's check a tautology.
- Reference `MAX_P0_REQUEST_BYTES` (147,519 with the current spec limits), never a literal.
- The codec maps an unknown method id to `Engine(Internal)` before the writer runs; test it there (§9.1).
- **Partial writes:** if a request is aborted or cancelled after some bytes were written (partial write), close the
  connection (W11). "Zero bytes written" (the stale-connection re-send rule, design §5.1) comes from the first
  write's result.
- **Shrink is Task 6's:** after the write completes, `out.clear();` then, only if `out.capacity()` >
  `MAX_P0_REQUEST_BYTES`, `out.shrink_to(MAX_P0_REQUEST_BYTES)` (§6.7, X5). The writer clears at its start, reserves
  exactly and never shrinks.
- `TCP_NODELAY` on every connection (curl and Go do the same; `tcp(7)`); no `TCP_CORK`/`MSG_MORE`.
- `ShardConfig` owns its `HeaderTemplate` by value (`Send + Sync`); the shard wraps it in an `Rc` on its own thread
  only if its workers share it (§6.2, X1).
- **Budget hook (P1a, X6):** `required_len(parts, template)` is the interface P1a adds for charging the budget before
  `write_request` (§6.7).

### 10.2 To the template registry (dispatcher/shard)
- Look up by `(id, generation)`; a stale generation fails and never falls back.
- Deduplicate on full bytes, host included, never on a hash alone.
- Release sensitive templates immediately when unreferenced (`is_sensitive()`).
- Test: release a template, reuse the slot, submit the old handle → error.

### 10.3 To P1b (redirects and proxies)
**Redirects (W1, High).**
- A **cross-origin** hop (scheme, host or port differs) never reuses the template or the delta as they are. It
  builds a **new template for the new origin** from the original fields minus every `sensitiveHeaders` name (the
  client's configured list, passed to Rust with the template), and filters the delta by the same list, matching
  names with the writer's spelling rule (ASCII case folded, `_` read as `-`, item 3), so `x_api_key` is stripped
  with `x-api-key`.
- A same-origin hop keeps both.
- The reasons are the same class in four clients: undici CVE-2024-24758 / GHSA-3787-6prv-h9w3 and CVE-2023-45143 /
  GHSA-wqq4-5wpv-mx2g, Go CVE-2023-45289 / GO-2024-2600, and curl CVE-2018-1000007.
- Test: `cross_origin_redirect_strips_template_and_delta_sensitive_fields` covering `authorization`, `cookie`,
  `proxy-authorization`, `x-api-key` and a custom configured name. The `conn_authority` check (§6.6) makes a
  forgotten template swap to another **authority** fail loudly instead of leaking; a scheme-only change is caught
  only once P1a binds the origin (§10.5).
- **Field count (fix round 2, W1).** A head built in Rust has no "entries JS iterated", so P1b introduces a
  `FieldBlock` type that owns the field bytes and their count and is built only by `push(name, value)` (one
  increment per call) or from the entry count the codec received with a JS block; `RequestParts::headers` then
  takes a `&FieldBlock`. A count derived from CRLFs in the bytes would disable the X3 check (§6.4). Test:
  `field_block_count_is_independent_of_value_bytes` (a value pushed with `\r\nx: y` inside is rejected by `push`
  or, if pushed raw, makes the writer return `Engine(Internal)`).
- **Redirect targets** are built from the `Location` value through the WHATWG URL parser (`url` crate, design §12),
  so they meet §6.3; the writer does not normalise dot segments, `//` or `%` sequences (§6.3 "caller's
  responsibility").

**Proxies (W6).** A separate `Target::Absolute` variant:
- allowed only on a connection to an HTTP proxy;
- scheme `http` only (HTTPS goes through CONNECT);
- no userinfo and no `@`;
- its authority must be byte-equal to the template host after normalisation (lowercase, punycode, default port
  removed);
- bytes per the §6.3 component rule applied to its path and query (the scheme and authority are checked as
  above).

The writer emits `proxy-authorization` from the `ProfileContext`'s proxy credentials **only with this variant**:
never with origin-form, never inside a CONNECT tunnel. The forbidden set is **never relaxed** for user input.

Proxy tests:
- origin-form on a proxy connection rejected;
- absolute-form on a direct connection rejected;
- userinfo rejected;
- authority/Host mismatch rejected.

**Proxy bodies (W2 ruling):** with a forward proxy configured, bodies on `GET`/`HEAD`/`DELETE` are rejected with
`INVALID_CONFIG` (RFC 9110 §9.3.1/§9.3.2/§9.3.5); `OPTIONS` with a body requires a valid `Content-Type` (P2).

**Large bodies (W5, W11):** the §6.7 rules.

### 10.4 To P2 (JS layer)
- Build deltas as `("name: value\r\n")*` with `buf.write(str, off, 'latin1')`, after rejecting code points > 0xFF
  (CVE-2018-12116, and the U+0154 → `T` case) and every §6.3–§6.5 violation with `INVALID_CONFIG`.
- `templateHost` is WHATWG `url.host`: punycode, lowercase, bracketed IPv6, default port omitted, no userinfo. A host
  outside the §6.2 class (WHATWG accepts e.g. `a{b`) is rejected with `INVALID_CONFIG {field: 'url', reason:
  'host'}` before it reaches Rust (X7).
- Pass `header_lines` and the template's `field_lines`: the number of header entries iterated, **never derived from
  the serialised bytes** (§6.4, items 1 and 9). P2 test: a value containing `\r\nx: y` gives a count of 1 and is
  rejected (by P2 first, by Rust as a mismatch otherwise).
- Hosts: lowercase, IPv6 literals in the URL Standard's serialisation, ports without a leading zero — WHATWG
  already emits these (item 8). **WHATWG can emit port 0 and empty labels** (`http://a:0/` → `a:0`,
  `http://a..b/` → `a..b`; fix round 2, W5): P2 rejects both with `INVALID_CONFIG {field: 'url', reason: 'host'}`.
  P2 tests: `a:0`, `a..b` and `a.example..` next to `a{b`.
- Reject `OPTIONS` content without a `Content-Type` **or with one that is not a valid `media-type`** (RFC 9110
  §8.3.1, §9.3.7; item 2; the writer checks presence only, W6), and the forbidden names of §6.4 including
  `trailer`, `http2-settings` and every `proxy-` name (item 6), with `INVALID_CONFIG`.
- One line per non-list field name: RFC 9110 §5.3; the writer passes such repeats through (item 7). P2 test:
  `rejects a repeated non-list field and a per-request header that repeats a default` (`INVALID_CONFIG`), next to
  the writer's pass-through vectors `duplicates / …`.
- Never repeat a template field name in a delta: a request that overrides a default uses a template without that
  default (or a host-only template). Rust enforces only the singleton subset (§6.4). A per-request skip mask was
  considered and rejected for now (YAGNI; add it if P2 measures template churn).
- Strip or reject a user `Content-Length` (design §12, "conflicting Content-Length → INVALID_CONFIG"). Reject a body
  on `HEAD` with `INVALID_CONFIG`.
- Join `baseURL` and `url` through WHATWG `URL`, so that no path can change the origin (undici CVE-2022-35949 class;
  `//x` paths are valid, §6.3).
- **P4 note (undici transport parity):** undici accepts DEL and 0x80–0xFF in paths, does not reject `#`, and allows
  `connection`, `proxy-authorization`, `te` and `host` headers and repeated singletons. The TS layer must apply our
  stricter rules before calling undici.

### 10.5 To P1a and Task 5b (fix round 1)
- **Origin binding (item 4):** P1a's TLS task introduces a typed `Origin { scheme, host, port }` used to dial, to key
  the pool and to build the template; `write_request` compares origins. Test: an `https` template on an `http`
  connection → `Engine(Internal)`.
- **Rule diagnostics (item 29):** about nine writer rules share `ENGINE/internal`. P1a adds a private `WriteRule` enum
  carried in the log/diagnostic context only (wire code unchanged; never bytes), with tests that assert the rule.
- **Budget hook (X6):** `required_len(parts, template)`, §6.7.
- **Criterion benches:** the example benches are replaced (#23).
- **Task 5b:** the `post1k` writer/appends gate (accepted miss in Task 5), the gate redefinition proposed in §8.4
  (owner decision), **winning back the `get`/`post1k` time lost to fix round 1's safety checks and then beating the
  pre-review writer** (1.16–1.21× pre-review in one binary, journal 2026-11-13 run 3; hypothesis: a per-call fixed
  cost in the added checks, since the check-free shape is not the cause and skipping empty pieces measured slower;
  `ideas.md`), and the exact-vs-amortised reservation growth (item 31: each new high
  reallocates; measure `try_reserve` with the §6.7 shrink bound and add a growth test).

## 11. Design changes (accepted; applied in the Task 5 PR)
Exact text changes for `docs/specs/2026-11-06-blazehttp-design.md`. This note round does not edit the design.

1. **§5.1 `RequestWriter` row, key rules:** append "`content-length` only when there is a body or the method is
   `POST`/`PUT`/`PATCH`/`QUERY` (then `0` for an empty body); a body on `GET`/`DELETE`/`OPTIONS` adds a generated
   `connection: close` and the connection is not reused; `HEAD` with a body is rejected; templates are bound to the
   connection's authority".
2. **§5.2 "Outgoing request target":** replace with "validated after `paramsSerializer` runs: origin-form only (`/`
   first), at most 16 KiB, and only bytes the WHATWG URL serializer emits in the component they sit in — no
   control, SP, DEL, byte ≥ 0x80, `#`, `"`, `<` or `>` anywhere, no `\`, `^`, `` ` ``, `{` or `}` in the path, no `'`
   in the query (`docs/notes/request-writer.md` §6.3); dot segments, `//` and `%` sequences are not normalised;
   absolute-form only to an HTTP proxy, with an authority equal to Host (P1b)." (fix round 2)
3. **§12 Injection row:** add "Rust also rejects the underscore spellings of every forbidden or singleton name
   (`content_length`, `transfer_encoding`, `keep_alive`, `http2_settings`, every `proxy_` name, `content_type`,
   `user_agent`), a repeat of `authorization`, `cookie`, `content-type`, `user-agent` across defaults and
   per-request headers, and a per-request header block whose line count differs from the count JS serialised;
   `HEAD` with a body → `INVALID_CONFIG`; with a forward proxy, bodies on `GET`/`HEAD`/`DELETE` → `INVALID_CONFIG`".
4. **§12 Redirects row:** add "a cross-origin hop builds a new header template without `sensitiveHeaders` names and
   filters the per-request headers the same way".
5. **§11.2 Header templates:** add "bound to one authority; looked up by (id, generation), never falling back;
   deduplicated on full bytes, never a hash alone; flagged sensitive from the client's `sensitiveHeaders`".
6. **§11.5 limits table and `spec.json`:** add `maxRequestTargetBytes` 16 KiB → `TOO_LARGE/url`,
   `maxRequestFieldsBytes` 64 KiB → `TOO_LARGE/headers`, `maxCopiedBodyBytes` 64 KiB (P0: rejected by the connection's `try_submit` with `TOO_LARGE/request-body`; larger bodies do not reach
   the writer); the writer exposes `MAX_P0_REQUEST_BYTES` (the three limits plus the fixed head overhead), above
   which the connection shrinks its write buffer after a write (X5); P1a adds the budget hook
   `required_len(parts, template)` (X6).
7. **§5.3:** replace "vectored writes only for large plaintext bodies" with "vectored writes only for plaintext
   bodies above a measured threshold (P1b); never `write_vectored` under TLS (rustls allocates per call)".
8. *(fix round 1)* **§5.1 row:** one pooled buffer, body copied (≤ `maxCopiedBodyBytes`), large-body path P1b; **§11.1:**
   the P1a `required_len` budget hook; **§12 Injection row:** `trailer`, `http2-settings`, `proxy-`, OPTIONS
   `Content-Type`, the template line count.
9. **§13.2 rule 1:** add "Exception: a passive, borrowed parameter object with no invariants and no state changes may
   have `pub` fields; its doc comment says so (e.g. `RequestParts`)."
10. *(fix round 2)* **§12 Injection row:** the JS `INVALID_CONFIG` list gains `Trailer`, `HTTP2-Settings`, every
    `Proxy-` field, `OPTIONS` content without a valid `Content-Type`, a repeated non-list field name, a per-request
    header repeating a default's name, and a host with port 0 or an empty label (P2 tests, §10.4); Rust's list gains
    the per-component target rule and the canonical IPv6 host; the line count closes a well-formed injected line
    only with a count each producer takes from its entries. **§13.2 rule 1 example:** `RequestParts` carries one
    caller contract, the independent `header_lines` count, stated on the type.

## 12. Engineering survey (blogs, docs, write-ups)
- Marc Brooker, "It's always TCP_NODELAY. Every damn time." (2024-05-09,
  [link](https://brooker.co.za/blog/2024/05/09/nagle)): Nagle × delayed ACK, and a recommendation to set
  `TCP_NODELAY`. Unverified for our hosts; consistent with `tcp(7)`.
- Node.js `writable.cork()` docs (v26.13.0, [link](https://nodejs.org/api/stream.html#writablecork), verified):
  buffered chunks are passed "to `writable._writev()`, if present", so undici's head and body leave in one `writev`.
- Integer formatting literature (§5.3): jeaiii's algorithm explained by Junekey Jeon
  ([link](https://jk-jeon.github.io/posts/2022/02/jeaiii-algorithm)), and Lemire's posts and paper. The gains are
  reported for ≥ 8-digit numbers; **their numbers are unverified** and not used for the decision (moved to
  `ideas.md`). Our own measurement (§8.2) found the three table-based variants within noise of each other for 0–10⁴
  and 0–10¹³.
- PortSwigger research (2019 unverified; 2022 and 2025 verified) — §4.2.

## 13. Sources (accessed 2026-11-07)

**Official documentation (verified):**
- Rust 1.99.1:
  - [`u64::format_into`](https://doc.rust-lang.org/1.99.1/std/primitive.u64.html#method.format_into) and
    [`usize::format_into`](https://doc.rust-lang.org/1.99.1/std/primitive.usize.html#method.format_into);
  - [`fmt::NumBuffer`](https://doc.rust-lang.org/1.99.1/std/fmt/struct.NumBuffer.html) ("1.98.0");
  - [`Vec::clear`](https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html#method.clear) ("no effect on the
    allocated capacity");
  - [`Vec::try_reserve`](https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html#method.try_reserve),
    [`Vec::try_reserve_exact`](https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html#method.try_reserve_exact),
    [`Vec::shrink_to`](https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html#method.shrink_to);
  - [`Vec::extend_from_slice`](https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html#method.extend_from_slice)
    (panics only past `isize::MAX`);
  - [`u8::is_ascii_graphic`](https://doc.rust-lang.org/1.99.1/std/primitive.u8.html#method.is_ascii_graphic);
  - [`io::Write::write_vectored`](https://doc.rust-lang.org/1.99.1/std/io/trait.Write.html#method.write_vectored);
  - the `core::fmt::num` source at 1.99.1 (§2).
- tokio 1.54.1 [`TcpStream`](https://docs.rs/tokio/1.54.1/tokio/net/struct.TcpStream.html): `try_write` ("only part
  of the buffer may be written"), `try_write_vectored`, `set_nodelay`.
- Linux man-pages 6.19:
  - [`tcp(7)`](https://man7.org/linux/man-pages/man7/tcp.7.html): `TCP_NODELAY`; `TCP_CORK` has a 200 ms ceiling and
    is not portable;
  - [`send(2)`](https://man7.org/linux/man-pages/man2/send.2.html): `MSG_MORE` is a per-call `TCP_CORK`;
  - [`writev(2)`](https://man7.org/linux/man-pages/man2/writev.2.html): written as a single block; `IOV_MAX` 1024.
- RFCs:
  - [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110) §4.1, §5.3, §5.5, §7.2, §8.3.1, §8.6, §9.3.1–9.3.2, §9.3.5,
    §9.3.7;
  - [RFC 9112](https://www.rfc-editor.org/rfc/rfc9112) §3, §3.2, §3.2.1–3.2.2, §5.1, §9.6;
  - [RFC 10008](https://www.rfc-editor.org/rfc/rfc10008) (QUERY).
- [Node.js stream docs v26.13.0](https://nodejs.org/api/stream.html#writablecork).
- [URL Standard](https://url.spec.whatwg.org/) (living standard, read 2026-11-13 for fix round 2): the C0-control,
  fragment, query, special-query and path percent-encode sets (§1.3) and the IPv6 serializer (§3.6).
- `allocation-counter` 0.8.1 source (crates.io download): `lib.rs` L84–121 and L137–212, `allocator.rs` L66.

**Source code (verified):** every file and line range in §2, plus:
- undici v8.12.4 tests;
- hyper v1.12.0 `role.rs` tests (L2745–2925);
- Go go1.27.3 `request_test.go` L577 and L774;
- x/net v0.62.0 `httplex_test.go` L112.

**Advisories (verified):**
- undici advisories in §4.1 and §10.3 ([GitHub](https://github.com/nodejs/undici/security/advisories)),
  incl. GHSA-8qr4-xgw6-wmr3;
- Go: [GO-2023-1878](https://pkg.go.dev/vuln/GO-2023-1878), [GO-2024-2600](https://pkg.go.dev/vuln/GO-2024-2600);
- NVD: [CVE-2019-9741](https://nvd.nist.gov/vuln/detail/CVE-2019-9741),
  [CVE-2018-12116](https://nvd.nist.gov/vuln/detail/CVE-2018-12116),
  [CVE-2019-9740](https://nvd.nist.gov/vuln/detail/CVE-2019-9740),
  [CVE-2019-18348](https://nvd.nist.gov/vuln/detail/CVE-2019-18348);
- curl: [CVE-2014-8150](https://curl.se/docs/CVE-2014-8150.html),
  [CVE-2018-1000007](https://curl.se/docs/CVE-2018-1000007.html);
- [hyper advisories](https://github.com/hyperium/hyper/security/advisories);
- [Beast GHSA-8c6g-xf48-2fvx](https://github.com/boostorg/beast/security/advisories/GHSA-8c6g-xf48-2fvx).

**Papers and research:**
- verified: T-Reqs (from the PDF), HTTP Garden (abstract), PortSwigger 2022 and 2025;
- unverified beyond descriptions: HDiff, Host of Troubles, and Kettle 2019 ("HTTP Desync Attacks: Request Smuggling
  Reborn", [link](https://portswigger.net/research/http-desync-attacks-request-smuggling-reborn), not re-read for
  this note);
- unverified and not used for decisions: Champagne Gareau & Lemire 2026, jeaiii/Jeon.

**Measurements (verified, this note):** §8.

## 14. Deferred (with phase)
- **P1b:**
  - the large-body `write_head` + `writev` path and its threshold; the TLS large-body strategy (§6.7);
  - `Target::Absolute` and proxy `proxy-authorization` (§10.3);
  - cross-origin template rebuild on redirects (§10.3).
- **P1a:**
  - budget charging of `out` growth (§6.7);
  - SIMD block classification of the delta.
- **When streaming uploads of unknown length are implemented:** chunked request bodies (`transfer-encoding: chunked`
  generated by the writer) and trailers. The §6.4 validator is reused for trailers (Go #78775).
- **Only if P2 measures template churn from overrides:** a per-request template skip mask.
