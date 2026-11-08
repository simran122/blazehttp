# Request writer comparisons

Reproduces the comparison numbers of `docs/notes/request-writer.md` §8.2/§8.4; results go to `docs/perf/journal.md`.
`hyper-emul/` (its own Cargo workspace, never an engine dependency) runs blazehttp's `write_request` against an
emulation of hyper 1.12's request encoder over `http` 1.5.0 on the same request sets as
`crates/engine-core/examples/h1_write_bench.rs`: `cargo run --release --manifest-path bench/writer/hyper-emul/Cargo.toml`.
`go/` benchmarks Go 1.27.3's `(*Request).Write` on the `get` and `post1k` requests:
`docker run --rm --cpus=1 -e GOMAXPROCS=1 -v "$PWD/bench/writer/go:/b" -w /b golang:1.27.3-bookworm go test -run '^$'
-bench . -benchmem -count 5`. The Linux numbers come from `rust:1-bookworm` / `golang:1.27.3-bookworm` containers with
`--cpus=1`; they are dev numbers outside the nightly window unless the journal says otherwise.
