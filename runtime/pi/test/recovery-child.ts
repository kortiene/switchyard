// Standalone Pi storage probe, not the Switchyard process bridge.
import { BACKGROUND_CONTEXT } from '@earendil-works/chord/context';
import { createModels } from '@earendil-works/pi-ai/models';
import { createRegistry, Harness } from '@earendil-works/pi-durable';
import { openNodeSqliteStorage } from '@earendil-works/pi-durable/storage/sqlite/node';

const [database, mode] = process.argv.slice(2);
if (!database || !['admit', 'recover'].includes(mode)) throw new Error('Expected database and admit/recover');
const context = BACKGROUND_CONTEXT;
const harness = await Harness.open(await openNodeSqliteStorage(database), {
  models: createModels(), registry: createRegistry(),
}, context);
const root = await harness.root(context);
const draft = { type: 'write' as const, requestId: 'lost-ack', entry: { kind: 'switchyard.audit', data: { marker: 'one' } } };
if (mode === 'admit') {
  await root.submit(draft, context);
  // Test-observer marker after admission, before any receipt is returned to a host.
  process.stdout.write('admitted\n');
  setInterval(() => {}, 1_000);
} else {
  const before = await harness.commit(tx => tx.submissionByRequest(root.id, draft.requestId), context);
  const replay = await root.submit(draft, context);
  const entries = await root.entries({ order: 'ascending' }, 100, undefined, context);
  console.log(JSON.stringify({ existingId: before?.id, replayId: replay.id,
    entryCount: entries.items.filter(entry => entry.kind === 'switchyard.audit').length,
    status: (await replay.status(context)).status }));
  await harness.close(context);
}
