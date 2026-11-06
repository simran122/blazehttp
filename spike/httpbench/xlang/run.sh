#!/bin/sh
# Cross-language HTTP client benchmark. Same method as ../dockerbench.sh:
# server container (2 CPUs, GOMAXPROCS=2), each client in its own container with --cpus=1
# sharing the server's network namespace (127.0.0.1:3999). Clients run one at a time.
# Usage: ./build.sh once, then ./run.sh [client ...]   (default: all)   ROUNDS=3 by default.
set -e
cd "$(dirname "$0")"
B=$(cd .. && pwd)            # httpbench/ (server binary, bench2.mjs, rustnode-linux.node)
X=$(pwd)
ROUNDS=${ROUNDS:-3}
SRV=xlang-srv
OUT=results.jsonl

docker rm -f $SRV >/dev/null 2>&1 || true
docker run -d --name $SRV --cpus=2 -e GOMAXPROCS=2 -v "$B":/bench debian:bookworm-slim /bench/bin-goserver-linux >/dev/null
trap 'docker rm -f $SRV >/dev/null 2>&1' EXIT
sleep 1

c() { docker run --rm --network container:$SRV --cpus=1 "$@"; }
node2() { c -e NATIVE=/bench/rustnode-linux.node -e RUST_HTTP_THREADS=1 -v "$B":/bench -w /bench node:26-bookworm-slim node bench2.mjs "$1" "$2"; }

client() {
  case $1 in
    c)       c xlang-c ;;
    cpp)     c xlang-cpp ;;
    zig)     c xlang-zig ;;
    bun)     c -v "$X":/x oven/bun:1 bun /x/fetch.mjs ;;
    deno)    c -v "$X":/x denoland/deno:latest run -A /x/fetch.mjs ;;
    rust)    c -v "$X/bin":/b debian:bookworm-slim /b/rustclient ;;
    go)      c -e GOMAXPROCS=1 -v "$X/bin":/b debian:bookworm-slim /b/goclient ;;
    undici)  node2 "undici Pool.request" "Node undici Pool.request" ;;
    blaze)   node2 "Rust engine: batched delivery" "blazehttp (Node+Rust, batched, 1 thread)" ;;
  esac
}

CLIENTS=${*:-"c cpp zig bun deno rust go undici blaze"}
: > $OUT
for r in $(seq "$ROUNDS"); do
  for k in $CLIENTS; do
    line=$(client "$k" | tail -1) || line=""
    [ -n "$line" ] && echo "$line" | tee -a $OUT || echo "FAILED: $k" >&2
  done
done

# median-by-rps table
docker run --rm -v "$X":/x -w /x node:26-bookworm-slim node -e '
const rows=require("fs").readFileSync("results.jsonl","utf8").trim().split("\n").map(JSON.parse);
const by={};for(const r of rows)(by[r.name]??=[]).push(r);
const med=Object.values(by).map(rs=>rs.sort((a,b)=>a.rps-b.rps)[rs.length>>1]).sort((a,b)=>b.rps-a.rps);
const base=med.find(r=>r.name.startsWith("Node undici"));
console.log("client | req/s | vs undici | p50 ms | p99 ms | all rounds");
for(const r of med)console.log([r.name,r.rps,base?(r.rps/base.rps).toFixed(2)+"x":"-",r.p50,r.p99,by[r.name].map(x=>x.rps).sort((a,b)=>a-b).join("/")].join(" | "));
'
