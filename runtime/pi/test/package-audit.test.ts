import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import test from 'node:test';
import { auditPackage } from '../src/package-audit.ts';
import { probeChangedPayload, probeIdleSteering } from '../src/behavior-probes.ts';
import { adoptionDecision } from '../src/feasibility.ts';

test('removing package blockers still requires the full S2 gate', () => {
  assert.equal(adoptionDecision(['incompatible API']), 'NO-GO');
  assert.equal(adoptionDecision([]), 'NEEDS-FULL-S2');
});

test('installed source matches the published tag, but not the accepted source baseline', async () => {
  const audit = await auditPackage();
  assert.equal(audit.pinMatches, true, 'Run under the exact .node-version and lockfile');
  assert(audit.sources.every(source => source.matchesPublished));
  assert(audit.sources.every(source => !source.matchesApproved));
  assert.equal(audit.declaredCapabilities.transactionalSubmit, false);
  assert.equal(audit.declaredCapabilities.rejectIdleInput, false);
  assert.equal(audit.declaredCapabilities.continueSubmission, false);
  assert.equal(audit.declaredCapabilities.durableToolTaskId, true);
  assert.equal(audit.declaredCapabilities.nestedToolCalls, true);
});

test('the published runtime ignores idle rejection and starts a faux-model run', { timeout: 10_000 }, async () => {
  assert.deepEqual(await probeIdleSteering(), { rejected: false, status: 'done', modelCalls: 1 });
});

test('Pi request deduplication alone does not detect changed payloads', { timeout: 10_000 }, async () => {
  assert.deepEqual(await probeChangedPayload(), { sameSubmission: true, modelCalls: 1, userEntries: 1 });
});

test('a Pi-only passive submission survives SIGKILL after admission and reopens once', { timeout: 15_000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'switchyard-pi-audit-'));
  const childPath = fileURLToPath(new URL('./recovery-child.ts', import.meta.url));
  const database = join(directory, 'pi.sqlite');
  const child = spawn(process.execPath, ['--experimental-strip-types', childPath, database, 'admit'], {
    stdio: ['ignore', 'pipe', 'pipe'], env: {},
  });
  let diagnostics = '';
  child.stderr.on('data', chunk => { diagnostics = (diagnostics + chunk).slice(-4096); });
  const exited = new Promise<{ code: number | null; signal: NodeJS.Signals | null }>((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', (code, signal) => resolve({ code, signal }));
  });
  try {
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`Admission timeout: ${diagnostics}`)), 5_000);
      let output = '';
      child.stdout.on('data', chunk => {
        output += chunk;
        if (output === 'admitted\n') { clearTimeout(timer); resolve(); }
        else if (output.length > 128) { clearTimeout(timer); reject(new Error('Unexpected child output')); }
      });
      exited.then(() => { clearTimeout(timer); reject(new Error(`Child exited before kill: ${diagnostics}`)); }, reject);
    });
    child.kill('SIGKILL');
    assert.equal((await exited).signal, 'SIGKILL');
    const { stdout } = await promisify(execFile)(process.execPath,
      ['--experimental-strip-types', childPath, database, 'recover'], { timeout: 5_000, env: {}, maxBuffer: 64 * 1024 });
    const recovered = JSON.parse(stdout);
    assert.equal(Number.isSafeInteger(recovered.existingId), true);
    assert(recovered.existingId > 0);
    assert.equal(recovered.replayId, recovered.existingId);
    assert.equal(recovered.entryCount, 1);
    assert.equal(recovered.status, 'done');
  } finally {
    child.kill('SIGKILL');
    await exited.catch(() => {});
    await rm(directory, { recursive: true, force: true });
  }
});

test('the adoption gate remains nonzero; a successful audit is not S2 GO', { timeout: 15_000 }, async () => {
  const script = fileURLToPath(new URL('../src/feasibility.ts', import.meta.url));
  try {
    await promisify(execFile)(process.execPath, ['--experimental-strip-types', script, '--gate'], {
      timeout: 10_000, env: {}, maxBuffer: 64 * 1024,
    });
    assert.fail('The unproven S2 gate must not succeed');
  } catch (error) {
    const failure = error as { code?: number; stdout?: string };
    assert.equal(failure.code, 1);
    const report = JSON.parse(failure.stdout!);
    assert.equal(report.decision, 'NO-GO');
    assert(report.blockers.length > 0, 'NO-GO must be backed by observed incompatibilities');
  }
});
