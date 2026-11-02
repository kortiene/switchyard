import assert from 'node:assert/strict';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { ADAPTER, canonical, effectError, hash, MAX_FRAME, MAX_PENDING, SOURCE, type Binding, type Frame, type Json } from '../src/protocol.ts';
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
    child.once('close', code => {
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

for (const mode of ['storage-submit', 'storage-abort-intent', 'storage-abort-cancel', 'storage-abort-settlement', 'storage-close']) {
  test(`${mode} exits without acknowledging the failed operation and recovers retained state`, { timeout: 20_000 }, async () => {
    const directory = await mkdtemp(join(tmpdir(), 'switchyard-storage-'));
    const aborting = mode.startsWith('storage-abort-');
    let originalWork: Json | undefined;
    let originalStop: Json | undefined;
    const p = await peer(mode, { directory, cleanup: false });
    try {
      assert.equal((await p.request('runtime.hello', hello)).error, undefined);
      assert.equal((await p.request('runtime.open', {})).error, undefined);
      if (aborting) {
        const work = await p.request('runtime.submit', { type: 'input', content: 'wait' }, { commandId: 'work' });
        assert.equal(work.error, undefined);
        originalWork = (work.result as { receiptId: string }).receiptId;
        const limit = Date.now() + 3_000;
        while (!p.diagnostics().includes('waiting tool entered') && Date.now() < limit) await new Promise(resolve => setTimeout(resolve, 5));
        assert(p.diagnostics().includes('waiting tool entered'), 'abort must cancel real active work');
      }
      const method = aborting ? 'runtime.abort' : mode === 'storage-close' ? 'runtime.close' : 'runtime.submit';
      await assert.rejects(p.request(method, method === 'runtime.submit' ? { type: 'input', content: 'wait' } : {},
        { commandId: aborting ? 'stop' : mode === 'storage-close' ? 'close' : 'work' }), /peer exited/);
      assert.equal(await p.exited, 1);
      assert.match(p.diagnostics(), mode === 'storage-close' ? /SQLite close acknowledgement lost/ : /attempt to write a readonly database/);
      assert.doesNotMatch(p.diagnostics(), /cancellation_timeout/);
    } finally { await p.close(); }
    try {
      for (const generation of [2, 3]) {
        const resumed = await peer(aborting ? 'cooperative-wait' : 'recover', { directory, generation, cleanup: false });
        try {
          assert.equal((await resumed.request('runtime.hello', hello)).error, undefined);
          assert.equal((await resumed.request('runtime.open', { requireExisting: true })).error, undefined);
          const inspect = async () => {
            const response = await resumed.request('runtime.inspect', { commandIds: ['work', 'stop'] });
            assert.equal(response.error, undefined);
            return response.result as { scheduling: string; stopping: boolean; runtimeIdle: boolean;
              commands: { receipt: { receipt: string; hash: string; settled: boolean } | null; reason: string | null }[] };
          };
          const before = await inspect();
          assert.equal(before.scheduling, 'paused');
          if (aborting) {
            assert.equal(before.commands[0].receipt?.receipt, originalWork);
            const hasIntent = generation > 2 || mode !== 'storage-abort-intent';
            assert.equal(before.commands[1].receipt !== null, hasIntent);
            assert.equal(before.stopping, generation === 2 && hasIntent);
            if (hasIntent) {
              assert.equal(before.commands[1].receipt?.settled, generation > 2);
              assert.equal(before.commands[1].receipt?.receipt, 'stop');
              assert.equal(before.commands[1].receipt?.hash, hash({ method: 'runtime.abort', params: {} }));
              const payload = { missionId: 'm', attemptId: 'a', changed: true };
              assert.equal((await resumed.request('runtime.abort', { hostCommand: { payload, payloadHash: hash(payload) } },
                { commandId: 'stop' })).error?.category, 'command_conflict');
              if (originalStop) assert.deepEqual(before.commands[1].receipt, originalStop);
              if (before.stopping) {
                assert.equal((await resumed.request('runtime.close', {}, { commandId: 'close' })).error?.category, 'busy');
                assert.equal((await resumed.request('runtime.submit', { type: 'input', content: 'blocked' }, { commandId: 'new' })).error?.category, 'stopping');
              }
            }
            assert.deepEqual((await resumed.request('runtime.abort', {}, { commandId: 'stop' })).result,
              { commandId: 'stop', accepted: true, runtimeSettled: true });
          } else if (mode === 'storage-submit') {
            assert.equal(before.commands[0].receipt !== null, generation > 2, 'failed admission must leave no receipt');
            const replay = await resumed.request('runtime.submit', { type: 'input', content: 'wait' }, { commandId: 'work' });
            assert.equal(replay.error, undefined);
            const receipt = (replay.result as { receiptId: string }).receiptId;
            if (originalWork) assert.equal(receipt, originalWork);
            originalWork = receipt;
          }
          const limit = Date.now() + 3_000;
          let after = await inspect();
          while ((!after.runtimeIdle || after.stopping) && Date.now() < limit) {
            await new Promise(resolve => setTimeout(resolve, 5)); after = await inspect();
          }
          assert.equal(after.runtimeIdle, true);
          assert.equal(after.stopping, false);
          if (aborting) {
            assert.equal(after.commands[0].reason, 'aborted');
            assert.equal(after.commands[1].receipt?.settled, true);
            assert.equal(after.commands[1].receipt?.hash, hash({ method: 'runtime.abort', params: {} }));
            originalStop = after.commands[1].receipt;
            assert.doesNotMatch(resumed.diagnostics(), /waiting tool entered/, 'reopen and abort must not rerun the tool');
            assert.equal(resumed.effects.length, 0);
          }
          assert.deepEqual((await resumed.request('runtime.close', {}, { commandId: 'close' })).result, { closed: true });
          assert.equal((await resumed.request('runtime.abort', {}, { commandId: 'late' })).error?.category, 'session_unavailable');
          assert.equal((await resumed.request('runtime.open', { requireExisting: true })).error?.category, 'already_open');
        } finally { await resumed.close(); }
      }
    } finally { await rm(directory, { recursive: true, force: true }); }
  });
}

test('a background SQLite failure terminates the peer without another host request', { timeout: 10_000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'switchyard-background-'));
  const p = await peer('storage-background', { directory, cleanup: false });
  try {
    await p.request('runtime.hello', hello);
    await p.request('runtime.open', {});
    // Admission committed before the injected fault. Its reply may race fatal exit.
    const reply = await p.request('runtime.submit', { type: 'input', content: 'wait' }, { commandId: 'work' }).catch(() => undefined);
    let timeout: NodeJS.Timeout | undefined;
    try {
      assert.equal(await Promise.race([p.exited, new Promise((_, reject) => {
        timeout = setTimeout(() => reject(new Error('background storage failure left the peer alive')), 3_000);
      })]), 1);
    } finally { clearTimeout(timeout); }
    assert.match(p.diagnostics(), /attempt to write a readonly database/);
    const resumed = await peer('recover', { directory, generation: 2, cleanup: false });
    try {
      await resumed.request('runtime.hello', hello);
      assert.equal((await resumed.request('runtime.open', { requireExisting: true })).error, undefined);
      const before = (await resumed.request('runtime.inspect', { commandIds: ['work'] })).result as {
        scheduling: string; commands: { receipt: { receipt: string; settled: boolean } }[];
      };
      assert.equal(before.scheduling, 'paused');
      assert.equal(before.commands[0].receipt.settled, true);
      if (reply) {
        assert.equal(reply.error, undefined);
        assert.equal(before.commands[0].receipt.receipt, (reply.result as { receiptId: string }).receiptId);
      }
      assert.equal(resumed.effects.length, 0);
      assert.equal((await resumed.request('runtime.abort', {}, { commandId: 'stop' })).error, undefined);
    } finally { await resumed.close(); }
  } finally { await p.close(); await rm(directory, { recursive: true, force: true }); }
});
