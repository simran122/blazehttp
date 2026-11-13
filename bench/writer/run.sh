#!/usr/bin/env bash
# Request-writer benchmarks (bench/writer/README.md) in pinned containers, in two separate steps so that nothing but
# the measured process runs while timing:
#   build   BIN_DIR                    compiles the Rust writer bench, the hyper 1.12 encoder emulation and Go's
#                                      Request.Write bench (as a test binary) into BIN_DIR, with the commit recorded;
#   measure BIN_DIR LOG_DIR [CPUSET]   runs those binaries one after the other, no compiler, no network, each with one
#                                      CPU of quota pinned to one vCPU (default 2) and a hard memory cap; one log per
#                                      bench in LOG_DIR. Refuses to start while any other container runs.
# Run from the repository root; measure alone on a quiet machine (docs/perf/journal.md method).
set -euo pipefail

# rustc 1.99.1 (rust-toolchain.toml); the digest bench/parsers/rust/Dockerfile pins.
readonly RUST_IMAGE='rust:1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c'
# Go 1.27.3; the digest bench/parsers/go/Dockerfile pins.
readonly GO_IMAGE='golang:1-bookworm@sha256:dc0963f32053e4bc451d24d87e3892d42d01021b61f093e98eb95c0352bb84b0'
readonly LIMITS=(--rm --cpus=1 --memory=2g --memory-swap=2g --label blazehttp-bench)

usage() {
  echo "usage: $0 build BIN_DIR | $0 measure BIN_DIR LOG_DIR [CPUSET]" >&2
  exit 2
}

absolute_dir() {
  mkdir -p "$1"
  (cd "$1" && pwd)
}

build() {
  local repo bin_dir
  repo="$(pwd)"
  bin_dir="$(absolute_dir "$1")"
  echo "commit $(git -C "${repo}" rev-parse HEAD) dirty=$(git -C "${repo}" status --porcelain | wc -l | tr -d ' ')" \
    | tee "${bin_dir}/provenance.txt"
  # The source is mounted read-only and copied inside, so the container never writes to the checkout.
  docker run "${LIMITS[@]}" -v "${repo}:/src:ro" -v "${bin_dir}:/bins" -e RUSTFLAGS='-C target-cpu=generic' \
    "${RUST_IMAGE}" bash -euo pipefail -c '
      mkdir /work && cd /src && tar --exclude=./target --exclude=./node_modules --exclude=./.git --exclude=./.claude --exclude=./.superpowers -cf - . | tar -C /work -xf -
      cd /work
      cargo build --release -p engine-core --example h1_write_bench
      cp target/release/examples/h1_write_bench /bins/
      cargo build --release --locked --manifest-path bench/writer/hyper-emul/Cargo.toml
      cp bench/writer/hyper-emul/target/release/writer-hyper-emul /bins/'
  docker run "${LIMITS[@]}" -v "${repo}/bench/writer/go:/src:ro" -v "${bin_dir}:/bins" "${GO_IMAGE}" \
    bash -euo pipefail -c 'cp -r /src /work && cd /work && go test -c -o /bins/go-writer.test'
  echo "binaries in ${bin_dir}"
}

measure() {
  local bin_dir log_dir cpuset
  bin_dir="$(cd "$1" && pwd)"
  log_dir="$(absolute_dir "$2")"
  cpuset="${3:-2}"
  if [[ -n "$(docker ps --quiet)" ]]; then
    echo "another container is running; measure alone" >&2
    docker ps >&2
    exit 1
  fi
  cp "${bin_dir}/provenance.txt" "${log_dir}/provenance.txt"
  local run=(docker run "${LIMITS[@]}" "--cpuset-cpus=${cpuset}" --network none -v "${bin_dir}:/bins:ro"
    -v "${log_dir}:/logs")
  "${run[@]}" "${RUST_IMAGE}" bash -euo pipefail -c \
    '/bins/h1_write_bench > /logs/rust-writer.log && /bins/writer-hyper-emul > /logs/hyper-emul.log'
  "${run[@]}" -e GOMAXPROCS=1 "${GO_IMAGE}" bash -euo pipefail -c \
    '/bins/go-writer.test -test.run "^$" -test.bench . -test.benchmem -test.count 5 > /logs/go.log'
  echo "logs in ${log_dir}"
}

if [[ ! -f bench/writer/run.sh ]]; then
  echo "run from the repository root" >&2
  exit 2
fi
case "${1:-}" in
  build) [[ $# -eq 2 ]] || usage; build "$2" ;;
  measure) [[ $# -eq 3 || $# -eq 4 ]] || usage; measure "$2" "$3" "${4:-2}" ;;
  *) usage ;;
esac
