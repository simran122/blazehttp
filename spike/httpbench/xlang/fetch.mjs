// fetch + res.json() benchmark for Bun and Deno; mirrors bench2.mjs (50 in flight, 3k warmup, 30k measured).
const URL_ = 'http://127.0.0.1:3999/data';
const TOTAL = 30_000, CONCURRENCY = 50, WARMUP = 3_000;
const name = globalThis.Deno ? 'Deno fetch' : 'Bun fetch';

async function run(total) {
  const latencies = new Float64Array(total);
  let next = 0;
  const worker = async () => {
    while (next < total) {
      const i = next++;
      const t = performance.now();
      const data = await (await fetch(URL_)).json();
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

await run(WARMUP);
console.log(JSON.stringify({ name, ...(await run(TOTAL)) }));
