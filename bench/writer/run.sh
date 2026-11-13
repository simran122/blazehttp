#!/usr/bin/env bash
# Runs the request-writer benchmarks (bench/writer/README.md) in pinned containers, one after the other: the Rust
# writer bench (write_request against its baselines), the hyper 1.12 encoder emulation, then Go's Request.Write.
# Each container gets one CPU of quota pinned to one vCPU and a hard memory cap. Output: one log per bench in LOG_DIR.
#
# Usage: bench/writer/run.sh LOG_DIR [CPUSET]   (CPUSET: the vCPU to pin to, default 2)
# Run from the repository root, alone on a quiet machine (docs/perf/journal.md method).
set -euo pipefail

# rustc 1.99.1 (rust-toolchain.toml); the digest bench/parsers/rust/Dockerfile pins.
readonly RUST_IMAGE='rust:1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c'
# Go 1.27.3; the digest bench/parsers/go/Dockerfile pins.
readonly GO_IMAGE='golang:1-bookworm@sha256:dc0963f32053e4bc451d24d87e3892d42d01021b61f093e98eb95c0352bb84b0'

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 LOG_DIR [CPUSET]" >&2
  exit 2
fi
log_dir="$1"
cpuset="${2:-2}"
mkdir -p "${log_dir}"
log_dir="$(cd "${log_dir}" && pwd)"
repo="$(pwd)"
if [[ ! -f "${repo}/bench/writer/run.sh" ]]; then
  echo "run from the repository root" >&2
  exit 2
fi

# The source is mounted read-only and copied inside, so the container never writes to the checkout.
limits=(--rm --cpus=1 "--cpuset-cpus=${cpuset}" --memory=2g --memory-swap=2g --label blazehttp-bench)

echo "commit $(git -C "${repo}" rev-parse HEAD) dirty=$(git -C "${repo}" status --porcelain | wc -l | tr -d ' ')" \
  | tee "${log_dir}/provenance.txt"

docker run "${limits[@]}" -v "${repo}:/src:ro" -v "${log_dir}:/logs" -e RUSTFLAGS='-C target-cpu=generic' \
  "${RUST_IMAGE}" bash -euo pipefail -c '
    mkdir /work && cd /src && tar --exclude=./target --exclude=./node_modules --exclude=./.git --exclude=./.claude --exclude=./.superpowers -cf - . | tar -C /work -xf -
    cd /work
    cargo run --release -p engine-core --example h1_write_bench > /logs/rust-writer.log
    cargo run --release --locked --manifest-path bench/writer/hyper-emul/Cargo.toml > /logs/hyper-emul.log'

docker run "${limits[@]}" -e GOMAXPROCS=1 -v "${repo}/bench/writer/go:/src:ro" -v "${log_dir}:/logs" \
  "${GO_IMAGE}" bash -euo pipefail -c '
    cp -r /src /work && cd /work
    go test -run "^$" -bench . -benchmem -count 5 > /logs/go.log'

echo "logs in ${log_dir}"
