// Forked loopback HTTP server for the real-transport round-trip tests.
//
// WHY a separate process (see providers-rest-transport.test.ts and
// DESIGN-declarative-providers.md §12.2): `restTransportViaNode` uses
// `spawnSync`, which blocks the caller's libuv event loop for the whole HTTP
// exchange. A loopback `http.Server` hosted in the test's OWN event loop would
// therefore be frozen while the transport runs and the round-trip would hang to
// its AbortSignal timeout. Running the server in a forked child gives it an
// independent event loop that keeps accepting connections while the test's main
// thread is blocked inside `spawnSync`. Do NOT "simplify" this back in-process.
//
// Plain ESM (the package is `"type": "module"`); Node runs this file directly via
// `child_process.fork`, so it is not TypeScript-transformed. It is not a runner
// child — it is a test fixture — so the secret-boundary env rules do not apply.
//
// IPC protocol with the parent:
//   child → parent  { type: 'listening', port }          once, on startup
//   parent → child  { type: 'requests' }   → child → parent { type: 'requests', items }
//   parent → child  { type: 'close' }       → destroys held sockets, closes, exits 0

import { createServer } from 'node:http';

const mode = process.env.LOOPBACK_MODE ?? 'echo'; // 'echo' | 'status404' | 'hang'
const captured = [];
const sockets = new Set();

const server = createServer((req, res) => {
  let body = '';
  req.on('data', (c) => (body += c));
  req.on('end', () => {
    captured.push({ method: req.method, url: req.url, headers: req.headers, body });
    if (mode === 'hang') return; // never respond ⇒ the client's AbortSignal fires
    if (mode === 'status404') {
      res.writeHead(404, { 'content-type': 'application/json' });
      res.end('{"message":"nope"}');
      return;
    }
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ ok: true, echoBody: body }));
  });
});

// Track open sockets so 'close' can force-destroy a connection the helper is
// holding open in 'hang' mode (otherwise server.close() would never resolve).
server.on('connection', (s) => {
  sockets.add(s);
  s.on('close', () => sockets.delete(s));
});

server.listen(0, '127.0.0.1', () => {
  process.send?.({ type: 'listening', port: server.address().port });
});

process.on('message', (m) => {
  if (m?.type === 'requests') {
    process.send?.({ type: 'requests', items: captured });
    return;
  }
  if (m?.type === 'close') {
    for (const s of sockets) s.destroy();
    server.close(() => process.exit(0));
  }
});
