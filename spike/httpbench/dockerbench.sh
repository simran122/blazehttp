#!/bin/sh
# CPU-capped benchmark on Linux: server container (2 CPUs), client container capped by --cpus (default 1).
# Client shares the server's network namespace so 127.0.0.1:3999 works.
set -e
cd "$(dirname "$0")"
CPUS=${CPUS:-1}
NODE_IMAGE=${NODE_IMAGE:-node:26-bookworm-slim}

echo "== build linux artifacts"
GOOS=linux GOARCH=arm64 go build -C goserver -o ../bin-goserver-linux .
docker run --rm -v "$PWD/rustnode":/src -w /src rust:1-bookworm \
  cargo build --release --target-dir /src/target-linux -q
cp rustnode/target-linux/release/librustnode.so rustnode-linux.node

docker rm -f gosrv >/dev/null 2>&1 || true
docker run -d --name gosrv --cpus=2 -e GOMAXPROCS=2 -v "$PWD":/bench debian:bookworm-slim /bench/bin-goserver-linux >/dev/null
sleep 1

run() { # $1 client, $2 label, $3 extra env
  docker run --rm --network container:gosrv --cpus="$CPUS" -e NATIVE=/bench/rustnode-linux.node $3 \
    -v "$PWD":/bench -w /bench "$NODE_IMAGE" node bench2.mjs "$1" "$2"
}

: > results-docker.jsonl
for round in 1 2 3; do
  run "undici Pool.request" "undici Pool.request" "" >> results-docker.jsonl
  run "axios" "axios" "" >> results-docker.jsonl
  run "Rust engine: promise per request" "Rust engine, promise per request (1 thread)" "-e RUST_HTTP_THREADS=1" >> results-docker.jsonl
  run "Rust engine: batched delivery" "Rust engine, batched (1 thread)" "-e RUST_HTTP_THREADS=1" >> results-docker.jsonl
done
docker rm -f gosrv >/dev/null

echo "== client capped at $CPUS CPU (Linux, $NODE_IMAGE)"
node -e '
const rows=require("fs").readFileSync("results-docker.jsonl","utf8").trim().split("\n").map(JSON.parse);
const by={};for(const r of rows)(by[r.name]??=[]).push(r);
const med=Object.values(by).map(rs=>rs.sort((a,b)=>a.rps-b.rps)[1]).sort((a,b)=>b.rps-a.rps);
const base=med.find(r=>r.name==="undici Pool.request");
console.log("client | req/s | vs undici | p50 ms | p99 ms | CPU ms per 1k req | main thread busy | peak MB");
for(const r of med)console.log([r.name,r.rps,(r.rps/base.rps).toFixed(2)+"x",r.p50,r.p99,r.cpuMsPer1k,r.mainBusy+"%",r.peakMb].join(" | "));
'
