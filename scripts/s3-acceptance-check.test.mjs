import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { collect, validate } from './s3-acceptance-check.mjs';

const pins = JSON.parse(readFileSync(new URL('../runtime/pi-source/pins.json', import.meta.url), 'utf8'));
const expected = { revision: 'a'.repeat(40), tree: 'b'.repeat(40), pins };
const ids = ['1', '2', '3'].map(value => `op-v1:${value.repeat(64)}`);
const cgroup = { path: '/delegated/bridge-old', dev: 1, ino: 2, boot: 'fixture-boot' };
const proof = { proven: true, events: 'populated 0\nfrozen 0\n', cgroup };
const probes = [
  'host TCP denied', 'host Unix socket denied', 'metadata TCP denied', 'metadata IPv6 denied',
  'public TCP denied', 'dns TCP denied', 'stub TCP denied', 'other Unix socket denied',
  'dns lookup denied', 'loopback listener denied', 'existing Node executable cannot spawn',
  'credential and configuration environment absent', 'configured model endpoint reachable', 'worker inherits model-endpoint denial',
  ...['read ', 'write '].flatMap(prefix => ['a', 'b', 'c', 'd'].map(name => prefix + name)),
];

function report() {
  const effects = ['write_file', 'exec_start', 'exec_status'].map((method, index) => ({
    method, operationId: ids[index], simulated: false, receiptRetained: method !== 'exec_start', state: method === 'exec_start' ? 'running' : 'settled',
  }));
  const processReceipt = { operationId: ids[1], disposition: 'cancelled', simulated: false, backend: 'linux-bubblewrap-0.9.0' };
  const original = { branch: 'refs/heads/main', head: 'c'.repeat(40), index: 'sha256:' + 'd'.repeat(64), readme: 'sha256:' + 'e'.repeat(64), status: '', diff: '' };
  return structuredClone({
    schema: 1, scenario: 'host-pi-linux-egress-recovery-stop-v1', decision: 'PENDING_REVIEW', ...expected,
    platform: 'linux-x86_64', environmentEvidence: 'bridge-environment.txt',
    scope: { seededPlan: true, model: 'faux', endpoint: 'local startup connectivity probe', liveProvider: false, productEnabled: false },
    missionId: 'm', attemptId: 'a', sessionId: 's', approvalCommandId: 'cmd-original',
    crash: { signal: 9, barrier: 'receipt', generation: 2, receipt: [ids[0], JSON.stringify({ operationId: ids[0] })] },
    recovery: { generation: 3, stableFileReceipt: true, fileNotRepeated: true, processNotRelaunched: true, staleBindingRejected: true,
      effects, processObservation: { operationId: ids[2], startOperationId: ids[1], startState: 'running', startReceiptRetained: false,
        quiescence: { proven: false }, simulated: false, backend: 'linux-bubblewrap-0.9.0' },
      oldRetirement: { generation: 2, cgroup, removed: true, authorization: proof } },
    pause: { phase: 'paused', executionBlocked: false, origin: 'host_supervisor', bridgeTermination: proof, heartbeatStopped: true,
      effects: effects.map(effect => ({ ...effect, quiescence: 'populated 0', receiptRetained: true })), processReceipt },
    inertReopen: { phase: 'paused', noNewEffects: true },
    cancel: { phase: 'cancelled', executionBlocked: false, origin: 'host_supervisor', bridgeTermination: proof, processReceipt },
    egress: { endpoint: 'http://127.0.0.1:12345/v1', generations: [2, 3], stubRequests: 2, decoyHits: 0,
      probes: [2, 3].map(generation => ({ generation, probes, token: 'f'.repeat(32) })) },
    receipts: { beforeCrash: 'sha256:' + '1'.repeat(64), recovered: 'sha256:' + '2'.repeat(64), final: 'sha256:' + '3'.repeat(64),
      recoveredFile: [ids[0], JSON.stringify({ operationId: ids[0] })], finalFile: [ids[0], JSON.stringify({ operationId: ids[0] })] },
    originalBefore: original, originalAfter: original,
  });
}

test('valid bounded evidence remains pending review', () => validate(report(), expected));

for (const [name, change] of [
  ['automatic acceptance', value => { value.decision = 'GO'; }],
  ['wrong revision', value => { value.revision = '0'.repeat(40); }],
  ['changed pins', value => { value.pins.node = 'latest'; }],
  ['enabled product', value => { value.scope.productEnabled = true; }],
  ['missing identity', value => { delete value.sessionId; }],
  ['missing receipt', value => { value.crash.receipt = [ids[0], null]; }],
  ['repeated file effect', value => { value.recovery.fileNotRepeated = false; }],
  ['changed final receipt', value => { value.receipts.finalFile[1] = 'changed'; }],
  ['no real process', value => { value.recovery.effects[1].simulated = true; }],
  ['invented running-process exit receipt', value => { value.recovery.effects[1].receiptRetained = true; }],
  ['wrong observed process handle', value => { value.recovery.processObservation.startOperationId = ids[0]; }],
  ['missing cancellation receipt', value => { delete value.pause.processReceipt; }],
  ['missing status effect', value => { value.recovery.effects.pop(); }],
  ['stale generation', value => { value.recovery.generation = 2; }],
  ['unproven retirement', value => { value.recovery.oldRetirement.authorization.proven = false; }],
  ['unproven bridge stop', value => { value.pause.bridgeTermination.events = 'populated 1\n'; }],
  ['writer remains live', value => { value.pause.effects[1].quiescence = 'nonempty'; }],
  ['new effects on inert reopen', value => { value.inertReopen.noNewEffects = false; }],
  ['missing denial probe', value => { value.egress.probes[0].probes.splice(0, 1); }],
  ['nonlocal endpoint', value => { value.egress.endpoint = 'http://169.254.169.254/'; }],
  ['unexpected network request', value => { value.egress.stubRequests += 1; }],
  ['changed original checkout', value => { value.originalAfter.index = 'changed'; }],
]) {
  test(`reject ${name}`, () => {
    const value = report();
    // Separate original snapshots: structuredClone preserves shared references.
    value.originalAfter = { ...value.originalAfter };
    change(value);
    assert.throws(() => validate(value, expected));
  });
}

test('collection binds exact files and rejects filtered, absent, or dirty evidence', () => {
  const temporary = mkdtempSync(join(tmpdir(), 'switchyard-s3-evidence-'));
  try {
    const root = join(temporary, 'source');
    const directory = join(temporary, 'evidence');
    mkdirSync(join(root, 'runtime/pi-source'), { recursive: true });
    mkdirSync(directory);
    writeFileSync(join(root, 'runtime/pi-source/pins.json'), JSON.stringify(pins));
    const git = (...args) => execFileSync('git', ['-C', root, ...args], { encoding: 'utf8' }).trim();
    git('init', '--initial-branch=main');
    git('config', 'user.email', 'fixture@example.com');
    git('config', 'user.name', 'Fixture');
    git('add', '.');
    git('commit', '-m', 'fixture');
    const value = report();
    value.revision = git('rev-parse', 'HEAD');
    value.tree = git('rev-parse', 'HEAD^{tree}');
    const name = join(directory, 's3-composed-acceptance.json');
    writeFileSync(name, JSON.stringify(value));
    writeFileSync(join(directory, 'bridge-environment.txt'), `${value.revision}\n${value.tree}\nfixture\n`);
    writeFileSync(join(directory, 'cgroup-delegation.txt'), 'fixture delegation\n');
    const log = 'test composed::s3_composed_acceptance_recovers_stops_and_enforces_egress ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n';
    writeFileSync(join(directory, 'pi-host-dispatch.log'), log);
    const manifest = collect(directory, root);
    assert.equal(manifest.files.length, 4);
    assert.equal(manifest.decision, 'PENDING_REVIEW');
    assert.ok(manifest.files.every(file => /^[0-9a-f]{64}$/.test(file.sha256)));
    assert.deepEqual(JSON.parse(readFileSync(join(directory, 's3-composed-evidence.json'), 'utf8')), manifest);
    // The named composed case alone is not a complete Host-suite run.
    writeFileSync(join(directory, 'pi-host-dispatch.log'), log.replace('0 filtered out', '30 filtered out'));
    assert.throws(() => collect(directory, root));
    writeFileSync(join(directory, 'pi-host-dispatch.log'), 'test result: ok. 0 passed; 0 failed; 0 ignored; 1 filtered out\n');
    assert.throws(() => collect(directory, root));
    assert.throws(() => readFileSync(join(directory, 's3-composed-evidence.json')), /ENOENT/);
    writeFileSync(join(directory, 'pi-host-dispatch.log'), log);
    writeFileSync(join(root, 'unreviewed.txt'), 'dirty');
    assert.throws(() => collect(directory, root), /dirty source/);
    unlinkSync(join(root, 'unreviewed.txt'));
    unlinkSync(name);
    assert.throws(() => collect(directory, root), /ENOENT/);
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
});
