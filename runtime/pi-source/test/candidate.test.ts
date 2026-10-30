import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { BACKGROUND_CONTEXT, withAbortSignal } from '@earendil-works/chord/context';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxAssistantMessage, fauxProvider } from '@earendil-works/pi-ai/providers/faux';
import { ConversationIdle, createRegistry, defineDoc, Harness, MemoryStorage } from '@earendil-works/pi-durable';

const Receipt = defineDoc({ kind: 'switchyard.candidate.receipt', scope: 'session', version: 1,
  initial: () => ({ hash: '' }), checkpointWhen: () => false });

async function fixture() {
  const faux = fauxProvider();
  faux.setResponses([fauxAssistantMessage('done')]);
  const models = createModels();
  models.setProvider(faux.provider);
  const context = withAbortSignal(AbortSignal.timeout(5_000), BACKGROUND_CONTEXT);
  const harness = await Harness.open(new MemoryStorage(), { models, registry: createRegistry() }, context);
  const root = await harness.root(context, { agent: { model: { provider: 'faux', modelId: 'faux-1' } } });
  return { faux, context, harness, root };
}

test('all Pi workspace packages resolve to the integrity-locked source artifacts', () => {
  const pins = JSON.parse(readFileSync(new URL('../pins.json', import.meta.url), 'utf8'));
  const lock = JSON.parse(readFileSync(new URL('../package-lock.json', import.meta.url), 'utf8'));
  const require = createRequire(import.meta.url);
  for (const pkg of pins.packages) {
    const bytes = readFileSync(new URL(`../.artifacts/${pkg.artifact}`, import.meta.url));
    assert.equal(createHash('sha256').update(bytes).digest('hex'), pkg.sha256);
    const locked = lock.packages[`node_modules/${pkg.name}`];
    assert.equal(locked.resolved, `file:.artifacts/${pkg.artifact}`);
    assert.equal(locked.integrity, `sha512-${createHash('sha512').update(bytes).digest('base64')}`);
  }
  for (const [path, value] of Object.entries(lock.packages)) {
    if (!path.includes('node_modules/@earendil-works/')) continue;
    assert.match((value as { resolved: string }).resolved, /^file:\.artifacts\//, `Registry fallback: ${path}`);
  }
  const root = dirname(require.resolve('@earendil-works/pi-durable/package.json'));
  for (const [name, expected] of [
    ['types', 'd7f4a05ec86b1977d2d1c318cded2a192588901f'],
    ['submissions', '66f116af3c5f36547a9a8e0670c334a3d85e800e'],
  ]) {
    const map = JSON.parse(readFileSync(join(root, `dist/harness/${name}.js.map`), 'utf8'));
    const index = map.sources.findIndex((p: string) => p.endsWith(`/harness/${name}.ts`));
    assert(index >= 0);
    const bytes = Buffer.from(map.sourcesContent[index]);
    assert.equal(createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex'), expected);
  }
});

test('idle steering refuses admission without starting a model run', async () => {
  const { faux, context, harness, root } = await fixture();
  try {
    await assert.rejects(root.submit({ type: 'input', content: 'steer', requestId: 'steer',
      whenBusy: 'steer', whenIdle: 'reject' }, context), ConversationIdle);
    assert.equal(faux.state.callCount, 0);
    assert.equal(await harness.commit(tx => tx.submissionByRequest(root.id, 'steer'), context), undefined);
  } finally { await harness.close(BACKGROUND_CONTEXT); }
});

test('transactional admission and payload binding roll back and commit together', async () => {
  const { context, harness, root } = await fixture();
  const draft = { type: 'write' as const, requestId: 'atomic', entry: { kind: 'note', data: { text: 'one' } } };
  try {
    await assert.rejects(harness.commit(async tx => {
      const receipt = await tx.doc(Receipt);
      await root.submit(tx, draft);
      receipt.hash = 'payload-hash';
      throw new Error('rollback');
    }, context), /rollback/);
    assert.equal(await harness.commit(tx => tx.submissionByRequest(root.id, 'atomic'), context), undefined);
    assert.equal(await harness.snapshot(Receipt, context), undefined);
    const id = await harness.commit(async tx => {
      const receipt = await tx.doc(Receipt);
      const id = await root.submit(tx, draft);
      receipt.hash = 'payload-hash';
      return id;
    }, context);
    assert.equal((await harness.snapshot(Receipt, context))?.hash, 'payload-hash');
    assert.equal((await root.submit(draft, context)).id, id);
    assert.equal((await root.entries({}, 100, undefined, context)).items.length, 1);
  } finally { await harness.close(BACKGROUND_CONTEXT); }
});

test('continue admission is public and empty context settles unanswered', async () => {
  const { faux, context, harness, root } = await fixture();
  try {
    const result = await (await root.submit({ type: 'continue', requestId: 'continue' }, context)).wait(context);
    assert.equal(result.status, 'unanswered');
    assert.equal(result.reason, 'nothing_to_answer');
    assert.equal(faux.state.callCount, 0);
  } finally { await harness.close(BACKGROUND_CONTEXT); }
});
