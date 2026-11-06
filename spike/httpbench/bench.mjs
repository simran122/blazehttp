// Run one client per process so clients don't share warm state: node bench.mjs <client>
import axios from 'axios';
import http from 'node:http';
import { request, Pool, fetch as undiciFetch } from 'undici';

const URL_ = 'http://127.0.0.1:3999/data';
const TOTAL = 30_000;
const CONCURRENCY = 50;
const WARMUP = 3_000;

const keepAlive = new http.Agent({ keepAlive: true, maxSockets: CONCURRENCY });
const axiosHttp = axios.create({ httpAgent: keepAlive });
const axiosFetch = axios.create({ adapter: 'fetch' });
const pool = new Pool('http://127.0.0.1:3999', { connections: CONCURRENCY });

// Every client must return parsed JSON, so all do the same work.
const clients = {
  'axios (http adapter, keep-alive)': async () => (await axiosHttp.get(URL_)).data,
  'axios (fetch adapter)': async () => (await axiosFetch.get(URL_)).data,
  'native fetch': async () => (await fetch(URL_)).json(),
  'undici.fetch': async () => (await undiciFetch(URL_)).json(),
  'undici.request': async () => (await request(URL_)).body.json(),
  'undici Pool.request': async () => (await pool.request({ path: '/data', method: 'GET' })).body.json(),
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
  const start = performance.now();
  await Promise.all(Array.from({ length: CONCURRENCY }, worker));
  const elapsed = performance.now() - start;
  latencies.sort();
  const pct = (p) => latencies[Math.min(total - 1, Math.floor(total * p))].toFixed(2);
  return { rps: Math.round(total / (elapsed / 1000)), p50: pct(0.5), p99: pct(0.99) };
}

const name = process.argv[2];
const fn = clients[name];
if (!fn) {
  console.log(JSON.stringify(Object.keys(clients)));
  process.exit(0);
}
await run(fn, WARMUP);
const r = await run(fn, TOTAL);
console.log(JSON.stringify({ name, ...r }));
process.exit(0);
