# Shard, connection tasks, buffer pool and `CompletionSink`: research note (Task 6)

Design §1-F, §4 (request flow steps 3–8), §5.1 (reuse rules, stale-connection re-send), §5.3, §9.4, §10, §11 (budget,
environments, panics, settle exactly once, bounds), §13 (State pattern), §13.1 (code structure; one function, one job,
≤ 60 lines), §16.2 (symmetric harness), §17 (definition of done). Builds on `docs/notes/response-parser.md` (§6.6
interface, §12 hand-off) and treats the Task 5 writer interface as given: `write_request(out: &mut Vec<u8>, parts:
&RequestParts, template: &HeaderTemplate)`.

Access date for every link: **2026-11-07**. **Verified** = checked against the primary source (the pinned release's
source file, an official doc page for the locked version, an RFC) or a measurement run for this note. **Unverified** =
stated by a source but not reproduced here; unverified ideas go only to `docs/perf/ideas.md`. All measurements in §6 are
**dev measurements** on the owner's M2 under normal desktop load (macOS, not the Linux reference container, not the
nightly window: other agents' containers were running, so no Docker run was made). They rank designs and verify
allocation and wake counts; absolute CPU numbers are re-measured on Linux by Task 6's `shard_bench` step (§11.2) and
later in the G0 harness (Tasks 7/8).

Review: fix round 1 (2026-11-07) applied the code-review findings F1–F20 and the security findings S1–S13 with the
controller's rulings (forced peek deferred to Linux data; CSD hardening and a 1,000-request cap adopted; P0 has a body
size limit; the owner's definition of done applies in full).

## 1. Summary and decision

**Request flow — design A, central FIFO + LIFO idle wake, exchange inside the connection task.** One command-loop task
per shard drains an unbounded `mpsc` with `recv_many` into a `VecDeque` FIFO in `Rc<Shard>` state. It wakes parked
connection tasks LIFO (`Notify::notify_last`) and spawns new ones only for requests no woken or spawned task has
already **claimed** (no over-wake, no over-spawn), never beyond `max_connections`. A connection task owns its socket,
read buffer, write buffer and parser; it pops the next request itself, writes it, reads and parses the response in the
same task, and pushes the completion. Requests are FIFO entries, never tasks. **Wakes:** 0 task wakes per request when
the FIFO is non-empty at completion (the finishing connection pops the next request in the same poll); a closed loop
with as many connections as requests in flight (the G0 shape) costs 1 idle notify per request plus 1 command-loop wake
per submit batch (measured: batch ≈ 15–16 at c = 50, 1 at c = 1, §6) — within §5.1's "≤ 1 per request + 1 per batch".

**Idle monitoring — I1 + I4 in P0; forced peek deferred to Linux data.** A parked connection polls its socket's read
readiness in the same poll as its `Notified` future (I4: no extra task or future), and at checkout calls
`TcpStream::try_read` (I1), which performs a syscall **only** when the reactor has flagged the socket (≈ 5 ns and zero
syscalls when clean, measured). Both only see what the reactor has seen. **Residual windows (§7.2):** (W1) a parked
connection on a *busy* shard, where the reactor may not have turned for up to 61 task polls; (W2) back-to-back reuse
by a busy connection; (W3) bytes still in flight after any check (inherent to HTTP/1.1). A response injected into W1/W2
is delivered to the next request, and because the connection keeps being reused back to back, **every later response
on it shifts by one** — across clients that share the profile's pool — until something closes it. P0 therefore adds
two containment rules (controller ruling): **no reuse after a non-2xx response to a request that carried a body** (the
client-side-desync shape) and **at most 1,000 requests per connection** (bounds any shift chain, like nginx
`keepalive_requests`). Whether to also pay a forced `recv(MSG_PEEK)` per reuse (full, or only when checking out a
*parked* connection) is decided on Linux data against a ≤ 2% CPU criterion (§11.2, §12 item 1). Proposed design §18
row: likelihood Low–Medium, impact High (cross-client response shift).

**Buffers — B2.** Each connection owns a pooled read buffer (64 KiB). A response whose bytes end at `end ≥ ¼` of the
buffer is **handed off** (`ResponseBytes::HandedOff(Vec<u8>)`, **truncated to `len == end`**, so no stale bytes from an
earlier exchange travel with it) and the connection takes a fresh pooled buffer; a smaller response is passed as the
**borrowed slice** `&rbuf[..end]` that the sink copies into its own capacity-reusing arena. This replaces the brief's
"copy into an exact-size `Vec`" (one allocation per request; on Node a free on another thread). Reclaimed buffers come
back through `CompletionSink::push`'s `reclaimed` and are zero-extended back to full length before re-entering a LIFO
free list capped at `2 × max_connections`.

**Measured (dev, §6):** 0.000 heap allocations per request in steady state (hyper `client::conn`: 10.0); CPU per
request at c = 50 within noise of the raw tokio loop (median 12.1 vs 13.7 µs, ±20–30% host noise) and below hyper
(17.7 µs). At c = 1 the command-loop hop shows (median 25.1 vs raw 16.6 and hyper 20.6 µs) — a risk for the c = 1 G0
cell, with three mitigations costed in §11.3.

**Interface changes vs the brief** (§9): `ResponseBytes` borrowed/handed-off body and a lifetime on `Completion`; an
owned `ExchangeOutcome`; pending bound counted by an `Arc<AtomicUsize>` over an unbounded channel; a validated
`ShardConfig::new` (private fields, `HeaderTemplate` instead of `host`, a **body** size limit `max_response_bytes`);
`spawn_shard_thread` builds a `LocalRuntime` and also returns the thread's `JoinHandle`; one brief test is split
because it contradicts idle detection (§10.1).

## 2. Scope and hand-offs

Decided here: the shard's data structures and task model, connection reuse and close rules (P0 subset of §5.1 plus the
CSD and request-cap rules), how idle connections are watched (the open item of design §5.1 and parser note §12), EOF
accounting, error mapping for the connection layer, buffer ownership and pooling, the `CompletionSink` contract,
settle-exactly-once and poisoning inside a shard, how `shard()` and `spawn_shard_thread()` run, and the test,
fuzz, mutation and performance plan. Not decided here (later phases, hooks in §11.4): timeouts (P1a timer heap),
stale-connection re-send (P1a), `Cancel`/`Credit`/`UploadChunk`/`Poison` commands, byte budget, per-origin scheduler and
fairness, TLS, DNS, chunked decoding (P1a `FramingDecoder`), proxy pool keys (P1b).

## 3. Reference implementations (latest releases, read 2026-11-07)

Pinned sources: hyper 1.12.0, hyper-util 0.1.21, reqwest 0.13.5, tokio 1.54.1, mio 1.2.4 (crates.io tarballs); Go
go1.27.3 `src/net/http/transport.go`, `request.go`, `transport_test.go`; undici v8.12.4
`lib/dispatcher/{client-h1,client,pool,pool-base}.js`, `test/{client-keep-alive,response-queue-poisoning}.js`;
fasthttp v1.77.0 `client.go`; curl `curl-8_23_0` `lib/{conncache,cf-socket,multi,url,transfer}.c`, `tests/data/test1479`;
h2o master `f35c735e` (2026-11-06; the project publishes no releases) `lib/common/{http1client,socketpool,httpclient}.c`;
Boost.Beast `boost-1.92.0` `example/http/client/async/http_client_async.cpp`, `include/boost/beast/http/impl/read.hpp`;
Bun `bun-v1.4.4` `src/http/{HTTPContext,HTTPThread}.rs`, `src/uws_sys/socket.rs`, `packages/bun-usockets/src/{bsd,socket}.c`.
All verified by reading the named lines.

### 3.1 What each does

| Engine | Task / thread model | How a request reaches a connection (hops, wakes) | Buffers and allocations per request | Idle connections: stray bytes / EOF | Reuse rules worth noting |
|---|---|---|---|---|---|
| **hyper 1.12 + hyper-util 0.1.21** | one task per connection (`proto/h1/dispatch.rs` `poll_loop` 166–223, ≤ 16 iterations then `yield_now`) | caller → `SendRequest` → `mpsc::unbounded_channel` + `want` (`client/dispatch.rs` 31–35) → conn task; each send allocates a `oneshot` (106–128) → caller woken by the oneshot. **2 task wakes and a channel hop per request** | `BytesMut` read buffer, `INIT_BUFFER_SIZE` 8 KiB, adaptive `ReadStrategy` up to ≈ 400 KiB (`io.rs` 15–23, 384–440); write strategy flatten/queue (`io.rs` 278–335); `HeaderMap` + `Response` + oneshot: **10.0 allocations per request measured** (§6) | the conn task keeps reading while idle: `poll_read_keep_alive` → `require_empty_read` (`conn.rs` 437–491): EOF closes quietly, any byte → "received unexpected N bytes on an idle connection" error; `maybe_notify` (523–568) reads after a message only when not read-blocked | hyper-util pool: `Mutex<PoolInner>`, idle `Vec` popped from the end (LIFO) skipping `!is_open()` (`pool.rs` 299–337); re-send only when the request was never started (`client.rs` 241–272, `retry_canceled_requests`) |
| **reqwest 0.13.5** | hyper-util legacy client (`async_impl/client.rs` 942) | as hyper-util | as hyper + reqwest layers | as hyper | pool idle timeout 90 s, unlimited idle per host (`client.rs` 302–303); `TCP_NODELAY` configurable (`connect.rs` 209) |
| **Go net/http (go1.27.3)** | two goroutines per connection: `readLoop` (2454–) and `writeLoop` (2830–); caller goroutine waits on channels | caller → `getConn` (LIFO idle: `list[len(list)-1]`, 1266–1271) → `pc.roundTrip` (2994–) sends on `writech` and `reqch` → 2 goroutine hand-offs + response channel | `bufio.Reader`/`Writer` per connection; allocations per request (Request/Response/Header maps) | **`readLoop` blocks in `br.Peek(1)` even while idle** (2493): data with no outstanding request → `readLoopPeekFailLocked` (2653–2670) closes, logs "Unsolicited response received on idle HTTP channel", silently for a `408` (issue #32310) | re-send only on a **reused** conn, `nothingWrittenError` (body rewindable) or `isReplayable` (GET/HEAD/OPTIONS/TRACE or `Idempotency-Key`; `request.go` 1561–1575) and EOF/read error before the first byte (`shouldRetryRequest` 844–890) |
| **undici v8.12.4** | one socket per `Client`, `Pool` of Clients; event-loop callbacks | `Pool` queue → `Client[kQueue]` → `writeH1` (1203–); no extra task | llhttp WASM parser per socket; JS objects per request | parser keeps reading while idle (`resumeH1` 1113–1161): `onMessageBegin`/`onHeadersComplete` with `kRunning === 0` → `SocketError('bad response')` (486–505, 628–650); **since 8.5.0 (CVE-2026-6733) every reuse of a used socket waits one event-loop poll first** (`scheduleIdleSocketValidation` 1089–1108): issues #5493 (+1.3 ms per reuse with `setTimeout(0)`), #5600/#5606 (500 ms stall with an unref'd `setImmediate`, reverted) | keep-alive timeout from `Keep-Alive: timeout` minus a threshold (707–723); a connect error with nothing running fails the Client's whole queue (`client.js` `onError` 472–493, 600–639) |
| **fasthttp v1.77.0** | the **caller goroutine does the exchange** (`transport.RoundTrip` 3559–3690): no hop | `AcquireConn` from `conns` slice; default `ConnPoolStrategy` is **FIFO via `copy(c.conns, c.conns[1:])` — O(n) per checkout** (794–799, 1895–1910) | `bufio.Reader`/`Writer` acquired from a `sync.Pool` **per request** and released after (3598–3608, 3643–3688): idle connections hold no buffers; Request/Response objects reused by the caller (zero-alloc style) | **no idle detection**; buffered leftover bytes are dropped by `ReleaseReader` and the connection is still pooled (3682–3688: no `Buffered()` check) | up to 5 attempts for idempotent methods incl. PUT and QUERY (`isIdempotent` 1730–1732, `DefaultMaxIdemponentCallAttempts` 730) |
| **curl 8.23.0** | multi handle, one thread, `multi_runsingle` state machine per transfer | transfer picks a connection from the pool (`conncache.c`) | download buffer per transfer | at reuse `Curl_cpool_conn_seems_healthy` (`conncache.c` 1040–1078) runs `cf_socket_conn_is_alive` — **`poll()` with 0 timeout** (`cf-socket.c` 1811–1846); input pending on an idle non-multiplexed conn → unhealthy. **Rate-limited: skipped if checked < 1000 ms ago** (1050–1051) | max age / idle limits (`cpool_conn_too_old`); a long CVE history of **reusing a connection whose configuration did not match** (§4) |
| **libh2o (`f35c735e`)** | event loop; `h2o_socketpool` shared across threads (mutex) | `h2o_socketpool_connect` pops an idle socket | `sock->input` buffer; trailing bytes after the response trimmed and keep-alive disabled (`http1client.c` 220–227); input trashed on return (97–114) | **`recv(fd, buf, 1, MSG_PEEK)` at every checkout** (`socketpool.c` 500–545): EAGAIN → reuse, else close and log ("see issue #679") | pooled sockets are exported/imported across loops |
| **Boost.Beast 1.92** | per-session async chain (user code) | user calls `async_write`/`async_read` | `flat_buffer` owned by the session and kept between reads ("Must persist between reads", example line 45); reads `read_size(b, 65536)` (`read.hpp` 209, 355) | none; leftover bytes stay in the buffer and are parsed as the next message | the user decides |
| **Bun v1.4.4** | one HTTP thread with uSockets (Rust since v1.4) | `HTTPThread` drains queued requests; `existing_socket` checks the keep-alive pool | per-client buffers | **both**: pooled sockets stay registered and `on_data` terminates on unexpected data (`HTTPContext.rs` 1513–1550, except an exact `0\r\n\r\n`), and at checkout `queued_input()` → `bsd_queued_input` = **`recv(MSG_PEEK \| MSG_DONTWAIT)`** (`HTTPContext.rs` 921–939; `bsd.c` 911–930) because "`HTTPThread::drain_events` hands a socket out before the loop polls it" | pool capped, longest-idle evicted |

### 3.2 What makes them fast, and what we take

- **No hop between the request and its connection.** fasthttp runs the exchange in the caller; hyper pays a channel hop
  plus a oneshot per request. We run the exchange inside the connection task and let the request be a FIFO entry
  (design §5.3). **Taken.**
- **LIFO idle reuse** (Go 1266–1271, hyper-util `pool.rs` 300): the hottest connection is the least likely to have been
  closed by the server's idle timer, and cold ones age out. fasthttp's default FIFO costs O(n) per checkout. **Taken**
  (`Notify::notify_last`).
- **Read interest on idle connections** (hyper, Go, Bun, undici): a server close or stray byte is seen as soon as the
  reactor sees it. **Taken** (I4), without hyper's or Go's extra task or goroutine.
- **One buffer per connection, reused** (Beast, h2o, hyper): **taken**, plus hand-off at completion (design §4 step 6).
- **Buffers only while busy** (fasthttp's per-request `sync.Pool` reader): saves memory with many idle connections, but
  costs a pool round trip per request. **Not taken in P0** (idea in `ideas.md`, for the 128 MiB cell).
- **A peek syscall per reuse** (h2o, Bun) and **a loop turn per reuse** (undici): narrows the race to bytes in flight at
  ≈ 1 syscall per request. **Deferred to Linux data** (§11.2, §12 item 1).

### 3.3 What we do not copy

- fasthttp: pooling a connection whose reader still held unparsed bytes (desync), O(n) FIFO checkout, 5 retries of
  idempotent requests (our §5.1 rule: at most one transparent re-send, P1a).
- Beast: leaving leftover bytes for the next read (pipelining semantics) — we never pipeline; leftover bytes close.
- curl: skipping the liveness check for 1 s after the last one — we check on every checkout (it is free with I1).
- hyper: a oneshot + channel hop and `HeaderMap`/`Response` allocations per request; a second task wake per request.
- Go: two goroutines per connection.
- undici's latency regressions (#5493, #5600): any per-reuse validation must not add a timer floor or depend on an
  unref'd handle.

## 4. Security and robustness history

| Finding | Source (verified) | Rule here |
|---|---|---|
| **Response queue poisoning on reused keep-alive sockets** — an attacker-controlled upstream injects an unsolicited response on an idle socket; the next request gets it. undici < 6.27.0 / 7.28.0 / 8.5.0, CVE-2026-6733 (low) | GHSA-35p6-xmwp-9g52 (`gh api /advisories`), undici `client-h1.js` 1089–1108, regression test `test/response-queue-poisoning.js` ("should not reuse an idle socket with buffered unsolicited response bytes") | I4 + I1; leftover → close; never pipeline; CSD rule + 1,000-request cap contain a shift; residual windows W1–W3 stated in §7.2; peek decision on Linux data |
| Response queue poisoning / stacked responses as a desync outcome (Kettle 2021 "HTTP/2: the sequel is always worse"; 2022 client-side desync; 2025 "HTTP/1.1 must die") | PortSwigger research pages; parser note §4.2 | the parser's framing rules + close on leftover bytes, on 204/304 with a framing header, on any error; **no reuse after a non-2xx response to a request that carried a body** (Kettle's client-side desync makes the server ignore the body and treat it as the next request) |
| Differential-fuzzing desync classes (T-Reqs, CCS 2021; HTTP Garden, 2024) | parser note §4.2, §15 (verified there) | the connection fuzz target (§10.4) checks reuse decisions under arbitrary byte streams |
| curl: reusing a connection whose configuration does not match the new transfer — CVE-2021-22924 (flawed path name checks), CVE-2022-22576 (OAUTH2 bearer bypass), CVE-2022-27775 (local IPv6), CVE-2022-27782 (TLS and SSH too eager reuse), CVE-2023-27535 (FTP), CVE-2023-27536 (GSS delegation), CVE-2023-27538 (SSH, again) | https://curl.se/docs/CVE-*.html (titles read) | P0 has one origin and one template per shard; **P1b pool keys** must include every security-relevant field — the profile key (design §11.2) plus origin, and for absolute-form proxy connections **the target origin** (controller ruling (e)) |
| RFC 9112 §9.2: data on a connection with no outstanding request "MUST NOT" be treated as a response; the client "SHOULD close the connection … unless the data consists only of one or more CRLF" | RFC 9112 §9.2 | close on **any** idle byte, CRLF included (stricter; §12 item 10) |
| RFC 9112 §9.5: "Implementations SHOULD constantly monitor open connections for a received closure signal"; a client sending a body "SHOULD monitor … for an error response" and "immediately cease transmitting" | RFC 9112 §9.5 | I4; the exchange polls the read state while writing (early final response, §8.6) |
| RFC 9112 §9.6: an immediate server close can turn the client's unread input into a reset | RFC 9112 §9.6 | read before mapping a write error (§8.6); reset after ≥ 1 response byte → `NETWORK/reset` |
| Server closes an idle connection just as the client writes ("unfortunate keep-alive timeout") — Go #22158, hyper #2136 (`IncompleteMessage`) | issue texts | `NETWORK/closed-before-response` (zero response bytes); the transparent re-send arrives in P1a (§5.1) |
| A `408` sent on an idle connection before the server closes (Go #32310) | Go 2653–2670, issue text | a stray byte while idle: close quietly, no request is affected |
| Leftover bytes silently dropped and the connection pooled | fasthttp 3682–3688 | never: leftover → close |
| Stale bytes past the response in a reused buffer handed to another client | review S2 | handed-off buffers are truncated to `end`; borrowed slices are exactly `[..end]`; pooled buffers are zero-extended (§8.10) |
| Server-forced reconnect churn (e.g. 204 + `Content-Length: 1`, an extra byte after each response) leaves the client with `TIME_WAIT` sockets | review S12 | documented; P1a adds a connect-rate cap/backoff per origin; `set_zero_linger()` on desync closes is a P1a option (not in P0) |
| `SO_LINGER` setter deprecated in tokio (`TcpStream::set_linger`, `#[deprecated]` at `net/tcp/stream.rs` 1334) | tokio 1.54.1 source | tests that need an RST use `set_zero_linger()` (1368), never `set_linger` (§14.1) |

Discarded idle or leftover bytes are **never** logged, traced or put into a diagnostic (they may be another client's
data); only a counter is kept (P1a diagnostics).

## 5. Runtime mechanics at the locked versions (verified)

`engine-core` has no tokio dependency yet; Task 6 adds `tokio = "=1.54.1"` (latest stable, 2026-10-21; same as the spike
lockfiles) with features `rt`, `net`, `io-util`, `sync` (and `time` arrives with P1a timers). Rust 1.99.1 (pinned).

1. **Read readiness is cached and cleared on short reads — on epoll/kqueue only.** `PollEvented::poll_read` clears
   readiness when `0 < n < len` on Linux/Android/illumos (epoll) and the BSDs/macOS (kqueue) (tokio
   `io/poll_evented.rs` 182–213, tokio issue #5866), and on `WouldBlock` everywhere. So after a response that arrives in
   one short read, the next read future returns `Pending` **without a syscall** — provided reads go through
   `AsyncRead::poll_read` (`TcpStream::split` halves, `net/tcp/split.rs` 371–377). **Windows** (level-triggered, not in
   that `cfg`): readiness stays set after a short read, so I1 costs one syscall per checkout (`WouldBlock`, which then
   clears it) and I4's first `poll_read_ready` while parking is ready at once (one `try_read` → `WouldBlock`, then park).
   Correctness is unchanged; tests assert behaviour, never syscall counts, and the idle-detection tests run on every CI
   OS (§10).
2. **`try_read` / `try_read_buf` do a syscall only when readiness is set** (`Registration::try_io`,
   `runtime/io/registration.rs` 194–213: "Don't attempt the operation if the resource is not ready") and do **not** apply
   the short-read clear (`net/tcp/stream.rs` 638–644, 782–800). Hence steady-state reads use `poll_read`; `try_read` is
   used only for idle checks, always with a non-empty slice.
3. **Closed states survive clearing.** `ScheduledIo::clear_readiness` masks out `READ_CLOSED`/`WRITE_CLOSED`
   (`runtime/io/scheduled_io.rs` 372–377), and `Interest::READABLE` includes `READ_CLOSED` (`io/interest.rs` 258–267).
   mio registers `EPOLLIN | EPOLLRDHUP` edge-triggered on Linux (`sys/unix/selector/epoll.rs` 129–133) and `EV_CLEAR`
   with `EV_EOF` on kqueue (`kqueue.rs` 127, 431–437). So a FIN seen by the reactor is never lost: `try_read` returns
   `Ok(0)`.
4. **One task may watch read readiness without a future.** `TcpStream::poll_read_ready(cx)` stores the waker; "only the
   Waker from the Context passed to the most recent call is scheduled" (`net/tcp/stream.rs` 542–573). A parked
   connection polls it in the same `poll_fn` as its `Notified`, so one task wake covers both events.
5. **`assume_ready`.** Since 1.54.0, *accepted* sockets start readable+writable (`net/tcp/stream.rs` 171–181, changelog
   #8435); `connect` waits for writability (147–163). No effect on the client path.
6. **`LocalRuntime` is stable** (changelog 1.51.0: "runtime: stabilize `LocalRuntime` (#7557)"; `runtime/mod.rs` 626–627
   exports it outside `cfg_unstable!`). It "supports `spawn_local` without the need for a `LocalSet` context" and "is
   incompatible with `LocalSet`" (`local_runtime/runtime.rs` 14–30). `task::spawn_local` works inside both a `LocalSet`
   and a `LocalRuntime` (`task/local.rs` 394–440).
7. **`Notify`**: `notify_one` wakes the oldest waiter (FIFO), `notify_last` the most recently added (LIFO)
   (`sync/notify.rs` 657–671); at most one permit is stored when nobody waits; the waiter list is an intrusive linked
   list under a mutex (211) — registering a waiter does not allocate; `notify_waiters` wakes every current waiter
   (740).
8. **`mpsc`**: bounded and unbounded share the block list (`BLOCK_CAP` = 32 on 64-bit, `sync/mpsc/mod.rs` 140) and
   reuse freed blocks (`list.rs` 200–245), so a steady state with bounded occupancy does not allocate (measured 0.000 per
   request, §6). `recv_many(buffer, limit)` "will never return 0 unless the channel has been closed and there are no
   remaining messages", `limit == 0` returns 0 immediately, and it is cancel-safe (`bounded.rs` 248–319).
   `UnboundedSender::send` is synchronous and fails only when closed (`unbounded.rs` 559); `Receiver::close` makes every
   later send fail while queued messages stay receivable with `try_recv`.
9. **Cooperative budget**: 128 units per task poll (`task/coop/mod.rs` 115–116); I/O and channel operations consume it
   (`registration.rs` 162, 233). A connection task that completes many back-to-back exchanges without `Pending` is
   forced to yield after the budget — at most one self-wake per ≈ 60 exchanges. We do **not** wrap tasks in
   `unconstrained`.
10. **The reactor turns only every `event_interval` = 61 task polls on a busy current_thread scheduler, or when it
    idles** (`runtime/builder.rs` 281–283), and `yield_now` does not guarantee a turn: "the runtime will not drive the IO
    driver between every poll of a task" (`task/yield_now.rs` 6–35). This is the bound of windows W1/W2 (§7.2), and why
    undici's "yield to the poll phase" has no exact tokio equivalent.
11. **Panics in spawned tasks are caught** by the task harness (`runtime/task/harness.rs` 504–530, `catch_unwind`); the
    panicking future is dropped inside the unwind guard, so `std::thread::panicking()` is true in its `Drop` impls. A
    `LocalSet` ignores task panics by default (`task/local.rs` 518). A panic inside the future passed to `block_on`
    propagates.
12. **Deprecations to avoid**: `TcpStream::set_linger` / `TcpSocket::set_linger` (use `set_zero_linger`).
13. **Rust std** (https://doc.rust-lang.org/1.99.1/std/): `Rc` is `!Send`; `RefCell::borrow_mut` "Panics if the value is
    currently borrowed", `try_borrow_mut` returns an error instead (`cell/struct.RefCell.html`); `VecDeque::push_back` /
    `pop_front` are O(1) amortised (`collections/struct.VecDeque.html`); `Vec::try_reserve`, `shrink_to`, `truncate`,
    `resize` (`vec/struct.Vec.html`); `std::mem::take`; `std::thread::panicking` (`thread/fn.panicking.html`);
    `io::ErrorKind::{ConnectionRefused, ConnectionReset, BrokenPipe, HostUnreachable, NetworkUnreachable}` (the last two
    stable since 1.83). Clippy `await_holding_refcell_ref`
    (https://rust-lang.github.io/rust-clippy/rust-1.99.0/index.html#await_holding_refcell_ref) is denied workspace-wide.
14. **`socket2`** (only if the forced peek is adopted): tokio's `net` feature already depends on `socket2 0.6` (tokio
    `Cargo.toml` 96–107, 981–984), so a direct dependency adds no crate; `SockRef::from(&stream).peek(..)` is safe
    (https://docs.rs/socket2/0.6.5/socket2/struct.SockRef.html). P0 production code does **not** add it; `shard_bench`
    uses it as a dev-dependency to measure the option.

## 6. Measurements (dev, verified for allocation and wake counts)

**Prototype** (scratch crate, not committed; ≈ 300 lines). **Driver:** one client thread runs a `LocalRuntime`; a single
driver task on it submits through `mpsc::Sender::try_send` and consumes completions from a sink on the same runtime
(`Rc<RefCell<Vec>>` + `Notify`, the G0 symmetric shape, §16.2); it resubmits one request per completion, keeping `c` in
flight on `c` connections (closed loop). Server: tokio multi-thread (4 workers) in the same process on other threads,
fixed response with 3 header fields and a 1 KiB body. Responses parsed with `engine_core::h1::ResponseParser`; FNV-1a
over the body in every mode. CPU = `CLOCK_THREAD_CPUTIME_ID` of the client thread (user + system; macOS has no per-thread
split); allocations = a thread-local counting allocator on the client thread only. Modes: `raw` (one task per connection
looping write/read — the §1-F 40-line loop), `shard` (design A with B2 buffers, idle check `none` / `tryread` (I1) /
`peek` (forced, via `socket2`)), `hyper` (`client::conn` + one caller task per connection, the spike/g0 shape). rustc
1.99.1, fat LTO, `codegen-units = 1`, tokio 1.54.1, hyper 1.12.0. macOS loopback; host load not controlled.

**c = 50, 200k measured requests after 20k warm-up, 5 alternating rounds (median, range):**

| Mode | CPU µs / request | Allocations / request | Task wakes / request (counted) |
|---|---|---|---|
| raw | 13.72 (11.83–14.95) | 0.000 | — (I/O only) |
| shard, no check | 12.12 (10.60–14.46) | **0.000** | idle notifies 1.000, connection parks 1.000, sink wakes 0.051–0.060 |
| shard, I1 `try_read` | 13.28 (10.46–14.92) | **0.000** | same |
| shard, forced peek | 13.36 (12.72–15.01) | **0.000** | same |
| hyper `client::conn` | 17.69 (14.11–19.86) | **10.0** | not instrumented (2 by construction) |

**c = 1, 30k requests after 3k warm-up, 3 rounds (median, range):** raw 16.57 (16.04–18.42) · shard 25.07
(21.30–30.01) · shard + I1 21.39 (18.80–26.35) · shard + forced peek 23.38 (20.14–25.66) · hyper 20.61 (13.65–21.43) µs.

**Command-loop wakes and batch size (fix round 1, 3 rounds each, I1):** c = 1: **1.000 command-loop wakes per request,
mean batch 1.00**, idle notifies 1.000, sink wakes 1.000; c = 50: **0.061–0.065 command-loop wakes per request, mean batch
15.5–16.4**, idle notifies 1.000, sink wakes 0.061–0.065. So at c = 1 each request costs three task wakes on the client
thread (command loop, connection, sink consumer) against hyper's two (connection task, caller) and the raw loop's zero.
Syscalls per request could not be counted here (no `strace` on macOS, `dtruss` needs root): Task 6's Linux
`shard_bench` step records them (§11.2).

**Idle-check micro-benchmark** (one clean idle keep-alive connection, 1M calls): `try_read` **5.1 ns** per check (no
syscall: readiness was cleared by the short read); `socket2` `peek` **2,130 ns** per check (one `recv(MSG_PEEK)` on
macOS; Linux not measured).

What this shows (and does not):
- **Verified:** zero heap allocations per request in steady state for design A with B2 buffers (deterministic); hyper's
  10 allocations per request; the wake counts claimed in §8.8; I1 is free when the connection is clean.
- **Dev only:** macOS loopback CPU is kernel-dominated (≈ 10–15 µs per request vs 4–5.5 µs on Linux in §1-F) and the
  host was loaded (±20–30% between rounds), so the ≈ 1.3× hyper/raw ratio agrees with §1-F but the shard-vs-raw
  difference is inside the noise. At c = 1, design A costs more than raw and hyper on these runs (the extra command-loop
  wake and poll per request). Re-measure on Linux (§11.2).

## 7. Options compared

### 7.1 How requests flow from the command queue to connections

| | **A. Central FIFO + idle `Notify` (chosen)** | B. Per-connection queues (hyper shape) | C. Direct hand-off to an idle-connection stack | D. Work stealing |
|---|---|---|---|---|
| Shape | command loop pushes into one `VecDeque`; parked connections are woken LIFO; a busy connection pops the next request itself | the command loop assigns each request to one connection's queue (round-robin or least-loaded) | the command loop pops an idle connection id (LIFO stack in a slab) and moves the request into its mailbox, then wakes its stored `Waker` | per-connection deques, idle connections steal |
| Time | O(1) enqueue, O(1) pop; wake O(1) (`notify_last`) | O(1) per assignment, but a request may wait behind a slow exchange while another connection is idle (head-of-line) | O(1) both; removal of an idle connection that dies needs an intrusive list (O(1)) | O(1) amortised |
| Space | one `VecDeque<Submit>` ≤ pending bound | N queues | slab of connection slots + idle stack | N deques |
| Allocations / request | 0 (measured) | 0 with reused queues | 0 | 0 |
| Task wakes / request | FIFO non-empty at completion: 0; closed loop / low load: 1 + 1 per batch | ≥ 1 per request (the target connection) even under load | low load 1; under load 0 if busy connections pop the overflow FIFO | as A |
| Failure modes | a woken connection may find the queue already emptied by a busy one (bounded by the `claimed` count, harmless re-park); idle and claimed counts must be exact | assignment to a connection that just died → re-queue logic; HOL blocking | more code: mailbox, per-slot waker, removal on death | needs threads to pay off; one shard is one thread |
| Verdict | **chosen**: least code, proven `Notify` semantics, LIFO, satisfies the wake budget | rejected: HOL and a wake per request under load | kept as the P1a upgrade path if the idle `Notify` mutex shows in profiles (it would also host per-origin scheduling) | rejected: no parallelism inside a shard |

### 7.2 Watching idle connections for stray bytes and EOF

| | I1. Cached readiness at checkout (`try_read`) | I2. Forced peek at checkout (`recv(MSG_PEEK)`) | I3. Reactor turn before reuse | **I4. Read interest while parked** |
|---|---|---|---|---|
| Used by | our checkout (hyper does the same implicitly via `poll_read`) | h2o (every checkout), Bun (every reuse), curl (`poll(0)`, ≤ 1/s) | undici ≥ 8.5.0 (one loop turn per reuse) | hyper, Go, Bun, undici (read while idle) |
| Cost per reuse | 0 syscalls when clean (5.1 ns measured); 1 syscall when the reactor flagged the socket; Windows: 1 syscall | 1 syscall (2.1 µs dev Mac; Linux to measure). Variant **I2-parked**: only when the request goes to a connection that was parked — 0 on back-to-back reuse | an extra scheduler round + driver poll; tokio cannot force it (§5 item 10) | per park: one `poll_read_ready` (waker store); 0 syscalls (Windows: one `WouldBlock` read per park) |
| Cost per idle connection | none | none | none | none (no task, no future, no timer) |
| What it catches | everything the reactor has seen before the check | everything in the kernel before the write | everything in the kernel before the poll | everything the reactor sees while parked; closes the socket at once (fds, RFC 9112 §9.5) |
| Verdict | **P0** (with I4) | **deferred**: measured on Linux as full, parked-only and none; adopted if ≤ 2% CPU per request (§11.2) | rejected: not expressible in tokio, and undici's own regressions | **P0** |

**Windows the P0 checks miss** (security review S1):
- **W1 — parked connection on a busy shard.** A stray response reaches the kernel while the connection is parked, but
  other tasks keep the scheduler busy, so the reactor has not turned (up to 61 task polls, §5 item 10). I4 has not fired,
  I1 sees cleared readiness, the request is written and receives the injected response. I2-parked closes this window.
- **W2 — back-to-back reuse.** A busy connection completes a response and pops the next request in the same poll; a
  stray response that arrived after that read is in the kernel but invisible. Only full I2 closes it (one syscall per
  request).
- **W3 — bytes in flight after any check.** Inherent to HTTP/1.1 (RFC 9112 §9.2 relies on order); no client check helps.

**Impact is not limited to one exchange.** After one misdelivery (request B receives injected X), B's real response RB
arrives next and is delivered to request C, C's to D, and so on: **every later response on that connection shifts by
one while it is reused back to back**, and the requests come from any client sharing the profile's pool (§11.2 of the
design) — a **cross-client data leak**. The shift ends when the connection closes: a leftover byte when the queue drains
and the connection parks (I4 then sees the pending response), any error, `Connection: close`, the CSD rule below, or the
request cap. **P0 containment rules (controller ruling):**
- **CSD rule:** never reuse a connection after a non-2xx final response to a request that carried a body (`body` range
  non-empty). That is the shape in which a server ignores a body (client-side desync, Kettle 2022), so the body's bytes
  would be read as the next request.
- **Request cap:** close a connection after its **1,000th** exchange (constant `MAX_REQUESTS_PER_CONNECTION`, nginx
  `keepalive_requests` default). It bounds a shift chain at ≤ 1,000 responses and costs one reconnect per 1,000 requests
  (< 0.1% of connects on a keep-alive workload).
- Never pipeline; close on leftover bytes; close on any error.

**Proposed design §18 row** (replacing the current client-side-desync row): likelihood **Low–Medium** (needs a hostile or
compromised upstream, or an intermediary that frames differently), impact **High** (responses delivered to other
clients' requests until the connection closes); controls: I1 + I4, CSD rule, request cap, leftover/error close, and the
Linux-data decision on I2.

### 7.3 Buffer ownership and pooling

| | B1. Brief: hand off ≥ ¼, else copy into an exact-size `Vec` | **B2. Hand off ≥ ¼ (truncated to `end`), else borrowed `&rbuf[..end]` copied by the sink into its arena (chosen)** | B3. Always hand off | B4. Second pool of small buffers |
|---|---|---|---|---|
| Allocations / request (1 KiB response) | **1** (+ a free on the consumer's thread in Node, against §4 step 4) | 0 (measured) | 0 | 0 |
| Copies | 1 | 1 (< 16 KiB) | 0 | 1 |
| Memory per pending small completion | exact | exact (in the sink arena) | 64 KiB (64× amplification of a 1 KiB body, all charged to the budget) | the small class size |
| Return traffic | none | a zero-extend (`resize`, ≤ 48 KiB memset) per reclaimed buffer, only for ≥ 16 KiB responses | one buffer per request through `reclaimed` | one buffer per request |
| Verdict | rejected (G0 zero-allocation) | **chosen** | rejected (memory, §11.1 budget) | rejected (extra pool and traffic for no gain over B2) |

The ¼ threshold (16 KiB of 64 KiB) caps the memory amplification of a handed-off buffer at 4×; a lower threshold saves a
memcpy but pins more memory. Measured in P1a (idea).

## 8. Chosen design (precise)

### 8.1 Files (Task 6 scope)

Per design §13.1 (organise by component; one responsibility per file; narrow public surface):
- `crates/engine-core/src/completion.rs` — `RequestKey`, `ResponseBytes`, `ResponseData`, `Completion`,
  `CompletionSink`.
- `crates/engine-core/src/shard/mod.rs` — public API: `Submit`, `ShardCommand`, `ShardConfig`, `ShardHandle`, `shard`,
  `spawn_shard_thread`; `shard/state.rs` — `Shard`, `ShardState`, enqueue plan, idle and claim accounting, the
  poison-on-unwind guard; `shard/worker.rs` — the connection task loop (`next_submit`, park, connect, complete);
  `shard/guard.rs` — `InFlight`, `WorkerGuard`, `ShardGuard`; `shard/buffers.rs` — `BufPool`.
- `crates/engine-core/src/h1/connection/mod.rs` — `Connection` (socket, buffers, parser), the idle check and park
  primitive, the reuse decision; `h1/connection/exchange.rs` — `Exchange` and `ExchangeOutcome`;
  `h1/connection/read.rs` — `ReadState`; `h1/connection/write.rs` — `WriteState`. (`ReadState`/`WriteState` avoid a
  clash with tokio's `ReadHalf`/`WriteHalf`.)
- Every file above carries the parser's lint discipline: `#![deny(clippy::indexing_slicing,
  clippy::arithmetic_side_effects, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unreachable)]`;
  arithmetic is `checked_*` (`None` → `Engine(Internal)`), slices by stored offsets use `get(..)`.
- `crates/engine-core/tests/shard_loopback.rs` (+ `tests/support/script_server.rs`: a scripted tokio server that replays
  scenario vectors) — §10.
- `tests/vectors/shard-connection/{blazehttp,undici,go,hyper-util,curl}-selected.json` — scenario vectors (§10.3).
- `crates/engine-core/fuzz/fuzz_targets/h1_connection.rs` — §10.4.
- `crates/engine-core/examples/shard_bench.rs` — §11.2 (raw loop vs shard; no hyper, no `unsafe`).

### 8.2 Types and interface

```rust
// completion.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub struct RequestKey(pub u64);
pub enum ResponseBytes<'a> { HandedOff(Vec<u8>), Borrowed(&'a [u8]) }
pub struct ResponseData<'a> { pub status: u16, pub bytes: ResponseBytes<'a>, pub head: Range<u32>, pub body: Range<u32> }
pub struct Completion<'a> { pub key: RequestKey, pub req: Vec<u8>, pub outcome: Result<ResponseData<'a>, ErrorCode> }
pub trait CompletionSink: 'static {
    /// Records one terminal completion and copies `Borrowed` bytes before returning. Appends read buffers the consumer
    /// has returned for this shard thread to `reclaimed`; a buffer appended there must have no remaining external views.
    /// Must not panic (it also runs from `Drop` guards during unwinding).
    fn push(&self, c: Completion<'_>, reclaimed: &mut Vec<Vec<u8>>);
}

// shard/mod.rs
pub struct Submit { pub key: RequestKey, pub method: Method, pub req: Vec<u8>,
                    pub target: Range<u32>, pub headers: Range<u32>, pub body: Range<u32> }
pub enum ShardCommand { Submit(Submit) }                      // P1a: Credit, Cancel, UploadChunk, Poison
pub struct ShardConfig { /* private: addr, template, max_connections, queue_capacity, read_buf_size, max_response_bytes */ }
impl ShardConfig {
    pub fn new(addr: SocketAddr, template: HeaderTemplate, max_connections: usize, queue_capacity: usize,
               read_buf_size: usize, max_response_bytes: usize) -> Result<Self, ErrorCode>;
}
#[derive(Clone)] pub struct ShardHandle { tx: mpsc::UnboundedSender<ShardCommand>, pending: Arc<AtomicUsize>, capacity:
usize }
impl ShardHandle { pub fn try_submit(&self, s: Submit) -> Result<(), (Submit, ErrorCode)>; }
pub fn shard<S: CompletionSink>(cfg: ShardConfig, sink: S) -> (ShardHandle, impl Future<Output = ()>);
pub fn spawn_shard_thread<S: CompletionSink + Send>(cfg: ShardConfig, sink: S)
    -> std::io::Result<(ShardHandle, std::thread::JoinHandle<()>)>;
```

- **`ShardConfig::new` validation** (each rejection → `Err(Engine(Internal))`: the binding validates user input first,
  so a failure here is a contract violation): `max_connections ≥ 1`; `queue_capacity ≥ 1`; `read_buf_size ≥
  H1_LIMITS.max_head_bytes` (65,536; so the head phase, interim heads included, never grows the buffer and the
  ¼ threshold is ≥ 16 KiB); `max_response_bytes ≤ u32::MAX − max_head_bytes` (every range fits `u32`). Defaults used by
  tests and the binding: 64 KiB read buffers, `max_response_bytes` 100 MiB.
- **`max_response_bytes` is a raw body-byte limit** (bytes after the head as received), **not** `maxContentLength`
  (which applies after decompression, P1b middleware).
- **Ranges** in `ResponseData` are absolute offsets into the delivered bytes, which always start at the first byte of
  the exchange's response (interim heads included): `Borrowed` = `rbuf[0..end]`, `HandedOff` buffer with `len == end`.
  `head` = the final head's field lines (the parser's `headers` shifted by `msg_start`, as in parser note §6.6), `body`
  = `body_start..end`; `status` is its own field. The reason phrase stays inside the bytes (parser note §6.6).
- **`try_submit`**: validates the `Submit` ranges against `req.len()` (`Engine(Internal)` at once, nothing queued);
  `pending.fetch_add(1, AcqRel)`, and if the previous value is ≥ `capacity`, undo with `fetch_sub` and return
  `QueueFull { scope: Global, bound }` (`bound = u32::try_from(capacity).unwrap_or(u32::MAX)`, the generated field is
  `u32`); a closed channel (shard future dropped, runtime dropped, thread ended, or the shard poisoned and closed its
  receiver) → undo and `Engine(Poisoned)`. The shard decrements `pending` (`Release`, with a `debug_assert!` that the
  previous value was > 0) **before** each `push`, so a consumer that resubmits from inside `push` sees the slot free.
  Total pending (channel + `batch` + FIFO + in flight) is exactly ≤ `queue_capacity`.

### 8.3 Shard state

```rust
pub(crate) struct Shard<S> {
    cfg: ShardConfig,                    // immutable after start
    state: RefCell<ShardState>,
    idle: Notify,                        // parked connection tasks; woken LIFO with notify_last
    drained: Notify,                     // closing: the command loop waits for live == 0
    poison: Notify,                      // wakes the command loop when `poisoned` is set
    poisoned: Cell<bool>,                // outside the RefCell: readable from any guard
    pending: Arc<AtomicUsize>,           // shared with ShardHandle
    sink: S,
}
pub(crate) struct ShardState {
    queue: VecDeque<Submit>,             // FIFO of requests not yet taken; ≤ queue_capacity
    idle: usize,                         // parked tasks not yet notified (exact)
    claimed: usize,                      // queued requests already promised to a woken or spawned task
    live: usize,                         // connection tasks alive (connecting, busy or parked); ≤ max_connections
    closing: bool,                       // every handle dropped or poisoned: drain (or fail) the queue, then stop
    bufs: BufPool,                       // free read buffers, LIFO, ≤ 2 × max_connections
}
```

Rules:
- Every `RefCell` borrow lives inside one small synchronous `ShardState` method (`enqueue`, `pop_or_park`,
  `unpark_dirty`, `release_claim`, `retire`, `put_buffers`, `take_queue`, `begin_closing`); none crosses an `.await`
  (lint denied). `Shard` is shared as `Rc<Shard<S>>`; the sink, the `Notify`s and `poisoned` sit outside the `RefCell`,
  so `push` never runs under a state borrow.
- **Poison on unwind:** each mutator starts with a `PoisonOnUnwind(&shard)` drop guard that sets `poisoned` and calls
  `poison.notify_one()` when `std::thread::panicking()` — a panic in shared shard code (design §11.3 second scope) is
  then shard-wide, not connection-scoped.
- **Bulk failure** (`take_queue`): `mem::take` the `VecDeque` out of the borrow, then settle each `Submit` outside it
  (`pending -= 1`, then `push`).
- `begin_closing` sets `closing = true` and `idle = 0` in one borrow, then (outside it) `idle.notify_waiters()`.

### 8.4 Command loop (the future returned by `shard`)

```text
guard = ShardGuard { shard, rx, batch }                 // owns the receiver and the batch (§8.9)
loop:
    n = poll_fn(poison.notified() first, then rx.poll_recv_many(&mut batch, 256))
    if poisoned or n == 0: break
    plan = state.enqueue(batch.drain(..), max_connections)          // one borrow:
        // q = queue.len(); need = q.saturating_sub(claimed)
        // wake = min(need, idle); idle -= wake                          ← the notifier owns the decrement
        // spawn = min(need - wake, max_connections - live); live += spawn
        // claimed += wake + spawn
    repeat plan.wake times: idle.notify_last()
    repeat plan.spawn times: spawn_local(worker(shard.clone()))
if poisoned: guard.fail_all(Engine(Poisoned))           // closes rx, fails batch + channel + queue
else:        state.begin_closing(); idle.notify_waiters()   // all handles dropped: workers drain the queue
while state.live > 0: drained.notified().await
guard.disarm()
```

A woken or spawned task releases its claim (`claimed -= 1`) on its first `pop_or_park` after the wake, whether or not it
finds a request; a task that exits while holding a claim releases it in `WorkerGuard`. Two batches that arrive before a
woken task runs therefore wake or spawn only for the requests not yet claimed. A busy task that pops without a claim
leaves `claimed` unchanged (`saturating_sub` keeps `need` correct).

### 8.5 Connection task (`worker`)

```text
wg = WorkerGuard::new(&shard, claim = true)            // live, claim, parked flag, buffers, drained, respawn
conn = Connection::new(state.take_buffer())            // pool buffer; write Vec and parser owned
    // take failure: pop one request, settle it Engine(ResourceExhausted), exit (each failure settles a request, so the
    // WorkerGuard's respawn always makes progress)
loop:
    submit = next_submit(&shard, &mut conn, &mut wg).await else break     // Exit / Closing
    guard = InFlight::new(&shard, submit)              // settles exactly once (§8.9)
    if !conn.has_socket(): match conn.connect(addr).await:
        Err(e) → settle(guard, map_connect(e)); if live == 1: fail the queue with the same error; break
    outcome = conn.exchange(guard.submit(), &template, &cfg).await       // owned ExchangeOutcome or ErrorCode
    reuse = outcome.reuse && conn.exchanges < MAX_REQUESTS_PER_CONNECTION && !csd(outcome, guard.submit())
    settle(guard, outcome, conn) (§8.6.4)              // pending -= 1, push, reclaim, replace the read buffer
    if !reuse: conn.close()                            // the next iteration reconnects if work remains
```

`next_submit`:

```text
loop:
    match state.pop_or_park(wg.take_claim(), conn.has_socket()):
        Popped(s)  → if conn.has_socket() and idle_check(&conn) is Dirty: conn.close()   // reconnects with s in hand
                     return Some(s)
        Exit       → return None            // queue empty AND no socket (F8): free the slot and the buffer
        Closing    → return None            // closing and queue empty
        Parked     → wg.parked = true
                     match park(&shard.idle, conn.socket()).await:
                        Notified → wg.parked = false; wg.claim = true   // notifier did idle -= 1
                        Dirty    → state.unpark_dirty(); wg.parked = false; conn.close()   // loop: Exit unless work
```

`park` is one `poll_fn`: poll the pinned `Notified` first (ready → `Notified`), then, while a socket is held,
`poll_read_ready`: ready → `try_read` into a non-empty slice of the read buffer: `WouldBlock` (a stale flag, e.g. after a
read that exactly filled the buffer, or Windows) → call `poll_read_ready` **again in the same poll** so the waker is
registered, then `Pending`; `Ok(0)`, `Ok(n)` or an error → `Dirty` (the socket is closed; the bytes are discarded,
never parsed or logged). Single-threaded, so `notify_last` has marked this task's waiter before it runs; polling
`Notified` first keeps `idle` exact. One rule for socketless tasks (F8): with no socket and an empty queue the task exits;
a task that closed its socket after an exchange (`Connection: close` servers) reconnects in place while work remains, so
no task is spawned per reconnect.

**Connect failure policy:** the task settles its own request with the mapped error. If it was the last live task
(`live == 1`), every queued request fails with the same error (a down server fails fast; parity with undici
`client.js` `onError` 472–493). Otherwise the queue stays for the live connections. **[author's call]**, §12 item 4.

### 8.6 The exchange (`h1/connection/`, State pattern §13)

`Exchange` is **generic over the I/O halves** (`R: AsyncRead + Unpin`, `W: AsyncWrite + Unpin`, static dispatch), so the
worker drives it with `TcpStream::split()` halves (borrowed, zero cost) and tests and the fuzz target drive it with
`tokio::io::duplex`. One `poll_fn`: if the write state is not `Done`, poll it; then poll the read state — so an early
final response is seen while writing (RFC 9112 §9.5).

**8.6.1 Per-exchange reset (S9, F15).** Every exchange starts with `parser.reset(submit.method)`, `filled = 0`,
`msg_start = 0`, `WriteState::Sending { sent: 0 }`, `ReadState::Head`; `Connection::close()` resets the same fields. The
read buffer content is never interpreted beyond `filled`.

**8.6.2 `WriteState`:** `Sending { sent }` → `Done`, or `Failed(io::Error)`.
- Before the exchange the worker reserves the write buffer with `try_reserve(req.len() + template.len() + 32)` (failure →
  `Engine(ResourceExhausted)`, nothing written, connection reusable) and calls `write_request(&mut wbuf, &parts,
  template)` (Task 5; capacity reused). A writer error → `Engine(Internal)` (the binding's validation was bypassed),
  nothing written, connection reusable. If Task 5's note puts the `try_reserve` inside `write_request`, the worker's
  reservation is dropped (coordinate in Task 5's review).
- After each exchange, `wbuf.shrink_to(64 KiB)` when its capacity exceeds 64 KiB, so one large POST does not pin its
  size per connection (vectored writes of large bodies straight from `Submit.req` are a P1a speed item, threshold from
  Task 5).
- `poll_write` until all bytes are sent (tokio clears write readiness on a short write, `poll_evented.rs` 240–267). A
  write error moves to `Failed`: the exchange keeps reading (an early response may already be in the receive buffer,
  RFC 9112 §9.6) and maps the error only when the read state ends without a response.

**8.6.3 `ReadState`:** `Head` → `Body { end }` (Content-Length) / `ToEof` (close-delimited) → `Done`.
- Reads use `AsyncRead::poll_read` into `ReadBuf::new(&mut rbuf[filled..])`; `rbuf` is initialised (`len ==` its
  allocated length), so no `unsafe`, and the short-read clear applies. **Never** read into an empty slice (an empty
  read returns `Ok(0)`, which would look like EOF).
- `Interim { consumed }` → `msg_start += consumed` (bytes stay; no shifting). `Complete(head)` → `body_start =
  msg_start + head_len`; framing: `ContentLength(n)`: `n > max_response_bytes` → `TooLarge(ResponseBody)` at head time,
  no allocation; else `end = body_start + n` (checked). `Empty` → `end = body_start`. `CloseDelimited` → `ToEof`.
  `Chunked` → `Protocol(Other)` and close (`// removal: P1a chunked FramingDecoder (#issue)`).
- **Body limit (F6):** the buffer may grow to `cap = body_start + max_response_bytes` for Content-Length bodies and to
  `body_start + max_response_bytes + 1` for close-delimited ones (one probe byte): `filled − body_start >
  max_response_bytes` → `TooLarge(ResponseBody)`; EOF with ≤ the limit → success. So a close-delimited body of exactly
  the limit succeeds, one byte more fails, and no read ever targets an empty slice.
- **Growth:** when `filled == rbuf.len()` and more bytes are expected, `grow` adds `min(rbuf.len(), remaining_expected,
  cap − rbuf.len())` with `try_reserve` (failure → `Engine(ResourceExhausted)`) and zero-extends with `resize`: doubling,
  never sized by `Content-Length` (design §11.3), O(n) total. A computed growth of 0 means the limit is reached →
  `TooLarge(ResponseBody)`. The head phase never grows (`read_buf_size ≥ max_head_bytes`).
- **No wasted reads:** the read state stops as soon as `filled >= end`; it never reads "until `WouldBlock`".
- **Early final response:** if a final head completes while the write state is still `Sending`, stop writing; deliver
  the response after its body; the connection is not reused (§5.1).

**8.6.4 `ExchangeOutcome` and building the completion (F5).** `exchange` returns `Result<ExchangeOutcome, ErrorCode>`
with `ExchangeOutcome { status, head: Range<usize>, body: Range<usize>, end, reuse }`, where `reuse` is computed before
anything is settled: write `Done`, `head.keep_alive`, `filled == end`, no early response. The worker then:
1. applies the cap and CSD rules to `reuse` (§8.5);
2. `pending -= 1`;
3. if `end ≥ read_buf_size / 4`: `buf = mem::take(&mut conn.rbuf)`, `buf.truncate(end)`, push `HandedOff(buf)`; else
   push `Borrowed(&conn.rbuf[..end])`;
4. moves `reclaimed` (a per-worker scratch `Vec`, capacity reused, outside any borrow) into the pool;
5. after a hand-off, `conn.rbuf = pool.take()`; if that fails, the connection is closed with an empty buffer and the
   next exchange takes one (failure → that request settles `Engine(ResourceExhausted)`).

### 8.7 EOF accounting, reuse and close rules, error mapping

Every exchange starts with an empty read buffer (a connection with leftover bytes is never reused), so "response bytes
received for this exchange, interim heads included" is `filled > 0`.

**Reuse** only when all hold: write `Done`; read `Done` with `head.keep_alive` (the parser folds `Connection: close` in
any head incl. 1xx (sticky), HTTP/1.0 without keep-alive, close-delimited, 204/304 with `Transfer-Encoding` or
`Content-Length > 0`); `filled == end` (no leftover); no error; **not** (status outside 200–299 and the request had a
non-empty body) — the CSD rule; fewer than 1,000 exchanges on the connection. **Close** otherwise, and additionally on an
early final response, `Chunked` (P0), a dirty idle check (I1) or idle readiness (I4).

| Condition | Outcome |
|---|---|
| connect `ConnectionRefused` | `Network(Refused)` |
| connect `HostUnreachable` / `NetworkUnreachable` | `Network(Unreachable)` (brief: `Other`; §12 item 6) |
| connect, any other error (incl. `set_nodelay` failure) | `Network(Other)` |
| EOF, `ConnectionReset`, or write error (`BrokenPipe`, `ConnectionReset`) with **0 response bytes** | `Network(ClosedBeforeResponse)` |
| EOF after ≥ 1 byte, before the final head ends (interim heads included: `interim-then-eof`) | `Protocol(MalformedResponse)` |
| EOF in a Content-Length body | `Protocol(IncompleteBody)` |
| EOF in a close-delimited body (≤ the limit) | success; close |
| `ConnectionReset` after ≥ 1 response byte | `Network(Reset)` |
| other read/write I/O errors | `Network(Other)` |
| parser errors | as returned (`Protocol(*)`, `TooLarge(Headers)`, `Engine(Internal)`) |
| `Content-Length` > `max_response_bytes`; a close-delimited body past it | `TooLarge(ResponseBody)` |
| `Framing::Chunked` (P0 only) | `Protocol(Other)` (removal: P1a) |
| `write_request` error; a `Submit` range outside `req`; checked arithmetic overflow | `Engine(Internal)` (nothing written; connection reusable for the writer case) |
| `try_reserve` failure (read or write buffer, pool) | `Engine(ResourceExhausted)` |
| request dropped unsettled: runtime dropped / its task's own panic / shard poisoned | `Aborted(EngineDropped)` / `Engine(Panic)` / `Engine(Poisoned)` (§8.9) |

One function `map_io(err, received: bool) -> ErrorCode` serves reads and writes. `Protocol(UnexpectedData)` has no
producer in P0: idle bytes close a connection without affecting any request, and leftover bytes after a complete
response close it after delivering that valid response (§12 item 13).

### 8.8 Wake budget (argued, and counted in §6)

- FIFO non-empty at completion: the finishing connection pops the next request inside the same task poll → **0 task
  wakes per request**; its socket wakes it (I/O readiness, excluded by §5.1).
- Closed loop / low load: submit → command loop = **1 per batch** (measured mean batch 15.5–16.4 at c = 50, 1.0 at
  c = 1); `notify_last` → the parked connection = **1 per request**; completion → the consumer's own wake (Node: TSFN on
  empty → non-empty; bench: its channel) = **1 per batch** on the consumer side.
- **Other wakes, all bounded:** a woken task finding the queue emptied by a busy one (≤ the number of claims, i.e. ≤
  requests); one I4 wake per server-closed or dirty idle connection (one per connection lifetime); `notify_waiters` at
  close (once per shard); coop self-yield (≤ 1 per ≈ 60 back-to-back exchanges); Windows: one extra `try_read` syscall
  per park and per checkout (syscalls, not wakes); one `poison` wake per shard lifetime.

### 8.9 Settle exactly once, panics, poisoning and shutdown (§11.3–§11.4)

Single-threaded, so "moved out or dropped" is the one transition per request; no atomic is needed (this replaces the
design's "one atomic transition" for in-shard settling; §12 item 15).
- **`InFlight`** (one request): holds the `Submit` until `settle` moves it into the `Completion`. If dropped first, its
  `Drop` decrements `pending` and pushes one completion: `Engine(Poisoned)` if `poisoned`, `Engine(Panic)` if
  `std::thread::panicking()` (the task's own panic, caught by tokio's harness, §5 item 11), else `Aborted(EngineDropped)`.
  It only settles; it never touches `live`. On this path `reclaimed` is a fresh empty `Vec` (no allocation unless the
  sink returns buffers) whose buffers are dropped.
- **`WorkerGuard`** (the task's whole life): on drop, via `try_borrow_mut` (a failed borrow sets `poisoned`): `live -= 1`;
  `idle -= 1` if `parked` and not notified; releases a held claim; returns the read buffer to the pool; wakes `drained`
  when closing and `live == 0`; when not closing and the queue still has unclaimed requests, spawns a replacement task
  (`live += 1`, claim) so a task that dies outside a request (connect error, panic in park, `Exit` races) never strands
  the queue.
- **`ShardGuard`** (the command-loop future): owns `rx` and `batch`. On drop or on poison it calls `rx.close()`,
  settles everything in `batch` and everything still in the channel (`try_recv` until empty) and the FIFO (`take_queue`)
  with `Engine(Poisoned)` (poisoned or unwinding) or `Aborted(EngineDropped)` (plain drop), decrementing `pending` per
  request, then `begin_closing()` + `idle.notify_waiters()`, so parked workers on a caller `LocalSet` exit instead of
  staying parked (S13). Order on a shard-thread panic: the `block_on` future unwinds first (poison, channel and queue
  failed), then the runtime drops the workers (their `InFlight` guards report `Engine(Poisoned)`).
- **Poisoning** (design §11.3 second scope): set by `PoisonOnUnwind` in any `ShardState` mutator or by a failed
  `try_borrow_mut` in a guard; the command loop wakes on `poison`, fails all as above, and `try_submit` returns
  `Engine(Poisoned)` from then on (the channel is closed).
- `CompletionSink::push` must not panic (it runs inside `Drop` during unwinding; a panic there aborts). Node's sink uses
  `parking_lot` (no poisoning).
- The design's `catch_unwind` requirement is met by tokio's task harness for workers; the command-loop future runs
  under `block_on`, whose panic ends the shard thread (the third §11.3 scope).
- Tests use `#[cfg(test)]` failpoints (a thread-local "panic here" flag checked in `park`, `connect` and one
  `ShardState` mutator) — compiled only in unit tests, so no production code path or feature exists for them.

### 8.10 Buffer pool (`shard/buffers.rs`)

`BufPool { free: Vec<Vec<u8>>, len: usize, cap: usize, max: usize }`: `len = read_buf_size`; `cap` = the `capacity()` of
the pool's first allocated buffer (the allocator may round up), recorded once; `max = 2 × max_connections`. `take()`:
`pop()` (LIFO, warm cache) or a new buffer (`try_reserve_exact(len)`, `resize(len, 0)`; failure →
`Engine(ResourceExhausted)`). `put(buf)`: accept only `buf.capacity() == cap`; zero-extend with `resize(len, 0)` (a
handed-off buffer comes back truncated to its `end`: the memset covers at most 48 KiB, only for ≥ 16 KiB responses);
drop when full or when the capacity differs (grown buffers are not pooled). Reclaimed buffers enter through `put` from
the per-worker `reclaimed` scratch outside any state borrow.

**Memory bounds (S6).** Pool ≤ `2 × max_connections × read_buf_size` (50 connections: 6.25 MiB). Worst case P0
aggregate: `max_connections × (max_head_bytes + max_response_bytes)` in connection buffers (50 × 100 MiB in theory —
P0 has no budget yet) + the pool + write buffers ≤ `max_connections × max(64 KiB, largest request in flight)` + the
sink backlog. Handed-off buffers in flight and the sink backlog are bounded by the consumer's pending caps (JS
`SlotTable`, §11.5) and, from P1a, by the byte budget — not by this shard's `pending` count, which drops at push. Idle
pool trimming (§11.1 burst recovery) arrives with P1a timers. **P1a budget hook:** reserve budget on **every** `grow`
and every `take` before calling `try_reserve`, release at hand-off to JS (§4 step 6) or when a buffer is dropped.

### 8.11 Running it

- `shard(cfg, sink)`: builds the channel, `Rc<Shard>` and returns `(handle, command_loop_future)`. The caller runs the
  future on a `LocalSet` (`LocalSet::run_until` / `spawn_local`) or a `LocalRuntime` — engine-bench's symmetric harness
  (§16.2) drives its sink on the same runtime, so no cross-thread wake is charged to us alone.
- `spawn_shard_thread(cfg, sink)`: creates the channel and handle on the caller, then `std::thread::Builder::new()
  .name(format!("blazehttp-shard-{n}"))` (`n` from a process-wide `AtomicUsize`) moves `rx`, `cfg`, `sink` into the
  thread, which builds `Builder::new_current_thread().enable_io().build_local(LocalOptions::default())`, reports success
  or the build error back through a `std::sync::mpsc::sync_channel(1)` (one blocking hand-shake at start-up) and
  `block_on`s the command loop. It returns the handle and the `JoinHandle` (tests join it to prove the thread ends when
  every handle is dropped and the queue has drained).
- Shard thread runtime: default `event_interval` (61); no blocking pool use in P0 (no DNS).

### 8.12 TCP options

`TCP_NODELAY` on every connection (`set_nodelay(true)`, docs.rs `TcpStream::set_nodelay`): requests are written in one
call and Nagle would delay small writes behind delayed ACKs. No `SO_KEEPALIVE` (the idle TTL in P1a covers it), no
`SO_RCVBUF`/`SO_SNDBUF` tuning (kernel autotuning), no `SO_LINGER` (deprecated in tokio; RST only in tests via
`set_zero_linger`).

## 9. Interface changes vs the brief (justified)

1. **`ResponseData.buf: Vec<u8>` → `bytes: ResponseBytes<'a>`, `Completion<'a>`, `push(&self, Completion<'_>, …)`.** The
   brief's exact-size copy is one allocation per small response (G0 zero allocations; and §4 step 4's rule against
   allocating on one thread and freeing on another). Measured 0 vs 1 allocations (§6). Hand-offs are truncated to `end`.
2. **Pending bound by `Arc<AtomicUsize>` over an unbounded channel**, not a bounded `mpsc::Sender`. A bounded channel
   only bounds messages not yet received; the FIFO behind it would be unbounded, or the bound would be 2×. The counter
   bounds submit → completion exactly, and P1a's `Cancel`/`Credit` must never fail with `QueueFull`.
3. **`ShardConfig::new(..) -> Result<ShardConfig, ErrorCode>` with private fields**: `host: String` → a validated
   `HeaderTemplate` (Task 5's `HeaderTemplate::new` at the caller); **`max_response_bytes`**, a raw **body** limit,
   because every buffer must be bounded (G6, §11.3) before the P1a budget exists — so `cl-1tib-no-body` settles
   `TooLarge(ResponseBody)` at head time in P0; the field bounds in §8.2.
4. **`spawn_shard_thread` builds a `LocalRuntime`** (stable since 1.51.0) and returns `(ShardHandle, JoinHandle<()>)`;
   `shard()` still runs on a caller `LocalSet`.
5. **`ShardCommand` dispatches with a `match`; the `Execute` trait (design §11.3/§13 Command) arrives with the second
   command (P1a)** — one implementation does not need a trait.
6. **Error mapping additions:** `Network(Unreachable)`, `Network(Reset)` after ≥ 1 byte, write errors treated like EOF
   when no response byte arrived (§8.7); `try_submit` validates ranges.
7. **`exchange` returns an owned `ExchangeOutcome`**; the worker builds the `Completion` (§8.6.4).

## 10. Test plan (owner's definition of done applies in full)

### 10.1 Brief tests (`tests/shard_loopback.rs`, scripted in-test tokio server per scenario)

All kept as written except where noted:
1. `get_round_trip_returns_status_head_body` (status, head range = field lines, body bytes).
2. `keep_alive_reuses_one_connection` (100 sequential, `max_connections = 1`, exactly 1 accept).
3. `never_exceeds_max_connections` (200 concurrent, `max_connections = 8`, peak simultaneous connections ≤ 8).
4. `connection_close_reconnects`.
5. `leftover_bytes_close_connection` — the server writes the response and junk **in one write** (parser vector
   `two-responses-one-read`; curl test1479 `http11-then-http09`); next request on a new connection, correct response.
6. **`server_closed_idle_connection_settles_with_network_error` is split**, because with idle detection (I4) the second
   request no longer meets the dead connection — the test would be racy:
   - 6a `idle_connection_closed_by_server_is_not_reused`: after the first response the server half-closes (shuts down
     its write side) and keeps reading; when I4 closes the client socket the server reads EOF and signals the test (1 s
     deadline, no sleeps); the second request then succeeds on a new connection (2 accepts).
   - 6b `close_after_request_settles_closed_before_response`: the server reads the second request and closes without
     answering → `Network(ClosedBeforeResponse)` within 1 s; the third request succeeds on a new connection.
7. `eof_mid_body_settles_incomplete_body`.
8. `refused_connect_settles_refused` (bind, take the port, drop the listener).
9. `queue_full_fails_fast` (`queue_capacity = 4`, shard future not polled; the 5th `try_submit` → `QueueFull { Global,
   4 }`; the first four complete once the future runs).
10. `each_key_completes_exactly_once` (10,000 requests, `max_connections = 50`, `queue_capacity ≥ 10,000`; no
    duplicate, no missing key).
11. `small_response_copied_large_handed_off`: a 1 KiB response arrives as `Borrowed` with `len == end`; a response with
    `end ≥ read_buf_size / 4` arrives as `HandedOff(buf)` with `buf.capacity() == ` the pool's recorded capacity and
    `buf.len() == end`.
12. `reclaimed_buffer_is_reused`, exact sequence: request 1 (large) hands off buffer A and the sink keeps it; request 2
    (large) hands off buffer B, and during its `push` the sink returns A in `reclaimed` → A enters the pool and becomes
    the connection's read buffer; request 3 (large) hands off A: `completion.buf.as_ptr() == A`.
13. `request_buffer_returned_in_completion` (`req.as_ptr()` equals the submitted one).

### 10.2 Parser-note hand-off vectors (connection layer)

- `two-responses-one-read`, `http11-then-http09` → test 5.
- `extra_response_while_idle_is_discarded`: the server sends a complete extra response after the first and keeps
  reading; it signals the test when it reads the client's close (1 s deadline); the next request uses a new connection
  and gets the correct response.
- `early_final_response_stops_writing_and_delivers`: POST with an 8 MiB body; the server reads only the head, answers
  `413` with `Content-Length` and `Connection: close`, and keeps the socket open without reading until the client
  closes → completion with status 413 (not a write error), connection closed.
- `content_length_without_body_settles_incomplete_body`: `Content-Length: 67108864` (64 MiB, below the limit), EOF after
  the head → `Protocol(IncompleteBody)`; a unit test asserts `grow` never adds more than `min(len, remaining, cap −
  len)`.
- `cl-1tib-no-body` → `TooLarge(ResponseBody)` at head time, no allocation (P0 has the body limit; ruling F1b/Q1).
- `interim_then_eof_settles_malformed_response` (`103` head, then EOF).
- `interim-flood-timer`, `tls-close-delimited-*`: P1a (timers, TLS).

### 10.3 Scenario vectors ported from upstream (behaviour only, licence notices kept)

Scenario vectors are JSON: a server script (read the request head / body, write bytes, wait for a test signal, half-close,
close, reset) and the expected outcome per request and accept count. Files under `tests/vectors/shard-connection/`, each
case with `source`, `derivedFrom: "<file>:<test> @ <tag>"`, and the source's licence in the file and in
`THIRD_PARTY_NOTICES` (no code copied):
- **undici v8.12.4 (MIT)** `test/response-queue-poisoning.js` "should not reuse an idle socket with buffered unsolicited
  response bytes" (the GHSA-35p6 regression); `test/client-keep-alive.js` "connection close", "Disable keep alive",
  "HEAD keep-alive header reuses socket when connection header is fragmented", "HEAD connection tokens preserve reuse and
  respect close" (the `Keep-Alive: timeout` cases "keep-alive header", "keep-alive header 0/1", "keep-alive threshold",
  "keep-alive max keepalive" are ported in P1a with the idle TTL).
- **Go go1.27.3 (BSD-3-Clause)** `src/net/http/transport_test.go`: `TestTransportKeepAlives`,
  `TestTransportConnectionCloseOnResponse`, `TestTransportConnectionCloseOnRequest`,
  `TestTransportRemovesDeadIdleConnections`,
  `TestTransportServerClosingUnexpectedly`, `TestTransportIdleConnCrash`, `TestTransportNoReuseAfterEarlyResponse`,
  `TestTransportPrefersResponseOverWriteError`, `TestTransportReuseConnEmptyResponseBody`, `TestTransportIgnores408`
  (as "408 on an idle connection closes it quietly"), `TestTransportIdleConnRacesRequest`; `TestRetryRequestsOnError`
  is ported in P1a with the re-send rule.
- **hyper-util 0.1.21 (MIT)** `tests/legacy_client.rs`: `drop_client_closes_idle_connections`,
  `no_keep_alive_closes_connection`, `socket_disconnect_closes_idle_conn`, `client_keep_alive_0`,
  `client_keep_alive_extra_body`, `client_keep_alive_when_response_before_request_body_ends`; **hyper 1.12.0 (MIT)**
  `tests/client.rs`: `client_error_unexpected_eof`, `client_head_ignores_body`, `dropped_conn_sends_incomplete_body_error`,
  `incoming_content_length`, `client_100_then_http09`, `http1_half_closed_peer_with_open_request_body_does_not_spin`
  (hyper 1.x has no pool; its keep-alive and close tests live in hyper-util).
- **curl 8.23.0 (curl licence)** `tests/data/test1479` (HTTP/1.1 response followed by HTTP/0.9 bytes).
- `blazehttp-selected.json`: our own cases from §10.1–§10.5 that are expressible as scripts.

### 10.4 New tests

- Idle and race: `unsolicited_response_while_parked_on_busy_shard_closes_when_reactor_turns` (inject a response while
  the connection is parked, let the runtime idle once, then submit → 2 accepts, correct response);
  `idle_crlf_closes_connection`; `idle_reset_closes_connection`; `stale_readiness_then_server_close_detected_while_parked`;
  `response_exactly_filling_buffer_then_extra_bytes_closes`; `dirty_at_checkout_reconnects`; `idle_reuse_is_lifo`.
- Containment: `non_2xx_after_request_body_closes_connection` (and a 2xx with a body, and a non-2xx without a body,
  reuse); `connection_closes_after_1000_requests` (exactly 2 accepts for 1,001 sequential requests).
- Buffers: `handed_off_buffer_has_no_bytes_past_end`; `borrowed_slice_len_equals_end`;
  `large_body_grows_and_hands_off` (1 MiB).
- Limits: `content_length_at_limit_succeeds`, `content_length_over_limit_fails_at_head_time`,
  `close_delimited_at_limit_succeeds`, `close_delimited_over_limit_settles_too_large` (the probe byte; never success on a
  truncated read); `content_length_body_plus_head_over_read_buf_size_grows`.
- Errors: `eof_mid_head_settles_malformed_response`; `reset_before_response_settles_closed_before_response` and
  `reset_mid_body_settles_reset` (server `set_zero_linger()` then drop — never `set_linger`);
  `writer_error_settles_internal_and_keeps_connection`
  (a `Submit` whose ranges pass `try_submit` but whose headers the writer rejects).
- Framing and reuse: `interim_heads_are_skipped` (`100`, `103` then `200`);
  `connection_close_in_interim_head_closes_after_exchange`; `no_content_with_content_length_closes`;
  `not_modified_with_content_length_closes`; `head_response_with_content_length_reuses`;
  `http10_without_keep_alive_closes`; `http10_with_keep_alive_reuses`; `close_delimited_body_completes_on_eof_and_closes`;
  `chunked_settles_protocol_other_and_closes` (removal: P1a).
- Lifecycle: `dropping_handles_drains_queue_then_future_returns`; `dropping_runtime_settles_every_pending_key_once`;
  `dropping_runtime_with_commands_still_in_channel_settles_them`; `shard_drop_settles_channel_and_batch_contents`;
  `pending_returns_to_zero_on_every_path`; `last_connect_failure_fails_queued_requests` (`max_connections = 1`, refused
  with 10 queued → 10 × `Refused`, one connect attempt); `spawn_shard_thread_round_trip` (joins the thread after dropping
  the handle).
- Panics (`#[cfg(test)]` failpoints): `worker_panic_outside_request_does_not_leak_connection_slot` (panic in `park`;
  `live` returns to its value and queued work still completes); `connect_panic_settles_engine_panic`;
  `panic_in_shared_state_poisons_shard_and_settles_all` (every key settles once, `try_submit` → `Engine(Poisoned)`).
- Unit tests: `ShardConfig::new` rejections (each bound); `enqueue` plans including **two batches before the woken task
  runs** (no over-wake, no over-spawn) and claim release on exit; `pop_or_park`/`unpark_dirty`/`begin_closing` accounting;
  `BufPool` (cap, wrong capacity rejected, truncated buffer re-extended with zeros); `map_io` table; the reuse decision
  table incl. CSD and the cap; `grow` (bounds, zero growth → `TooLarge`); `try_submit` (undo on full, on closed, range
  validation).
- Idle-detection tests (6a, `extra_response_while_idle_is_discarded`, the S1 test, `idle_crlf_closes_connection`) run on
  every CI OS (Windows has no short-read clear, §5 item 1) and assert behaviour, never syscall counts.
- Coverage ≥ 90% (`cargo llvm-cov nextest -p engine-core --fail-under-lines 90`).

### 10.5 Fuzz target `h1_connection`

`crates/engine-core/fuzz/fuzz_targets/h1_connection.rs` (cargo-fuzz on the pinned nightly, design §6.3): the input is
decoded into a sequence of requests (method, body length) and a peer script — byte chunks (arbitrary, plus a
dictionary of heads, `Content-Length`/`Transfer-Encoding` values, CRLF runs, 1xx heads), `Idle(bytes)` events between
exchanges, EOF, reset (an error from the duplex), and write stalls. A scripted in-memory peer on `tokio::io::duplex`
replays it against a `Connection` driven exactly as the worker drives it (exchange, reuse decision, idle check between
exchanges), on a current-thread runtime with paused time. **Oracle:** exactly one outcome per request; never reuse after
leftover bytes, idle bytes, an error, `Connection: close`, the CSD condition or the cap; `Borrowed`/`HandedOff` lengths
equal `end` and the ranges stay inside them; the error class matches the §8.7 table, computed by a small independent
model of the script (bytes received before EOF, head complete, body complete); no allocation larger than the limits
(a capped test allocator inside the fuzz crate, which may use `unsafe` because it is outside `engine-core`).

### 10.6 Mutation testing

`cargo mutants -p engine-core -f 'src/shard/**' -f 'src/h1/connection/**'` (cargo-mutants, latest release at Task 6
start, recorded in `docs/notes/toolchain.md`) with the test suite above: target **≥ 90% of viable mutants caught**;
every survivor is either killed with a new test or listed in the Task 6 report with the reason it is equivalent.

## 11. Performance plan and P1a hooks

### 11.1 Budget

The shard path (submit → completion, plaintext 1 KiB, c = 50) must be ≤ 1.10× the raw loop's CPU µs per request (§16.2
ceiling) and ≤ hyper `client::conn` in the same run (P0 acceptance, binding). With the raw loop at 4.0–5.5 µs on Linux
(§1-F), the shard machinery may add **≤ 400–550 ns of CPU per request**; its expected user-space parts (unverified,
to be confirmed by `perf stat` instructions per request): FIFO push + pop ≈ 10 ns, the claim/idle bookkeeping ≈ 10 ns,
`notify_last` + `Notified` registration ≈ 50–100 ns (mutex), one local task wake + poll ≈ 100–200 ns, sink push ≈
50 ns. Allocations per request in steady state: 0. Task wakes: as §8.8.

### 11.2 Task 6 step: `shard_bench` on Linux (added to the Task 6 plan)

`crates/engine-core/examples/shard_bench.rs`: an in-process fixed-response tokio server on its own threads; clients on
one `LocalRuntime` thread: the raw loop (our parser, one task per connection) and the shard through a channel sink on the
same runtime (the §6 driver); no hyper (hyper stays in `engine-bench`), no `unsafe`. CPU from
`/proc/thread-self/stat` (client thread user and system time, Linux); sink wakes and mean batch size are counted by the
example's own sink; command-loop wakes and idle notifies need internal counters, so they are taken with a local,
uncommitted counting patch as in §6 (no counter feature or test-only code ships); syscalls per request with `strace -c
-f` and `perf stat -e 'syscalls:sys_enter_*',instructions` on a separate short run. `socket2` as a dev-dependency for
the peek micro-cost.
Run in the Linux arm64 container (`--cpus=1` client, nightly window, no other container running), c = 1, 50, 500,
**≥ 10 rounds**, median + bootstrap CI (§16.4). Record in `docs/perf/journal.md`: the c = 1 hop (shard vs raw), user
vs system CPU, syscalls and wakes per request, and the forced-peek cost measured three ways — **full** (every reuse),
**parked-only** (only when the request goes to a connection woken from park) and **none** — against the **≤ 2% CPU per
request** criterion of §12 item 1.

### 11.3 c = 1 mitigations (if the c = 1 cell misses non-inferiority)

| Option | What changes | Saves at c = 1 | Cost / risk |
|---|---|---|---|
| **Receiver baton** | the receiver lives in the shard (`RefCell<Option<Receiver>>`, polled only inside `poll_fn`, never borrowed across `.await`); a parked connection polls `poll_recv_many` itself, so a submit wakes it directly; the command loop polls only while no connection holds the baton | 1 task wake + 1 task poll per request (≈ 150–300 ns, unverified) | baton hand-over logic when the holder takes work; spawning decisions move into the holder; more states to test |
| **Inline drain before parking** | a connection about to park calls `try_recv` on the shared receiver and takes new commands itself | the command-loop round when commands are already queued (moderate load); **nothing at c = 1** (the submit still wakes the command loop) | small; needs the shared receiver as above |
| **engine-bench-only local submit** | the bench submits straight into the FIFO + `notify_last` (same thread, no channel) | the channel and command-loop wake in G0 | measures a path Node never takes (Node submits from another thread), so it would flatter G0 — **rejected for gate runs**; usable only as a diagnostic |

### 11.4 P1a hooks (designed now, not built)

- **Timeouts (S7):** each `InFlight` gets a timer-heap slot index; the worker's `poll_fn` also checks an "expired" flag
  set by the shard's single timer task, which wakes the worker's stored waker. Phases: `connect` checked while
  connecting; while writing, `idleSend` and `headers` are armed together; `headers` spans interim heads (never refreshed
  by them) up to the final head; `idleBody` and the minimum body rate in `Body` **and** `ToEof`; `total` spans
  everything; parked connections get the idle TTL (30 s, §11.5) and the server's `Keep-Alive: timeout`. Until then a
  slow-loris server can pin a connection and its buffer indefinitely (P0 has no timers; documented).
- **Stale-connection re-send (S8, F19):** the `InFlight` guard records `reused`, `bytes_written` and a `resent` flag; on
  `ClosedBeforeResponse` (or a write error before any byte) on a reused connection, if the rule of design §5.1 holds,
  the request is still unsettled and not cancelled, and `resent` is false, the **same worker reconnects in place and
  re-sends from its guard** with `resent = true` — never `push_front` onto the shared queue.
- **`Cancel`:** a generational slab of request entries with an intrusive FIFO replaces the `VecDeque` (O(1) cancel of a
  queued request); an in-flight cancel sets a flag on the worker's slot and wakes it; the worker settles
  `Aborted(Signal)` and closes the connection (the exchange cannot be resumed). ≤ 1 `Cancel` per pending key.
- **`UploadChunk` / `Credit` (S11):** outside the pending counter; `UploadChunk` bytes are charged to the byte budget,
  `Credit`s are coalesced per key.
- **`Poison`:** routed to the same path as `PoisonOnUnwind` (the `ShardGuard` fail-all).
- **Budget:** reserve on every `take` and every `grow` before `try_reserve` (§8.10).
- **Reconnect churn (S12):** a per-origin connect-rate cap with backoff; `set_zero_linger()` on desync closes evaluated
  then.
- **P1b pool keys:** profile key + origin, and the **target origin** for absolute-form proxy connections (§4 curl row,
  controller ruling (e)).
- **`io_uring`** (research only, §7): tokio supports it only for files under `tokio_unstable` (changelog 1.54.0 "fs: fall
  back when `io_uring` has no IO driver"), and Docker's default seccomp profile blocks it (design §7). The design is
  compatible with completion-based I/O because every buffer is owned by exactly one connection or completion at a time.

## 12. Proposed design changes for the owner (not applied to the design doc)

1. **§5.1 idle detection + §18 row.** Record: "parked connections keep read interest (I4) and a checkout `try_read` (I1)
   closes on any byte or EOF; windows W1–W3 remain (note §7.2)". **Forced `recv(MSG_PEEK)` deferred to Linux data**
   (Task 6 `shard_bench`): measured full / parked-only / none; adopted (full or parked-only) if it costs ≤ 2% CPU per
   request in the reference cell **[author's call]**. Replace the §18 client-side-desync row with: likelihood
   **Low–Medium**, impact **High** (cross-client response shift until the connection closes); controls I1 + I4, the CSD
   rule, the 1,000-request cap, leftover/error close, the peek decision.
2. **§5.1 (new reuse rules):** no reuse after a non-2xx final response to a request that carried a body; close a
   connection after 1,000 exchanges **[author's call]**.
3. **§4 step 6 / §10:** a small response (< ¼ of the read buffer) is copied **by the sink into its arena** from a
   borrowed slice, not into a per-response `Vec`; handed-off buffers are truncated to the response's end;
   `CompletionSink::push` takes `Completion<'_>`; §11.5's "Completion push" row gains "+ a memcpy of < 16 KiB".
4. **§5.1 (new):** when the last live connection to an origin fails to connect, every queued request for it fails with
   that error (undici parity); with live connections, the queue waits for them **[author's call]**.
5. **§4 step 4 / §11.5:** the shard thread's data queue is unbounded and the **pending count is an atomic per shard**
   (submit → completion), so data-plane control commands never fail on a full queue.
6. **§8 / brief:** connect `HostUnreachable`/`NetworkUnreachable` → `NETWORK/unreachable`; a reset after ≥ 1 response
   byte → `NETWORK/reset`; a write error before any response byte (after checking for an early response) →
   `NETWORK/closed-before-response`.
7. **Brief test 6** split into 6a/6b (§10.1).
8. **§5.2 304 + `Content-Length` > 0:** keep "not reused" (W1–W3 remain); revisit after item 1's decision.
9. **Parser note §12 `cl-1tib-no-body`:** P0 has a body-size limit, so the expected outcome is `TOO_LARGE/response-body`
   at head time; `PROTOCOL/incomplete-body` applies below the limit.
10. **RFC 9112 §9.2 CRLF exception:** we close on idle CRLF too (stricter; avoids a special case) **[author's call]**.
11. **§9.4 / §18:** the command-loop hop is a measured low-load cost (dev c = 1: 3 client-thread wakes per request vs
    hyper's 2); carry it into the G1-low risk row with the §11.3 mitigations, decided with Linux c = 1 data.
12. **§13.1:** the shard and the connection are directory modules from the start
(`shard/{mod,state,worker,guard,buffers}.rs`,
    `h1/connection/{mod,exchange,read,write}.rs`).
13. **§8 `PROTOCOL/unexpected-data`:** no producer in the native engine in P0 (idle and leftover bytes close silently);
    keep the code for the undici transport's mapping or remove it in P4 if nothing produces it.
14. **§13 State pattern:** `WriteState` is `Sending → Done | Failed` in P0 (one buffer per request); `Head → Body → Done`
    returns with streamed bodies (P1a credits). No `Execute` trait in P0 (one command; §11.3/§13 Command).
15. **§11.3/§11.4 "one atomic transition":** inside a shard, settling is single-threaded "moved out or dropped" (no
    atomic); the atomic remains only where threads race (JS-side abort vs completion).
16. **§11.5 / §3.1:** `max_response_bytes` (raw body bytes, engine bound) is distinct from `maxContentLength` (after
    decompression, P1b middleware).
17. **P1b pool keys:** include the target origin for absolute-form proxy connections (curl reuse CVEs, §4).
18. **Owner DoD for Task 6:** the plan adds the `h1_connection` fuzz target, the `cargo mutants` pass and the Linux
    `shard_bench` step (§10.5–§10.6, §11.2).

## 13. Sources

Official Rust and crate documentation (versions locked or pinned; read 2026-11-07, verified):
- tokio 1.54.1 docs: `TcpStream` (`try_read`, `poll_read_ready`, `split`, `set_nodelay`, `set_zero_linger`)
  https://docs.rs/tokio/1.54.1/tokio/net/struct.TcpStream.html · `LocalRuntime`
  https://docs.rs/tokio/1.54.1/tokio/runtime/struct.LocalRuntime.html · `Builder`
  https://docs.rs/tokio/1.54.1/tokio/runtime/struct.Builder.html · `LocalSet`
  https://docs.rs/tokio/1.54.1/tokio/task/struct.LocalSet.html · `spawn_local`
  https://docs.rs/tokio/1.54.1/tokio/task/fn.spawn_local.html · `Notify`
  https://docs.rs/tokio/1.54.1/tokio/sync/struct.Notify.html · `mpsc::Receiver` (`recv_many`, `poll_recv_many`,
  `close`, `try_recv`) https://docs.rs/tokio/1.54.1/tokio/sync/mpsc/struct.Receiver.html · `task::coop`
  https://docs.rs/tokio/1.54.1/tokio/task/coop/index.html · `yield_now`
  https://docs.rs/tokio/1.54.1/tokio/task/fn.yield_now.html · `io::duplex`
  https://docs.rs/tokio/1.54.1/tokio/io/fn.duplex.html — and the 1.54.1 source files cited in §5.
- tokio issue #5866 (clear readiness on short reads): https://github.com/tokio-rs/tokio/issues/5866.
- mio 1.2.4 source (`sys/unix/selector/{epoll,kqueue}.rs`), crates.io tarball.
- Rust std 1.99.1: https://doc.rust-lang.org/1.99.1/std/cell/struct.RefCell.html,
  https://doc.rust-lang.org/1.99.1/std/rc/struct.Rc.html, https://doc.rust-lang.org/1.99.1/std/collections/struct.VecDeque.html,
  https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html, https://doc.rust-lang.org/1.99.1/std/task/struct.Waker.html,
  https://doc.rust-lang.org/1.99.1/std/panic/fn.catch_unwind.html, https://doc.rust-lang.org/1.99.1/std/thread/fn.panicking.html;
  Clippy 1.99 lint list https://rust-lang.github.io/rust-clippy/rust-1.99.0/index.html.
- socket2 0.6.5 `SockRef`: https://docs.rs/socket2/0.6.5/socket2/struct.SockRef.html.

Reference sources (exact tags in §3; read 2026-11-07, verified): hyper 1.12.0 `src/proto/h1/{conn,dispatch,io}.rs`,
`src/client/dispatch.rs`, https://github.com/hyperium/hyper/blob/v1.12.0/tests/client.rs; hyper-util 0.1.21
`src/client/legacy/{pool,client}.rs`, `tests/legacy_client.rs`; reqwest 0.13.5 `src/async_impl/client.rs`,
`src/connect.rs`; https://github.com/golang/go/blob/go1.27.3/src/net/http/transport.go, `request.go`,
`transport_test.go`; https://github.com/nodejs/undici/blob/v8.12.4/lib/dispatcher/client-h1.js, `client.js`, `pool.js`,
`test/response-queue-poisoning.js`, `test/client-keep-alive.js`; https://github.com/valyala/fasthttp/blob/v1.77.0/client.go;
https://github.com/curl/curl/blob/curl-8_23_0/lib/conncache.c, `lib/cf-socket.c`, `tests/data/test1479`;
https://github.com/h2o/h2o/blob/f35c735ebe0657235702c2b91912542052cc05db/lib/common/socketpool.c,
`lib/common/http1client.c`;
https://github.com/boostorg/beast/blob/boost-1.92.0/example/http/client/async/http_client_async.cpp,
`include/boost/beast/http/impl/read.hpp`; https://github.com/oven-sh/bun/blob/bun-v1.4.4/src/http/HTTPContext.rs,
`packages/bun-usockets/src/bsd.c`.

Advisories and issues (read 2026-11-07, verified): GHSA-35p6-xmwp-9g52 / CVE-2026-6733
https://github.com/advisories/GHSA-35p6-xmwp-9g52; undici #5493 https://github.com/nodejs/undici/issues/5493, #5600
https://github.com/nodejs/undici/issues/5600, PR #5606 https://github.com/nodejs/undici/pull/5606; hyper #2136
https://github.com/hyperium/hyper/issues/2136; Go #32310 https://github.com/golang/go/issues/32310, #22158
https://github.com/golang/go/issues/22158; curl CVE-2021-22924, CVE-2022-22576, CVE-2022-27775, CVE-2022-27782,
CVE-2023-27535, CVE-2023-27536, CVE-2023-27538 at https://curl.se/docs/<CVE>.html (titles read); h2o issue #679 (cited in
`socketpool.c`, not read).

RFCs (verified): RFC 9112 §9.2, §9.3.1, §9.5, §9.6 https://www.rfc-editor.org/rfc/rfc9112.

Research and engineering writing (read 2026-11-07):
- PortSwigger, "Response queue poisoning"
https://portswigger.net/web-security/request-smuggling/advanced/response-queue-poisoning
  — verified (page read); J. Kettle, "HTTP/2: The Sequel is Always Worse" (2021) https://portswigger.net/research/http2
  — verified (page reachable; origin of the term); "Browser-Powered Desync Attacks" (2022)
  https://portswigger.net/research/browser-powered-desync-attacks — verified in the parser note §4.2.
- T-Reqs (CCS 2021) and the HTTP Garden (arXiv 2405.17737) — verified in the parser note §15; used here for the fuzz
  oracle's desync classes.
- Cloudflare, "How we built Pingora" (2022)
  https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/ — verified (text
  read): per-worker pools lowered connection reuse; sharing across threads raised one customer's reuse ratio from 87.1%
  to 99.92%. For us: one pool per profile on its shard thread, never split per worker.
- tokio blog, "Making the Tokio scheduler 10x faster" (2019) https://tokio.rs/blog/2019-10-scheduler and "Reducing tail
  latencies with automatic cooperative task yielding" (2020) https://tokio.rs/blog/2020-04-preemption — pages reachable;
  background for the LIFO slot and the coop budget, whose current values are verified in source (§5 items 9–10).
- nginx `keepalive_requests` https://nginx.org/en/docs/http/ngx_http_core_module.html#keepalive_requests — verified
  (page read): default 1000 since 1.19.10 (previously 100); "Closing connections periodically is necessary to free
  per-connection memory allocations".
