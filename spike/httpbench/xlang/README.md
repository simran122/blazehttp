# Cross-language HTTP client benchmark

Same method as `../dockerbench.sh`: Go server container (`--cpus=2`, `GOMAXPROCS=2`, `GET /data`, ~1 KB JSON),
each client in its own container with `--cpus=1`, sharing the server's network namespace (`127.0.0.1:3999`).
Workload for every client: 50 in flight, keep-alive, 3,000 warmup + 30,000 measured, parse JSON, check `id == 42`,
per-request latency. Clients run one at a time, 3 rounds interleaved, median by req/s.

    ./build.sh      # once; everything builds inside Docker (Zig 0.17.1 tarball is downloaded in a container)
    ./run.sh        # all clients, ROUNDS=3; or e.g. ./run.sh c zig bun
    # raw lines -> results.jsonl; median table printed at the end

| dir / key | client | concurrency model | JSON |
|---|---|---|---|
| `c/` `c` | libcurl multi, 50 reusable easy handles, `curl_multi_poll` loop, 1 thread | event loop | tiny hand-written scanner for top-level `"id"` (not a validating parser) |
| `cpp/` `cpp` | Boost.Beast + Asio C++20 coroutines, 50 keep-alive sockets, `io_context{1}` | event loop | Boost.JSON full parse |
| `zig/` `zig` | Zig 0.17.1 `std.http.Client`, shared client, 50 tasks via `Io.Group.concurrent` | **50 OS threads, blocking I/O** | `std.json` into struct |
| `fetch.mjs` `bun` | Bun `fetch` + `res.json()` (Bun's HTTP client is Zig) | event loop | JSON.parse (JSC) |
| `fetch.mjs` `deno` | Deno `fetch` + `res.json()` | event loop | JSON.parse (V8) |
| `rust` | `../rustclient` reqwest, tokio `current_thread` | event loop | serde_json into struct |
| `go` | `../goclient` net/http, `GOMAXPROCS=1` | 50 goroutines | encoding/json into struct |
| `undici`, `blaze` | `../bench2.mjs` in node:26 (undici Pool.request; blazehttp batched, `RUST_HTTP_THREADS=1`) | event loop | JSON.parse |
