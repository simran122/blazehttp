#!/bin/sh
# 3 rounds per client, rotated order; prints the median-rps round per client.
cd "$(dirname "$0")"
node server.mjs & SERVER=$!
sleep 1
CLIENTS=$(node bench.mjs --list | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>JSON.parse(s).forEach(c=>console.log(c)))')
: > results.jsonl
for round in 1 2 3; do
  echo "$CLIENTS" | while IFS= read -r c; do node bench.mjs "$c" >> results.jsonl; done
done
kill $SERVER
node -e '
const rows=require("fs").readFileSync("results.jsonl","utf8").trim().split("\n").map(JSON.parse);
const by={};for(const r of rows)(by[r.name]??=[]).push(r);
const med=Object.values(by).map(rs=>rs.sort((a,b)=>a.rps-b.rps)[1]).sort((a,b)=>b.rps-a.rps);
const base=med.find(r=>r.name.startsWith("axios (http")).rps;
console.log("client | req/s | vs axios | p50 ms | p99 ms");
for(const r of med)console.log([r.name,r.rps,(r.rps/base).toFixed(2)+"x",r.p50,r.p99].join(" | "));
'
