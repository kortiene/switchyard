// Credential-free conformance launcher. No live provider or product CLI exposes this bridge yet.
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxAssistantMessage, fauxProvider, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import { forwarders } from '../src/forwarders.ts';
import { type Binding } from '../src/protocol.ts';
import { Runtime } from '../src/runtime.ts';
import { Transport } from '../src/transport.ts';

const [database, bindingJson, mode] = process.argv.slice(2);
if (!database || !bindingJson || !['answer', 'tools', 'recover', 'crash-before-admission', 'crash-after-admission'].includes(mode)) throw new Error('Invalid launcher arguments');
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
transport.start(frame => {
  if (mode === 'crash-before-admission' && frame.method === 'runtime.submit') process.kill(process.pid, 'SIGKILL');
  return runtime.dispatch(frame);
});
