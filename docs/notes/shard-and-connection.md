# Shard, connection tasks, buffer pool and `CompletionSink`: research note (Task 6)

Design §1-F, §4 (request flow steps 3–8), §5.1 (reuse rules, stale-connection re-send), §5.3, §9.4, §10, §11 (budget,
environments, panics, settle exactly once, bounds), §13 (State pattern), §13.1 (code structure; one function, one job,
≤ 60 lines), §16.2 (symmetric harness). Builds on `docs/notes/response-parser.md` (§6.6 interface, §12 hand-off) and
treats the Task 5 writer interface as given: `write_request(out: &mut Vec<u8>, parts: &RequestParts, template:
&HeaderTemplate)`.

Access date for every link: **2026-11-07**. **Verified** = checked against the primary source (the pinned release's
source file, an official doc page for the locked version, an RFC) or a measurement run for this note. **Unverified** =
stated by a source but not reproduced here; unverified ideas go only to `docs/perf/ideas.md`. All measurements in §6 are
**dev measurements** on the owner's M2 under normal desktop load (macOS, not the Linux reference container, not the
nightly window: other agents' containers were running, so no Docker run was made). They rank designs and verify
allocation and wake counts; absolute CPU numbers must be re-measured in the G0 harness (Tasks 7/8).

## 1. Summary and decision

**Request flow — design A, central FIFO + LIFO idle wake, exchange inside the connection task.** One command-loop task
per shard drains an unbounded `mpsc` with `recv_many` into a `VecDeque` FIFO in `Rc<Shard>` state, wakes at most as many
parked connection tasks as there are new requests (`Notify::notify_last`, LIFO, the notifier owns the idle-count
decrement) and spawns connection tasks only up to `max_connections`. A connection task owns its socket, read buffer,
write buffer and parser; it pops the next request itself, writes it, reads and parses the response in the same task, and
pushes the completion. A connection that finishes takes the next queued request without any task wake. Requests are
FIFO entries, never tasks. Wake budget: 0 task wakes per request under load (FIFO non-empty), 1 per request at low load
(the idle `Notify`), plus 1 per batch (the command loop) — within §5.1's "≤ 1 per request + 1 per batch".

**Idle monitoring — I1 + I4.** A parked connection polls its socket's read readiness in the same poll as its `Notified`
future (no extra task, no extra future), so a byte or EOF that the reactor sees while it is idle closes it at once (RFC
9112 §9.5 "SHOULD constantly monitor"); at checkout it calls `TcpStream::try_read`, which performs a syscall **only** when
the reactor has flagged the socket (tokio 1.54.1 `Registration::try_io`), so a clean connection costs ≈ 5 ns and zero
syscalls (measured). The residual window — bytes that reach the kernel after the last reactor turn, mainly when a busy
connection takes the next request back to back — is closed only by a forced `recv(MSG_PEEK)` per reuse (Bun, h2o) or a
reactor turn per reuse (undici's CVE-2026-6733 fix). That costs one syscall per request (2.1 µs on the dev Mac); it is
put to the owner with Linux data as a proposed design change (§12 item 1), not built in P0.

**Buffers — B2.** Each connection owns a pooled read buffer (64 KiB, `len == capacity`, initialised once). A response
that fills ≥ ¼ of it is **handed off** (`ResponseBytes::HandedOff(Vec<u8>)`) and the connection takes a fresh pooled
buffer; a smaller response is passed as a **borrowed slice** (`ResponseBytes::Borrowed(&[u8])`) that the sink copies into
its own capacity-reusing arena. This replaces the brief's "copy into an exact-size `Vec`", which costs one allocation per
request (and, on Node, a free on another thread) and would break G0's zero-allocation target. Reclaimed buffers come back
through `CompletionSink::push`'s `reclaimed` and go into a LIFO free list capped at `2 × max_connections`.

**Measured (dev, §6):** the prototype of this design made **0.000 heap allocations per request** in steady state (hyper
`client::conn`: 10.0); CPU per request at c = 50 within noise of the raw tokio loop (median 12.1 vs 13.7 µs, ±20–30%
host noise) and below hyper (17.7 µs); wakes per request at c = 50: 1.000 idle notifies (closed loop, every connection
parks between requests) + 0.05 sink wakes. At c = 1 the extra command-loop hop shows: median 25.1 µs vs raw 16.6 µs and
hyper 20.6 µs — a risk for the c = 1 G0 cell, recorded with a mitigation idea (§11, §12 item 11).

**Interface changes vs the brief** (§9): `ResponseBytes` borrowed/handed-off body and a lifetime on `Completion`;
pending bound counted by an `Arc<AtomicUsize>` in the handle over an unbounded channel (exact bound, and P1a's `Cancel`
can never fail on a full queue); `ShardConfig` takes a validated `HeaderTemplate` instead of `host`, plus
`max_response_bytes`; `spawn_shard_thread` builds a `LocalRuntime` (stable since tokio 1.51); one brief test is split
because it contradicts idle detection (§10.1).

## 2. Scope and hand-offs

Decided here: the shard's data structures and task model, connection reuse and close rules (P0 subset of §5.1), how idle
connections are watched (the open item of design §5.1 and parser note §12), EOF accounting, error mapping for the
connection layer, buffer ownership and pooling, the `CompletionSink` contract, how `shard()` and `spawn_shard_thread()`
run, and the test and performance plan. Not decided here (later phases, hooks in §11): timeouts (P1a timer heap),
stale-connection re-send (P1a), `Cancel`/`Credit`/`UploadChunk`/`Poison` commands, byte budget, per-origin scheduler and
fairness, TLS, DNS, chunked decoding (P1a `FramingDecoder`).

## 3. Reference implementations (latest releases, read 2026-11-07)

Pinned sources: hyper 1.12.0, hyper-util 0.1.21, reqwest 0.13.5, tokio 1.54.1, mio 1.2.4 (crates.io tarballs); Go
go1.27.3 `src/net/http/transport.go`, `request.go`; undici v8.12.4 `lib/dispatcher/{client-h1,client,pool,pool-base}.js`;
fasthttp v1.77.0 `client.go`; curl `curl-8_23_0` `lib/{conncache,cf-socket,multi,url,transfer}.c`; h2o master
`f35c735e` (2026-11-06; the project publishes no releases) `lib/common/{http1client,socketpool,httpclient}.c`;
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
| **curl 8.23.0** | multi handle, one thread, `multi_runsingle` state machine per transfer | transfer picks a connection from the pool (`conncache.c`) | download buffer per transfer | at reuse `Curl_cpool_conn_seems_healthy` (`conncache.c` 1040–1078) runs `cf_socket_conn_is_alive` — **`poll()` with 0 timeout** (`cf-socket.c` 1811–1846); input pending on an idle non-multiplexed conn → unhealthy. **Rate-limited: skipped if checked < 1000 ms ago** (1050–1051) | max age / idle limits (`cpool_conn_too_old`) |
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
- **A peek syscall per reuse** (h2o, Bun) and **a loop turn per reuse** (undici): closes the race window at ≈ 1 syscall
  per request. **Not taken in P0; owner decision with Linux data** (§12 item 1).

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
| **Response queue poisoning on reused keep-alive sockets** — an attacker-controlled upstream injects an unsolicited response on an idle socket; the next request gets it. undici < 6.27.0 / 7.28.0 / 8.5.0, CVE-2026-6733 (low) | GHSA-35p6-xmwp-9g52 (`gh api /advisories`), undici `client-h1.js` 1089–1108 | I4 + I1 (§7.2); leftover → close; never pipeline; residual window documented, owner decision on I2 (§12 item 1) |
| Response queue poisoning / stacked responses as a desync outcome (Kettle 2021 "HTTP/2: the sequel is always worse"; 2022 client-side desync; 2025 "HTTP/1.1 must die") | PortSwigger research pages; parser note §4.2 | the parser's framing rules + close on leftover bytes, on 204/304 with a framing header, on any error |
| RFC 9112 §9.2: data on a connection with no outstanding request "MUST NOT" be treated as a response; the client "SHOULD close the connection … unless the data consists only of one or more CRLF" | RFC 9112 §9.2 | close on **any** idle byte, CRLF included (stricter; §12 item 10) |
| RFC 9112 §9.5: "Implementations SHOULD constantly monitor open connections for a received closure signal"; a client sending a body "SHOULD monitor … for an error response" and "immediately cease transmitting" | RFC 9112 §9.5 | I4; the exchange polls the read half while writing (early final response, §8.6) |
| RFC 9112 §9.6: an immediate server close can turn the client's unread input into a reset | RFC 9112 §9.6 | read before mapping a write error (§8.6); reset after ≥ 1 response byte → `NETWORK/reset` |
| Server closes an idle connection just as the client writes ("unfortunate keep-alive timeout") — Go #22158, hyper #2136 (`IncompleteMessage`) | issue texts | `NETWORK/closed-before-response` (zero response bytes); the transparent re-send arrives in P1a (§5.1) |
| A `408` sent on an idle connection before the server closes (Go #32310) | Go 2653–2670, issue text | a stray byte while idle: close quietly, no request is affected |
| Leftover bytes silently dropped and the connection pooled | fasthttp 3682–3688 | never: leftover → close |
| `SO_LINGER` setter deprecated in tokio (`TcpStream::set_linger`, `#[deprecated]` at `net/tcp/stream.rs` 1334) | tokio 1.54.1 source | tests that need an RST use `set_zero_linger()` (1368), never `set_linger` (§14.1) |

## 5. Runtime mechanics at the locked versions (verified)

`engine-core` has no tokio dependency yet; Task 6 adds `tokio = "=1.54.1"` (latest stable, 2026-10-21; same as the spike
lockfiles) with features `rt`, `net`, `io-util`, `sync` (and `time` arrives with P1a timers). Rust 1.99.1 (pinned).

1. **Read readiness is cached and cleared on short reads.** `PollEvented::poll_read` clears readiness when `0 < n < len`
   on epoll/kqueue targets (tokio `io/poll_evented.rs` 182–213, tokio issue #5866), and on `WouldBlock`. So after a
   response that arrives in one short read, the next read future returns `Pending` **without a syscall**. This is how
   "no wasted read attempts after a complete response" (§5.3) comes for free — provided reads go through `AsyncRead::
   poll_read` (`TcpStream::split` halves, `net/tcp/split.rs` 371–377).
2. **`try_read` / `try_read_buf` do a syscall only when readiness is set** (`Registration::try_io`,
   `runtime/io/registration.rs` 194–213: "Don't attempt the operation if the resource is not ready") and do **not** apply
   the short-read clear (`net/tcp/stream.rs` 638–644, 782–800). Hence: steady-state reads use `poll_read`; `try_read` is
   used only for the checkout check, where readiness is normally clear (measured 5.1 ns, §6).
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
   and a `LocalRuntime` (`task/local.rs` 394–440). Docs: https://docs.rs/tokio/1.54.1/tokio/runtime/struct.LocalRuntime.html.
7. **`Notify`** (https://docs.rs/tokio/1.54.1/tokio/sync/struct.Notify.html): `notify_one` wakes the oldest waiter (FIFO),
   `notify_last` the most recently added (LIFO) (`sync/notify.rs` 657–671); at most one permit is stored when nobody
   waits; the waiter list is an intrusive linked list under a mutex (211) — registering a waiter does not allocate.
8. **`mpsc`**: bounded and unbounded share the block list (`BLOCK_CAP` = 32 on 64-bit, `sync/mpsc/mod.rs` 140) and
   reuse freed blocks (`list.rs` 200–245), so a steady state with bounded occupancy does not allocate (measured 0.000 per
   request, §6). `recv_many(buffer, limit)` "will never return 0 unless the channel has been closed and there are no
   remaining messages", `limit == 0` returns 0 immediately, and it is cancel-safe (`bounded.rs` 248–319).
   `UnboundedSender::send` is synchronous and fails only when closed (`unbounded.rs` 559).
9. **Cooperative budget**: 128 units per task poll (`task/coop/mod.rs` 115–116); I/O and channel operations consume it
   (`registration.rs` 162, 233). A connection task that completes many back-to-back exchanges without `Pending` is
   forced to yield after the budget — at most one self-wake per ≈ 60 exchanges, and the right fairness behaviour. We do
   **not** wrap tasks in `unconstrained`.
10. **`yield_now` does not guarantee a reactor turn**: "the runtime will not drive the IO driver between every poll of a
    task" (`task/yield_now.rs` 6–35). So undici's "yield to the poll phase" has no exact tokio equivalent; the
    current_thread scheduler polls the driver every `event_interval` = 61 ticks or when idle (`runtime/builder.rs`
    281–283).
11. **Panics in spawned tasks are caught** by the task harness (`runtime/task/harness.rs` 504–530, `catch_unwind`), and a
    `LocalSet` ignores them by default (`task/local.rs` 518). A panic inside the future passed to `block_on` propagates.
12. **Deprecations to avoid**: `TcpStream::set_linger` / `TcpSocket::set_linger` (use `set_zero_linger`).
13. **Rust std** (https://doc.rust-lang.org/1.99.1/std/): `Rc` is `!Send`; `RefCell::borrow_mut` "Panics if the value is
    currently borrowed", `try_borrow_mut` returns an error instead (`cell/struct.RefCell.html`); `VecDeque::push_back` /
    `pop_front` are O(1) amortised (`collections/struct.VecDeque.html`); `Vec::try_reserve` returns `TryReserveError`
    instead of aborting (`vec/struct.Vec.html#method.try_reserve`); `std::thread::panicking`
    (`thread/fn.panicking.html`); `io::ErrorKind::{ConnectionRefused, ConnectionReset, BrokenPipe, HostUnreachable,
    NetworkUnreachable}` (the last two stable since 1.83). Clippy `await_holding_refcell_ref`
    (https://rust-lang.github.io/rust-clippy/rust-1.99.0/index.html#await_holding_refcell_ref) is denied workspace-wide.
14. **`socket2`** (only for the I2 option): tokio's `net` feature already depends on `socket2 0.6` (tokio `Cargo.toml`
    96–107, 981–984), so a direct dependency adds no crate; `SockRef::from(&stream).peek(..)` is safe
    (https://docs.rs/socket2/0.6.5/socket2/struct.SockRef.html).

## 6. Measurements (dev, verified for allocation and wake counts)

**Prototype** (scratch crate, not committed; ≈ 300 lines): server = tokio multi-thread (4 workers) in the same process,
fixed response with 3 header fields and a 1 KiB body; client = one `LocalRuntime` thread, closed loop with `c` requests in
flight on `c` connections, request bytes fixed, response parsed with `engine_core::h1::ResponseParser`, FNV-1a over the
body in every mode. CPU = `CLOCK_THREAD_CPUTIME_ID` of the client thread; allocations = a thread-local counting allocator
on the client thread only. Modes: `raw` (one task per connection looping write/read — the §1-F 40-line loop), `shard`
(design A as in §8, sink copying small bodies into a reused arena, idle check `none` / `tryread` (I1) / `peek` (I2 via
`socket2`)), `hyper` (`client::conn` + one caller task per connection, the spike/g0 shape). rustc 1.99.1, fat LTO,
`codegen-units = 1`, tokio 1.54.1, hyper 1.12.0. macOS loopback; host load not controlled.

**c = 50, 200k measured requests after 20k warm-up, 5 alternating rounds (median, range):**

| Mode | CPU µs / request | Allocations / request | Task wakes / request (counted) |
|---|---|---|---|
| raw | 13.72 (11.83–14.95) | 0.000 | — (I/O only) |
| shard, no check | 12.12 (10.60–14.46) | **0.000** | idle notifies 1.000, conn parks 1.000, sink wakes 0.051–0.060 |
| shard, I1 `try_read` | 13.28 (10.46–14.92) | **0.000** | same |
| shard, I2 `peek` | 13.36 (12.72–15.01) | **0.000** | same |
| hyper `client::conn` | 17.69 (14.11–19.86) | **10.0** | not instrumented (2 by construction) |

**c = 1, 30k requests after 3k warm-up, 3 rounds (median, range):** raw 16.57 (16.04–18.42) · shard 25.07
(21.30–30.01) · shard + I1 21.39 (18.80–26.35) · shard + I2 23.38 (20.14–25.66) · hyper 20.61 (13.65–21.43) µs; shard
wakes per request: idle notify 1, sink 1 (plus the command-loop wake per submit).

**Idle-check micro-benchmark** (one clean idle keep-alive connection, 1M calls): `try_read` **5.1 ns** per check (no
syscall: readiness was cleared by the short read); `socket2` `peek` **2,130 ns** per check (one `recv(MSG_PEEK)` on
macOS; Linux not measured).

What this shows (and does not):
- **Verified:** zero heap allocations per request in steady state for design A with B2 buffers (deterministic); hyper's
  10 allocations per request; the wake counts claimed in §8.8 at c = 50; I1 is free when the connection is clean.
- **Dev only:** macOS loopback CPU is kernel-dominated (≈ 10–15 µs per request vs 4–5.5 µs on Linux in §1-F) and the
  host was loaded (±20–30% between rounds), so the ≈ 1.3× hyper/raw ratio agrees with §1-F but the shard-vs-raw
  difference is inside the noise. At c = 1, design A costs more than raw and hyper on these runs (one extra local wake
  plus the command-loop poll per request). Re-measure all of it in the G0 harness on Linux (§11).

## 7. Options compared

### 7.1 How requests flow from the command queue to connections

| | **A. Central FIFO + idle `Notify` (chosen)** | B. Per-connection queues (hyper shape) | C. Direct hand-off to an idle-connection stack | D. Work stealing |
|---|---|---|---|---|
| Shape | command loop pushes into one `VecDeque`; parked connections are woken LIFO; a busy connection pops the next request itself | the command loop assigns each request to one connection's queue (round-robin or least-loaded) | the command loop pops an idle connection id (LIFO stack in a slab) and moves the request into its mailbox, then wakes its stored `Waker` | per-connection deques, idle connections steal |
| Time | O(1) enqueue, O(1) pop; wake O(1) (`notify_last`) | O(1) per assignment, but a request may wait behind a slow exchange while another connection is idle (head-of-line) | O(1) both; removal of an idle connection that dies needs an intrusive list (O(1)) | O(1) amortised |
| Space | one `VecDeque<Submit>` ≤ pending bound | N queues | slab of connection slots + idle stack | N deques |
| Allocations / request | 0 (measured) | 0 with reused queues | 0 | 0 |
| Task wakes / request | under load 0 (busy connection takes the next one); low load 1 | ≥ 1 per request (the target connection) even under load | low load 1; under load 0 if busy connections pop the overflow FIFO | as A |
| Failure modes | spurious wake when a busy connection pops first (rare, harmless re-park); idle count must be exact (the notifier decrements it) | assignment to a connection that just died → re-queue logic; HOL blocking | more code: mailbox, per-slot waker, removal on death | needs threads to pay off; one shard is one thread |
| Verdict | **chosen**: least code, proven `Notify` semantics, LIFO, satisfies the wake budget | rejected: HOL and a wake per request under load | kept as the P1a upgrade path if the idle `Notify` mutex shows in profiles (it would also host per-origin scheduling) | rejected: no parallelism inside a shard |

### 7.2 Watching idle connections for stray bytes and EOF

| | I1. Cached readiness at checkout (`try_read`) | I2. Forced peek at checkout (`recv(MSG_PEEK)`) | I3. Reactor turn before reuse | **I4. Read interest while parked** |
|---|---|---|---|---|
| Used by | hyper (implicitly via `poll_read`), our checkout | h2o (every checkout), Bun (every reuse), curl (`poll(0)`, ≤ 1/s) | undici ≥ 8.5.0 (one loop turn per reuse) | hyper, Go, Bun, undici (read while idle) |
| Cost per reuse | 0 syscalls when clean (5.1 ns measured); 1 syscall when the reactor flagged the socket | 1 syscall always (2.1 µs dev Mac; Linux to measure) | an extra scheduler round + driver poll; tokio cannot force it (§5 item 10) | per park: one `poll_read_ready` (waker store); 0 syscalls |
| Cost per idle connection | none | none | none | none (no task, no future, no timer) |
| What it catches | everything the reactor has seen before the check | everything in the kernel before the write | everything in the kernel before the poll | everything the reactor sees while parked, and closes the socket at once (fds, RFC 9112 §9.5) |
| Misses | bytes that arrived after the last reactor turn — mainly back-to-back reuse under load (≤ 61 task polls, §5 item 10) | bytes in flight after the peek (inherent to HTTP/1.1, RFC 9112 §9.2) | same as I2 | same as I1 while busy |
| Verdict | **chosen** (with I4) | **owner decision with Linux data** (§12 item 1) | rejected: not expressible in tokio, and undici's own regressions | **chosen** |

No client-side check removes the race completely: a byte that arrives after the check and before the real response is
indistinguishable from a response (RFC 9112 §9.2 relies on order). The checks bound the window; never pipelining,
closing on leftover bytes and closing on any error keep a desync from spreading beyond one exchange.

### 7.3 Buffer ownership and pooling

| | B1. Brief: hand off ≥ ¼, else copy into an exact-size `Vec` | **B2. Hand off ≥ ¼, else borrowed slice copied by the sink into its arena (chosen)** | B3. Always hand off | B4. Second pool of small buffers |
|---|---|---|---|---|
| Allocations / request (1 KiB response) | **1** (+ a free on the consumer's thread in Node, against §4 step 4) | 0 (measured) | 0 | 0 |
| Copies | 1 | 1 (≤ 16 KiB) | 0 | 1 |
| Memory per pending small completion | exact | exact (in the sink arena) | 64 KiB (64× amplification of a 1 KiB body, all charged to the budget) | the small class size |
| Return traffic | none | none | one buffer per request through `reclaimed` | one buffer per request |
| Verdict | rejected (G0 zero-allocation) | **chosen** | rejected (memory, §11.1 budget) | rejected (extra pool and traffic for no gain over B2) |

The ¼ threshold (16 KiB of 64 KiB) caps the memory amplification of a handed-off buffer at 4×; a lower threshold saves a
memcpy (≈ 1.6 µs at 16 KiB, unverified) but pins more memory. Measured in P1a (idea).

## 8. Chosen design (precise)

### 8.1 Files (Task 6 scope)

Per design §13.1 (organise by component; one responsibility per file; narrow public surface):
- `crates/engine-core/src/completion.rs` — `RequestKey`, `ResponseBytes`, `ResponseData`, `Completion`,
  `CompletionSink`.
- `crates/engine-core/src/shard/mod.rs` — public API: `Submit`, `ShardCommand`, `ShardConfig`, `ShardHandle`, `shard`,
  `spawn_shard_thread`; `shard/state.rs` — `Shard`, `ShardState`, the enqueue plan and idle accounting;
  `shard/worker.rs` — the connection task loop (`next_submit`, park, connect, complete); `shard/guard.rs` — the
  settle-exactly-once guards. (The brief's single `shard.rs` would pass 500 lines.)
- `crates/engine-core/src/h1/connection.rs` — `Connection` (socket, buffers, parser), `Exchange` with `WriteHalf` and
  `ReadHalf` states, the reuse decision and the idle check.
- `crates/engine-core/src/buf.rs` — `BufPool`.
- `crates/engine-core/tests/shard_loopback.rs` (+ `tests/support/` server helpers) — §10.
- No benchmark in `engine-core` (hyper and a counting allocator belong to `engine-bench`, Tasks 7/8; §11).

### 8.2 Types and interface

```rust
// completion.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub struct RequestKey(pub u64);
pub enum ResponseBytes<'a> { HandedOff(Vec<u8>), Borrowed(&'a [u8]) }
pub struct ResponseData<'a> { pub status: u16, pub bytes: ResponseBytes<'a>, pub head: Range<u32>, pub body: Range<u32> }
pub struct Completion<'a> { pub key: RequestKey, pub req: Vec<u8>, pub outcome: Result<ResponseData<'a>, ErrorCode> }
pub trait CompletionSink: 'static {
    /// Records one terminal completion; copies `Borrowed` bytes before returning; appends read buffers the consumer
    /// has returned for this shard thread to `reclaimed`. Must not panic (it also runs from `Drop` guards).
    fn push(&self, c: Completion<'_>, reclaimed: &mut Vec<Vec<u8>>);
}

// shard/mod.rs
pub struct Submit { pub key: RequestKey, pub method: Method, pub req: Vec<u8>,
                    pub target: Range<u32>, pub headers: Range<u32>, pub body: Range<u32> }
pub enum ShardCommand { Submit(Submit) }                      // P1a: Credit, Cancel, UploadChunk, Poison
pub struct ShardConfig { pub addr: SocketAddr, pub template: HeaderTemplate, pub max_connections: usize,
                         pub queue_capacity: usize, pub read_buf_size: usize /* 65_536 */,
                         pub max_response_bytes: usize /* 100 MiB, §11.5 maxContentLength default */ }
#[derive(Clone)] pub struct ShardHandle { tx: mpsc::UnboundedSender<ShardCommand>, pending: Arc<AtomicUsize>, capacity: usize }
impl ShardHandle { pub fn try_submit(&self, s: Submit) -> Result<(), (Submit, ErrorCode)>; }
pub fn shard<S: CompletionSink>(cfg: ShardConfig, sink: S) -> (ShardHandle, impl Future<Output = ()>);
pub fn spawn_shard_thread<S: CompletionSink + Send>(cfg: ShardConfig, sink: S) -> std::io::Result<ShardHandle>;
```

`ResponseData::head` and `body` index the bytes (`HandedOff` buffer or `Borrowed` slice). A handed-off buffer keeps
`len == capacity` (§8.6); consumers use the ranges, never `len()`. `try_submit`: `pending.fetch_add(1, AcqRel)`; if the
previous value is ≥ `capacity`, undo with `fetch_sub` and return `QueueFull { scope: Global, bound }` where `bound =
u32::try_from(capacity).unwrap_or(u32::MAX)` (the generated `bound` is `u32`); if the channel is closed (shard gone),
undo and return `Engine(Poisoned)`. The shard decrements `pending` (`Release`) right after each `push`. Total pending
(channel + FIFO + in flight) is therefore exactly ≤ `queue_capacity`, and the channel holds at most that many commands.

### 8.3 Shard state

```rust
pub(crate) struct Shard<S> {
    cfg: ShardConfig,                    // immutable after start
    state: RefCell<ShardState>,
    idle: Notify,                        // parked connection tasks; woken LIFO with notify_last
    drained: Notify,                     // closing: the command loop waits for live == 0
    pending: Arc<AtomicUsize>,           // shared with ShardHandle
    sink: S,
}
pub(crate) struct ShardState {
    queue: VecDeque<Submit>,             // FIFO of requests not yet taken; ≤ queue_capacity
    idle: usize,                         // parked connection tasks not yet notified (exact)
    live: usize,                         // connection tasks alive (connecting, busy or parked); ≤ max_connections
    closing: bool,                       // every handle dropped: drain the queue, then stop
    bufs: BufPool,                       // free read buffers, LIFO, ≤ 2 × max_connections
}
```

Rules: every `RefCell` borrow lives inside one small synchronous method of `ShardState` (`enqueue`, `pop_or_park`,
`unpark_dirty`, `retire`, `put_buffers`); none crosses an `.await` (the lint is denied). `Shard` is shared as
`Rc<Shard<S>>` between the command loop and the connection tasks of one shard thread; the sink and the `Notify`s sit
outside the `RefCell` so `push` never runs under a state borrow.

### 8.4 Command loop (the future returned by `shard`)

```text
batch: Vec<ShardCommand> with capacity 256 (reused)
while rx.recv_many(&mut batch, 256).await > 0:
    plan = state.enqueue(batch.drain(..), max_connections)   // one borrow
        // q = queue.len(); wake = min(q, idle); idle -= wake;     ← the notifier owns the decrement
        // spawn = min(q - wake, max_connections - live); live += spawn
    repeat plan.wake times: shard.idle.notify_last()
    repeat plan.spawn times: spawn_local(worker(shard.clone()))
// channel closed: every ShardHandle dropped
state.closing = true; shard.idle.notify_waiters()
while state.live > 0: shard.drained.notified().await          // workers drain the queue, then exit
```

The closing path drains: queued requests still run and complete normally, then the future returns. A `ShardGuard` held by
this future settles the queue if the future is dropped or unwinds (§8.9).

### 8.5 Connection task (`worker`)

```text
conn = Connection::new(shard.take_buffer())            // read buffer from BufPool; write Vec and parser owned
loop:
    submit = next_submit(&shard, &mut conn).await       // None when closing and the queue is empty → exit
    guard = InFlight::new(&shard, submit)               // settles exactly once (§8.9)
    if !conn.has_clean_socket():                        // no socket, or the checkout try_read was not WouldBlock
        conn.connect(addr).await → on Err(e): settle(guard, map_connect(e)); fail_queue_if_last(e); exit
    outcome = conn.exchange(guard.submit(), &template, max_response_bytes).await
    settle(guard, outcome)                              // push (Borrowed or HandedOff), reclaim, pending -= 1
    if outcome closes the connection: conn.close()      // the next iteration reconnects if work remains
exit: state.retire() (live -= 1; wake `drained` when closing and live == 0); buffers back to the pool
```

`next_submit` (≤ 60 lines with `park`):

```text
loop:
    match state.pop_or_park():                      // pop_front, else idle += 1 (or None when closing)
        Popped(s)  → if conn has a socket: idle_check (try_read) → Clean | Dirty(close socket); return s
        Closing    → return None
        Parked     → match park(&shard.idle, conn.socket()).await:
                        Notified → continue               // the notifier already did idle -= 1
                        Dirty    → state.unpark_dirty()   // idle -= 1 (not notified: we still hold our place)
                                   conn.close(); exit the task (live -= 1) unless the queue is non-empty
```

`park` is one `poll_fn` over the pinned `Notified` (polled first) and `TcpStream::poll_read_ready` (only while a socket
is held): `Notified` ready → `Notified`; read-ready → `try_read` into the read buffer: `WouldBlock` (a stale readiness
flag, e.g. after a read that exactly filled the buffer; the call clears it) → keep parking; `Ok(0)`, `Ok(n)` or an
error → `Dirty` (the socket is closed; the bytes are discarded, never parsed). Single-threaded, so `notify_last` has already marked this task's waiter when it runs; polling `Notified` first
keeps the idle count exact. A worker that closed its socket after an exchange (`Connection: close` servers) reconnects in
place when the queue has work, so no task is spawned per reconnect; with no work it exits and frees its buffer.

**Connect failure policy:** the worker settles its own request with the mapped error. If it was the last live worker
(`live == 1` before retiring), every queued request fails with the same error (a down server fails fast; parity with
undici `client.js` `onError` 472–493). Otherwise the queue stays for the live connections. **[author's call]**, §12
item 4.

### 8.6 The exchange (`h1/connection.rs`, State pattern §13)

`Exchange` holds a `WriteHalf` and a `ReadHalf` and is driven by one `poll_fn` over `TcpStream::split()` halves
(borrowed, zero cost; https://docs.rs/tokio/1.54.1/tokio/net/struct.TcpStream.html#method.split). Each poll: if the
write half is not `Done`, poll it; then poll the read half. The read half is polled even while writing, so an early
final response is seen (RFC 9112 §9.5).

- `WriteHalf`: `Sending { sent }` → `Done`. Before the exchange, `write_request(&mut wbuf, &parts, template)` builds
  head + body in one reused `Vec` (Task 5; capacity reused, no allocation in steady state); `poll_write` until all
  bytes are sent (tokio clears write readiness on a short write, `poll_evented.rs` 240–267). A write error moves the
  half to `Failed(err)`: the exchange keeps reading (an early response may already be in the receive buffer, RFC 9112
  §9.6) and maps the error only when the read half ends without a response.
- `ReadHalf`: `Head { msg_start }` → `Body { head, end }` (Content-Length) / `ToEof { head }` (close-delimited) →
  `Done`. Reads use `AsyncRead::poll_read` into `ReadBuf::new(&mut rbuf[filled..])` (the buffer is initialised once,
  `len == capacity`, so no `unsafe` and the short-read clear applies). `Interim { consumed }` → `msg_start += consumed`
  (the bytes stay in the buffer; no shifting). `Complete(head)` → framing: `ContentLength(n)` → `end = msg_start +
  head_len + usize::try_from(n)` (checked; > `max_response_bytes` → `TooLarge(ResponseBody)` at head time, no
  allocation); `Empty` → `end = msg_start + head_len`; `CloseDelimited` → read to EOF; `Chunked` → `Protocol(Other)` and
  close (`// removal: P1a chunked FramingDecoder (#issue)`).
- **No wasted reads:** the read half stops polling as soon as `filled >= end`; it never reads "until `WouldBlock`".
- **Growth:** when `filled == rbuf.len()` and more is expected, `grow` reserves `min(rbuf.len(), remaining_expected,
  max_response_bytes - rbuf.len())` with `try_reserve` (failure → `Engine(ResourceExhausted)`) and zero-extends:
  doubling, never sized by `Content-Length` (design §11.3), O(n) total. Close-delimited bodies grow the same way up to
  `max_response_bytes` (→ `TooLarge(ResponseBody)`).
- **Early final response:** if the read half completes a final head while the write half is still `Sending`, stop
  writing; deliver the response after its body; the connection is not reused (§5.1).

### 8.7 EOF accounting, reuse and close rules, error mapping

Every exchange starts with an empty read buffer (a connection with leftover bytes is never reused), so "response bytes
received for this exchange, interim heads included" is simply `filled > 0`.

**Reuse** only when all hold: write half `Done`; read half `Done` with `head.keep_alive` (the parser folds `Connection:
close` in any head incl. 1xx (sticky), HTTP/1.0 without keep-alive, close-delimited, 204/304 with `Transfer-Encoding` or
`Content-Length > 0`); `filled == end` (no leftover); no error. **Close** otherwise, and additionally on: an early final
response; `Chunked` (P0); a dirty idle check (I1) or idle readiness (I4).

| Condition | Outcome |
|---|---|
| connect `ConnectionRefused` | `Network(Refused)` |
| connect `HostUnreachable` / `NetworkUnreachable` | `Network(Unreachable)` (brief: `Other`; §12 item 6) |
| connect, any other error (incl. `set_nodelay` failure) | `Network(Other)` |
| EOF, `ConnectionReset`, or write error (`BrokenPipe`, `ConnectionReset`) with **0 response bytes** | `Network(ClosedBeforeResponse)` |
| EOF after ≥ 1 byte, before the final head ends (interim heads included: `interim-then-eof`) | `Protocol(MalformedResponse)` |
| EOF in a Content-Length body | `Protocol(IncompleteBody)` |
| EOF in a close-delimited body | success; close |
| `ConnectionReset` after ≥ 1 response byte | `Network(Reset)` |
| other read/write I/O errors | `Network(Other)` |
| parser errors | as returned (`Protocol(*)`, `TooLarge(Headers)`, `Engine(Internal)`) |
| `Content-Length` > `max_response_bytes`, or a close-delimited body past it | `TooLarge(ResponseBody)` |
| `Framing::Chunked` (P0 only) | `Protocol(Other)` (removal: P1a) |
| `try_reserve` failure; a `Submit` range outside `req` | `Engine(ResourceExhausted)`; `Engine(Internal)` |
| worker dropped without completing (runtime dropped) / unwinding from its own panic / shard poisoned | `Aborted(EngineDropped)` / `Engine(Panic)` / `Engine(Poisoned)` (§8.9) |

One mapping function (`map_io(err, received: bool) -> ErrorCode`) serves reads and writes; the `ClosedBeforeResponse`
row is the fact the P1a stale-connection re-send rule keys on (design §5.1), so the worker also records "reused
connection" and "bytes written" now (two fields on the in-flight guard; used by P1a, set by tests only through outcomes).

### 8.8 Wake budget (argued, and counted in §6)

- Under load (FIFO non-empty): a finishing connection pops the next request inside the same task poll → **0 task wakes
  per request**; its socket wakes it (I/O readiness, excluded by §5.1).
- Low load: submit → (cross-thread) command loop wake = **1 per batch**; `notify_last` → the parked connection = **1
  per request**; completion → the sink's own wake (Node: TSFN on empty → non-empty; bench: its channel) = **1 per batch**
  on the consumer side.
- No other wakes: no timer per request (P1a timer heap), no oneshot, no per-request task, coop self-yield at most every
  ≈ 60 back-to-back exchanges.

### 8.9 Settle exactly once, panics and shutdown (§11.3–§11.4)

- `InFlight` guard (owned by the worker for one request): holds the `Submit` until `settle` moves it into the
  `Completion`; if dropped first, `Drop` pushes one completion: `Engine(Poisoned)` if the shard is poisoned,
  `Engine(Panic)` if `std::thread::panicking()` (the worker's own panic, caught by tokio's task harness, §5 item 11),
  else `Aborted(EngineDropped)` (runtime or `LocalSet` dropped). Then `pending -= 1` and `live -= 1` via `try_borrow_mut`
  (a failed borrow marks the shard poisoned). Single-threaded, so "moved out or dropped" is the one transition; no
  atomic is needed.
- `ShardGuard` (owned by the command-loop future): if dropped before the loop returns, it marks the shard poisoned when
  unwinding (otherwise dropped) and fails every queued `Submit` with `Engine(Poisoned)` / `Aborted(EngineDropped)`.
  Order on a shard-thread panic: the `block_on` future unwinds first (sets poisoned, fails the queue), then the
  runtime drops the workers (their guards report `Engine(Poisoned)`), so every pending key settles.
- `CompletionSink::push` must not panic (it runs inside `Drop` during unwinding; a panic there aborts). Node's sink uses
  `parking_lot` (no poisoning).
- The design's `catch_unwind` requirement is met by tokio's task harness for workers; the command-loop future runs
  under `block_on`, whose panic ends the shard thread (the "kills a shard thread" scope of §11.3).

### 8.10 Buffer pool (`buf.rs`)

`BufPool { free: Vec<Vec<u8>>, size: usize, max: usize }`, `max = 2 × max_connections`. `take()`: `pop()` (LIFO, warm
cache) or a new buffer (`try_reserve_exact(size)` then `resize(size, 0)`; failure → `Engine(ResourceExhausted)`).
`put(buf)`: accept only `buf.capacity() == size`; re-extend to `len == size` if the consumer truncated it; drop when
full or when the capacity differs (grown buffers are not pooled). Reclaimed buffers from `push` go through `put`. Memory:
pool ≤ `2 × max_connections × read_buf_size` (50 connections: 6.25 MiB); handed-off buffers in flight ≤ pending.
Shrink-after-idle (§11.1 burst recovery) arrives with P1a timers.

### 8.11 Running it

- `shard(cfg, sink)`: builds the channel, `Rc<Shard>` and returns `(handle, command_loop_future)`. The caller runs the
  future on a `LocalSet` (`LocalSet::run_until` / `spawn_local`) or a `LocalRuntime` — engine-bench's symmetric harness
  (§16.2) drives its sink on the same runtime, so no cross-thread wake is charged to us alone.
- `spawn_shard_thread(cfg, sink)`: creates the channel and handle on the caller, then `std::thread::Builder::new()
  .name("blazehttp-shard-0")` moves `rx`, `cfg`, `sink` into the thread, which builds `Builder::new_current_thread()
  .enable_io().build_local(LocalOptions::default())`, reports success or the build error back through a
  `std::sync::mpsc::sync_channel(1)` (one blocking hand-shake at start-up), and `block_on`s the command loop. The thread
  exits when every handle is dropped and the queue has drained.
- Shard thread runtime: default `event_interval` (61); no blocking pool use in P0 (no DNS).

### 8.12 TCP options

`TCP_NODELAY` on every connection (`set_nodelay(true)`, docs.rs `TcpStream::set_nodelay`): requests are written in one
call and Nagle would delay small writes behind delayed ACKs. No `SO_KEEPALIVE` (the idle TTL in P1a covers it), no
`SO_RCVBUF`/`SO_SNDBUF` tuning (kernel autotuning), no `SO_LINGER` (deprecated in tokio; RST only in tests via
`set_zero_linger`).

## 9. Interface changes vs the brief (justified)

1. **`ResponseData.buf: Vec<u8>` → `bytes: ResponseBytes<'a>`, `Completion<'a>`, `push(&self, Completion<'_>, …)`.** The
   brief's exact-size copy is one allocation per small response (G0 zero allocations; and §4 step 4's rule against
   allocating on one thread and freeing on another). Measured 0 vs 1 allocations (§6).
2. **Pending bound by `Arc<AtomicUsize>` over an unbounded channel**, not a bounded `mpsc::Sender`. A bounded channel
   only bounds messages not yet received; the FIFO behind it would be unbounded, or the bound would be 2×. The counter
   bounds submit → completion exactly, and P1a's `Cancel`/`Credit` must never fail with `QueueFull`.
3. **`ShardConfig.host: String` → `template: HeaderTemplate`** (validated by Task 5's `HeaderTemplate::new` at the
   caller), so `shard()` has no new error path; **`max_response_bytes`** added because every buffer must be bounded (G6,
   §11.3) before the P1a budget exists.
4. **`spawn_shard_thread` builds a `LocalRuntime`** (stable since 1.51.0) instead of `current_thread` + `LocalSet`
   (design §11.3 already allows it); `shard()` still runs on a caller `LocalSet`.
5. **`ShardCommand` dispatches with a `match`; the `Execute` trait (design §11.3) arrives with the second command
   (P1a)** — one implementation does not need a trait.
6. **Error mapping additions:** `Network(Unreachable)`, `Network(Reset)` after ≥ 1 byte, write errors treated like EOF
   when no response byte arrived (§8.7).

## 10. Test plan

### 10.1 Brief tests (`tests/shard_loopback.rs`, in-test tokio server per scenario)

All kept as written except where noted:
1. `get_round_trip_returns_status_head_body` (status, head range, body bytes).
2. `keep_alive_reuses_one_connection` (100 sequential, `max_connections = 1`, exactly 1 accept).
3. `never_exceeds_max_connections` (200 concurrent, `max_connections = 8`, peak simultaneous connections ≤ 8).
4. `connection_close_reconnects`.
5. `leftover_bytes_close_connection` — the server writes the response and junk **in one write** (parser vector
   `two-responses-one-read`; also curl test1479 `http11-then-http09`: the junk is an HTTP/0.9-style body and is never
   parsed); next request on a new connection, correct response.
6. **`server_closed_idle_connection_settles_with_network_error` is split**, because with idle detection (I4) the second
   request no longer meets the dead connection — the test would be racy:
   - 6a `idle_connection_closed_by_server_is_not_reused`: the server closes after the first response; the test waits
     50 ms; the second request succeeds on a new connection (2 accepts).
   - 6b `close_after_request_settles_closed_before_response`: the server reads the second request and closes without
     answering → `Network(ClosedBeforeResponse)` within 1 s; the third request succeeds on a new connection.
7. `eof_mid_body_settles_incomplete_body`.
8. `refused_connect_settles_refused` (bind, take the port, drop the listener).
9. `queue_full_fails_fast` (`queue_capacity = 4`, shard future not polled; the 5th `try_submit` → `QueueFull { Global,
   4 }`; the first four complete once the future runs).
10. `each_key_completes_exactly_once` (10,000 requests, `max_connections = 50`; no duplicate, no missing key).
11. `small_response_copied_large_handed_off`: a 1 KiB response arrives as `Borrowed`; a response ≥ ¼ of
    `read_buf_size` arrives as `HandedOff(buf)` with `buf.capacity() == read_buf_size`.
12. `reclaimed_buffer_is_reused` (the next large completion's `as_ptr()` equals the returned buffer).
13. `request_buffer_returned_in_completion` (`req.as_ptr()` equals the submitted one).

### 10.2 Parser-note hand-off vectors (connection layer)

- `two-responses-one-read` → test 5. `http11-then-http09` → test 5.
- `extra_response_while_idle_is_discarded`: the server sends a complete extra response 20 ms after the first; the client
  submits 100 ms later → new connection, the correct response (2 accepts).
- `early_final_response_stops_writing_and_delivers`: POST with an 8 MiB body; the server reads only the head, answers
  `413` with `Connection: close` and stops reading → completion with status 413 (not a write error), connection closed.
- `content_length_without_body_settles_incomplete_body`: `Content-Length: 67108864` (64 MiB, below
  `max_response_bytes`), EOF after the head → `Protocol(IncompleteBody)`; a unit test asserts that `grow` never
  reserves more than `min(len, remaining, limit)`.
- `cl-1tib-no-body`: with P0's `max_response_bytes`, `Content-Length: 1099511627776` now fails at head time with
  `TooLarge(ResponseBody)` (design §11.1 fail-fast), still with no allocation — the parser note's `IncompleteBody`
  expectation holds only below the limit (§12 item 9).
- `interim_then_eof_settles_malformed_response` (`103` head, then EOF).
- `interim-flood-timer`, `tls-close-delimited-*`: P1a (timers, TLS).

### 10.3 New tests

- `eof_mid_head_settles_malformed_response`; `reset_before_response_settles_closed_before_response` and
  `reset_mid_body_settles_reset` (server `set_zero_linger()` then drop — never the deprecated `set_linger`).
- `interim_heads_are_skipped` (`100`, `103` then `200`); `connection_close_in_interim_head_closes_after_exchange`.
- `no_content_with_content_length_closes` (204 + `Content-Length: 5`) and `not_modified_with_content_length_closes`;
  `head_response_with_content_length_reuses`.
- `http10_without_keep_alive_closes`; `http10_with_keep_alive_reuses`.
- `close_delimited_body_completes_on_eof_and_closes`; `close_delimited_body_over_limit_settles_too_large`.
- `chunked_settles_protocol_other_and_closes` (removal: P1a).
- `content_length_over_limit_fails_at_head_time`.
- `dropping_handles_drains_queue_then_future_returns`.
- `dropping_runtime_settles_every_pending_key_once` (`Aborted(EngineDropped)`; no panic injection needed).
- `last_connect_failure_fails_queued_requests` (refused with 10 queued → 10 × `Refused`, one connect attempt per live
  worker).
- `spawn_shard_thread_round_trip` (the `LocalRuntime` path).
- Unit tests: `ShardState::enqueue` plan (wake/spawn counts, idle decrement), `pop_or_park`/`unpark_dirty` accounting,
  `BufPool` (cap, wrong capacity rejected, truncated buffer re-extended), `map_io` table, the reuse decision table,
  `grow`, `try_submit` counter (undo on full and on closed).
- Deferred to `engine-bench` (Tasks 7/8): allocations per request (needs a counting `GlobalAlloc`, i.e. `unsafe`, which
  `engine-core` tests must not contain) and wakes per request.
- Coverage ≥ 90% (`cargo llvm-cov nextest -p engine-core --fail-under-lines 90`).

## 11. Performance plan and P1a hooks

**Budget.** The shard path (submit → completion, plaintext 1 KiB, c = 50) must be ≤ 1.10× the raw loop's CPU µs per
request (§16.2 ceiling) and ≤ hyper `client::conn` in the same run (P0 acceptance, binding). Allocations per request in
steady state: 0. Task wakes per request: ≤ 1 + 1 per batch.

**How it is measured.** Tasks 7/8's G0 harness (`engine-bench`, Linux reference container, nightly window): the same
three clients as §6 (raw loop, our shard through a channel sink on the same runtime, hyper with a minimal pool), CPU µs
per request at the fixed offered load, a counting allocator on the client thread, and wake counters (counting the
command-loop polls, `notify_last` calls and sink wakes — the three counters the prototype used). Cells c = 1, 50, 500.
Also `strace -c`/`perf stat -e syscalls` per request to confirm 1 `write` + 1 `read` per request and 0 extra reads.

**Known risk — c = 1.** The prototype's command-loop hop added roughly one local wake and one task poll per request at
c = 1 (dev: 25.1 vs 16.6 µs raw, 20.6 µs hyper). If the G0 c = 1 cell misses non-inferiority, the upgrade path is to let
a parked connection hold the channel receiver (a "receiver baton": the command loop exists only while every connection
is busy), removing that hop (`ideas.md`).

**P1a hooks (designed now, not built):**
- Timeouts: each in-flight guard gets a timer-heap slot index; the worker's `poll_fn` also checks an "expired" flag set
  by the shard's single timer task, which wakes the worker's stored waker. Phases map to `WriteHalf`/`ReadHalf` states
  (`idleSend` in `Sending`, `headers` in `Head` — interim heads never refresh it, `idleBody` in `Body`).
- Stale-connection re-send: the guard's `reused` and `bytes_written` fields plus `ClosedBeforeResponse` decide it; the
  `Submit` is still owned by the guard, so a re-send is a `push_front` onto the queue.
- `Cancel`: a key → queue position map is not needed in P0; P1a adds a generational slab of entries with an intrusive
  FIFO (O(1) cancel), replacing the `VecDeque`.
- `Poison`: the `ShardGuard` already fails the queue; P1a routes the command to it.
- Budget: `BufPool::take` charges `read_buf_size`; a handed-off buffer's charge is released at hand-off to JS (§4 step 6).
- `io_uring` (research only, §7): tokio supports it only for files under `tokio_unstable` (changelog 1.54.0 "fs: fall back
  when `io_uring` has no IO driver"), and Docker's default seccomp profile blocks it (design §7). The design is
  compatible with completion-based I/O because every buffer is owned by exactly one connection or completion at a time.

## 12. Proposed design changes for the owner (not applied to the design doc)

1. **§5.1 idle detection:** record "parked connections keep read interest (I4) and a checkout `try_read` (I1) closes on
   any byte or EOF; the window after the last reactor turn remains". **Decision needed with Linux G0 data:** add a
   forced `recv(MSG_PEEK)` per reuse (I2, as h2o and Bun do; parity with undici's CVE-2026-6733 fix, which pays a loop
   turn per reuse) if it costs ≤ 2% CPU per request in the reference cell; otherwise keep I1 + I4 and list the window in
   §18. (`socket2` is already in the tree through tokio's `net` feature.)
2. **§4 step 6 / §10:** a small response (< ¼ of the read buffer) is copied **by the sink into its arena** from a
   borrowed slice, not into a per-response `Vec`; `CompletionSink::push` takes `Completion<'_>`; §11.5's "Completion
   push" row gains "+ a memcpy of ≤ 16 KiB".
3. **§4 step 4 / §11.5:** the shard thread's data queue is unbounded and the **pending count is an atomic per shard**
   (submit → completion), so data-plane control commands (`Cancel`, `Credit`) never fail on a full queue.
4. **§5.1 (new):** when the last live connection to an origin fails to connect, every queued request for it fails with
   that error (undici parity); with live connections, the queue waits for them. **[author's call]**
5. **§11.3:** "`LocalRuntime` may replace the pair" → "shard threads use `LocalRuntime` (stable since tokio 1.51.0)".
6. **§8 / brief:** connect `HostUnreachable`/`NetworkUnreachable` → `NETWORK/unreachable`; a reset after ≥ 1 response
   byte → `NETWORK/reset`; a write error before any response byte (after checking for an early response) →
   `NETWORK/closed-before-response`.
7. **Brief test 6** split into 6a/6b (§10.1).
8. **§5.2 304 + `Content-Length` > 0:** keep "not reused" — the post-checkout window (item 1) still exists. Revisit
   after item 1's decision.
9. **Parser note §12 `cl-1tib-no-body`:** with a response-size limit the expected outcome is `TOO_LARGE/response-body`
   at head time (design §11.1 fail-fast); `PROTOCOL/incomplete-body` applies below the limit.
10. **RFC 9112 §9.2 CRLF exception:** we close on idle CRLF too (stricter; avoids a special case). **[author's call]**
11. **§9.4 / §18:** the command-loop hop is a measured low-load cost (dev c = 1); carry it into the G1-low risk row with
    the "receiver baton" mitigation, decided with G0 c = 1 data.
12. **§13.1:** the shard is a directory module from the start (`shard/{mod,state,worker,guard}.rs`).

## 13. Sources

Official Rust and crate documentation (versions locked or pinned; read 2026-11-07, verified):
- tokio 1.54.1 docs: `TcpStream` (`try_read`, `poll_read_ready`, `split`, `set_nodelay`, `set_zero_linger`)
  https://docs.rs/tokio/1.54.1/tokio/net/struct.TcpStream.html · `LocalRuntime`
  https://docs.rs/tokio/1.54.1/tokio/runtime/struct.LocalRuntime.html · `Builder`
  https://docs.rs/tokio/1.54.1/tokio/runtime/struct.Builder.html · `LocalSet`
  https://docs.rs/tokio/1.54.1/tokio/task/struct.LocalSet.html · `spawn_local`
  https://docs.rs/tokio/1.54.1/tokio/task/fn.spawn_local.html · `Notify`
  https://docs.rs/tokio/1.54.1/tokio/sync/struct.Notify.html · `mpsc::Receiver::recv_many`
  https://docs.rs/tokio/1.54.1/tokio/sync/mpsc/struct.Receiver.html · `mpsc::Sender::try_send`
  https://docs.rs/tokio/1.54.1/tokio/sync/mpsc/struct.Sender.html · `task::coop`
  https://docs.rs/tokio/1.54.1/tokio/task/coop/index.html · `yield_now`
  https://docs.rs/tokio/1.54.1/tokio/task/fn.yield_now.html — and the 1.54.1 source files cited in §5.
- tokio issue #5866 (clear readiness on short reads): https://github.com/tokio-rs/tokio/issues/5866.
- mio 1.2.4 source (`sys/unix/selector/{epoll,kqueue}.rs`), crates.io tarball.
- Rust std 1.99.1: https://doc.rust-lang.org/1.99.1/std/cell/struct.RefCell.html,
  https://doc.rust-lang.org/1.99.1/std/rc/struct.Rc.html, https://doc.rust-lang.org/1.99.1/std/collections/struct.VecDeque.html,
  https://doc.rust-lang.org/1.99.1/std/vec/struct.Vec.html, https://doc.rust-lang.org/1.99.1/std/task/struct.Waker.html,
  https://doc.rust-lang.org/1.99.1/std/panic/fn.catch_unwind.html; Clippy 1.99 lint list
  https://rust-lang.github.io/rust-clippy/rust-1.99.0/index.html.
- socket2 0.6.5 `SockRef`: https://docs.rs/socket2/0.6.5/socket2/struct.SockRef.html.

Reference sources (exact tags in §3; read 2026-11-07, verified): hyper 1.12.0 `src/proto/h1/{conn,dispatch,io}.rs`,
`src/client/dispatch.rs`; hyper-util 0.1.21 `src/client/legacy/{pool,client}.rs`; reqwest 0.13.5
`src/async_impl/client.rs`, `src/connect.rs`; https://github.com/golang/go/blob/go1.27.3/src/net/http/transport.go,
`request.go`; https://github.com/nodejs/undici/blob/v8.12.4/lib/dispatcher/client-h1.js, `client.js`, `pool.js`;
https://github.com/valyala/fasthttp/blob/v1.77.0/client.go; https://github.com/curl/curl/blob/curl-8_23_0/lib/conncache.c,
`lib/cf-socket.c`; https://github.com/h2o/h2o/blob/f35c735ebe0657235702c2b91912542052cc05db/lib/common/socketpool.c,
`lib/common/http1client.c`; https://github.com/boostorg/beast/blob/boost-1.92.0/example/http/client/async/http_client_async.cpp,
`include/boost/beast/http/impl/read.hpp`; https://github.com/oven-sh/bun/blob/bun-v1.4.4/src/http/HTTPContext.rs,
`packages/bun-usockets/src/bsd.c`.

Advisories and issues (read 2026-11-07, verified): GHSA-35p6-xmwp-9g52 / CVE-2026-6733
https://github.com/advisories/GHSA-35p6-xmwp-9g52; undici #5493 https://github.com/nodejs/undici/issues/5493, #5600
https://github.com/nodejs/undici/issues/5600, PR #5606 https://github.com/nodejs/undici/pull/5606; hyper #2136
https://github.com/hyperium/hyper/issues/2136; Go #32310 https://github.com/golang/go/issues/32310, #22158
https://github.com/golang/go/issues/22158; h2o issue #679 (cited in `socketpool.c`, not read).

RFCs (verified): RFC 9112 §9.2, §9.3.1, §9.5, §9.6 https://www.rfc-editor.org/rfc/rfc9112.

Research and engineering writing (read 2026-11-07):
- PortSwigger, "Response queue poisoning" https://portswigger.net/web-security/request-smuggling/advanced/response-queue-poisoning
  — verified (page read); J. Kettle, "HTTP/2: The Sequel is Always Worse" (2021) https://portswigger.net/research/http2
  — verified (page reachable; origin of the term); "Browser-Powered Desync Attacks" (2022)
  https://portswigger.net/research/browser-powered-desync-attacks — verified in the parser note §4.2.
- Cloudflare, "How we built Pingora" (2022)
  https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/ — verified (text
  read): per-worker pools lowered connection reuse; sharing across threads raised one customer's reuse ratio from 87.1%
  to 99.92%. For us: one pool per profile on its shard thread, never split per worker.
- tokio blog, "Making the Tokio scheduler 10x faster" (2019) https://tokio.rs/blog/2019-10-scheduler and "Reducing tail
  latencies with automatic cooperative task yielding" (2020) https://tokio.rs/blog/2020-04-preemption — pages reachable;
  background for the LIFO slot and the coop budget, whose current values are verified in source (§5 items 9–10).
- fasthttp README "Tricks with `[]byte` buffers" / zero-allocation reuse — unverified here (behaviour verified in
  `client.go` instead).
