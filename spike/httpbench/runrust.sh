#!/bin/sh
# Rust-for-Node prototype vs undici. Multi-core Go server. 3 rounds, rotated; median by req/s.
cd "$(dirname "$0")"
GOMAXPROCS=4 ./bin-goserver > /dev/null & SERVER=$!
sleep 1
: > results-rust.jsonl
for round in 1 2 3; do
  node bench2.mjs "undici Pool.request" >> results-rust.jsonl
  UNDICI_USE_MILO=1 node bench2.mjs "undici Pool.request" "undici Pool.request + Milo (Rust parser)" >> results-rust.jsonl
  node bench2.mjs "undici.request" >> results-rust.jsonl
  node bench2.mjs "axios" >> results-rust.jsonl
  RUST_HTTP_THREADS=1 node bench2.mjs "Rust engine: promise per request" "Rust engine, promise per request (1 thread)" >> results-rust.jsonl
  RUST_HTTP_THREADS=1 node bench2.mjs "Rust engine: batched delivery" "Rust engine, batched (1 thread)" >> results-rust.jsonl
  RUST_HTTP_THREADS=2 node bench2.mjs "Rust engine: batched delivery" "Rust engine, batched (2 threads)" >> results-rust.jsonl
done
kill $SERVER
node -e '
const rows=require("fs").readFileSync("results-rust.jsonl","utf8").trim().split("\n").map(JSON.parse);
const by={};for(const r of rows)(by[r.name]??=[]).push(r);
const med=Object.values(by).map(rs=>rs.sort((a,b)=>a.rps-b.rps)[1]).sort((a,b)=>b.rps-a.rps);
const base=med.find(r=>r.name==="undici Pool.request");
console.log("client | req/s | vs undici | p50 ms | p99 ms | CPU ms per 1k req | main thread busy | peak MB");
for(const r of med)console.log([r.name,r.rps,(r.rps/base.rps).toFixed(2)+"x",r.p50,r.p99,r.cpuMsPer1k,r.mainBusy+"%",r.peakMb].join(" | "));
'
