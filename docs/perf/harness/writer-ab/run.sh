#!/usr/bin/env bash
# One-binary writer A/B (journal 2026-11-13): the workspace build.py made (three engine-core revisions plus the
# current baselines in one binary, rotated), three separate process launches, in the pinned rustc 1.99.1 image with
# one CPU pinned and a 2 GiB cap. Then the committed benches through bench/writer/run.sh.
#
# Usage, from the repository root, alone on a quiet machine:
#   python3 docs/perf/harness/writer-ab/build.py AB_DIR 12d82d7 89a028c HEAD
#   docs/perf/harness/writer-ab/run.sh AB_DIR LOG_DIR [CPUSET]
set -euo pipefail

# The digest bench/writer/run.sh pins.
readonly RUST_IMAGE='rust:1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c'
readonly LAUNCHES=3

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "usage: $0 AB_DIR LOG_DIR [CPUSET]" >&2
  exit 2
fi
ab_dir="$(cd "$1" && pwd)"
log_dir="$2"
cpuset="${3:-2}"
mkdir -p "${log_dir}"
log_dir="$(cd "${log_dir}" && pwd)"
if [[ -n "$(docker ps --quiet)" ]]; then
  echo "another container is running; run the A/B alone" >&2
  docker ps >&2
  exit 1
fi

docker run --rm --cpus=1 "--cpuset-cpus=${cpuset}" --memory=2g --memory-swap=2g --label blazehttp-bench \
  -e RUSTFLAGS='-C target-cpu=generic' -e "LAUNCHES=${LAUNCHES}" \
  -v "${ab_dir}:/src:ro" -v "${log_dir}:/logs" "${RUST_IMAGE}" \
  bash -euo pipefail -c '
    cp -r /src /work && cd /work
    cargo build --release -p writer-ab 2> /logs/ab-build.log
    for launch in $(seq 1 "${LAUNCHES}"); do
      echo "launch ${launch} $(date -u +%FT%TZ)" >> /logs/ab.log
      ./target/release/writer-ab >> /logs/ab.log
    done'
bench/writer/run.sh "${log_dir}/committed" "${cpuset}"
echo "done: ${log_dir}"
