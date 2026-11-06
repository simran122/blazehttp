#!/bin/sh
# Cross-language HTTP client benchmark against a multi-core Go server.
# Every client is limited to ~1 CPU core. 3 rounds, median by req/s.
cd "$(dirname "$0")"
GOMAXPROCS=4 ./bin-goserver & SERVER=$!
sleep 1
: > results-langs.jsonl
for round in 1 2 3; do
  for c in "undici Pool.request" "undici.request" "native fetch" "axios (http adapter, keep-alive)"; do
    node bench.mjs "$c" >> results-langs.jsonl
  done
  GOMAXPROCS=1 ./bin-goclient >> results-langs.jsonl
  ./rustclient/target/release/rustclient >> results-langs.jsonl
  python3 pyclient.py aiohttp >> results-langs.jsonl
  python3 pyclient.py httpx >> results-langs.jsonl
  java -XX:ActiveProcessorCount=1 -Djdk.virtualThreadScheduler.parallelism=1 -cp javabin JavaClient >> results-langs.jsonl
done
kill $SERVER
node -e '
const rows=require("fs").readFileSync("results-langs.jsonl","utf8").trim().split("\n").map(JSON.parse);
const by={};for(const r of rows)(by[r.name]??=[]).push(r);
const med=Object.values(by).map(rs=>rs.sort((a,b)=>a.rps-b.rps)[1]).sort((a,b)=>b.rps-a.rps);
const base=med.find(r=>r.name.startsWith("axios")).rps;
console.log("client | req/s | vs axios | p50 ms | p99 ms");
for(const r of med)console.log([r.name,r.rps,(r.rps/base).toFixed(2)+"x",r.p50,r.p99].join(" | "));
'
