// Only the Rust test harness selects this entrypoint. None of these probes is
// registered as a model tool. Run actual operations before the real Pi bridge.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, unlinkSync, renameSync, realpathSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { createConnection, createServer } from 'node:net';
import { Worker } from 'node:worker_threads';

const spec = JSON.parse(readFileSync('/case.json', 'utf8'));
const results = [];
function denied(label, operation, codes) {
  assert.throws(operation, error => codes.includes(error.code), label);
  results.push(label);
}
for (const path of spec.protected) {
  denied(`read ${path}`, () => readFileSync(path), ['ENOENT', 'EACCES']);
  denied(`write ${path}`, () => writeFileSync(path, 'escape'), ['ENOENT', 'EACCES', 'EROFS']);
}
denied('symlink escape', () => readFileSync('/state/escape'), ['ENOENT', 'EACCES']);
for (const path of ['/runtime/pins.json', '/node', '/guard.cjs', '/lockdown.so']) {
  assert.ok(readFileSync(path).length, `read-only runtime exists: ${path}`);
  denied(`runtime write ${path}`, () => writeFileSync(path, 'escape'), ['EROFS', 'EACCES']);
}
denied('owner lock write', () => writeFileSync('/state/session.sqlite.owner.lock', 'escape'), ['EROFS', 'EACCES']);
denied('owner lock unlink', () => unlinkSync('/state/session.sqlite.owner.lock'), ['EBUSY', 'EROFS', 'EACCES']);
denied('owner lock rename', () => renameSync('/state/session.sqlite.owner.lock', '/state/replaced'), ['EBUSY', 'EROFS', 'EACCES']);
writeFileSync('/state/positive', 'private state works');
writeFileSync('/tmp/positive', 'private temp works');
assert.equal(readFileSync('/tmp/positive', 'utf8'), 'private temp works');
assert.equal(realpathSync('/proc/self/root'), '/');
assert.equal(spawnSync('/node', ['--version']).error?.code, 'EPERM');
results.push('existing Node executable cannot spawn');
for (const key of ['HOME', 'GH_TOKEN', 'GITHUB_TOKEN', 'NODE_OPTIONS', 'HTTP_PROXY', 'HTTPS_PROXY', 'SWITCHYARD_CREDENTIAL_SENTINEL']) {
  assert.equal(process.env[key], undefined, `unexpected inherited environment: ${key}`);
}
results.push('credential and configuration environment absent');

async function refused(label, start, codes) {
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${label}: did not refuse promptly`)), 1000);
    const object = start();
    object.once('error', error => {
      clearTimeout(timer);
      try { assert.ok(codes.includes(error.code), `${label}: ${error.code}`); results.push(label); resolve(); }
      catch (error) { reject(error); }
    });
    object.once('connect', () => reject(new Error(`${label}: connected`)));
    object.once('listening', () => { object.close(); reject(new Error(`${label}: listening`)); });
  });
}
async function network(label, start) {
  await refused(label, start, ['EPERM']);
}
await network('host TCP denied', () => createConnection({host: '127.0.0.1', port: spec.port}));
if (spec.egress === 'model-endpoint') {
  // AF_UNIX socket() is allowed, but the host socket is not mounted.
  await refused('host Unix socket denied', () => createConnection(spec.socket), ['ENOENT', 'EACCES', 'EPERM']);
} else {
  await network('host Unix socket denied', () => createConnection(spec.socket));
}
await network('loopback listener denied', () => createServer().listen(0, '127.0.0.1'));
// Positive worker control, then the same native subprocess denial on that thread.
await new Promise((resolve, reject) => {
  let asserted = false;
  const worker = new Worker(`const { parentPort } = require('node:worker_threads');
    const { spawnSync } = require('node:child_process');
    parentPort.postMessage(spawnSync('/node', ['--version']).error?.code);`, {eval: true});
  worker.once('message', code => {
    try { assert.equal(code, 'EPERM'); asserted = true; } catch (error) { reject(error); }
  });
  worker.once('error', reject);
  worker.once('exit', code => {
    if (code !== 0) reject(new Error(`worker exit ${code}`));
    else if (!asserted) reject(new Error('worker exited without an asserted subprocess denial'));
    else resolve();
  });
});
results.push('worker works and inherits subprocess denial');
if (spec.egress === 'model-endpoint') {
  // The socket path is fixed by the host mount. case.json cannot retarget it.
  const proxyPath = '/model-proxy/endpoint.sock';
  assert.notEqual(spec.endpoint, proxyPath);
  assert.notEqual(spec.socket, proxyPath);
  for (const value of Object.values(process.env)) {
    assert.equal(String(value).includes(String(spec.endpoint)), false);
    assert.equal(String(value).includes('169.254.169.254'), false);
  }
  assert.equal(process.env.SWITCHYARD_BRIDGE_EGRESS, 'model-socket');
  assert.equal(process.env.HTTP_PROXY, undefined);
  assert.equal(process.env.HTTPS_PROXY, undefined);
  results.push('model endpoint URL is not bridge configuration');
  await network('metadata TCP denied', () => createConnection({host: '169.254.169.254', port: 80}));
  await network('metadata IPv6 denied', () => createConnection({host: 'fd00:ec2::254', port: 80}));
  await network('public TCP denied', () => createConnection({host: '1.1.1.1', port: 443}));
  await network('dns TCP denied', () => createConnection({host: '8.8.8.8', port: 53}));
  await network('stub TCP denied', () => createConnection({host: '127.0.0.1', port: spec.stubPort}));
  await refused('other Unix socket denied', () => createConnection('/tmp/not-the-proxy.sock'), ['ENOENT', 'EACCES', 'EPERM']);
  denied('model proxy unlink', () => unlinkSync(proxyPath), ['EROFS', 'EACCES']);
  const { lookup } = await import('node:dns/promises');
  const dns = await Promise.race([
    lookup('example.com').then(() => 'resolved', () => 'denied'),
    new Promise(resolve => setTimeout(() => resolve('hung'), 2000)),
  ]);
  assert.equal(dns, 'denied');
  results.push('dns lookup denied');
  const response = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('model endpoint timed out')), 2000);
    const socket = createConnection(proxyPath);
    const chunks = [];
    socket.once('error', error => { clearTimeout(timer); reject(error); });
    socket.on('data', chunk => chunks.push(chunk));
    socket.on('end', () => { clearTimeout(timer); resolve(Buffer.concat(chunks).toString('utf8')); });
    // Absolute-form request names a forbidden host. The proxy must not dial it.
    socket.end(`GET ${spec.endpoint} HTTP/1.0\r\nHost: 169.254.169.254\r\n\r\n`);
  });
  const splitAt = response.indexOf('\r\n\r\n');
  assert.ok(splitAt > 0, response);
  const header = response.slice(0, splitAt);
  const body = response.slice(splitAt + 4);
  assert.ok(header.startsWith('HTTP/1.0 200'), header);
  assert.ok(body.length > 0);
  writeFileSync('/state/endpoint-body.txt', body);
  results.push('configured model endpoint reachable');
  await new Promise((resolve, reject) => {
    const worker = new Worker(`const { parentPort } = require('node:worker_threads');
      const net = require('node:net');
      const socket = net.createConnection({host: '169.254.169.254', port: 80});
      socket.once('error', error => parentPort.postMessage(error.code));
      socket.once('connect', () => parentPort.postMessage('connected'));`, {eval: true});
    const timer = setTimeout(() => reject(new Error('worker metadata denial timed out')), 2000);
    worker.once('message', code => {
      clearTimeout(timer);
      try { assert.equal(code, 'EPERM'); resolve(); } catch (error) { reject(error); }
    });
    worker.once('error', error => { clearTimeout(timer); reject(error); });
  });
  results.push('worker inherits model-endpoint denial');
}
writeFileSync('/state/probe.json', JSON.stringify(results));
await import('/runtime/test/bridge-child.ts');
