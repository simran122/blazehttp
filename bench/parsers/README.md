# Cross-language response-head parser bench

How fast is blazehttp's HTTP/1.1 response-head parser (`engine_core::h1::ResponseParser`) against the fastest head
parsers in Rust, C, C++, Zig and Go, parsing the same bytes on the same CPU? This is a **micro-benchmark of one
component** (design §7, §16.2); it says nothing about whole clients (that is the G0 harness, epic E7).

```sh
pnpm bench:parsers                        # build every image, gate, time, print tables, write JSONL
node bench/parsers/run.ts --rounds 10     # more rounds; --run-ms, --feed-rounds, --out, --no-build
```

Output: a markdown table per section on stdout and `bench/results/<date>-<arch>-dev-parsers.jsonl` (one `meta`
line with host, versions and method, then one line per parser × workload).

## Parsers

| id                      | language | API used (each parser's fastest idiomatic API)                | work per head                                            |
| ----------------------- | -------- | ------------------------------------------------------------- | -------------------------------------------------------- |
| `blazehttp`             | Rust     | `ResponseParser::new(Method::Get).parse(buf)`                 | validates every byte; offsets only; framing facts folded |
| `httparse`              | Rust     | `ParserConfig::parse_response_with_uninit_headers`, 100 slots | validates; slices into caller array; + same facts pass   |
| `picohttpparser`        | C        | `phr_parse_response`, 100 slots                               | validates; pointers into caller array; + same facts pass |
| `llhttp`                | C        | `llhttp_execute`, strict mode, only `on_headers_complete`     | state machine; facts built in                            |
| `beast-response-parser` | C++      | `http::response_parser<http::empty_body>::put`                | builds `http::fields` (allocates); facts built in        |
| `beast-basic-parser`    | C++      | `http::basic_parser<false>` subclass                          | one virtual call per field; facts built in               |
| `zig-std-http`          | Zig      | `std.http.HeadParser.feed` + `Client.Response.Head.parse`     | finds the end; splits lines; no token/value validation   |
| `go-net-http`           | Go       | `http.ReadResponse` on a reused `bufio.Reader`                | builds `*Response` + canonical `Header` map (allocates)  |

"Framing facts" are `Content-Length`, chunked and `Connection: close` → keep-alive: every parser produces them, natively
or through the same extraction pass (`rust/src/engines.rs`, `c/pico.c`). Versions are pinned in each Dockerfile
(tags, commits, SHA-256 of every download) and recorded per run from `/versions.txt` in each image.

## Fairness rules

- Same machine, one container per invocation, `docker run --cpus=1 --network=none`, the same workload files. The
  runner refuses to start while any other container is running (it never stops one).
- Same CPU baseline everywhere: generic Armv8-A on arm64 (Rust `-C target-cpu=generic`, C/C++ `-march=armv8-a`, Zig
  `-mcpu=baseline`, Go `GOARM64=v8.0`). Nobody gets `native`. x64 (x86-64-v2 or v3) is decided with the x64 host.
- Release flags per design §16.2: Rust release, fat LTO, `codegen-units = 1`; C/C++ `-O3 -flto`; Zig
  `ReleaseFast`; Go default.
- SIMD as upstream ships it for the target: picohttpparser's only SIMD path is SSE4.2 (x86), llhttp's NEON span runs
  only under `LENIENT_HEADER_VALUE_RELAXED` (not the strict default), httparse picks NEON at compile time, Zig's
  `HeadParser` uses `@Vector`.
- Limits raised where a default would reject the 60 KB feed inputs (Beast `header_limit`); nothing else is tuned.
- **Differential gate before timing:** every parser must report the same status, head length, `Content-Length`,
  field count and field name/value bytes as ours for every head of every workload; any mismatch aborts the run. Go's
  map is compared as a case-insensitive name multiset (values in order per name).
- Statistic: each round runs every parser once per workload, starting at a different parser each round; per
  invocation 100 ms warm-up, then blocks of 1,000 parses until `--run-ms` elapses; ns/head per round. Reported: median
  and min–max ns over rounds, and the median and min–max of the per-round ratio **ours / theirs** (below 1: ours is
  faster). At least 5 rounds (default 7).
- Feed inputs (a)–(d) are fed 1 byte per call (`docs/notes/response-parser.md` §8.1); each parser in its native
  partial-input mode: `incremental` (resumes at any byte), `line-restart` (keeps complete lines; a partial line is
  re-parsed from its start on every call), `restart` (re-parses from byte 0), `restart-skip` (re-parses once
  complete; meanwhile scans only new bytes for the end of the head).
- Results from the owner's Mac outside the nightly window are **dev** numbers, never gate results (design §16.1).

## Workloads

`workloads.ts` builds them from `tests/fixtures/h1-heads/` at start-up: `1k-3-headers` (109 B), `1k-20-headers`
(779 B), `rotated-64` (64 heads cycling over the three fixtures with digits varied by the rule of
`crates/engine-core/examples/h1_parse_bench.rs`, so no single head trains the branch predictor), and the four feed
inputs. Each workload file is the concatenation of its heads; a head ends at its first CRLF CRLF.

## Harness protocol

Every image's entry point is `harness <parser id> <mode> <workload file> [run ms]`:

| mode    | prints                                                                                                                                                          |
| ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `check` | per head `head <status> <head len> <content-length or -> <field count>`, then `field <hex name> <hex value>` per field; finally `allocs <allocations per head>` |
| `time`  | `ns <ns per head> <sink>`                                                                                                                                       |
| `feed`  | `feed <total ns> <incremental\|line-restart\|restart\|restart-skip> <head len>`                                                                                 |

Allocations are counted by a counting global allocator (Rust), `--wrap=malloc` (C), a replaced `operator new`
(C++), `runtime.MemStats.Mallocs` (Go); Zig's APIs take no allocator. `protocol.ts` parses this output and holds the
gate's comparison (`protocol.test.ts`).
