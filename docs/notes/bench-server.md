# `bench-server`: research note (Task 9)

The fixed-response HTTP/1.1 server that every G0/G1/G2 benchmark cell runs against (design §16.2–16.3). Its only
requirement beyond correct bytes is that it is **never the bottleneck**: a run is discarded when the server uses ≥ 60%
of its CPUs (§16.2). Research written before any code (owner rule), access date of every source **2026-11-07**.
**Verified** = checked against the source code or docs at the locked version, or measured; **unverified** = taken from a
secondary source, not checked.

## 1. Summary and decision

- One tokio **multi-thread** runtime (`--threads`, default = available CPUs), one listener per port, one task per
  connection (design A, §4). Per-core `current_thread` runtimes with `SO_REUSEPORT` (design B) were measured against it
  and were not cheaper (§7.1), so A stays.
- Every response is a **pre-built `&'static [u8]`**, built once at start-up (`LazyLock`) from the shared fixtures
  (`tests/fixtures/h1-heads/{1k,1k-h20}.http` + `tests/fixtures/bodies/1k.json`). Per request the server does: find
  `\r\n\r\n`, take the target between the first two spaces, look it up in a 5-entry table, write the bytes.
- **Pipelining:** every complete request in a read is answered; the responses of one read go out in **one write** (one
  response → written straight from the static bytes, no copy; several → appended into a per-connection buffer).
- Bounded memory (G6): a fixed 16 KiB read buffer per connection; a request head that does not fit closes the
  connection. A `content-length` request body is skipped by counting (no buffering); a `transfer-encoding` request is
  refused (connection closed) instead of being mis-framed.
- TLS: rustls 0.23 with the **ring** provider (as the Rust competitors use, §16.2), a self-signed certificate generated
  by rcgen at start-up, ALPN `http/1.1`. The same connection code serves both ports (generic over the stream type).
- Interface (two changes from the plan's sketch, both for correctness): `serve(cfg, ready)` sends a `Bound` (the
  addresses actually bound and the TLS certificate) instead of `()`, so tests bind port 0 with no free-port race and
  a TLS client can verify the self-signed certificate; the thread count is a binary flag (`--threads`, it sizes the
  runtime `main` builds), not a `ServerConfig` field `serve` would ignore. The binary prints
  `ready plain=<addr> [tls=<addr>]` when it accepts connections.
- Verification that the server is not the bottleneck: §7 (dev, Docker on the owner's Mac, `--cpus=4`, wrk at c = 50),
  and in every harness run the server container's cgroup `cpu.stat` (§6).

## 2. How the fastest fixed-response servers work

| Server | What makes it fast | Status |
|---|---|---|
| **may-minihttp** (Rust, TechEmpower plaintext leader for years) — `src/http_server.rs` | Per-connection 32 KiB request and response buffers; reads until `WouldBlock`, then **decodes every complete request in the buffer** (pipelining), appends each encoded response to one buffer and writes it **once** per read batch; a per-request write was removed (comment: needs `TCP_NODELAY`). Coroutine runtime with one scheduler per core. | Verified (source, master, [1]) |
| **TechEmpower plaintext rules** | Requests are pipelined; the response must carry `Server` and `Date`; framework must compose the response per request (no fully pre-rendered payload) — **we deliberately do not follow this rule**: our server is a measurement fixture, not a contestant, so it pre-renders everything. | Verified ([2]) |
| **nginx `return code text`** | Static response from config, no file I/O; a common "fixed response" reference server. | Verified (docs, [3]) |
| **actix-web / Go fasthttp** | actix: one accept thread hands sockets to N single-threaded workers; fasthttp: worker pool + zero-allocation request parsing, reused buffers. | Unverified (from memory, not re-read) |
| **wrk / wrk2 reference servers** | wrk's own README points at nginx for a baseline; wrk2 adds constant-rate (open-loop) load with coordinated-omission correction (that is the harness's job, Task 10). | Unverified |

What we take: pre-built bytes (stronger than may-minihttp, which still encodes), batch-per-read writes, `TCP_NODELAY`,
no per-request allocation. What we do not copy: may-minihttp's coroutine runtime (`may`, stackful coroutines; a
non-standard runtime adds a dependency for a server that only needs to stay below 60% CPU); TechEmpower's
per-request composition rule (it would only add server CPU).

## 3. Building blocks at the locked versions

| Crate (locked) | API used | Source |
|---|---|---|
| tokio 1.54.1 | `runtime::Builder::new_multi_thread().worker_threads(n)`; `TcpListener::bind/accept`; `TcpStream::set_nodelay`; `AsyncReadExt::read`, `AsyncWriteExt::{write_all, flush}`; `sync::oneshot` | [4], [5]; registry source |
| tokio-rustls 0.26.6 (`default-features = false`, `ring`, `tls12`) | `TlsAcceptor::from(Arc<ServerConfig>)`, `.accept(tcp)`; client side in tests `TlsConnector` | registry source |
| rustls 0.23.46 | `ServerConfig::builder().with_no_client_auth().with_single_cert(vec![cert], key)`; `alpn_protocols`. With only the `ring` feature on, `builder()` uses ring as the process default provider. | registry source `src/server/builder.rs:65`, `server_conn.rs:471` |
| rcgen 0.14.10 (`default-features = false`, `ring`) | `generate_simple_self_signed(names) -> CertifiedKey { cert, signing_key }`; `cert.der()`, `signing_key.serialize_der()` (PKCS#8) | registry source `src/lib.rs:128` |
| flate2 1.1.11 | `write::GzEncoder::new(Vec::new(), Compression::default())`, `finish()`; header has `mtime = 0` and OS byte 255 unless set (`src/gz/mod.rs:446-462`), so the gzip bytes are deterministic for a given flate2 + miniz_oxide version | registry source |

rustls 0.23 is the current stable line (0.24 not released; the design's "0.24 migration checkpoint" is a P1a item).
Verified on crates.io (`max_stable_version`, 2026-11-07): tokio 1.54.1, tokio-rustls 0.26.6, rustls 0.23.46, rcgen
0.14.10, flate2 1.1.11.

Findings that shape the code (verified in source):
- **tokio-rustls `poll_write` returns as soon as the socket would block, with ciphertext still queued** in the rustls
  session (`src/common/mod.rs:278-308`), so every batch write is followed by `flush()` (a no-op on `TcpStream`).
- `TcpSocket::set_reuseport` exists in tokio on Unix (not Solaris/illumos/Cygwin/NuttX) — design B needs no `socket2`.
- No `memchr`: request heads from benchmark clients are 40–200 B, so a `windows(4)` scan is cheap, and the scan resumes
  from `len - 3` after a partial read so a byte-at-a-time peer costs O(n), not O(n²).

## 4. Designs compared

| | A: multi-thread runtime, shared listener | B: per-core `current_thread` + `SO_REUSEPORT` | C: thread per connection, blocking I/O |
|---|---|---|---|
| Model | N workers, work stealing, LIFO slot for woken tasks [4] | N OS threads, each its own runtime and listener; kernel spreads connections by hash [6] | one OS thread per connection |
| Per request | 1 `read` + 1 `write` syscall, ≤ 1 task wake; cross-thread steal possible | same syscalls; no cross-thread wake, no stealing atomics | same syscalls; a context switch per request |
| Balance | dynamic (stealing) | static per connection hash: at c = 50 over 4 listeners the busiest listener gets well above 12.5 connections; at c = 1 one core does all the work | OS scheduler |
| Portability | all platforms | Linux balances; macOS/BSD `SO_REUSEPORT` semantics differ (no balancing on macOS — unverified), so local runs would use one core | all |
| Memory | 16 KiB / connection + task | same | 16 KiB + a thread stack per connection; c = 500 → 500 threads |
| Code | smallest | + thread spawn, per-thread runtime, socket options | small but does not scale to c = 500 |

Time per request is O(head bytes) in every design (one scan, a 5-entry lookup); space O(1) per connection.

**Decision: A.** It is the simplest, balances at every connection count, and works the same on macOS and Linux. B's
expected gain (no stealing/atomics on wakes) did not show up when measured (§7.1: median 11.05 vs 10.17 µs CPU per
request), because 85% of the cost is the kernel's. C is rejected (c = 500 cells).

## 5. Request handling (exact)

1. Read into the connection buffer (16 KiB).
2. If bytes of a previous request body are still owed, drop them first.
3. Find `\r\n\r\n` from the resume point. None → keep reading (buffer full → close).
4. Target = bytes between the first and second space of the request line (no second space → close).
5. Header lines: `content-length` (case-insensitive name, digits only, else close) → body bytes to skip;
   `transfer-encoding` → close (chunked request bodies are not supported by this fixture; failing loudly beats
   mis-framing the following requests).
6. Look the target up in the cell table; unknown → the pre-built 404.
7. After the buffer has no complete request left, write the batch (one write), `flush()`, compact the leftover bytes.

`--stall`: the connection is accepted (TLS handshake done on the TLS port) and read until EOF, and nothing is ever
written — the G2 stalled-server cell.

## 6. Measuring server CPU from outside

cgroup v2 `cpu.stat` reports `usage_usec`, `user_usec`, `system_usec` for all processes in the cgroup, and the file
exists whether the cpu controller is enabled or not [7] (verified, kernel docs). The harness (Task 10) reads the server
container's `/sys/fs/cgroup/cpu.stat` before and after a run: server CPU% = Δ`usage_usec` / (wall µs × CPUs given).
§7 uses the same method by hand.

## 7. Headroom measurement (dev, not a gate run)

Method: Linux arm64 in Docker Desktop on the owner's M2 (8 CPUs for the VM), release build (fat LTO), server container
`--cpus=4` with `--threads 4`, wrk 4.1.0 (Debian bookworm package) in a second container `--cpus=3 -t3` sharing the
server's network namespace (loopback, so the Docker bridge is not measured), `GET /1k`, c = 50, 5 s warm-up then 20 s;
server CPU = Δ`usage_usec` of the server container's `cpu.stat` (§6) over the 20 s load window. **The host was not idle**
(load average 14–28 from other work on the same 8 CPUs, so wrk and the server competed with it): dev numbers only,
not a gate run.

### 7.1 Results (2026-11-07, verified by measurement)

| Run | req/s | Server CPU (of 4) | Server CPU µs / request | Kernel share |
|---|---|---|---|---|
| A, `/1k`, c = 50, round 1 / 2 / 3 | 116,713 / 150,225 / 107,359 | 29.8% / 36.7% / 28.7% | 10.17 / 9.74 / 10.64 | 85–86% |
| B (per-core, `SO_REUSEPORT`), same, round 1 / 2 / 3 | 127,381 / 143,549 / 78,128 | 35.4% / 36.5% / 26.8% | 11.05 / 10.13 / 13.67 | 87% |
| A, `/1k`, c = 50, Docker bridge network (load avg 13) | 110,364 | 42.7% | 15.43 | — |
| A, `/1k`, c = 500 | 140,021 | 47.4% | 13.50 | 85% |
| A, `/1k-h20`, c = 50 | 136,447 | 34.8% | 10.17 | 85% |
| A, HTTPS `/1k`, c = 50 | 85,833 | 31.7% | 14.75 | 70% |

Reading:
- **The load generator, not the server, was the limit in every run**: the server never exceeded 48% of its 4 CPUs.
- Server cost is ≈ 10 µs CPU per request on `/1k`, and **85% of it is kernel time** (loopback TCP `read`/`write`/epoll
  inside the Docker Desktop VM); our user-space work is ≈ 1.5 µs. At the 60% limit (2.4 CPU-s per second) that is
  ≈ 236,000 req/s of capacity on `/1k` on this host — above the ~200k target but not "well above"; the margin is set by
  the kernel, not by this code. The binding gate case is far below it: the reference cell's client has 1 CPU and
  targets ≥ 63,790 req/s, which costs the server ≈ 16% of 4 CPUs.
- B was not cheaper (median 11.05 vs 10.17 µs), so A stays (`docs/perf/journal.md`, 2026-11-07).
- The Docker bridge adds ≈ 50% server CPU per request (15.4 vs 10.2 µs): the harness (Task 10) should put the client in
  the server's network namespace (`--network container:<server>`) or on the host network, and must keep asserting
  server CPU < 60% per run (§16.2), since a contended host raises the cost per request.

## 8. Cells

All share `date: Fri, 06 Nov 2026 12:00:00 GMT` (pre-built, fixed) and the 847-byte payload
`tests/fixtures/bodies/1k.json` (the spike payload, byte-identical to `spike/g0/body.json`, moved into the shared
fixtures because `spike/` is deleted at the end of P0, §14.2).

| Path | Head | Body |
|---|---|---|
| `/1k` | `tests/fixtures/h1-heads/1k.http` (3 fields) | payload |
| `/1k-h20` | `tests/fixtures/h1-heads/1k-h20.http` (20 fields, parser note §8.3) | payload |
| `/1k-gzip` | `content-type`, `content-encoding: gzip`, `content-length`, `date` | payload gzipped at start-up (flate2, default level) |
| `/1k-chunked` | `content-type`, `transfer-encoding: chunked`, `date` | payload in 256-byte chunks (`100` ×3, `4f`) + `0\r\n\r\n` |
| `/1m` | `content-type: application/octet-stream`, `content-length: 1048576`, `date` | payload repeated to 1 MiB |
| anything else | `404 Not Found`, `content-length: 0`, `date` | none |

`bench/cells.json` (generated by `bench-server --print-cells`, checked by a test) lists per cell: path, response
bytes, header count and FNV-1a 64 of the **decoded** body (after chunked framing and gzip are removed — the bytes a
client's application sees, so every 1 KiB cell has the same checksum and every client checks it the same way, §16.2
"work per request").

A unit test proves the head builder plus the shared constants reproduce `1k.http` byte for byte, so the built heads and
the fixture cannot drift.

## 9. Sources (accessed 2026-11-07)

1. may-minihttp `src/http_server.rs` — https://github.com/Xudong-Huang/may_minihttp/blob/master/src/http_server.rs (verified, read)
2. TechEmpower framework tests overview (plaintext rules) — https://github.com/TechEmpower/FrameworkBenchmarks/wiki/Project-Information-Framework-Tests-Overview (verified, read)
3. nginx `ngx_http_rewrite_module` `return` — https://nginx.org/en/docs/http/ngx_http_rewrite_module.html#return (verified, read)
4. tokio 1.54.1 runtime docs — https://docs.rs/tokio/1.54.1/tokio/runtime/index.html (verified, read)
5. tokio 1.54.1 `TcpSocket::set_reuseport`, `TcpStream::set_nodelay` — https://docs.rs/tokio/1.54.1/tokio/net/struct.TcpSocket.html (verified in registry source)
6. socket(7) `SO_REUSEPORT` — https://man7.org/linux/man-pages/man7/socket.7.html (verified, read)
7. Linux cgroup v2 `cpu.stat` — https://docs.kernel.org/admin-guide/cgroup-v2.html (verified, read)
8. rustls 0.23.46 — https://docs.rs/rustls/0.23.46/rustls/server/struct.ServerConfig.html (verified in registry source)
9. tokio-rustls 0.26.6 — https://docs.rs/tokio-rustls/0.26.6/tokio_rustls/ (verified in registry source)
10. rcgen 0.14.10 — https://docs.rs/rcgen/0.14.10/rcgen/fn.generate_simple_self_signed.html (verified in registry source)
11. flate2 1.1.11 — https://docs.rs/flate2/1.1.11/flate2/write/struct.GzEncoder.html (verified in registry source)
12. TechEmpower Round 23 announcement — https://github.com/TechEmpower/FrameworkBenchmarks/issues/9589 (unverified: the results page was not opened)
