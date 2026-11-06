#!/bin/sh
# Builds every client inside Docker (nothing installed on the host). Run once before run.sh.
set -e
cd "$(dirname "$0")"
X=$(pwd); B=$(cd .. && pwd)
docker build -q -t xlang-c c
docker build -q -t xlang-cpp cpp
docker build -q -t xlang-zigbase -f zig/Dockerfile.base zig   # downloads Zig 0.17.1
docker build -q -t xlang-zig zig
mkdir -p bin
docker run --rm -v "$B/goclient":/src -v "$X/bin":/out -w /src -e CGO_ENABLED=0 golang:1-bookworm go build -o /out/goclient .
docker run --rm -v "$B/rustclient":/src -v "$X/bin":/out -w /src rust:1-bookworm \
  sh -c 'cargo build --release --target-dir /src/target-linux -q && cp /src/target-linux/release/rustclient /out/'
docker pull -q oven/bun:1; docker pull -q denoland/deno:latest; docker pull -q node:26-bookworm-slim
