# Request writer comparisons

Reproduces the comparison numbers of `docs/notes/request-writer.md` §8.2/§8.4; results go to `docs/perf/journal.md`.
`bench/writer/run.sh build BIN_DIR` compiles the three benches below in pinned containers (rustc 1.99.1 and Go 1.27.3
images by digest) and records the commit; `bench/writer/run.sh measure BIN_DIR LOG_DIR [CPUSET]` then runs the
prebuilt binaries one after the other (`--cpus=1` pinned to one vCPU, `--memory=2g`, no network). Measuring is a
separate step so that nothing else (no build, test, fuzz or download) runs during a timing; `measure` refuses to
start while any other container runs.

`crates/engine-core/examples/h1_write_bench.rs` times `write_request` against three baselines in one binary, in
rotated order (`crates/engine-core/tests/support/write_bench.rs`): `memcpy` of the finished request (the lower bound),
the unchecked appends (the same bytes as plain `extend_from_slice` calls; called the "copy floor" before 2026-11-13,
and not a lower bound) and the writer's own append shape without checks. `writer − shape` is what the checks cost.

`hyper-emul/` (its own Cargo workspace, never an engine dependency) runs `write_request` against an **emulation** of
hyper 1.12's request encoder over `http` 1.5.0, on the same request sets. No hyper code runs: the encoder is
re-implemented and checked against our bytes only; Task 5c replaces it with hyper itself. Variants, each checked to
write exactly our bytes:

- **construct+encode** (the headline): per request, parse the target, clone the defaults-only map, build each
  per-request field with `from_bytes`, insert `content-length` (and `connection: close` when due), encode. It
  validates as much per call as our writer except our forbidden-name, singleton, line-count and authority checks.
  Its input is `(name, value)` pairs; compare it with **ours + join**, our writer fed the same pairs and joining them
  per call. Plain "ours" gets the fields already joined, as the JS layer will hand them over.
- **clone+encode**: clones an already-validated map and never re-parses the target; a lower bound on the emulation's
  cost and the conservative comparison for our writer.
- **encode-only**: the emulation's serialisation with no validation; compare it with our unchecked appends, never
  with our writer.

`go/` benchmarks Go 1.27.3's `(*Request).Write` on the `get` and `post1k` requests. The body is what
`http.NewRequest` builds for a byte slice (`io.NopCloser` over a `*bytes.Reader`, reset in place), so net/http takes
its in-memory path and every reported allocation is `Request.Write`'s; `TestPost1kBodyIsInMemory` checks that path.

Comparing two writer revisions: `docs/perf/harness/writer-ab/` compiles several engine-core revisions into **one**
binary (so every baseline is literally the same machine code for all of them) and runs them in rotated order; its
`run.sh` header gives the commands. The journal's 2026-11-13 entry was measured with it.

`-- --verify` on either Rust binary runs only the byte-equality checks (CI does, with the Go tests and one Go
iteration). Numbers are dev numbers outside the nightly window unless the journal says otherwise. The owner's goal is
to beat every writer; the cross-language writer race is Task 5c.
