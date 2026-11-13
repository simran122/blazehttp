#!/usr/bin/env bash
# One-binary writer A/B (journal 2026-11-13): the workspace build.py made (three engine-core revisions plus the
# current baselines in one binary, rotated) and the committed benches, in two separate steps so that nothing but the
# measured process runs while timing:
#   build   AB_DIR BIN_DIR                    compiles the A/B binary and the committed benches into BIN_DIR, with
#                                             build.py's provenance.txt (revision -> commit) as ab-provenance.txt;
#   measure BIN_DIR LOG_DIR [CPUSET]          three separate launches of the A/B binary in the pinned rustc 1.99.1
#                                             image (one CPU pinned, 2 GiB cap, no network), then the committed
#                                             benches through bench/writer/run.sh measure. LOG_DIR/ab.log starts
#                                             empty and ab-provenance.txt is copied next to it. Refuses to start
#                                             while any other container runs.
#
# Usage, from the repository root (measure alone on a quiet machine):
#   python3 docs/perf/harness/writer-ab/build.py AB_DIR 68c48f0 abe8c87 HEAD
#   docs/perf/harness/writer-ab/run.sh build AB_DIR BIN_DIR
#   docs/perf/harness/writer-ab/run.sh measure BIN_DIR LOG_DIR [CPUSET]
set -euo pipefail

# The digest bench/writer/run.sh pins.
readonly RUST_IMAGE='rust:1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c'
readonly LAUNCHES=3
readonly LIMITS=(--rm --cpus=1 --memory=2g --memory-swap=2g --label blazehttp-bench)

usage() {
  echo "usage: $0 build AB_DIR BIN_DIR | $0 measure BIN_DIR LOG_DIR [CPUSET] (from the repository root)" >&2
  exit 2
}

# Both steps call bench/writer/run.sh by its repository path.
[[ -f bench/writer/run.sh ]] || usage

absolute_dir() {
  mkdir -p "$1"
  (cd "$1" && pwd)
}

build() {
  local ab_dir bin_dir
  ab_dir="$(cd "$1" && pwd)"
  bin_dir="$(absolute_dir "$2")"
  docker run "${LIMITS[@]}" -e RUSTFLAGS='-C target-cpu=generic' -v "${ab_dir}:/src:ro" -v "${bin_dir}:/bins" \
    "${RUST_IMAGE}" bash -euo pipefail -c '
      cp -r /src /work && cd /work
      cargo build --release -p writer-ab
      cp target/release/writer-ab /bins/'
  cp "${ab_dir}/provenance.txt" "${bin_dir}/ab-provenance.txt"
  bench/writer/run.sh build "${bin_dir}/committed"
  echo "binaries in ${bin_dir}"
}

measure() {
  local bin_dir log_dir cpuset
  bin_dir="$(cd "$1" && pwd)"
  log_dir="$(absolute_dir "$2")"
  cpuset="${3:-2}"
  if [[ -n "$(docker ps --quiet)" ]]; then
    echo "another container is running; run the A/B alone" >&2
    docker ps >&2
    exit 1
  fi
  : > "${log_dir}/ab.log"
  cp "${bin_dir}/ab-provenance.txt" "${log_dir}/ab-provenance.txt"
  docker run "${LIMITS[@]}" "--cpuset-cpus=${cpuset}" --network none -e "LAUNCHES=${LAUNCHES}" \
    -v "${bin_dir}:/bins:ro" -v "${log_dir}:/logs" "${RUST_IMAGE}" bash -euo pipefail -c '
      for launch in $(seq 1 "${LAUNCHES}"); do
        echo "launch ${launch} $(date -u +%FT%TZ)" >> /logs/ab.log
        /bins/writer-ab >> /logs/ab.log
      done'
  bench/writer/run.sh measure "${bin_dir}/committed" "${log_dir}/committed" "${cpuset}"
  echo "done: ${log_dir}"
}

case "${1:-}" in
  build) [[ $# -eq 3 ]] || usage; build "$2" "$3" ;;
  measure) [[ $# -eq 3 || $# -eq 4 ]] || usage; measure "$2" "$3" "${4:-2}" ;;
  *) usage ;;
esac
