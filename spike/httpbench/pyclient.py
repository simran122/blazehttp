import asyncio
import json
import sys
import time

import aiohttp
import httpx

URL = "http://127.0.0.1:3999/data"
TOTAL = 30_000
CONCURRENCY = 50
WARMUP = 3_000


async def run(get_json, n):
    lat = [0.0] * n
    nxt = 0

    async def worker():
        nonlocal nxt
        while nxt < n:
            i = nxt
            nxt += 1
            t = time.perf_counter()
            data = await get_json()
            assert data["id"] == 42
            lat[i] = (time.perf_counter() - t) * 1000

    start = time.perf_counter()
    await asyncio.gather(*(worker() for _ in range(CONCURRENCY)))
    elapsed = time.perf_counter() - start
    lat.sort()
    return int(n / elapsed), lat[n // 2], lat[n * 99 // 100]


async def main(which):
    if which == "aiohttp":
        session = aiohttp.ClientSession(connector=aiohttp.TCPConnector(limit=CONCURRENCY))

        async def get_json():
            async with session.get(URL) as r:
                return await r.json()

        name = "Python aiohttp"
    else:
        client = httpx.AsyncClient(limits=httpx.Limits(max_connections=CONCURRENCY, max_keepalive_connections=CONCURRENCY))

        async def get_json():
            r = await client.get(URL)
            return r.json()

        name = "Python httpx"

    await run(get_json, WARMUP)
    rps, p50, p99 = await run(get_json, TOTAL)
    print(json.dumps({"name": name, "rps": rps, "p50": f"{p50:.2f}", "p99": f"{p99:.2f}"}))
    if which == "aiohttp":
        await session.close()
    else:
        await client.aclose()


asyncio.run(main(sys.argv[1]))
