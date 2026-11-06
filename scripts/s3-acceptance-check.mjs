// Evidence collection gate, not the reviewed S3 acceptance decision.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, rmSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const hash = value => createHash('sha256').update(value).digest('hex');
const nonempty = value => assert.ok(typeof value === 'string' && value.length > 0, 'missing identity');
const emptyProof = proof => {
  assert.equal(proof?.proven, true, 'missing proven termination');
  assert.ok(proof.events?.split('\n').includes('populated 0'), 'missing fresh empty proof');
  nonempty(proof.cgroup?.path);
};
const requiredProbes = [
  'host TCP denied', 'host Unix socket denied', 'metadata TCP denied', 'metadata IPv6 denied',
  'public TCP denied', 'dns TCP denied', 'stub TCP denied', 'other Unix socket denied',
  'dns lookup denied', 'loopback listener denied', 'existing Node executable cannot spawn',
  'credential and configuration environment absent', 'configured model endpoint reachable',
  'worker inherits model-endpoint denial',
];

export function validate(report, expected) {
  assert.equal(report.schema, 1);
  assert.equal(report.scenario, 'host-pi-linux-egress-recovery-stop-v1');
  assert.equal(report.decision, 'PENDING_REVIEW', 'collection must not grant S3 approval');
  assert.equal(report.revision, expected.revision);
  assert.equal(report.tree, expected.tree);
  assert.deepEqual(report.pins, expected.pins);
  assert.equal(report.platform, 'linux-x86_64');
  assert.equal(report.environmentEvidence, 'bridge-environment.txt');
  assert.deepEqual(report.scope, {
    seededPlan: true, model: 'faux', endpoint: 'local startup connectivity probe', liveProvider: false, productEnabled: false,
  });
  for (const key of ['missionId', 'attemptId', 'sessionId', 'approvalCommandId']) nonempty(report[key]);
  assert.equal(report.crash?.signal, 9);
  assert.equal(report.crash.barrier, 'receipt');
  assert.ok(Number.isSafeInteger(report.crash.generation) && report.crash.generation > 0);
  const [operationId, receipt] = report.crash.receipt ?? [];
  assert.match(operationId ?? '', /^op-v1:[0-9a-f]{64}$/);
  nonempty(receipt);
  assert.equal(JSON.parse(receipt).operationId, operationId);
  const recovery = report.recovery;
  assert.ok(Number.isSafeInteger(recovery?.generation) && recovery.generation > report.crash.generation);
  for (const key of ['stableFileReceipt', 'fileNotRepeated', 'processNotRelaunched', 'staleBindingRejected']) assert.equal(recovery[key], true, key);
  assert.equal(recovery.effects?.length, 3);
  for (const method of ['write_file', 'exec_start', 'exec_status']) {
    assert.equal(recovery.effects.filter(effect => effect.method === method).length, 1, method);
  }
  const effectIds = recovery.effects.map(effect => effect.operationId).sort();
  assert.equal(new Set(effectIds).size, 3);
  assert.ok(effectIds.includes(operationId));
  for (const effect of recovery.effects) {
    assert.match(effect.operationId, /^op-v1:[0-9a-f]{64}$/);
    assert.equal(effect.simulated, false);
    assert.equal(effect.receiptRetained, effect.method !== 'exec_start');
  }
  const start = recovery.effects.find(effect => effect.method === 'exec_start');
  const status = recovery.effects.find(effect => effect.method === 'exec_status');
  assert.equal(start.state, 'running');
  const observation = recovery.processObservation;
  assert.equal(observation?.operationId, status.operationId);
  assert.equal(observation.startOperationId, start.operationId);
  assert.equal(observation.startState, 'running');
  assert.equal(observation.startReceiptRetained, false, 'a running handle is not an exit receipt');
  assert.equal(observation.quiescence?.proven, false);
  assert.equal(observation.simulated, false);
  assert.equal(observation.backend, 'linux-bubblewrap-0.9.0');
  const retired = recovery.oldRetirement;
  assert.equal(retired?.removed, true);
  assert.equal(retired.generation, report.crash.generation);
  assert.deepEqual(retired.authorization?.cgroup, retired.cgroup);
  emptyProof(retired.authorization);
  for (const [key, phase] of [['pause', 'paused'], ['cancel', 'cancelled']]) {
    const stopped = report[key];
    assert.equal(stopped?.phase, phase);
    assert.equal(stopped.executionBlocked, false);
    assert.equal(stopped.origin, 'host_supervisor');
    emptyProof(stopped.bridgeTermination);
  }
  assert.equal(report.pause.heartbeatStopped, true);
  assert.deepEqual(report.pause.effects?.map(effect => effect.operationId).sort(), effectIds);
  assert.equal(report.pause.effects.find(effect => effect.method === 'exec_start')?.quiescence, 'populated 0');
  assert.equal(report.pause.processReceipt?.operationId, start.operationId);
  assert.equal(report.pause.processReceipt.disposition, 'cancelled');
  assert.equal(report.pause.processReceipt.simulated, false);
  assert.equal(report.pause.processReceipt.backend, 'linux-bubblewrap-0.9.0');
  assert.deepEqual(report.cancel.processReceipt, report.pause.processReceipt);
  assert.deepEqual(report.inertReopen, { phase: 'paused', noNewEffects: true });
  const egress = report.egress;
  const endpoint = new URL(egress?.endpoint);
  assert.equal(endpoint.protocol, 'http:');
  assert.equal(endpoint.hostname, '127.0.0.1');
  assert.equal(endpoint.username + endpoint.password, '');
  assert.equal(egress.decoyHits, 0);
  assert.ok(Array.isArray(egress.generations) && egress.generations.length >= 2);
  assert.ok(egress.generations.includes(report.crash.generation));
  assert.ok(egress.generations.includes(recovery.generation));
  assert.equal(egress.probes?.length, egress.generations.length);
  assert.equal(egress.stubRequests, egress.generations.length);
  for (const [index, generation] of egress.generations.entries()) {
    assert.ok(Number.isSafeInteger(generation) && generation > 0);
    if (index) assert.ok(generation > egress.generations[index - 1]);
    const probe = egress.probes[index];
    assert.equal(probe.generation, generation);
    assert.match(probe.token ?? '', /^[0-9a-f]{32}$/);
    assert.equal(probe.token, egress.probes[0].token);
    for (const required of requiredProbes) assert.ok(probe.probes?.includes(required), `missing ${required}`);
    for (const prefix of ['read ', 'write ']) assert.ok(probe.probes.filter(value => value.startsWith(prefix)).length >= 4);
  }
  for (const key of ['beforeCrash', 'recovered', 'final']) assert.match(report.receipts?.[key] ?? '', /^sha256:[0-9a-f]{64}$/);
  assert.deepEqual(report.receipts.recoveredFile, report.crash.receipt);
  assert.deepEqual(report.receipts.finalFile, report.crash.receipt);
  assert.deepEqual(report.originalBefore, report.originalAfter);
  assert.equal(report.originalBefore?.status, '');
  assert.equal(report.originalBefore.diff, '');
  for (const key of ['branch', 'head', 'index', 'readme']) nonempty(report.originalBefore[key]);
}

export function collect(directory, root) {
  rmSync(resolve(directory, 's3-composed-evidence.json'), { force: true });
  const git = (...args) => execFileSync('git', ['-C', root, ...args], { encoding: 'utf8' }).trim();
  assert.equal(git('status', '--porcelain'), '', 'dirty source tree cannot produce acceptance evidence');
  const expected = {
    revision: git('rev-parse', 'HEAD'), tree: git('rev-parse', 'HEAD^{tree}'),
    pins: JSON.parse(readFileSync(resolve(root, 'runtime/pi-source/pins.json'), 'utf8')),
  };
  const report = JSON.parse(readFileSync(resolve(directory, 's3-composed-acceptance.json'), 'utf8'));
  validate(report, expected);
  const environment = readFileSync(resolve(directory, 'bridge-environment.txt'), 'utf8').split('\n');
  assert.equal(environment[0], expected.revision);
  assert.equal(environment[1], expected.tree);
  const log = readFileSync(resolve(directory, 'pi-host-dispatch.log'), 'utf8').replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '');
  assert.match(log, /test composed::s3_composed_acceptance_recovers_stops_and_enforces_egress \.\.\. ok/);
  const summaries = log.match(/^test result: .+$/gm) ?? [];
  assert.equal(summaries.length, 1, 'expected exactly one complete Host-suite summary');
  assert.match(summaries[0], /^test result: ok\. [1-9][0-9]* passed; 0 failed; 0 ignored; 0 measured; 0 filtered out(?:;|$)/,
    'failed, ignored, measured-only, or filtered Host evidence is incomplete');
  const files = ['s3-composed-acceptance.json', 'pi-host-dispatch.log', 'bridge-environment.txt', 'cgroup-delegation.txt'];
  const manifest = {
    schema: 1, decision: 'PENDING_REVIEW', ...expected,
    files: files.map(name => ({ name, sha256: hash(readFileSync(resolve(directory, name))) })),
  };
  writeFileSync(resolve(directory, 's3-composed-evidence.json'), `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const directory = process.env.SWITCHYARD_CONFINEMENT_EVIDENCE;
  assert.ok(directory, 'SWITCHYARD_CONFINEMENT_EVIDENCE is required; no skip');
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  collect(directory, root);
  console.log('Composed S3 evidence collected and hash-bound; explicit acceptance review is still required.');
}
