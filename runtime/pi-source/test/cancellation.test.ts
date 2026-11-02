import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { awaitWithContext } from '@earendil-works/chord/context';
import { Type } from '@earendil-works/pi-ai';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxAssistantMessage, fauxProvider, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import type { SqliteDatabase } from '@earendil-works/pi-durable/storage/sqlite';
import { defineTool } from '@earendil-works/pi-durable';
import { ADAPTER, CAPABILITIES, MAX_FRAME, MAX_PENDING, SOURCE, hash, type Json } from '../src/protocol.ts';
import { Runtime, type RuntimeBarriers } from '../src/runtime.ts';

function barrier() {
  let release!: () => void;
  const promise = new Promise<void>(resolve => { release = resolve; });
  return { promise, release };
}

async function fixture(barriers: RuntimeBarriers) {
  const directory = await mkdtemp(join(tmpdir(), 'switchyard-abort-'));
  const binding = { missionId: 'm', attemptId: 'a', sessionId: 's', generation: 1 };
  const entered = barrier();
  const faux = fauxProvider();
  faux.setResponses([fauxAssistantMessage([fauxToolCall('wait', {})], { stopReason: 'toolUse' }), fauxAssistantMessage('done')]);
  const models = createModels();
  models.setProvider(faux.provider);
  const tools = [defineTool({ name: 'wait', description: 'Cooperative cancellation fixture.', replay: 'safe',
    parameters: Type.Object({}), execute: async (_args, _api, context) => {
      entered.release();
      await awaitWithContext(new Promise<void>(() => {}), context);
      return { output: [] };
    } })];
  const runtime = new Runtime(join(directory, 'pi.sqlite'), binding, models, tools, barriers);
  let sequence = 0;
  const call = (method: string, commandId?: string, params: Record<string, Json> = {}) => runtime.dispatch({
    protocol: 0, kind: 'request', id: `host:${++sequence}`, binding, method, commandId, params,
  });
  await call('runtime.hello', undefined, { source: SOURCE, adapter: ADAPTER, maxFrame: MAX_FRAME,
    maxPending: MAX_PENDING, capabilities: CAPABILITIES });
  await call('runtime.open');
  return { call, entered, directory, failed: runtime.failed, async close() {
    await call('runtime.abort', 'cleanup');
    await call('runtime.close', 'close');
    await rm(directory, { recursive: true, force: true });
  } };
}

test('a delayed duplicate abort cannot cancel work admitted after the original settled', { timeout: 10_000 }, async () => {
  const firstRecorded = barrier();
  const first = barrier();
  const duplicate = barrier();
  let calls = 0;
  const p = await fixture({ afterAbortRecorded: async () => {
    calls++;
    if (calls === 1) { firstRecorded.release(); await first.promise; }
    else if (calls === 2) await duplicate.promise;
  } });
  try {
    const once = p.call('runtime.abort', 'stop');
    await firstRecorded.promise;
    const twice = p.call('runtime.abort', 'stop');
    // Drain the serialized Pi commit queue after both requests entered it.
    await p.call('runtime.inspect');
    first.release();
    const receipt = await once;
    await p.call('runtime.submit', 'new-work', { type: 'input', content: 'wait' });
    await p.entered.promise;
    duplicate.release();
    assert.deepEqual(await twice, receipt);
    assert.equal(calls, 1, 'coalesced duplicates must record one cancellation');
    const snapshot = await p.call('runtime.inspect', undefined, { commandIds: ['new-work'] }) as {
      commands: { status: string; reason: string | null }[]; runtimeIdle: boolean;
    };
    assert.notEqual(snapshot.commands[0].reason, 'aborted', 'the old duplicate cancelled newly admitted work');
    assert.equal(snapshot.runtimeIdle, false);
  } finally { first.release(); duplicate.release(); await p.close(); }
});

test('a pending abort fences new work and competing stops while retaining payload conflicts', { timeout: 10_000 }, async () => {
  const recorded = barrier();
  const release = barrier();
  const p = await fixture({ afterAbortRecorded: async () => { recorded.release(); await release.promise; } });
  try {
    const stopping = p.call('runtime.abort', 'stop');
    await recorded.promise;
    const payload = { missionId: 'm', attemptId: 'a', changed: true };
    await assert.rejects(p.call('runtime.abort', 'stop', { hostCommand: { payload, payloadHash: hash(payload) } }),
      { category: 'command_conflict' });
    await assert.rejects(p.call('runtime.abort', 'other-stop'), { category: 'stopping' });
    await assert.rejects(p.call('runtime.submit', 'new-work', { type: 'input', content: 'no' }), { category: 'stopping' });
    await assert.rejects(p.call('runtime.steer', 'steer', { type: 'input', content: 'no' }), { category: 'stopping' });
    const snapshot = await p.call('runtime.inspect', undefined, { commandIds: ['stop', 'other-stop', 'new-work', 'steer'] }) as {
      commands: { receipt: { settled: boolean } | null }[]; stopping: boolean;
    };
    assert.equal(snapshot.stopping, true);
    assert.equal(snapshot.commands[0].receipt?.settled, false);
    assert.deepEqual(snapshot.commands.slice(1).map(row => row.receipt), [null, null, null]);
    release.release();
    await stopping;
  } finally { release.release(); await p.close(); }
});

for (const stage of ['afterAbortRecorded', 'afterAbortSettled'] as const) {
  test(`close refuses an abort at ${stage} even on an idle runtime`, { timeout: 10_000 }, async () => {
    const recorded = barrier();
    const release = barrier();
    const p = await fixture({ [stage]: async () => { recorded.release(); await release.promise; } });
    const stopping = p.call('runtime.abort', 'stop').then(result => ({ result }), error => ({ error }));
    await recorded.promise;
    const closed = await p.call('runtime.close', 'close').then(result => ({ result }), error => ({ error }));
    release.release();
    const stopped = await stopping;
    const cleanup = await p.close().then(() => undefined, error => error);
    assert('error' in closed, 'close must not succeed while an abort still owns storage');
    assert.equal(closed.error.category, 'busy');
    assert('result' in stopped, 'the refused close must leave cancellation able to settle');
    assert.equal(cleanup, undefined);
  });
}

test('close refuses an admitted submit before its storage transaction starts', { timeout: 10_000 }, async () => {
  const entered = barrier();
  const release = barrier();
  const p = await fixture({ beforeAdmission: async () => { entered.release(); await release.promise; } });
  const submitting = p.call('runtime.submit', 'work', { type: 'input', content: 'wait' });
  try {
    await entered.promise;
    await assert.rejects(p.call('runtime.close', 'close'), { category: 'busy' });
    release.release();
    const admitted = await submitting as { receiptId: string };
    await p.entered.promise;
    const snapshot = await p.call('runtime.inspect', undefined, { commandIds: ['work'] }) as {
      commands: { receipt: { receipt: string } }[];
    };
    assert.equal(snapshot.commands[0].receipt.receipt, admitted.receiptId);
  } finally { release.release(); await submitting; await p.close(); }
});

test('an admitted close fences later submit, abort and inspection through completion', { timeout: 10_000 }, async () => {
  const entered = barrier();
  const release = barrier();
  const p = await fixture({ beforeClose: async () => { entered.release(); await release.promise; } });
  const closing = p.call('runtime.close', 'close');
  try {
    await entered.promise;
    for (const method of ['runtime.submit', 'runtime.abort', 'runtime.inspect', 'runtime.close']) {
      await assert.rejects(p.call(method, 'late', { type: 'input', content: 'late' }), { category: 'session_unavailable' });
    }
    release.release();
    assert.deepEqual(await closing, { closed: true });
    await assert.rejects(p.call('runtime.abort', 'late'), { category: 'session_unavailable' });
    await assert.rejects(p.call('runtime.open'), { category: 'already_open' });
  } finally { release.release(); await closing; await rm(p.directory, { recursive: true, force: true }); }
});

test('a storage failure inside Pi cancellation retains the SQLite cause', { timeout: 10_000 }, async () => {
  let database: SqliteDatabase;
  const p = await fixture({ databaseOpened: db => { database = db; },
    afterAbortRecorded: async () => { await database.exec('PRAGMA query_only = ON'); } });
  try {
    await p.call('runtime.submit', 'work', { type: 'input', content: 'wait' });
    await p.entered.promise;
    await assert.rejects(p.call('runtime.abort', 'stop'), /attempt to write a readonly database/);
    const error = await p.failed;
    assert.match(String(error), /attempt to write a readonly database/);
    await assert.rejects(p.call('runtime.inspect'), { category: 'session_unavailable' });
  } finally { await rm(p.directory, { recursive: true, force: true }); }
});
