// Only these bundled tools are registered. Every effect goes back to the Rust owner.
import { Type } from '@earendil-works/pi-ai';
import { awaitWithContext, type Context } from '@earendil-works/chord/context';
import { defineTool, type ToolExecutionApi } from '@earendil-works/pi-durable';
import { hash, operationId, type Binding, type Json } from './protocol.ts';
import { Transport } from './transport.ts';

export function forwarders(binding: Binding, transport: Transport) {
  const effect = async (api: ToolExecutionApi, key: string, path: string, content: string, context: Context) => {
    const payload = { method: 'file.write', path, content, capability: 'workspace-write', deadlinePolicy: 'attempt-bound' };
    const reply = await awaitWithContext(transport.request('execution.request', { operationId: operationId(binding, String(api.taskId), key),
      taskId: String(api.taskId), effectKey: key, payloadHash: hash(payload), payload }), context);
    return JSON.stringify(reply);
  };
  const processEffect = async (api: ToolExecutionApi, key: string, method: string,
    fields: Record<string, Json>, context: Context) => {
    const payload = { method, ...fields, capability: 'workspace-process', deadlinePolicy: 'attempt-bound' };
    return JSON.stringify(await awaitWithContext(transport.request('execution.request', {
      operationId: operationId(binding, String(api.taskId), key), taskId: String(api.taskId),
      effectKey: key, payloadHash: hash(payload), payload }), context));
  };
  const handle = Type.Object({ startOperationId: Type.String(), startPayloadHash: Type.String() });
  return [
    defineTool({ name: 'process_start', description: 'Start a confined workspace process through Rust; returns a handle, not an exit result.', replay: 'safe',
      parameters: Type.Object({ argv: Type.Array(Type.String()) }),
      execute: async (args, api, context) => ({ output: [{ type: 'text' as const,
        text: await processEffect(api, 'process-start:v1', 'process.start', args, context) }] }) }),
    defineTool({ name: 'process_status', description: 'Observe a process handle through Rust. Replay returns this historical observation.', replay: 'safe',
      parameters: handle,
      execute: async (args, api, context) => ({ output: [{ type: 'text' as const,
        text: await processEffect(api, 'process-status:v1', 'process.status', args, context) }] }) }),
    defineTool({ name: 'process_cancel', description: 'Terminate a process and descendants through Rust.', replay: 'safe',
      parameters: handle,
      execute: async (args, api, context) => ({ output: [{ type: 'text' as const,
        text: await processEffect(api, 'process-cancel:v1', 'process.cancel', args, context) }] }) }),
    defineTool({ name: 'write_file', description: 'Request a file write through the owner.', replay: 'safe',
      parameters: Type.Object({ path: Type.String(), content: Type.String() }),
      execute: async (args, api, context) => ({ output: [{ type: 'text' as const,
        text: await effect(api, 'write-file:v1', args.path, args.content, context) }] }) }),
    defineTool({ name: 'write_pair', description: 'Two ordered writes with stable effect slots.', replay: 'safe',
      parameters: Type.Object({ prefix: Type.String() }),
      execute: async (args, api, context) => {
        const first = await effect(api, 'write-pair:first:v1', `${args.prefix}-1`, 'first', context);
        const second = await effect(api, 'write-pair:second:v1', `${args.prefix}-2`, 'second', context);
        return { output: [{ type: 'text' as const, text: `${first}\n${second}` }] };
      } }),
    defineTool({ name: 'nested_write', description: 'A stable nested write through the same owner.', replay: 'safe',
      parameters: Type.Object({ path: Type.String(), content: Type.String() }),
      execute: async (args, api, context) => {
        const result = await api.executeTool('write_file', args, context, { key: 'nested-file' });
        return { output: [{ type: 'text' as const, text: JSON.stringify(result) }],
          ...(result.isError ? { isError: true } : {}) };
      } }),
  ];
}
