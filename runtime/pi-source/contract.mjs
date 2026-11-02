// Complete credential-free conformance runner. Green tests are evidence, not review approval.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawnSync, execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const source = dirname(fileURLToPath(import.meta.url));
const root = resolve(source, '../..');
const output = join(source, '.contract-results');
mkdirSync(output, { recursive: true });
const pins = JSON.parse(readFileSync(join(source, 'pins.json'), 'utf8'));
const cases = JSON.parse(readFileSync(join(source, 'contract-cases.json'), 'utf8'));
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const command = (name, args, cwd = root) => execFileSync(name, args, { cwd, encoding: 'utf8' }).trim();
const evidence = {
  conformance: 'failed', acceptance: 'NO-GO until reviewed',
  commit: command('git', ['rev-parse', 'HEAD']),
  tree: command('git', ['rev-parse', 'HEAD^{tree}']),
  dirty: command('git', ['status', '--porcelain', '--untracked-files=all']) !== '',
  node: process.versions.node, pins, cases, checks: [],
};

function run(label, executable, args, cwd = root) {
  console.log(`Contract: ${label}`);
  const result = spawnSync(executable, args, {
    cwd, encoding: 'utf8', timeout: 600_000, maxBuffer: 32 * 1024 * 1024,
    env: { ...process.env, SWITCHYARD_PI_NODE: process.execPath, CARGO_TERM_COLOR: 'never' },
  });
  const stdout = result.stdout ?? '';
  const stderr = result.stderr ?? '';
  writeFileSync(join(output, `${label}.log`), stdout + stderr);
  const passed = !result.error && result.status === 0;
  evidence.checks.push({ label, executable, args, passed, logSha256: sha256(stdout + stderr) });
  if (!passed) {
    process.stderr.write((stdout + stderr).slice(-16_000));
    throw new Error(`${label} failed: ${result.error?.message ?? result.status}`);
  }
  return stdout;
}

try {
  assert.equal(process.versions.node, pins.node, 'The contract requires the exact Node pin; no skip');
  evidence.npm = command('npm', ['--version'], source);
  assert.equal(evidence.npm, pins.npm);
  evidence.rust = command('rustc', ['--version']);
  const toolchain = readFileSync(join(root, 'rust-toolchain.toml'), 'utf8').match(/channel\s*=\s*"([^"]+)"/)[1];
  assert(evidence.rust.startsWith(`rustc ${toolchain} `), 'Use the committed Rust toolchain');
  evidence.provenance = JSON.parse(readFileSync(join(source, '.artifacts/provenance.json'), 'utf8'));
  assert.equal(evidence.provenance.sourceCommit, pins.commit);
  assert.equal(evidence.provenance.upstreamLockSha256, pins.lockSha256);
  assert.equal(evidence.provenance.node, pins.node);
  assert.equal(evidence.provenance.npm, pins.npm);
  assert.deepEqual(evidence.provenance.packages, pins.packages);
  evidence.lockSha256 = sha256(readFileSync(join(source, 'package-lock.json')));
  evidence.casesSha256 = sha256(readFileSync(join(source, 'contract-cases.json')));
  evidence.vectorsSha256 = sha256(readFileSync(join(source, 'contract-vectors.json')));
  run('format', 'cargo', ['fmt', '--all', '--', '--check']);
  run('clippy', 'cargo', ['clippy', '--workspace', '--all-targets', '--features', 'pi-contract', '--locked', '--', '-D', 'warnings']);
  run('typecheck', process.execPath, ['node_modules/typescript/bin/tsc', '--noEmit'], source);
  const files = readdirSync(join(source, 'test')).filter(file => file.endsWith('.test.ts')).sort().map(file => `test/${file}`);
  assert(files.length > 0, 'No Node contract tests found');
  const node = run('node-tests', process.execPath, ['--experimental-strip-types', '--test', '--test-reporter=tap', ...files], source);
  for (const field of ['fail', 'cancelled', 'skipped', 'todo']) {
    assert.match(node, new RegExp(`^# ${field} 0$`, 'm'), `Node ${field} must be zero`);
  }
  const nodePassed = new Set([...node.matchAll(/^ok \d+ - (.+)$/gm)].map(match => match[1]));
  const rust = run('rust-tests', 'cargo', ['test', '--workspace', '--features', 'pi-contract', '--locked']);
  const summaries = [...rust.matchAll(/^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;/gm)];
  assert(summaries.length > 0, 'No Rust test summaries found');
  for (const summary of summaries) assert(summary.slice(2).every(count => count === '0'), 'Rust tests must not fail, skip or filter');
  const rustPassed = new Set([...rust.matchAll(/^test (\S+) \.\.\. ok$/gm)].map(match => match[1].split('::').at(-1)));
  for (const [property, required] of Object.entries(cases)) {
    assert(required.node.length + required.rust.length > 0, `No evidence for ${property}`);
    for (const name of required.node) assert(nodePassed.has(name), `Missing Node evidence for ${property}: ${name}`);
    for (const name of required.rust) assert(rustPassed.has(name), `Missing Rust evidence for ${property}: ${name}`);
  }
  evidence.nodeTests = Number(node.match(/^# tests (\d+)$/m)[1]);
  evidence.rustTests = summaries.reduce((total, match) => total + Number(match[1]), 0);
  evidence.conformance = 'passed';
  console.log(`Contract passed: ${evidence.nodeTests} Node and ${evidence.rustTests} Rust tests, none skipped. Acceptance still requires review.`);
} catch (error) {
  evidence.error = String(error);
  process.stderr.write(`${error}\n`);
  process.exitCode = 1;
} finally {
  writeFileSync(join(output, 'evidence.json'), `${JSON.stringify(evidence, null, 2)}\n`);
}
