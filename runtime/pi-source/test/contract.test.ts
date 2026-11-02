import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxAssistantMessage, fauxProvider, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import { ADAPTER, CAPABILITIES, MAX_FRAME, MAX_PENDING, SOURCE, canonical, hash, operationId, type Json } from '../src/protocol.ts';
import { Runtime } from '../src/runtime.ts';
import { forwarders } from '../src/forwarders.ts';
import { Transport } from '../src/transport.ts';

test('shared contract vectors freeze canonical hashes and generation-independent operation identity', () => {
  const vectors = JSON.parse(readFileSync(new URL('../contract-vectors.json', import.meta.url), 'utf8'));
  for (const vector of vectors.values) {
    assert.equal(canonical(vector.value), vector.canonical);
    assert.equal(hash(vector.value), vector.hash);
  }
  for (const vector of vectors.operations) {
    for (const generation of [1, 42]) {
      assert.equal(operationId({ ...vector.binding, generation }, vector.task, vector.key), vector.id);
    }
  }
});

test('the bundled registry refuses unregistered code loading and host tools', { timeout: 15_000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'switchyard-registry-'));
  const binding = { missionId: 'm', attemptId: 'a', sessionId: 's', generation: 1 };
  const denied = ['eval', 'exec', 'shell', 'load_extension', 'tool_search', 'mcp', 'verification_pass', 'publish'];
  let checked = false;
  let effects = 0;
  const faux = fauxProvider();
  faux.setResponses([
    fauxAssistantMessage(denied.map(name => fauxToolCall(name, { code: 'throw new Error("must not run")' })), { stopReason: 'toolUse' }),
    context => {
      const results = context.messages.filter(message => message.role === 'toolResult');
      assert.equal(results.length, denied.length);
      for (const result of results) assert.equal(result.isError, true);
      checked = true;
      return fauxAssistantMessage('done');
    },
  ]);
  const models = createModels();
  models.setProvider(faux.provider);
  const transport = new Transport(binding);
  transport.request = async () => { effects++; throw new Error('unregistered tool reached execution'); };
  const tools = forwarders(binding, transport);
  assert.deepEqual(tools.map(tool => tool.name).sort(), ['nested_write', 'write_file', 'write_pair']);
  const runtime = new Runtime(join(directory, 'pi.sqlite'), binding, models, tools);
  let sequence = 0;
  const call = (method: string, commandId?: string, params: Record<string, Json> = {}) => runtime.dispatch({
    protocol: 0, kind: 'request', id: `host:${++sequence}`, binding, method, commandId, params,
  });
  try {
    await call('runtime.hello', undefined, { source: SOURCE, adapter: ADAPTER, maxFrame: MAX_FRAME,
      maxPending: MAX_PENDING, capabilities: CAPABILITIES });
    await call('runtime.open');
    for (const key of ['tools', 'extensions', 'provider', 'model', 'code', 'mcpServers']) {
      await assert.rejects(call('runtime.submit', 'injected', { type: 'input', content: 'x', [key]: [] }), { category: 'invalid_params' });
    }
    await call('runtime.submit', 'unregistered', { type: 'input', content: 'try unregistered tools' });
    const until = Date.now() + 5_000;
    let snapshot: Json;
    do {
      snapshot = await call('runtime.inspect', undefined, { commandIds: ['injected', 'unregistered'] });
      if ((snapshot as { runtimeIdle: boolean }).runtimeIdle) break;
      await new Promise(resolve => setTimeout(resolve, 10));
    } while (Date.now() < until);
    const result = snapshot as { runtimeIdle: boolean; commands: { receipt: Json; status: string }[] };
    assert.equal(result.runtimeIdle, true);
    assert.equal(result.commands[0].receipt, null);
    assert.equal(result.commands[1].status, 'done');
    assert(checked, 'the faux provider must observe all unknown-tool failures');
    assert.equal(effects, 0);
  } finally {
    await call('runtime.abort', 'cleanup');
    await call('runtime.close', 'close');
    await rm(directory, { recursive: true, force: true });
  }
});
