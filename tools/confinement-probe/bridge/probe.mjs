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

async function network(label, start) {
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${label}: did not refuse promptly`)), 1000);
    const object = start();
    object.once('error', error => {
      clearTimeout(timer);
      try { assert.equal(error.code, 'EPERM'); results.push(label); resolve(); } catch (error) { reject(error); }
    });
    object.once('connect', () => reject(new Error(`${label}: connected`)));
    object.once('listening', () => { object.close(); reject(new Error(`${label}: listening`)); });
  });
}
await network('host TCP denied', () => createConnection({host: '127.0.0.1', port: spec.port}));
await network('host Unix socket denied', () => createConnection(spec.socket));
await network('loopback listener denied', () => createServer().listen(0, '127.0.0.1'));
// Positive worker control, then the same native subprocess denial on that thread.
await new Promise((resolve, reject) => {
  const worker = new Worker(`const { parentPort } = require('node:worker_threads');
    const { spawnSync } = require('node:child_process');
    parentPort.postMessage(spawnSync('/node', ['--version']).error?.code);`, {eval: true});
  worker.once('message', code => { try { assert.equal(code, 'EPERM'); } catch (error) { reject(error); } });
  worker.once('error', reject);
  worker.once('exit', code => code === 0 ? resolve() : reject(new Error(`worker exit ${code}`)));
});
results.push('worker works and inherits subprocess denial');
writeFileSync('/state/probe.json', JSON.stringify(results));
await import('/runtime/test/bridge-child.ts');
