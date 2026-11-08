# Request writer comparisons

Reproduces the comparison numbers of `docs/notes/request-writer.md` §8.2/§8.4; results go to `docs/perf/journal.md`.

`hyper-emul/` (its own Cargo workspace, never an engine dependency) runs blazehttp's `write_request` and its copy floor
against an emulation of hyper 1.12's request encoder over `http` 1.5.0, on the same request sets as
`crates/engine-core/examples/h1_write_bench.rs`: `cargo run --release --manifest-path bench/writer/hyper-emul/Cargo.toml`.
Three hyper variants, each checked to write exactly our bytes:

- **construct+encode** (the headline, equal work): per request, parse the target, clone the defaults-only map, build
  each per-request field with `from_bytes`, insert `content-length` (and `connection: close` when due), encode. It
  validates as much per call as our writer except our forbidden-name, singleton, line-count and authority checks.
  Compare it with our writer.
- **clone+encode**: clones an already-validated map and never re-parses the target; a lower bound on hyper's cost.
- **encode-only**: hyper's serialisation floor with no validation; compare it with our copy floor, never with our
  writer.

`go/` benchmarks Go 1.27.3's `(*Request).Write` on the `get` and `post1k` requests (the body reader is reset in place,
so every reported allocation is `Request.Write`'s): `docker run --rm --cpus=1 -e GOMAXPROCS=1 -v
"$PWD/bench/writer/go:/b" -w /b golang:1.27.3-bookworm go test -run '^$' -bench . -benchmem -count 5`.

`-- --verify` on either Rust binary runs only the byte-equality checks (CI does, with one Go iteration). The Linux
numbers come from `rust:1-bookworm` / `golang:1.27.3-bookworm` containers with `--cpus=1`; they are dev numbers
outside the nightly window unless the journal says otherwise. The owner's goal is to beat every writer; the
cross-language writer race is Task 5c.
