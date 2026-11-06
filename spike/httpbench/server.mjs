import { createServer } from 'node:http';

// ~1 KB JSON body, typical small API response
const body = JSON.stringify({
  id: 42,
  name: 'campaign',
  items: Array.from({ length: 20 }, (_, i) => ({ i, label: `item-${i}`, active: i % 2 === 0 })),
});

createServer((req, res) => {
  res.writeHead(200, { 'content-type': 'application/json', 'content-length': Buffer.byteLength(body) });
  res.end(body);
})
  .listen(3999, '127.0.0.1', () => console.log('ready'));
