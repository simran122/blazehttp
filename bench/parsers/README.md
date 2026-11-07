# Cross-language response-head parser bench

How fast is blazehttp's HTTP/1.1 response-head parser (`engine_core::h1::ResponseParser`) against the fastest head
parsers in Rust, C, C++, Zig and Go, parsing the same bytes on the same CPU? This is a **micro-benchmark of one
component** (design §7, §16.2); it says nothing about whole clients (that is the G0 harness, epic E7).

```sh
pnpm bench:parsers                        # build every image, gate, time, print tables, write JSONL
node bench/parsers/run.ts --rounds 10     # more rounds; --run-ms, --feed-rounds, --cpuset, --cc, --parsers, --out, --no-build
node bench/parsers/run.ts --cc gcc-12 --parsers picohttpparser,llhttp,beast-response-parser,beast-basic-parser --out <file>
```

Output: a markdown table per section on stdout and `bench/results/<date>-<arch>-dev-parsers.jsonl` (one `meta` line
with host, load, container flags, compiler choice, versions, commit and the dirty-file list with a diff hash, then one
line per parser × workload). The runner refuses to overwrite an existing results file (`--out` names another).

## Parsers

| id                      | language | API used (each parser's fastest idiomatic API)                | work per head                                                  |
| ----------------------- | -------- | ------------------------------------------------------------- | -------------------------------------------------------------- |
| `blazehttp`             | Rust     | `ResponseParser::new(Method::Get).parse(buf)`                 | validates every byte; framing facts; field block as a range    |
| `blazehttp+fields`      | Rust     | `blazehttp` + `split_fields` into 100 name/value slots        | the above + per-field name/value slices, like httparse         |
| `httparse`              | Rust     | `ParserConfig::parse_response_with_uninit_headers`, 100 slots | validates; slices into caller array; + same facts pass         |
| `picohttpparser`        | C        | `phr_parse_response`, 100 slots                               | validates; pointers into caller array; + same facts pass       |
| `llhttp`                | C        | `llhttp_execute`, strict mode, only `on_headers_complete`     | state machine; facts built in                                  |
| `beast-response-parser` | C++      | `http::response_parser<http::empty_body>::put`                | builds `http::fields` (fields + 1 allocations); facts built in |
| `beast-basic-parser`    | C++      | `http::basic_parser<false>` subclass                          | one virtual call per field; facts built in                     |
| `zig-std-http`          | Zig      | `std.http.HeadParser.feed` + `Client.Response.Head.parse`     | finds the end; splits lines; no token/value validation         |
| `go-net-http`           | Go       | `http.ReadResponse` on a reused `bufio.Reader`                | builds `*Response` + canonical `Header` map (allocates)        |

**Which of our rows to compare:** our parser returns the field lines as one offset range; every other parser hands
the caller each field (slices, pointers, callbacks or a container). `blazehttp+fields` adds the zero-allocation
name/value split a consumer needs (`rust/src/engines.rs` `split_fields`, mirroring httparse's output), so it is the
row to compare with the others. The plain `blazehttp` row is what the engine itself pays when it only needs the
framing facts.

"Framing facts" are the length (`Content-Length`, chunked, or neither), and keep-alive: every parser produces them,
natively or through the same extraction pass (`rust/src/engines.rs`, `c/pico.c`). Versions are pinned in each
Dockerfile: base images by digest; Zig, picohttpparser, llhttp and Boost downloads by SHA-256 (with the llhttp and
Boost tag commits recorded); the Rust crates by `Cargo.lock`; the GCC package by its Debian version. Each image
records its versions and flags in `/versions.txt`, copied into the run's `meta` line.

## Fairness rules

- Same machine, one container per invocation: `docker run --cpus=1 --cpuset-cpus=<n> --network=none`, the same
  workload files. `--cpus=1` is a CPU-time quota; `--cpuset-cpus` (default `2`, `--cpuset`) also pins the container
  to one vCPU of the Docker VM. On Docker Desktop that vCPU is a host thread the macOS scheduler can still move
  between cores, so pinning removes in-VM migration, not host placement noise. The runner refuses to start while any
  other container is running (it never stops one).
- Same CPU baseline everywhere: generic Armv8-A on arm64 (Rust `-C target-cpu=generic`, C/C++ `-march=armv8-a`, Zig
  `-mcpu=baseline`, Go `GOARM64=v8.0`). Nobody gets `native`. x64 (x86-64-v2 or v3) is decided with the x64 host.
- Release flags per design §16.2: Rust release, fat LTO, `codegen-units = 1`; C/C++ `-O3 -DNDEBUG -flto=auto`; Zig
  `ReleaseFast`; Go default.
- C/C++ compiler: GCC 14.2 (Debian 13) by default, pulled through the ECR mirror of Docker Hub's official Debian
  image because Docker Hub pulls hung on the bench host. `--cc gcc-12` builds the Debian 12 / GCC 12.2 variant used by
  the first dev run, so the compiler's effect can be measured.
- SIMD as upstream ships it for the target: picohttpparser's only SIMD path is SSE4.2 (x86), llhttp's NEON span runs
  only under `LENIENT_HEADER_VALUE_RELAXED` (not the strict default), httparse picks NEON at compile time, Zig's
  `HeadParser` uses `@Vector`.
- Limits raised where a default would reject the 60 KB feed inputs (Beast `header_limit`); nothing else is tuned.
- Each harness calls its parser directly in the timed loop (no indirect call per head) and folds the facts into a
  printed sink so no work is optimised away.
- **Differential gate before timing:** for every head of every workload, every parser's timed `parse` path must
  report the input head's exact length, and the same status, length (number, chunked or none) and keep-alive as ours
  (keep-alive is not compared when the body runs to EOF: parsers differ on whether that alone ends reuse); its field
  view must give the same field count and name/value bytes. Go's map is compared as a case-insensitive name multiset
  (values in order per name) without the `Connection` and `Transfer-Encoding` fields it consumes into
  `resp.Close`/`resp.TransferEncoding` (checked through the facts instead). Any mismatch aborts the run.
- Statistic: each round runs every parser once per workload, starting at a different parser each round; per
  invocation 100 ms warm-up, then blocks of 1,000 parses until `--run-ms` elapses; ns/head per round. Reported: median
  and min–max ns over rounds, and the median and min–max of the per-round ratios **ours+fields / theirs** and **ours /
  theirs** (below 1: ours is faster). At least 5 rounds (default 7). Gate runs need design §16.4 (10 rounds,
  bootstrap 95% CI), which this dev bench does not implement.
- Feed inputs (a)–(d) are fed 1 byte per call (`docs/notes/response-parser.md` §8.1); each parser in its native
  partial-input mode: `incremental` (resumes at any byte), `line-restart` (keeps complete lines; a partial line is
  re-parsed from its start on every call), `restart` (re-parses from byte 0), `restart-skip` (re-parses once
  complete; meanwhile scans only new bytes for the end of the head).
- Results from the owner's Mac outside the nightly window are **dev** numbers, never gate results (design §16.1).

## Workloads

`workloads.ts` builds them from `tests/fixtures/h1-heads/` at start-up:

- timed: `1k-3-headers` (109 B) and `1k-20-headers` (779 B), each one head repeated; `1k-3-headers-x64` and
  `1k-20-headers-x64`, the 64 same-length variants of each that `crates/engine-core/examples/h1_parse_bench.rs` times
  (digits varied so no single head trains the branch predictor); `mixed-64` (64 heads cycling over the three
  fixtures, digits varied the same way);
- feed: the four 1-byte feed inputs;
- gate only: `gate-framing` (chunked, `Connection: close`, trailing OWS, an empty value, mixed-case names).

Each workload file is the concatenation of its heads; a head ends at its first CRLF CRLF.

## Harness protocol

Every image's entry point is `harness <parser id> <mode> <workload file> [run ms]`; bad arguments (including a
missing or non-positive run time) exit non-zero:

| mode    | prints                                                                                                                                                                                                           |
| ------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `check` | per head `head <status> <head len> <length: n, chunked or -> <keep-alive 0/1> <field count>` (from the timed parse path), then `field <hex name> <hex value>` per field; finally `allocs <allocations per head>` |
| `time`  | `ns <ns per head> <sink>`                                                                                                                                                                                        |
| `feed`  | `feed <total ns> <incremental\|line-restart\|restart\|restart-skip> <head len>`                                                                                                                                  |

Allocations are counted by a counting global allocator (Rust), `--wrap=malloc` (C), a replaced `operator new`
(C++), `runtime.MemStats.Mallocs` (Go); Zig's APIs take no allocator. `protocol.ts` parses this output, `gate.ts`
holds the gate, `report.ts` assembles the results; each has a `*.test.ts`. CI builds every image and runs a short
smoke run on `ubuntu-24.04-arm` (job `bench-parsers`).
