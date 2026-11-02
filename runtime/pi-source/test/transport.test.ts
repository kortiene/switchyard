import assert from 'node:assert/strict';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { ADAPTER, canonical, effectError, MAX_FRAME, MAX_PENDING, SOURCE, type Binding, type Frame, type Json } from '../src/protocol.ts';
import { Transport } from '../src/transport.ts';

const binding: Binding = { missionId: 'm', attemptId: 'a', sessionId: 's', generation: 1 };
const hello = { source: SOURCE, adapter: ADAPTER, maxFrame: MAX_FRAME, maxPending: 16, capabilities: ['durable-admission'] };

async function peer(mode = 'answer', options: { directory?: string; generation?: number; cleanup?: boolean } = {}) {
  const directory = options.directory ?? await mkdtemp(join(tmpdir(), 'switchyard-wire-'));
  const cleanup = options.cleanup ?? options.directory === undefined;
  const session: Binding = { ...binding, generation: options.generation ?? binding.generation };
  const child: ChildProcessWithoutNullStreams = spawn(process.execPath, ['--experimental-strip-types',
    fileURLToPath(new URL('./bridge-child.ts', import.meta.url)), join(directory, 'pi.sqlite'), JSON.stringify(session), mode],
    { env: {}, stdio: ['pipe', 'pipe', 'pipe'] });
  let diagnostics = '';
  child.stderr.on('data', chunk => { diagnostics = (diagnostics + chunk.toString()).slice(-4096); });
  let buffer = '';
  let id = 0;
  const effects: Frame[] = [];
  const pending = new Map<string, { resolve: (frame: Frame) => void; reject: (error: Error) => void }>();
  child.stdout.on('data', chunk => {
    buffer += chunk.toString();
    while (buffer.includes('\n')) {
      const end = buffer.indexOf('\n');
      const frame = JSON.parse(buffer.slice(0, end)) as Frame;
      buffer = buffer.slice(end + 1);
      if (frame.kind === 'request') { effects.push(frame); continue; }
      pending.get(frame.id)?.resolve(frame);
      pending.delete(frame.id);
    }
  });
  const exited = new Promise<number | null>((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', code => {
      for (const wait of pending.values()) wait.reject(new Error('peer exited'));
      pending.clear(); resolve(code);
    });
  });
  const request = (method: string, params: Record<string, Json>, extra: Partial<Frame> = {}) => {
    const wireId = `host:${++id}`;
    return new Promise<Frame>((resolve, reject) => {
      pending.set(wireId, { resolve, reject });
      child.stdin.write(`${JSON.stringify({ protocol: 0, kind: 'request', id: wireId, method, binding: session, params, ...extra })}\n`);
    });
  };
  return { child, exited, request, effects, directory, diagnostics: () => diagnostics, async close() {
    child.kill('SIGKILL'); await exited; if (cleanup) await rm(directory, { recursive: true, force: true });
  } };
}

test('sustained bounded request bursts leave abort and inspection responsive', { timeout: 30_000 }, async () => {
  const rounds = 8;
  const p = await peer('cooperative-wait');
  try {
    assert.equal((await p.request('runtime.hello', hello)).error, undefined);
    assert.equal((await p.request('runtime.open', {})).error, undefined);
    const entered = () => p.diagnostics().split('waiting tool entered').length - 1;
    for (let round = 0; round < rounds; round++) {
      const work = `work-${round}`;
      const stop = `stop-${round}`;
      assert.equal((await p.request('runtime.submit', { type: 'input', content: 'wait' }, { commandId: work })).error, undefined);
      const limit = Date.now() + 3_000;
      while (entered() < round + 1 && Date.now() < limit) await new Promise(resolve => setTimeout(resolve, 5));
      assert.equal(entered(), round + 1, `round ${round} must enter a fresh waiting tool`);
      const responses = await Promise.all(Array.from({ length: MAX_PENDING }, (_, i) => i === 0
        ? p.request('runtime.abort', {}, { commandId: stop })
        : p.request('runtime.inspect', { commandIds: [work, stop] })));
      for (const response of responses) assert.equal(response.error, undefined, JSON.stringify(response.error));
      assert.deepEqual(responses[0].result, { commandId: stop, accepted: true, runtimeSettled: true });
    }
    const ids = Array.from({ length: rounds }, (_, round) => [`work-${round}`, `stop-${round}`]).flat();
    const snapshot = (await p.request('runtime.inspect', { commandIds: ids })).result as {
      commands: { reason: string | null; receipt: { settled: boolean } | null }[]; runtimeIdle: boolean;
    };
    for (let round = 0; round < rounds; round++) {
      assert.equal(snapshot.commands[round * 2].reason, 'aborted', `work-${round}`);
      assert.equal(snapshot.commands[round * 2 + 1].receipt?.settled, true, `stop-${round}`);
    }
    assert.equal(snapshot.runtimeIdle, true);
    assert.equal(p.child.exitCode, null);
  } finally { await p.close(); }
});

test('pending request overflow exits and an interrupted abort recovers from its durable intent', { timeout: 15_000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'switchyard-overflow-'));
  const p = await peer('hold-abort', { directory, cleanup: false });
  try {
    await p.request('runtime.hello', hello);
    await p.request('runtime.open', {});
    const held = [p.request('runtime.abort', {}, { commandId: 'stop' }).catch(() => null)];
    const limit = Date.now() + 3_000;
    while (!p.diagnostics().includes('abort recorded') && Date.now() < limit) await new Promise(resolve => setTimeout(resolve, 5));
    assert(p.diagnostics().includes('abort recorded'), 'stop must be durably recorded before overflow');
    for (let i = 0; i < MAX_PENDING; i++) held.push(p.request('runtime.abort', {}, { commandId: 'stop' }).catch(() => null));
    assert.equal(await p.exited, 1);
    assert((await Promise.all(held)).every(frame => frame === null), 'no stop acknowledgement may escape');
  } finally { await p.close(); }
  const resumed = await peer('recover', { directory, generation: 2, cleanup: false });
  try {
    await resumed.request('runtime.hello', hello);
    await resumed.request('runtime.open', { requireExisting: true });
    const before = (await resumed.request('runtime.inspect', { commandIds: ['stop'] })).result as {
      stopping: boolean; scheduling: string; commands: { receipt: { receipt: string; settled: boolean } }[];
    };
    assert.equal(before.scheduling, 'paused');
    assert.equal(before.stopping, true);
    assert.equal(before.commands[0].receipt.settled, false);
    assert.equal(before.commands[0].receipt.receipt, 'stop');
    const result = await resumed.request('runtime.abort', {}, { commandId: 'stop' });
    assert.equal(result.error, undefined);
    assert.deepEqual(result.result, { commandId: 'stop', accepted: true, runtimeSettled: true });
  } finally { await resumed.close(); await rm(directory, { recursive: true, force: true }); }
});

test('shared JSON subset rejects floats, unsafe integers and lone surrogates', () => {
  assert.equal(canonical({ '\uE000': 1, '\u{10000}': 2, text: 'é\n"\\' }), '{"text":"é\\n\\"\\\\","𐀀":2,"":1}');
  for (const value of [0.5, 1e21, 9_007_199_254_740_992, '\uD800']) assert.throws(() => canonical(value));
  assert.equal(canonical([-9_007_199_254_740_991, 9_007_199_254_740_991]), '[-9007199254740991,9007199254740991]');
});

test('the effect queue rejects a non-effect Pi-to-Rust method before any write', async () => {
  const transport = new Transport(binding);
  await assert.rejects(
    () => transport.request('runtime.hello', {}),
    /only execution\.request may wait for an effect response/,
  );
});

test('effect errors retain only the three owner-defined dispositions', () => {
  for (const [category, admission] of [['execution_rejected', 'rejected'],
    ['operation_conflict', 'rejected'], ['execution_uncertain', 'uncertain']]) {
    const error = effectError({ category, admission });
    assert.equal(error.category, category);
    assert.equal(error.admission, admission);
  }
});

for (const error of [null, {}, { category: 'future_error', admission: 'rejected' },
  { category: 'execution_uncertain', admission: 'rejected' },
  { category: 'operation_conflict', admission: 'uncertain' },
  { category: 'execution_rejected', admission: 'rejected', receipt: 'forged' }]) {
  test(`invalid effect error fails closed: ${JSON.stringify(error)}`, { timeout: 10_000 }, async () => {
    const p = await peer('tools');
    try {
      await p.request('runtime.hello', hello);
      await p.request('runtime.open', {});
      await p.request('runtime.submit', { type: 'input', content: 'write' }, { commandId: 'write' });
      const limit = Date.now() + 2_000;
      while (p.effects.length === 0 && Date.now() < limit) await new Promise(resolve => setTimeout(resolve, 5));
      assert(p.effects.length > 0, 'effect must be pending');
      p.child.stdin.write(`${JSON.stringify({ protocol: 0, kind: 'response', id: p.effects[0].id, binding, error })}\n`);
      assert.equal(await p.exited, 1);
    } finally { await p.close(); }
  });
}

test('abort settles Pi without claiming that unacknowledged execution has stopped', { timeout: 10_000 }, async () => {
  const p = await peer('tools');
  try {
    await p.request('runtime.hello', hello);
    await p.request('runtime.open', {});
    await p.request('runtime.submit', { type: 'input', content: 'write' }, { commandId: 'write' });
    const limit = Date.now() + 2_000;
    while (p.effects.length === 0 && Date.now() < limit) await new Promise(resolve => setTimeout(resolve, 5));
    assert(p.effects.length > 0, 'at least one remote effect is pending');
    const abort = await p.request('runtime.abort', {}, { commandId: 'stop' });
    assert.equal(abort.error, undefined);
    assert.deepEqual(abort.result, { commandId: 'stop', accepted: true, runtimeSettled: true });
    const replay = await p.request('runtime.abort', {}, { commandId: 'stop' });
    assert.deepEqual(replay.result, abort.result);
    // External effects may still exist. Only the Rust supervisor can settle them.
    for (const effect of p.effects) p.child.stdin.write(`${JSON.stringify({ protocol: 0, kind: 'response',
      id: effect.id, binding, result: { receipt: 'external-result' } })}\n`);
    const inspect = await p.request('runtime.inspect', { commandIds: ['write', 'stop'] });
    const snapshot = inspect.result as Record<string, Json>;
    assert.equal(snapshot.runtimeIdle, true);
    assert.equal(snapshot.quiescent, undefined);
    assert.equal(snapshot.verified, undefined);
    assert.equal((await p.request('runtime.close', {}, { commandId: 'close' })).error, undefined);
  } finally { await p.close(); }
});

test('protocol, capabilities, binding and generation are checked before opening storage', { timeout: 10_000 }, async () => {
  const p = await peer();
  try {
    assert.equal((await p.request('runtime.open', {})).error?.category, 'handshake_required');
    assert.equal((await p.request('runtime.hello', hello, { protocol: 9 })).error?.category, 'incompatible_protocol');
    assert.equal((await p.request('runtime.hello', { ...hello, capabilities: ['unknown'] })).error?.category, 'unsupported_capability');
    assert.equal((await p.request('runtime.hello', hello, { binding: { ...binding, sessionId: 'other' } })).error?.category, 'binding_mismatch');
    assert.equal((await p.request('runtime.hello', hello, { binding: { ...binding, generation: 0 } })).error?.category, 'stale_generation');
    assert.equal((await p.request('runtime.hello', hello)).error, undefined);
    assert.equal((await p.request('runtime.open', {})).error, undefined);
    assert.equal((await p.request('verification_pass', {})).error?.category, 'unsupported_method');
    assert.equal((await p.request('runtime.submit', { type: 'input', content: 'x', approved: true }, { commandId: 'x' })).error?.category, 'invalid_params');
  } finally { await p.close(); }
});

test('crash-after-admission kills a fresh submit and leaves an idempotent replay alive', { timeout: 15_000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'switchyard-crash-'));
  const first = await peer('crash-after-admission', { directory, generation: 1, cleanup: false });
  try {
    assert.equal((await first.request('runtime.hello', hello)).error, undefined);
    assert.equal((await first.request('runtime.open', {})).error, undefined);
    let frame: Frame | undefined;
    const submitted = first.request('runtime.submit', { type: 'input', content: 'plan' }, { commandId: 'c1' })
      .then(value => { frame = value; }, () => undefined);
    const code = await first.exited;
    await submitted;
    assert.equal(frame, undefined);
    assert.equal(code, null);
  } finally { await first.close(); }

  const replay = await peer('crash-after-admission', { directory, generation: 2, cleanup: false });
  try {
    assert.equal((await replay.request('runtime.hello', hello)).error, undefined);
    assert.equal((await replay.request('runtime.open', { requireExisting: true })).error, undefined);
    const once = await replay.request('runtime.submit', { type: 'input', content: 'plan' }, { commandId: 'c1' });
    const twice = await replay.request('runtime.submit', { type: 'input', content: 'plan' }, { commandId: 'c1' });
    assert.equal(once.error, undefined);
    assert.equal(twice.error, undefined);
    const receipt = (once.result as { receiptId?: string }).receiptId;
    assert.equal(typeof receipt, 'string');
    assert.equal((twice.result as { receiptId?: string }).receiptId, receipt);
    assert.equal(replay.child.exitCode, null);
    const limit = Date.now() + 3_000;
    let status: Json | null = null;
    while (Date.now() < limit) {
      const inspect = await replay.request('runtime.inspect', { commandIds: ['c1'] });
      const commands = (inspect.result as { commands?: { status?: Json }[] }).commands ?? [];
      status = commands[0]?.status ?? null;
      if (status === 'done') break;
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    assert.equal(status, 'done');
    let fresh: Frame | undefined;
    const killed = replay.request('runtime.submit', { type: 'input', content: 'next' }, { commandId: 'c2' })
      .then(value => { fresh = value; }, () => undefined);
    const exitCode = await replay.exited;
    await killed;
    assert.equal(fresh, undefined);
    assert.equal(exitCode, null);
  } finally {
    await replay.close();
    await rm(directory, { recursive: true, force: true });
  }
});

for (const [name, input] of [
  ['malformed', '{not json}\n'],
  ['oversized', 'x'.repeat(MAX_FRAME)],
  ['unknown response', JSON.stringify({ protocol: 0, kind: 'response', id: 'bridge:999', binding, result: {} }) + '\n'],
] as const) {
  test(`${name} input terminates the peer instead of assuming success`, { timeout: 10_000 }, async () => {
    const p = await peer();
    try { p.child.stdin.write(input); assert.equal(await p.exited, 1); }
    finally { await p.close(); }
  });
}
