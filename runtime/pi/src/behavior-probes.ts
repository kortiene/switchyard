import { BACKGROUND_CONTEXT, withAbortSignal } from '@earendil-works/chord/context';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxAssistantMessage, fauxProvider } from '@earendil-works/pi-ai/providers/faux';
import { createRegistry, Harness, MemoryStorage } from '@earendil-works/pi-durable';

async function fixture() {
  const faux = fauxProvider();
  faux.setResponses([fauxAssistantMessage('first answer'), fauxAssistantMessage('second answer')]);
  const models = createModels();
  models.setProvider(faux.provider);
  const context = withAbortSignal(AbortSignal.timeout(5_000), BACKGROUND_CONTEXT);
  const harness = await Harness.open(new MemoryStorage(), { models, registry: createRegistry() }, context);
  const root = await harness.root(context, { agent: { model: { provider: 'faux', modelId: 'faux-1' } } });
  return { faux, context, harness, root };
}

export async function probeIdleSteering() {
  const { faux, context, harness, root } = await fixture();
  try {
    // Deliberately send the newer baseline's field at the JS boundary. No type assertion
    // hides the missing API in production: this is a compatibility probe, never a bridge.
    const draft = { type: 'input' as const, content: 'steer only an existing run',
      requestId: 'idle-steer', whenBusy: 'steer' as const, whenIdle: 'reject' as const };
    try {
      const submission = await root.submit(draft, context);
      const settled = await submission.wait(context);
      await root.waitForIdle(context);
      return { rejected: false, status: settled.status, modelCalls: faux.state.callCount };
    } catch (error) {
      // Only the specific public idle refusal proves the behavior. Storage, timeout,
      // provider and unrelated exceptions must fail the probe, never manufacture GO.
      if (error instanceof Error && error.constructor.name === 'ConversationIdle') {
        return { rejected: true, status: 'idle_rejected', modelCalls: faux.state.callCount };
      }
      throw error;
    }
  } finally {
    await harness.close(BACKGROUND_CONTEXT);
  }
}

export async function probeChangedPayload() {
  const { faux, context, harness, root } = await fixture();
  try {
    const first = await root.submit({ type: 'input', content: 'original', requestId: 'command-1' }, context);
    await first.wait(context);
    await root.waitForIdle(context);
    const duplicate = await root.submit({ type: 'input', content: 'different', requestId: 'command-1' }, context);
    await duplicate.wait(context);
    const entries = await root.entries({ order: 'ascending' }, 100, undefined, context);
    return { sameSubmission: first.id === duplicate.id, modelCalls: faux.state.callCount,
      userEntries: entries.items.filter(entry => entry.kind === 'pi.user').length };
  } finally {
    await harness.close(BACKGROUND_CONTEXT);
  }
}
