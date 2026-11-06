// One client per process: node bench2.mjs <client> [label]
// Measures throughput, latency, peak memory, CPU per 1k requests (all threads), main-thread busy %.
import { createRequire } from 'node:module';
import { request, Pool } from 'undici';
import axios from 'axios';
import http from 'node:http';

const require = createRequire(import.meta.url);
const native = require(process.env.NATIVE ?? './rustnode.node');

const URL_ = 'http://127.0.0.1:3999/data';
const TOTAL = Number(process.env.TOTAL ?? 30_000);
const CONCURRENCY = Number(process.env.CONCURRENCY ?? 50);
const WARMUP = 3_000;

const pool = new Pool('http://127.0.0.1:3999', { connections: CONCURRENCY });
const axiosHttp = axios.create({ httpAgent: new http.Agent({ keepAlive: true, maxSockets: CONCURRENCY }) });

// Variant B plumbing: one pump loop resolves every response in each batch.
const pending = new Map();
let nextId = 0;
let pumping = false;
async function pump() {
  pumping = true;
  for (;;) {
    const batch = await native.drain();
    for (const c of batch) {
      pending.get(c.id)(c);
      pending.delete(c.id);
    }
  }
}
function batchedGet(url) {
  if (!pumping) pump();
  return new Promise((resolve) => {
    const id = nextId++ >>> 0;
    pending.set(id, resolve);
    native.submit(id, url);
  });
}

const clients = {
  'undici Pool.request': async () => (await pool.request({ path: '/data', method: 'GET' })).body.json(),
  'undici.request': async () => (await request(URL_)).body.json(),
  'axios': async () => (await axiosHttp.get(URL_)).data,
  'Rust engine: promise per request': async () => JSON.parse(await native.get(URL_)),
  'Rust engine: batched delivery': async () => JSON.parse((await batchedGet(URL_)).body),
};

async function run(fn, total) {
  const latencies = new Float64Array(total);
  let next = 0;
  const worker = async () => {
    while (next < total) {
      const i = next++;
      const t = performance.now();
      const data = await fn();
      if (data.id !== 42) throw new Error('bad body');
      latencies[i] = performance.now() - t;
    }
  };
  const cpu0 = process.cpuUsage();
  const elu0 = performance.eventLoopUtilization();
  const start = performance.now();
  await Promise.all(Array.from({ length: CONCURRENCY }, worker));
  const elapsed = performance.now() - start;
  const cpu = process.cpuUsage(cpu0);
  const elu = performance.eventLoopUtilization(elu0);
  latencies.sort();
  const pct = (p) => latencies[Math.min(total - 1, Math.floor(total * p))].toFixed(2);
  return {
    rps: Math.round(total / (elapsed / 1000)),
    p50: pct(0.5),
    p99: pct(0.99),
    cpuMsPer1k: ((cpu.user + cpu.system) / 1000 / (total / 1000)).toFixed(1),
    mainBusy: Math.round(elu.utilization * 100),
  };
}

const [name, label] = process.argv.slice(2);
const fn = clients[name];
if (!fn) {
  console.log(JSON.stringify(Object.keys(clients)));
  process.exit(0);
}
await run(fn, WARMUP);
const r = await run(fn, TOTAL);
const peakMb = Math.round(process.resourceUsage().maxRSS / 1024);
console.log(JSON.stringify({ name: label ?? name, ...r, peakMb }));
process.exit(0);
