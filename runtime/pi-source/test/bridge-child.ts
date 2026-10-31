// Credential-free conformance launcher. No live provider or product CLI exposes this bridge yet.
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxAssistantMessage, fauxProvider, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import { forwarders } from '../src/forwarders.ts';
import { ProtocolError, type Binding } from '../src/protocol.ts';
import { Runtime } from '../src/runtime.ts';
import { Transport } from '../src/transport.ts';

const [database, bindingJson, mode] = process.argv.slice(2);
if (!database || !bindingJson || !['answer', 'tools', 'recover', 'crash-before-admission', 'crash-after-admission', 'busy-once', 'stopping-once', 'mismatch'].includes(mode)) throw new Error('Invalid launcher arguments');
const binding = JSON.parse(bindingJson) as Binding;
const faux = fauxProvider();
faux.setResponses(mode === 'tools' ? [
  fauxAssistantMessage([fauxToolCall('write_pair', { prefix: 'pair' }, { id: 'parent-pair' }),
    fauxToolCall('nested_write', { path: 'nested', content: 'child' }, { id: 'parent-nested' })], { stopReason: 'toolUse' }),
  fauxAssistantMessage('done'),
] : [fauxAssistantMessage('done')]);
const models = createModels();
models.setProvider(faux.provider);
const transport = new Transport(binding);
const runtime = new Runtime(database, binding, models, forwarders(binding, transport),
  mode === 'crash-after-admission' ? () => { process.kill(process.pid, 'SIGKILL'); } : undefined);
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
