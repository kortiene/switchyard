// Public Pi admission APIs plus application documents; no private storage tables or mission transitions.
import { BACKGROUND_CONTEXT, withAbortSignal } from '@earendil-works/chord/context';
import { type Models } from '@earendil-works/pi-ai/models';
import { ConversationBusy, ConversationIdle, createRegistry, defineDoc, defineDocFamily, defineExtension,
  Harness, type Conversation, type SubmissionDraft, type ToolRegistration } from '@earendil-works/pi-durable';
import { SqliteStorage } from '@earendil-works/pi-durable/storage/sqlite';
import { openNodeSqliteDatabase } from '@earendil-works/pi-durable/storage/sqlite/node';
import { ADAPTER, canonical, CAPABILITIES, DEADLINE_MS, hash, MAX_FRAME, MAX_PENDING, PROTOCOL, ProtocolError, SOURCE,
  type Binding, type Frame, type Json } from './protocol.ts';

const Session = defineDoc({ kind: 'switchyard.bridge.session', scope: 'session', version: 1,
  initial: () => ({ missionId: '', attemptId: '', sessionId: '', generation: 0, source: SOURCE, adapter: ADAPTER, stopCommand: '' }),
  checkpointWhen: () => true });
const Command = defineDocFamily({ kind: 'switchyard.bridge.command', scope: 'session', version: 1, family: true,
  initial: (_seed: null) => ({ hash: '', method: '', receipt: '', settled: false as boolean }), checkpointWhen: () => true });

export class Runtime {
  private binding: Binding;
  private database: string;
  private models: Models;
  private tools: ToolRegistration[];
  private harness?: Harness;
  private root?: Conversation;
  private negotiated = false;
  private opening = false;
  private closing = false;
  private afterAdmission?: () => void;
  constructor(database: string, binding: Binding, models: Models, tools: ToolRegistration[], afterAdmission?: () => void) {
    this.database = database; this.binding = binding; this.models = models; this.tools = tools; this.afterAdmission = afterAdmission;
  }

  async dispatch(frame: Frame): Promise<Json> {
    const params = frame.params ?? {};
    canonical(params);
    if (frame.method === 'runtime.hello') {
      if (this.negotiated || params.source !== SOURCE || params.adapter !== ADAPTER
        || params.maxFrame !== MAX_FRAME || params.maxPending !== MAX_PENDING) {
        throw new ProtocolError('unsupported_capability');
      }
      if (!Array.isArray(params.capabilities) || params.capabilities.some(c => !CAPABILITIES.includes(String(c)))) {
        throw new ProtocolError('unsupported_capability');
      }
      this.negotiated = true;
      return { protocol: PROTOCOL, source: SOURCE, adapter: ADAPTER, node: process.versions.node, maxFrame: MAX_FRAME,
        maxPending: MAX_PENDING, deadlineMs: DEADLINE_MS, capabilities: CAPABILITIES, storage: 'sqlite-full' };
    }
    if (!this.negotiated) throw new ProtocolError('handshake_required');
    if (frame.method === 'runtime.open') {
      if (Object.keys(params).some(key => key !== 'requireExisting')
        || (params.requireExisting !== undefined && typeof params.requireExisting !== 'boolean')) throw new ProtocolError('invalid_params');
      return this.open(params.requireExisting === true);
    }
    if (!this.root || !this.harness || this.closing) throw new ProtocolError('session_unavailable');
    if (frame.method === 'runtime.inspect' || frame.method === 'runtime.watch') {
      const snapshot = await this.inspect(params);
      // Snapshot polling is explicit. This adapter does not advertise lossless event replay.
      return frame.method === 'runtime.watch' ? { resyncRequired: true, snapshot } : snapshot;
    }
    if (!['runtime.submit', 'runtime.steer', 'runtime.abort', 'runtime.close'].includes(frame.method ?? '')) {
      throw new ProtocolError('unsupported_method');
    }
    if (typeof frame.commandId !== 'string' || frame.commandId.length === 0 || frame.commandId.length > 256) {
      throw new ProtocolError('invalid_command');
    }
    if (frame.method === 'runtime.close') {
      // Closing does not certify process-tree quiescence or any mission fact.
      this.closing = true;
      if ((await this.harness.inspect(BACKGROUND_CONTEXT)).tasks.length > 0) {
        this.closing = false;
        throw new ProtocolError('busy', 'retry');
      }
      await this.harness.close(withAbortSignal(AbortSignal.timeout(DEADLINE_MS), BACKGROUND_CONTEXT));
      return { closed: true };
    }
    // Keep S1's full canonical command under the Pi conflict hash, including
    // abort/continue payloads that otherwise have identical Pi parameters.
    if (params.hostCommand !== undefined) {
      const host = params.hostCommand as { payload?: Record<string, Json>; payloadHash?: string };
      if (!host || !host.payload || typeof host.payloadHash !== 'string'
        || hash(host.payload) !== host.payloadHash || host.payload.missionId !== this.binding.missionId
        || host.payload.attemptId !== this.binding.attemptId) throw new ProtocolError('host_command_mismatch');
    }
    if (frame.method === 'runtime.abort' && Object.keys(params).some(key => key !== 'hostCommand')) throw new ProtocolError('invalid_params');
    const payloadHash = hash({ method: frame.method, params });
    try {
      if (frame.method === 'runtime.abort') return await this.abort(frame.commandId, payloadHash);
      return await this.submit(frame.commandId, frame.method!, payloadHash, params);
    } catch (error) {
      if (error instanceof ConversationIdle) throw new ProtocolError('idle', 'rejected');
      // Busy is retryable: a Continue racing a running turn is sent again later.
      if (error instanceof ConversationBusy) throw new ProtocolError('busy', 'retry');
      throw error;
    }
  }

  private async open(requireExisting: boolean): Promise<Json> {
    if (this.harness || this.opening) throw new ProtocolError('already_open');
    this.opening = true;
    const database = await openNodeSqliteDatabase(this.database);
    await database.exec('PRAGMA synchronous = FULL');
    const mode = await database.get<{ synchronous: number }>('PRAGMA synchronous');
    if (mode?.synchronous !== 2) throw new Error('SQLite durability unavailable');
    const registry = createRegistry();
    registry.install(defineExtension({ name: 'switchyard-forwarders', tools: this.tools }));
    const harness = await Harness.open(await SqliteStorage.open(database), { models: this.models, registry }, BACKGROUND_CONTEXT);
    this.harness = harness;
    await harness.commit(async tx => {
      const state = await tx.doc(Session);
      if (requireExisting && !state.sessionId) throw new ProtocolError('session_missing');
      if (state.sessionId && (state.sessionId !== this.binding.sessionId || state.missionId !== this.binding.missionId
        || state.attemptId !== this.binding.attemptId || state.source !== SOURCE || state.adapter !== ADAPTER)) throw new ProtocolError('binding_mismatch');
      if (state.sessionId && this.binding.generation <= state.generation) throw new ProtocolError('stale_generation');
      Object.assign(state, this.binding);
    }, BACKGROUND_CONTEXT);
    this.root = await harness.root(BACKGROUND_CONTEXT, { agent: { model: { provider: 'faux', modelId: 'faux-1' },
      tools: this.tools } });
    // Opening and inspection never resume interrupted tools. Replaying an authorized
    // submission or abort is the explicit scheduling step after outbox reconciliation.
    return { opened: true, conversationId: String(this.root.id), source: SOURCE, scheduling: 'paused' };
  }

  private async submit(commandId: string, method: string, payloadHash: string, params: Record<string, Json>): Promise<Json> {
    const harness = this.harness!;
    const root = this.root!;
    if (Object.keys(params).some(key => !['content', 'type', 'hostCommand'].includes(key))) throw new ProtocolError('invalid_params');
    let draft: SubmissionDraft;
    const requestId = canonical([this.binding.attemptId, commandId]);
    if (params.type === 'continue' && method === 'runtime.submit') {
      if (params.content !== undefined) throw new ProtocolError('invalid_params');
      draft = { type: 'continue', requestId };
    } else {
      if (params.type !== 'input' || typeof params.content !== 'string') throw new ProtocolError('invalid_params');
      draft = { type: 'input', content: params.content, requestId, whenBusy: method === 'runtime.steer' ? 'steer' : 'reject',
        ...(method === 'runtime.steer' ? { whenIdle: 'reject' as const } : {}) };
    }
    const admitted = await harness.commit(async tx => {
      if (this.closing) throw new ProtocolError('session_unavailable');
      const state = await tx.doc(Session);
      const command = await tx.doc(Command, commandId, null);
      if (command.hash && (command.hash !== payloadHash || command.method !== method)) throw new ProtocolError('command_conflict');
      if (state.stopCommand) throw new ProtocolError('stopping', 'rejected');
      if (command.receipt) return { receipt: command.receipt, fresh: false };
      const id = await root.submit(tx, draft);
      command.hash = payloadHash; command.method = method; command.receipt = String(id); command.settled = true;
      return { receipt: command.receipt, fresh: true };
    }, BACKGROUND_CONTEXT);
    // Only a new admission is a crash point. Replaying an existing receipt must
    // stay alive so recovery can resume that submission with the fixture armed.
    if (admitted.fresh) this.afterAdmission?.();
    harness.resume();
    return { receiptId: admitted.receipt, commandId, requestId, payloadHash, admitted: true };
  }

  private async abort(commandId: string, payloadHash: string): Promise<Json> {
    const harness = this.harness!;
    const complete = await harness.commit(async tx => {
      const state = await tx.doc(Session);
      const command = await tx.doc(Command, commandId, null);
      if (command.hash && (command.hash !== payloadHash || command.method !== 'runtime.abort')) throw new ProtocolError('command_conflict');
      if (command.settled) return true;
      if (state.stopCommand && state.stopCommand !== commandId) throw new ProtocolError('stopping', 'rejected');
      state.stopCommand = commandId;
      command.hash = payloadHash; command.method = 'runtime.abort'; command.receipt = commandId;
      return false;
    }, BACKGROUND_CONTEXT);
    if (!complete) {
      try {
        await this.root!.abort(withAbortSignal(AbortSignal.timeout(DEADLINE_MS), BACKGROUND_CONTEXT), { background: true });
      } catch { throw new ProtocolError('cancellation_timeout', 'uncertain'); }
      await harness.commit(async tx => {
        const state = await tx.doc(Session);
        const command = await tx.doc(Command, commandId, null);
        command.settled = true; state.stopCommand = '';
      }, BACKGROUND_CONTEXT);
    }
    return { commandId, accepted: true, runtimeSettled: true };
  }

  private async inspect(params: Record<string, Json>): Promise<Json> {
    const ids = params.commandIds ?? [];
    if (!Array.isArray(ids) || ids.length > 32 || ids.some(id => typeof id !== 'string' || id.length > 256)) {
      throw new ProtocolError('invalid_params');
    }
    const commands: Json[] = [];
    for (const id of ids as string[]) {
      const record = await this.harness!.snapshot(Command, id, BACKGROUND_CONTEXT);
      const submission = await this.harness!.commit(tx => tx.submissionByRequest(this.root!.id,
        canonical([this.binding.attemptId, id])), BACKGROUND_CONTEXT);
      commands.push({ commandId: id, receipt: record ? { ...record } : null,
        status: submission?.status ?? null,
        reason: submission && 'reason' in submission ? String(submission.reason) : null });
    }
    const inspection = await this.harness!.inspect(BACKGROUND_CONTEXT);
    const state = await this.harness!.snapshot(Session, BACKGROUND_CONTEXT);
    return { source: SOURCE, scheduling: inspection.scheduling, commands, stopping: !!state?.stopCommand,
      runtimeIdle: inspection.tasks.length === 0, liveTaskCount: inspection.tasks.length,
      unsettledSubmissionCount: inspection.submissions.length };
  }
}
