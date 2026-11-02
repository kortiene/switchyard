// Credential-free conformance launcher. No live provider or product CLI exposes this bridge yet.
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxAssistantMessage, fauxProvider, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import { Type } from '@earendil-works/pi-ai';
import { defineDoc, defineTool, type ToolRegistration } from '@earendil-works/pi-durable';
import type { SqliteDatabase } from '@earendil-works/pi-durable/storage/sqlite';
import { awaitWithContext } from '@earendil-works/chord/context';
import { appendFileSync, existsSync, writeSync } from 'node:fs';
import { forwarders } from '../src/forwarders.ts';
import { ProtocolError, type Binding } from '../src/protocol.ts';
import { Runtime } from '../src/runtime.ts';
import { Transport } from '../src/transport.ts';

const [database, bindingJson, mode] = process.argv.slice(2);
if (!database || !bindingJson || !['answer', 'tools', 'recover', 'crash-before-admission', 'crash-after-admission',
  'crash-after-abort-recorded', 'crash-after-abort-settled', 'hold-abort', 'ignore-abort', 'cooperative-wait',
  'storage-submit', 'storage-abort-intent', 'storage-abort-cancel', 'storage-abort-settlement', 'storage-background', 'storage-close',
  'storage-background-hang', 'gated-tools',
  'busy-once', 'stopping-once', 'mismatch'].includes(mode)) throw new Error('Invalid launcher arguments');
const binding = JSON.parse(bindingJson) as Binding;
const faux = fauxProvider();
const waitingTool = () => fauxAssistantMessage([fauxToolCall('uncooperative', {})], { stopReason: 'toolUse' });
// One fresh tool turn per burst round in transport.test.ts. Abort ends the run,
// so the next submit consumes the next queued response.
faux.setResponses(mode === 'cooperative-wait' ? Array.from({ length: 8 }, waitingTool)
  : mode === 'ignore-abort' || mode.startsWith('storage-abort-') || mode === 'storage-background-hang' ? [waitingTool()]
  : mode === 'tools' || mode === 'gated-tools' ? [
  fauxAssistantMessage([fauxToolCall('write_pair', { prefix: 'pair' }, { id: 'parent-pair' }),
    fauxToolCall('nested_write', { path: 'nested', content: 'child' }, { id: 'parent-nested' })], { stopReason: 'toolUse' }),
  fauxAssistantMessage('done'),
] : [fauxAssistantMessage('done')]);
const models = createModels();
models.setProvider(faux.provider);
const transport = new Transport(binding);
const tools: ToolRegistration[] = forwarders(binding, transport);
if (mode === 'gated-tools') {
  // Stop/recovery proof: interrupt real durable tool tasks before forwarding.
  // Kept armed on reopen so any accidental scheduling leaves durable evidence.
  for (let index = 0; index < tools.length; index++) {
    const tool = tools[index];
    tools[index] = { ...tool, execute: async (args, api, context) => {
      appendFileSync(`${database}.entered`, `${JSON.stringify({ generation: binding.generation, task: String(api.taskId), name: tool.name })}\n`);
      await awaitWithContext(new Promise<void>(() => {}), context);
      return tool.execute(args, api, context);
    } };
  }
}
const FailedWrite = defineDoc({ kind: 'switchyard.fixture.failed-write', scope: 'session', version: 1,
  initial: () => ({ attempted: false as boolean }), checkpointWhen: () => true });
// Only the explicit conformance fixture can select this deliberately hung tool.
if (['ignore-abort', 'cooperative-wait', 'storage-background-hang'].includes(mode) || mode.startsWith('storage-abort-')) tools.push(defineTool({ name: 'uncooperative', description: 'Cancellation fixture.',
  replay: 'safe', parameters: Type.Object({}), execute: async (_args, api, context) => {
    writeSync(process.stderr.fd, 'fixture: waiting tool entered\n');
    const waiting = new Promise<void>(() => {});
    if (mode === 'storage-background-hang') {
      // The test releases this only after every host request has replied, so no
      // request timer or later host frame can terminate the failed session.
      while (!existsSync(`${database}.fail`)) await new Promise(resolve => setTimeout(resolve, 5));
      await readOnly();
      try {
        await api.commit(async tx => { (await tx.doc(FailedWrite)).attempted = true; }, context);
      } catch { /* The failed storage must terminate even if this tool ignores it. */ }
      await waiting; // Deliberately ignore cancellation during session shutdown.
    }
    // One variant cooperates; the other deliberately ignores the tool context.
    await (mode === 'ignore-abort' ? waiting : awaitWithContext(waiting, context));
    return { output: [] };
  } }));
const kill = () => { process.kill(process.pid, 'SIGKILL'); };
let sqlite: SqliteDatabase;
// A real SQLite write refusal on this connection. No private Pi tables are
// read or altered; reopening a new connection removes this injected fault.
const readOnly = async () => { await sqlite.exec('PRAGMA query_only = ON'); };
const runtime = new Runtime(database, binding, models, tools, {
  databaseOpened: db => { sqlite = db; },
  beforeAdmission: mode === 'storage-submit' ? readOnly : undefined,
  afterAdmission: mode === 'crash-after-admission' ? kill : mode === 'storage-background' ? readOnly : undefined,
  beforeAbortRecorded: mode === 'storage-abort-intent' ? readOnly : undefined,
  afterAbortRecorded: async fresh => {
    if (mode === 'storage-abort-cancel') await readOnly();
    if (mode === 'crash-after-abort-recorded' && fresh) kill();
    if (mode === 'hold-abort') {
      writeSync(process.stderr.fd, 'fixture: abort recorded\n');
      await new Promise<void>(() => {});
    }
  },
  beforeAbortSettled: mode === 'storage-abort-settlement' ? readOnly : undefined,
  afterAbortSettled: mode === 'crash-after-abort-settled' ? kill : undefined,
  beforeClose: mode === 'storage-close' ? () => {
    const close = sqlite.close.bind(sqlite);
    // Simulate a lost close acknowledgement through the public database facade.
    sqlite.close = async () => { await close(); throw new Error('fixture: SQLite close acknowledgement lost'); };
  } : undefined,
});
void runtime.failed.then(error => transport.failFatal(error));
let busyContinue = mode === 'busy-once';
let stoppingOnce = mode === 'stopping-once';
transport.start(frame => {
  if (mode === 'crash-before-admission' && frame.method === 'runtime.submit') process.kill(process.pid, 'SIGKILL');
  // One forced busy Continue, then the same child accepts the retry.
  if (busyContinue && frame.method === 'runtime.submit' && frame.params?.type === 'continue') {
    busyContinue = false;
    throw new ProtocolError('busy', 'retry');
  }
  if (stoppingOnce && ['runtime.submit', 'runtime.steer', 'runtime.abort'].includes(frame.method ?? '')) {
    stoppingOnce = false;
    throw new ProtocolError('stopping', 'rejected');
  }
  if (mode === 'mismatch' && frame.method === 'runtime.submit') throw new ProtocolError('host_command_mismatch', 'rejected');
  return runtime.dispatch(frame);
});
