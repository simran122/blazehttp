# `RequestWriter` and header templates: research note (Task 5)

Design §4 (request flow: per-request bytes in pooled buffers, header templates), §5.1 (`RequestWriter`: pre-encoded
template bytes, one syscall per request where possible), §5.2 (outgoing request-target), §5.3 (head + body in one
buffer for small requests, vectored writes only for large plaintext bodies), §10 (latin1 header deltas, values
≤ 0xFF, Rust re-validates), §11.2 (templates), §12 (Injection row), §13.1, §16.2. Access date for every link:
**2026-11-07**. **Verified** = checked against the pinned source, an official document, or a measurement run for this
note; **unverified** = everything else (unverified ideas are listed only in `docs/perf/ideas.md`).

## 1. Summary and decision

**Decision: Design B — a pre-encoded, pre-validated header template plus a validated per-request delta, appended
with `extend_from_slice` into one reused `Vec<u8>` sized once with `try_reserve`; `Content-Length` formatted with the
standard library's `u64::format_into` (stable since 1.98.0); head and body in one buffer for one write.**

- **Template** (built once per client and origin on the control plane): `host: <authority>\r\n` followed by the
  client's default fields, stored as one immutable `Box<[u8]>`. Validated once when it is built.
- **Per request:** `METHOD SP target SP "HTTP/1.1" CRLF` + template + delta + `content-length: N CRLF` (when due) +
  `CRLF` + body. Zero allocations once `out` has grown to the connection's working size (`Vec::clear` keeps the
  capacity; a single `try_reserve` per request is a no-op then).
- **Re-validation in Rust on every request** for the bytes that change per request: the target (VCHAR only, origin
  form, ≤ 16 KiB) and the delta (`token ": " field-content CRLF` per line, forbidden names in any case). This is the
  single choke point every outgoing head passes through — the JS layer (P2), redirects built in Rust (P1b) and any
  future source. undici's history (§4) is five CVEs from the same class, each a new sink that skipped validation;
  a single choke point in Rust closes the class.
- **Measured (dev, Linux arm64 container on a loaded host, §8.2):** the chosen writer costs **≈ 28–35 ns** for a
  template-only `GET` and **≈ 110–220 ns** for a `POST` with a 1 KiB JSON body and 3 extra headers (half to two thirds of it is
  copying the bytes, the rest validation) — **0.18× (GET) and 0.26–0.27× (POST)** an emulation of hyper 1.12's
  encoder with the per-request `HeaderMap` it requires, measured in the same runs, and ≈ 1.8–2.2 µs / 2.7–5.6 µs with
  5 / 10 allocations for Go 1.27.3 `(*Request).Write`. Against the ≈ 5 µs CPU budget of a whole request (design
  §1-F) the writer is ≈ 0.5–4%.

**Interface: one change to the brief, one rule change** (§6.6): `HeaderTemplate::new` takes the host and the
default fields as **wire bytes** in the same `("name: value\r\n")*` format as `RequestParts::headers` (latin1 values
cannot round-trip through `&str`, and one format means one validator); `content-length: 0` is also sent for an empty
`QUERY` (RFC 10008 defines content for it; RFC 9110 §8.6). Error mapping refined (§7): limits map to `TooLarge`,
allocation failure to `Engine(ResourceExhausted)`, grammar violations to `Engine(Internal)` as the brief says.

**Deferred with numbers, not opinions (§8.3):** the copy-vs-`writev` threshold for large plaintext bodies. On
loopback the single copied buffer is as cheap as `writev` up to 4 KiB (within noise in 3 runs), `writev` wins from
16 KiB in 2 of 3 runs and clearly from 64 KiB (1.6–3.3×); two separate writes are the worst at 1–4 KiB (1.5–2×).
Under TLS rustls 0.23.46's `write_vectored` allocates a `Vec` per call, so TLS always uses the single buffer. The
large-body path (a `write_head` sharing this code, threshold to be set between 4 and 16 KiB on the reference host)
lands with Task 6 or the task that owns bodies > 64 KiB; until then `write_request` copies every body (correct, one
write).

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
| undici (Node) | `v8.12.4` | `779ee3e12cb8c1f365f6e69ee34b088159402d2f` | [`lib/dispatcher/client-h1.js`](https://github.com/nodejs/undici/blob/779ee3e12cb8c1f365f6e69ee34b088159402d2f/lib/dispatcher/client-h1.js): `shouldSendContentLength` L1168–1170, `writeH1` L1203–1397 (`expectsPayload` incl. `QUERY` L1217–1223; head built by string concatenation L1343–1371; `HEAD` → no reuse L1314–1320), `writeBuffer` L1536–1564 (`cork`, head `write(..., 'latin1')`, body `write`, `uncork`); [`lib/core/request.js`](https://github.com/nodejs/undici/blob/779ee3e12cb8c1f365f6e69ee34b088159402d2f/lib/core/request.js): `invalidPathRegex = /[^!-ÿ]/` L29 + L124, `processHeader` L472–570; [`lib/core/util.js`](https://github.com/nodejs/undici/blob/779ee3e12cb8c1f365f6e69ee34b088159402d2f/lib/core/util.js): `isValidHTTPToken` L738+, `headerCharRegex = /[^\t\x20-\x7e\x80-\xff]/` L759–767; tests `test/headers-crlf.js`, `test/content-length.js` |
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
| undici 8.12.4 | String concatenation per request: `${method} ${path} HTTP/1.1\r\n`, `host:` (or cached `kHostHeader`), `connection: keep-alive`, user headers | Template literal | At request construction (`processHeader`): token names, `headerCharRegex` values, path `/[^!-ÿ]/` (**allows DEL and 0x80–0xFF in the path**), forbids `transfer-encoding`, `keep-alive`, `upgrade`, `expect`; one `content-length` | `socket.cork()`, `write(head, 'latin1')`, `write(body)`, `uncork()` → one `writev` | Buffer, stream, iterable, blob | Five CRLF-class CVEs in sinks that skipped validation (§4) |
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
- **Stricter target than undici and Bun:** VCHAR only (no DEL, no 0x80–0xFF), and origin-form only until P1b adds
  absolute-form for proxies (§6.3).
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
| undici GHSA-3cvr-822r-rqcc / CVE-2022-31150 (< 5.8.0) | CRLF in header values, path and method | values: `FIELD_CONTENT` only; target: VCHAR only; method from a closed enum | JS + Rust | `rejects_header_value_ctl_except_htab`, `rejects_target_with_space_ctl_or_non_ascii` |
| undici GHSA-f772-66g8-q5h3 / CVE-2022-35948 | CRLF via `content-type` (a sink that skipped validation) | every delta byte re-validated in Rust whatever its source | Rust | `injection_vectors` |
| undici GHSA-5r9g-qh6m-jxff / CVE-2023-23936 | CRLF via `host` | Host only from the template; `HOST` byte class at template build; user `host` forbidden | JS + Rust | `template_rejects_invalid_host`, `rejects_forbidden_user_headers` |
| undici GHSA-4992-7rv2-5pvq / CVE-2026-1527 | CRLF via the `upgrade` option | `upgrade`/`connection` forbidden; no upgrade path in 1.0 | JS + Rust | `rejects_forbidden_user_headers` |
| undici GHSA-m8rv-5g2x-5cg5 / CVE-2026-15157 (fixed 8.9.0) | CRLF via a blob-like body's `.type` pushed into headers unvalidated | same as CVE-2022-35948: Rust validates the assembled delta, not individual sinks | Rust | `injection_vectors` |
| undici GHSA-2mjp-6q6p-2qxm / CVE-2026-1525 | Two `Content-Length` fields via case variants in a header array | user `content-length` forbidden in any case; exactly one generated | JS + Rust | `rejects_forbidden_user_headers` (case variants), fuzz property "exactly one CL" |
| undici GHSA-35p6-xmwp-9g52 / CVE-2026-6733 | Unsolicited response on an idle keep-alive socket attached to the next request | not a writer rule → Task 6 (`any byte while idle → close`, design §5.1) | Rust (Task 6) | Task 6 |
| Node CVE-2018-12116 (< 6.15 / 8.14) | Unicode path written as latin1: U+010D U+010A → CR LF | JS writes deltas with `buf.write(str, off, 'latin1')`: code points > 0xFF must be rejected **before** the write (design §10); Rust sees only bytes and rejects CR/LF either way | JS + Rust | P2 test; Rust `injection_vectors` (CR/LF bytes) |
| Go CVE-2019-9741 (GO-2019 net/http) | CRLF in URL query via `NewRequest` | target VCHAR only | Rust | `rejects_target_with_space_ctl_or_non_ascii` |
| Go CVE-2023-29406 / GO-2023-1878 | Host header not fully validated → header/request injection | `HOST` byte class (Go's `validHostByte` set), reject (Go blanks) | Rust | `template_rejects_invalid_host` |
| Go #78775 (go1.27.x) | CR/LF in trailer names/values | no trailers are sent in 1.0; a future chunked writer reuses the field validator | — | deferred (§14) |
| Python CVE-2019-9740 / -9947 / -18348 | CRLF in URL query, path, host (`urllib`) | target VCHAR; Host byte class | Rust | as above |
| curl CVE-2014-8150 | CR/LF in a URL copied into an absolute-form target sent to a proxy | absolute-form (P1b) gets the same VCHAR check | Rust (P1b) | P1b proxy tests |
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
  `docs/notes/response-parser.md`). → The writer's output is checked against a request parser oracle (§9.4).
- **HDiff** (Shen et al., DSN 2022, DOI 10.1109/DSN53405.2022.00014): semantic-gap differential testing from RFC
  grammar. Only the description via the HTTP Garden paper was readable — **unverified** beyond that.
- **Host of Troubles** (Chen, Jiang, Duan, Weaver, Wan, Paxson, CCS 2016): multiple-Host, whitespace-wrapped Host and
  absolute-URI-vs-Host ambiguities enable cache poisoning and filter bypass (abstract-level, via search summaries —
  **unverified** in detail). RFC 9112 §3.2.2 (verified): an origin server receiving absolute-form "MUST ignore the
  received Host header field". → exactly one Host, first, from the template; **origin-form only** on a direct
  connection (an attacker-shaped absolute-form target would silently override Host at the origin).
- **Kettle, "HTTP Desync Attacks: Request Smuggling Reborn"** (PortSwigger, 2019) — background; **"Browser-Powered
  Desync Attacks"** (10 Aug 2022, [link](https://portswigger.net/research/browser-powered-desync-attacks), verified):
  client-side desync needs a server that "must ignore the request's Content-Length"; the client's connection pool is
  then poisoned. **"HTTP/1.1 must die"** (6 Aug 2025, [link](https://portswigger.net/research/http1-must-die),
  verified): 0.CL desyncs from bodies an intermediary ignores, `Expect`-based desyncs ("Simply sending a valid Expect
  header causes a 0.CL desync on numerous different servers"), and the advice "Reject requests that have a body, if the
  method doesn't require one to be present (GET/HEAD/OPTIONS)".
  → Never send `Expect` (design §5.1 already); a body on `GET`/`HEAD`/`DELETE`/`OPTIONS` is always framed with
  `content-length` by the writer, and Task 6 must not reuse the connection after one (undici does the same through
  `!expectsPayload` → `kReset`); whether P2 allows such bodies at all is an owner decision (§11 item 3).
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
| Risk | slow; `fmt` cannot carry latin1 bytes as `&str` | template must be validated at build time and must not repeat a delta name (P2 rule, §10.2) | many small segments without `TCP_CORK` |

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
Junekey Jeon, [blog](https://jk-jeon.github.io/posts/2022/02/jeaiii-algorithm)); Lemire's AVX-512 conversion
(2022 blog, 2.5 vs 8.8 ns for 16 digits) and Champagne Gareau & Lemire, "Converting an Integer to a Decimal String
in Under Two Nanoseconds" (SPE, [arXiv 2604.26019](https://arxiv.org/abs/2604.26019)) — both aimed at long numbers.
Content-Length values are 1–7 digits in practice.

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
| Measured (loopback, §8.3) | equal to S2 up to 4 KiB | cheaper from 16 KiB (2 of 3 runs), 1.6–3.3× at 64 KiB | 1.5–2× worse at 1–4 KiB |

`TCP_CORK`/`MSG_MORE` (Linux-only, `tcp(7)`: "should not be used in code intended to be portable", 200 ms ceiling)
are not needed: one `write`/`writev` per request already carries the whole request (verified, man-pages 6.19).

## 6. Chosen design (precise)

### 6.1 Files (Task 5 scope)
- `crates/engine-core/src/h1/grammar.rs` (moved from the parser branch, design §13.1 rule 2): add `VCHAR` (0x21–0x7E)
  and `HOST` tables next to `TCHAR`/`FIELD_CONTENT`, and `field_lines_ok`-style shared scanning only if the parser can
  use it too; otherwise the field-line validator lives in the writer.
- `crates/engine-core/src/h1/writer.rs`: `HeaderTemplate`, `RequestParts`, `write_request`, the forbidden-name check.
  One responsibility (serialise a request head); split into `writer/{mod.rs, template.rs}` only if it passes ≈ 500
  lines.
- `crates/engine-core/tests/h1_writer.rs`, `crates/engine-core/examples/h1_write_bench.rs`,
  `crates/engine-core/fuzz/fuzz_targets/h1_writer.rs`, `tests/vectors/h1-request/` (injection vectors as data).
- `policy/spec.json` (+ regenerated `spec.rs`): `limits.h1.maxRequestTargetBytes = 16384` (design §11.5 URL limit)
  and `limits.h1.maxRequestFieldsBytes = 65536` (template + delta), so the limits have one source of truth.
- `h1/mod.rs` re-exports `HeaderTemplate`, `RequestParts`, `write_request`.

**Dependency on Task 4:** `grammar.rs` lives on `p0/parser` (not yet on `milestone/p0`). Task 5's branch starts
after Task 4 merges.

### 6.2 Template
```rust
pub struct HeaderTemplate { bytes: Box<[u8]> }   // "host: <host>\r\n" ("name: value\r\n")*
impl HeaderTemplate {
    pub fn new(host: &[u8], fields: &[u8]) -> Result<Self, ErrorCode>;
}
```
- `host`: non-empty, every byte in `HOST` = ALPHA / DIGIT / `!$%&'()*+,-.:;=[]_~` (Go's `validHostByte`, which
  covers reg-name, IPv4, bracketed IPv6 and `:port`); comes punycoded from the WHATWG URL parser (design §12), so
  non-ASCII is an error.
- `fields`: same grammar and forbidden set as the per-request delta (§6.4), validated **once** here.
- Total `host` line + `fields` ≤ `maxRequestFieldsBytes`.
- Built with one exact allocation on the control plane (dispatcher, design §11.3); immutable afterwards; shared
  per shard (refcount and LRU are the template registry's job, later task). Sensitive templates are released
  immediately when unreferenced (design §11.2) — the writer holds only `&HeaderTemplate`.

### 6.3 Request target
- 1..=`maxRequestTargetBytes` bytes, every byte in `VCHAR` (0x21–0x7E): no SP, no CTL, no DEL, no 0x80–0xFF.
  Stricter than undici (`[!-ÿ]`) and Bun (accepts ≥ 0x80) — the URL layer percent-encodes (design §5.2).
- **First byte `/`** (origin-form, RFC 9112 §3.2.1). Absolute-form for HTTP proxies arrives in P1b as an explicit
  variant whose authority must equal the template's Host (RFC 9112 §3.2.2); asterisk-form is never produced (no
  server-wide `OPTIONS` in the API).
- Scanned with a branch-free fold over `b.wrapping_sub(0x21) < 0x5E` (auto-vectorisable; targets are short and
  bounded) — or `VCHAR`-table `position` if the fold does not win in the committed bench.

### 6.4 Per-request delta (`RequestParts::headers`)
Grammar, checked in one pass with no allocation:
```
delta      = *( field-name ": " field-value CRLF )
field-name = 1*TCHAR                       ; grammar::token_len
field-value= *FIELD_CONTENT                ; grammar::value_len (HTAB, SP–'~', 0x80–0xFF)
```
- Separator exactly `": "` (our own JS emits it; no other whitespace before or after the colon, RFC 9112 §5.1).
- The value scan stops at the first byte outside `FIELD_CONTENT`; the next two bytes must be exactly CR LF. Hence a
  bare CR, a bare LF, NUL, any other CTL and DEL anywhere in a value are errors, and a line can never start with
  SP/HTAB (T-Reqs "manipulated termination", obs-fold).
- Empty values are allowed (`x: \r\n` — trailing OWS, valid). Leading/trailing SP/HTAB inside the value are allowed
  as OWS; no trimming (undici and axios do not trim either; Go does) — P2 decides user-facing behaviour.
- **Forbidden names** (ASCII case-insensitive, matched by length first, then `eq_ignore_ascii_case` against ≤ 2
  candidates — O(1) per line): `host`, `transfer-encoding`, `connection`, `upgrade`, `expect`, `te`, `keep-alive`,
  `proxy-connection`, `proxy-authorization`, `content-length`. Same set for the template (with `host` supplied
  separately). Design §12 lists the user-facing set; `content-length` is added because the writer is the only
  producer of it (P2 strips or rejects a user value; Rust never accepts one).
- Template + delta ≤ `maxRequestFieldsBytes`.
- Duplicate names between template and delta are **not** checked in Rust (not a smuggling vector once framing
  headers are forbidden); P2 guarantees the delta never repeats a template name (§10.2).

### 6.5 Content-Length and methods
- Emit `content-length: <body.len()>` **iff** the body is non-empty **or** the method is `POST`, `PUT`, `PATCH` or
  `QUERY`. RFC 9110 §8.6 (verified): "A user agent SHOULD send Content-Length in a request when the method defines a
  meaning for enclosed content … normally sends Content-Length in a POST request even when the value is 0" and
  "SHOULD NOT send a Content-Length header field when the request message does not contain content and the method
  semantics do not anticipate such data". RFC 10008 (QUERY, June 2026, verified) defines the request content as
  the query → `QUERY` belongs with `POST`. Go (`shouldSendContentLength`, #40978) and undici (`expectsPayload`)
  agree; undici also includes `QUERY`.
- `GET`, `HEAD`, `DELETE`, `OPTIONS` with an empty body: no `content-length`. With a non-empty body: framed with
  `content-length` (an unframed body would become the next request on the connection); Task 6 closes the
  connection after such an exchange (§10.1).
- Never `transfer-encoding` in Task 5 (chunked request bodies are a later feature, §14). Never `Expect`.
- Method bytes from `Method::as_str()` (generated; `CONNECT`/`TRACE` are not representable).
- Order: request line, `host` (first, RFC 9110 §7.2), template defaults, delta, `content-length`, CRLF, body.

### 6.6 Interface (changes to the brief, justified)
```rust
pub struct HeaderTemplate { /* private: Box<[u8]> */ }
impl HeaderTemplate {
    /// `host`: the authority (punycoded host[:port]); `fields`: ("name: value\r\n")* wire bytes (latin1).
    pub fn new(host: &[u8], fields: &[u8]) -> Result<Self, ErrorCode>;
}
pub struct RequestParts<'a> {
    pub method: Method,
    pub target: &'a [u8],
    pub headers: &'a [u8], // ("name: value\r\n")*
    pub body: &'a [u8],
}
pub fn write_request(out: &mut Vec<u8>, parts: &RequestParts<'_>, template: &HeaderTemplate) -> Result<(), ErrorCode>;
```
1. **`HeaderTemplate::new(host: &[u8], fields: &[u8])` instead of `(&str, &[(&str, &str)])`.** Header values are
   latin1 bytes (design §10: values ≤ 0xFF, written by JS with `'latin1'`, as undici does). A `&str` would carry
   `é` as UTF-8 `C3 A9` while undici sends `E9` — a transport divergence (G5). One wire format for template and
   delta means one validator (DRY) and no pair splitting in the codec. The brief's `template_rejects_invalid_defaults`
   test keeps its meaning with byte inputs.
2. `write_request` unchanged: clears `out`, computes the exact total length, one `out.try_reserve(total)` (no-op in
   steady state), then appends. All validation runs before the first append, so on error `out` is left **cleared**
   (capacity kept) and the caller never sends a partial head.
3. Rule change: `QUERY` joins the always-`content-length` set (§6.5).
4. No large-body variant in Task 5 (§8.3).

### 6.7 No panics, no `unsafe`
`engine-core` is `#![forbid(unsafe_code)]`. `extend_from_slice` "Panics if the new capacity exceeds isize::MAX
bytes" (Vec docs 1.99.1, verified) — impossible after a successful `try_reserve(total)` (the total is bounded by the
limits above plus the body, itself bounded by the 1 MiB batch rule). `try_reserve` "returns an error … if the
capacity overflows, or the allocator reports a failure" (verified) → `Engine(ResourceExhausted)`. Slicing uses
`split_at_checked`/`strip_prefix`/`get`, as in the parser (the `#![deny(clippy::indexing_slicing, …)]` set at the top of
`grammar.rs` applies to `writer.rs` too). The length sum uses checked arithmetic.

## 7. Error mapping (decided)

| Condition | Error | Why |
|---|---|---|
| target empty, not starting with `/`, or containing a byte outside `VCHAR` | `Engine(Internal)` | JS (P2) validates first with `INVALID_CONFIG`; reaching Rust means that contract was bypassed (brief) |
| delta line not `token ": " field-content CRLF`; forbidden name in any case | `Engine(Internal)` | same |
| template host or fields invalid (at `HeaderTemplate::new`) | `Engine(Internal)` | same (control plane; the `RegisterTemplate` reply carries it) |
| target > 16,384 bytes | `TooLarge(Url)` | a limit, not a grammar violation; for Rust-built targets (redirects, P1b) this is the correct user-facing code (design §11.5) |
| template + delta > 65,536 bytes | `TooLarge(Headers)` | same reasoning (design §11.5 header limit) |
| `try_reserve` fails | `Engine(ResourceExhausted)` | design §11.3: "`try_reserve` → `ENGINE/resource-exhausted`" |

The brief's "re-validation failures are `Engine(Internal)`" is **confirmed** for grammar violations. Size limits
are argued above. P1b's redirect layer must validate a `Location`-derived target itself and map failures to
`REDIRECT_REJECTED/invalid-location` before calling the writer, so `Engine(Internal)` keeps meaning "bug".
Errors carry no bytes (design §5.2 "Errors never echo raw header bytes").

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
then fill) is 20–30% slower than the pair loop for small values. **Decision: `u64::format_into`** — same speed,
std, no dependency, no code of ours to keep correct.

**What the numbers say:**
- Pre-encoding (B vs A) removes ≈ 70–85% of the cost; the remaining cost is copying (the floor) plus validation.
- V2 (the parser's SWAR `value_len`) beats the per-byte table by 1.5–2.4× on headers and V3 by 7–18%; on the
  template-only `GET` (no delta) all three are equal within noise (the only per-request validation is the 5-byte
  target).
- Validation costs 0.5–1.5× the copy floor. That is the price of the single choke point (§1); P1a's block
  classification is the lever (§8.4).
- hyper's own serialisation (encode-only) is already 1.3–1.5× our full writer with validation; the per-request
  `HeaderMap` its API requires doubles it.

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
request" matters far more than how the bytes were assembled; S3 (head and body in separate writes) costs 1.5–2× at
1–4 KiB. Up to 4 KiB S1 ≈ S2. From 16 KiB S2 wins in 2 of 3 runs; at 64 KiB S2 wins by 1.6–3.3×. **Decision for Task
5:** `write_request` always copies (all requests that reach the writer in P0 are small; TLS must copy anyway). The
threshold for a `write_head` + `writev` path is measured by the task that adds it, on the reference host in the
nightly window and on the x64 VM, between 4 and 16 KiB. The kernel side is as documented: one `write`/`writev` = one
atomic block (`writev(2)`), and with `TCP_NODELAY` a second write is sent "as soon as possible" as its own segment
(`tcp(7)`), which is what S3 pays for.

### 8.4 Budget (committed benchmark, P0)
**Committed benchmark (Task 5): `crates/engine-core/examples/h1_write_bench.rs`** (run with `cargo run --release -p
engine-core --example h1_write_bench`; replaced by a criterion bench in P1a), mirroring `h1_parse_bench.rs`:
- Inputs: the three request sets of §8.1 (`get`, `post1k`, `varied` ring of 256), generated deterministically
  (fixed xorshift seed).
- Compares `write_request` with the **copy floor** (same appends, no validation) in the same process, interleaved;
  N ≥ 5 runs, median ns and the ratio writer/floor; min/max reported. 100 ms warm-up.
- A counting allocator around the timed loop asserts **0 allocations** after the first request.
- Dev label unless run in the owner's nightly window (design §16.1); results in `docs/perf/journal.md`.

**Budget (Linux arm64 reference container, median of ≥ 5):**
- Gated on the **ratio writer / copy floor** (robust to host load, as the parser's ratio to httparse): `post1k`
  ≤ 2.0×, `varied` ≤ 2.6× (prototype 1.54–1.97× and 2.14–2.51×).
- Advisory absolute values: `get` ≤ 60 ns, `post1k` ≤ 250 ns, `varied` ≤ 300 ns (prototype 28–35 / 111–220 /
  110–247 ns).
- **0 allocations per request** in steady state (counting allocator; hard gate).
- Comparison targets (reported in the PR and `journal.md`, not gated in P0, measured with this note's scratch
  method because hyper and `http` may not be engine-core dependencies, design §14): ≤ 0.5× the hyper-encoder
  emulation (prototype 0.18–0.28×); Go `Request.Write` for context.

Margins over the prototype cover what the product adds (error paths, the `HOST`/`VCHAR` tables, checked length
arithmetic) and the host noise seen in §8.2. **Share of a request:** ≈ 0.03–0.22 µs of ≈ 5 µs CPU (design §1-F),
next to the parser's ≈ 0.05–0.27 µs (`docs/notes/response-parser.md` §8.2) and ≈ 1–3 µs for the `write` syscall
itself (§8.3).

**P1a:** `value_len`/`token_len` become `engine-simd` block classification (`docs/notes/simd-scanning-research.md`
§6.1); the delta block is contiguous, so it can be classified in one call — target: validation overhead ≤ 50 ns on
`post1k`. **G0:** Task 7's harness reports CPU µs per request end to end; the writer is not separately gated there.

## 9. Test plan (beyond the brief)

### 9.1 Unit and integration (`tests/h1_writer.rs`)
The brief's eight tests, plus:
- `query_and_patch_send_content_length_zero`, `put_post_empty_body_send_zero`, `get_head_delete_options_empty_body_send_none`,
  `get_with_body_is_framed`, `content_length_digits` (0, 9, 10, 99, 100, 847, 1 MiB, `u32::MAX`, `u64::MAX` via a
  unit test of the formatting helper).
- `field_order_is_line_host_template_delta_length` (exact bytes for a full request).
- `template_rejects_invalid_host` (empty, SP, `/`, `@`, `?`, `#`, CR, LF, NUL, DEL, 0x80, `é` UTF-8) and accepts
  `example.com`, `127.0.0.1:3999`, `[::1]:8080`, `xn--bcher-kva.example`.
- `template_rejects_forbidden_defaults` (every forbidden name, also in mixed case: `Content-Length`, `TRANSFER-ENCODING`,
  `Te`, `Keep-Alive`).
- `rejects_forbidden_user_headers` in **three cases each** (lower, upper, mixed) — CVE-2026-1525.
- `accepts_obs_text_and_htab_in_values`, `accepts_empty_value`.
- `target_must_start_with_slash` (`http://evil/`, `*`, `?x`), `target_limit` (16,384 accepted, 16,385 →
  `TooLarge(Url)`), `fields_limit` (65,536 / 65,537 → `TooLarge(Headers)`).
- `error_leaves_out_cleared` and `reuses_output_capacity` across a failing request.

### 9.2 Injection vectors (`tests/vectors/h1-request/blazehttp-rules.json`)
For **names, values and targets**, every byte position class (start, middle, end) × {CR, LF, CR LF, NUL, HTAB in
name/target, SP in name/target, DEL, VT, FF, 0x80, 0xFF in name/target}, plus: `a: b\r\nc: d` inside one value,
`\r\n\r\nGET / HTTP/1.1` smuggled request in a value and in the target, `%0d%0a` (accepted as plain VCHAR — it is
percent-encoded data, the server decodes it after parsing), U+010D U+010A latin1-truncation bytes (`0x0D 0x0A` after
truncation — CVE-2018-12116), name with trailing SP before colon, `Content-Length` with leading SP, obs-fold
continuation line (`\r\n x`).

### 9.3 Ported from upstream (behaviour only, as data vectors with licence notices)
- Go `requestwrite_test.go` (BSD-3-Clause): case 25 (CTL in `RawQuery` → error), case 26 (`PATCH` with no body →
  `Content-Length: 0`), the `ContentLength` vs body mismatch cases (P2: lengths come from the body in our API),
  #78775 trailer cases (kept for the future chunked writer).
- undici `test/headers-crlf.js`, `test/content-length.js` (MIT): CRLF in `host`, duplicate/case-variant
  `content-length`, `content-length: 0` on bodiless `POST`/`PUT`/`PATCH`.
- curl `tests/data` cases for custom headers with embedded newlines (P1b, with the proxy work).

### 9.4 Fuzz (`fuzz_targets/h1_writer.rs`, `cargo-fuzz` on the pinned nightly)
Structured input (`arbitrary`): method id, target bytes, template host + fields bytes, delta bytes, body length.
Properties when `write_request` returns `Ok`:
1. `httparse::Request::parse` (oracle, fuzz crate only — already a fuzz dependency) returns `Complete(n)` with the
   same method, path == target, the headers == template ++ delta ++ generated `content-length`, and
   `out[n..] == body`.
2. Exactly one `host` (first), at most one `content-length` (== body length, `1*DIGIT`), zero `transfer-encoding`,
   no `expect`.
3. **Strictness beyond httparse:** every byte of the head is in `VCHAR ∪ {SP, HTAB, CR, LF} ∪ [0x80, 0xFF]`, CR only
   before LF, LF only after CR, exactly two SP in the request line — so a lenient oracle cannot hide a defect.
4. On `Err`, `out` is empty.
Seed corpus: the §9.2 vectors and the brief's requests. Dictionary: forbidden names in mixed case, `\r\n`,
`: `, `HTTP/1.1`.

## 10. Hand-offs

### 10.1 To Task 6 (connection)
- One `write_request` per request into the connection's own reused `Vec<u8>`; one `write`/`try_write` loop of that
  buffer (no `writev` for bodies ≤ the §8.3 threshold, and never under TLS).
- `TCP_NODELAY` on every connection (curl and Go do the same; `tcp(7)`); no `TCP_CORK`/`MSG_MORE`.
- After a request with a non-empty body on `GET`/`HEAD`/`DELETE`/`OPTIONS`, do not pool the connection (undici
  `expectsPayload`/`kReset` parity; Kettle 2022/2025 client-side and 0.CL desyncs).
- "Zero bytes written" (stale-connection re-send rule, design §5.1) is observable from the first write's result; the
  writer exposes no extra state.
- Shrink `out` with the other buffers after 30 s idle (design §11.1), so one large request does not pin capacity.

### 10.2 To P2 (JS layer)
- Build deltas as `("name: value\r\n")*` with `buf.write(str, off, 'latin1')`, after rejecting code points > 0xFF
  (CVE-2018-12116) and every §6.4 violation with `INVALID_CONFIG` (user-facing error).
- Never repeat a template field name in a delta: a request that overrides a default uses a template without that
  default (or a host-only template). A per-request skip mask was considered and rejected for now (YAGNI; add it if P2
  measures frequent overrides causing template churn).
- Strip or reject a user `Content-Length` (design §12 "conflicting Content-Length → INVALID_CONFIG").
- Note for P4 (undici transport parity): undici accepts DEL and 0x80–0xFF in paths and allows `connection`,
  `proxy-authorization`, `te` and `host` headers; the TS layer must apply our stricter rules before calling undici.

## 11. Proposed design changes for the owner (not applied; the design doc is unchanged)
1. **§5.1/§12:** state that `QUERY` with an empty body sends `content-length: 0` (RFC 10008, RFC 9110 §8.6), like
   `POST`/`PUT`/`PATCH`.
2. **§5.2 "Outgoing request target":** add "origin-form only (`/` first) except the absolute-form sent to an HTTP
   proxy, whose authority equals Host; DEL is a control character here too".
3. **§12 Injection / P2 behaviour (owner decision):** whether a body on `GET`/`HEAD`/`DELETE`/`OPTIONS` is allowed.
   Options: (a) allow, framed and never pooled afterwards (axios/undici parity; recommended); (b) reject with
   `INVALID_CONFIG` for `HEAD` only (no defined semantics, and the response can never carry a body) and allow the
   rest; (c) reject all four (Kettle 2025's server advice applied to the client; breaks axios parity).
4. **§11.5:** add the request-side limits as `spec.json` fields (`maxRequestTargetBytes` 16 KiB,
   `maxRequestFieldsBytes` 64 KiB) and map them to `TOO_LARGE/url` and `TOO_LARGE/headers`.
5. **§5.3:** "vectored writes only for large plaintext bodies" — add "(threshold measured, §8.3 of the writer note;
   never under TLS: rustls's `write_vectored` allocates per call)".

## 12. Engineering survey (blogs, docs, write-ups)
- Marc Brooker, "It's always TCP_NODELAY. Every damn time." (2024-05-09,
  [link](https://brooker.co.za/blog/2024/05/09/nagle)) — Nagle × delayed ACK; recommends `TCP_NODELAY`
  (unverified for our hosts; consistent with `tcp(7)`).
- Node.js `writable.cork()` docs (v26.13.0, [link](https://nodejs.org/api/stream.html#writablecork), verified):
  buffered chunks are passed "to `writable._writev()`, if present" — undici's head + body leave in one `writev`.
- Junekey Jeon on jeaiii's algorithm ([link](https://jk-jeon.github.io/posts/2022/02/jeaiii-algorithm)) and Lemire's
  integer-to-string posts — gains concentrate on ≥ 8-digit numbers; Content-Length is short (§8.2: the three
  table-based variants are within noise of each other for 0–10⁴ and 0–10¹³).
- PortSwigger research (2019, 2022, 2025) — §4.2.

## 13. Sources (accessed 2026-11-07)

**Official documentation (verified):**
- Rust 1.99.1: [`u64::format_into`](https://doc.rust-lang.org/1.99.1/std/primitive.u64.html#method.format_into),
  [`fmt::NumBuffer`](https://doc.rust-lang.org/1.99.1/std/fmt/struct.NumBuffer.html) ("1.98.0"),
  [`Vec::clear`](https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html#method.clear) ("no effect on the allocated
  capacity"), [`Vec::try_reserve`](https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html#method.try_reserve),
  [`Vec::extend_from_slice`](https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html#method.extend_from_slice)
  (panics only past `isize::MAX`), [`io::Write::write_vectored`](https://doc.rust-lang.org/1.99.1/std/io/trait.Write.html#method.write_vectored);
  `core::fmt::num` source at 1.99.1 (§2).
- tokio 1.54.1 [`TcpStream`](https://docs.rs/tokio/1.54.1/tokio/net/struct.TcpStream.html) (`try_write`: "only part of
  the buffer may be written"; `try_write_vectored`: "equivalently to a single call to `try_write()` with concatenated
  buffers"; `set_nodelay`).
- Linux man-pages 6.19: [`tcp(7)`](https://man7.org/linux/man-pages/man7/tcp.7.html) (`TCP_NODELAY`, `TCP_CORK`
  200 ms ceiling, not portable), [`send(2)`](https://man7.org/linux/man-pages/man2/send.2.html) (`MSG_MORE` = per-call
  `TCP_CORK`), [`writev(2)`](https://man7.org/linux/man-pages/man2/writev.2.html) (atomic single block; `IOV_MAX`
  1024).
- [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110) §5.5, §7.2, §8.6, §9.3.1–9.3.2; [RFC 9112](https://www.rfc-editor.org/rfc/rfc9112)
  §3, §3.2, §3.2.1–3.2.2, §5.1; [RFC 10008](https://www.rfc-editor.org/rfc/rfc10008) (QUERY).
- [Node.js stream docs v26.13.0](https://nodejs.org/api/stream.html#writablecork).

**Source code (verified):** every file and line range in §2.

**Advisories (verified):** undici advisories listed in §4.1 ([GitHub](https://github.com/nodejs/undici/security/advisories));
[GO-2023-1878](https://pkg.go.dev/vuln/GO-2023-1878); [NVD CVE-2019-9741](https://nvd.nist.gov/vuln/detail/CVE-2019-9741),
[CVE-2018-12116](https://nvd.nist.gov/vuln/detail/CVE-2018-12116), [CVE-2019-9740](https://nvd.nist.gov/vuln/detail/CVE-2019-9740),
[CVE-2019-18348](https://nvd.nist.gov/vuln/detail/CVE-2019-18348); [curl CVE-2014-8150](https://curl.se/docs/CVE-2014-8150.html);
[hyper advisories](https://github.com/hyperium/hyper/security/advisories); [Beast GHSA-8c6g-xf48-2fvx](https://github.com/boostorg/beast/security/advisories/GHSA-8c6g-xf48-2fvx).

**Papers and research:** T-Reqs (verified from the PDF); HTTP Garden (abstract verified); HDiff and Host of Troubles
(unverified beyond descriptions); PortSwigger 2022 and 2025 (verified); Champagne Gareau & Lemire 2026 (abstract
via search, unverified); jeaiii/Jeon (unverified, not used for the decision).

**Measurements (verified, this note):** §8.

## 14. Deferred (with phase)
- Large-body `write_head` + `writev` path and its threshold — with the bodies > 64 KiB / upload-credit work (P1b).
- Absolute-form target for HTTP proxies — P1b (proxy).
- Chunked request bodies (`transfer-encoding: chunked` generated by the writer) and trailers — when streaming
  uploads of unknown length are implemented; reuse the §6.4 validator for trailers (Go #78775).
- SIMD block classification of the delta — P1a (`engine-simd`).
- Per-request template skip mask — only if P2 measures template churn from overrides.
