// Bounded bidirectional JSON Lines. The reader continues while a runtime call awaits a tool receipt.
import { writeSync } from 'node:fs';
import { TextDecoder } from 'node:util';
import { checkBinding, effectError, DEADLINE_MS, MAX_FRAME, MAX_PENDING, PROTOCOL, ProtocolError,
  type Binding, type Frame, type Json } from './protocol.ts';

export class Transport {
  private buffer = Buffer.alloc(0);
  private lastHostId = 0;
  private nextId = 0;
  private active = 0;
  private dead = false;
  // Effect-only. `request` rejects every other method, and `effectError` below
  // validates every error waiting in this map. A future Pi-to-Rust call needs
  // its own queue: feeding it through here would shut the transport down on
  // an error shape this probe does not define.
  private pendingEffects = new Map<string, { method: string; resolve: (value: Json) => void; reject: (error: Error) => void; timer: NodeJS.Timeout }>();
  private binding: Binding;
  constructor(binding: Binding) { this.binding = binding; }

  start(dispatch: (frame: Frame) => Promise<Json>) {
    process.stdin.on('data', (chunk: Buffer) => {
      try {
        // Consume each complete frame without accumulating an unbounded multi-frame chunk.
        let offset = 0;
        while (offset < chunk.length) {
          const end = chunk.indexOf(10, offset);
          const stop = end < 0 ? chunk.length : end;
          // Reserve one byte for the terminating LF, including fragmented frames.
          if (this.buffer.length + stop - offset >= MAX_FRAME) throw new Error('oversized frame');
          this.buffer = Buffer.concat([this.buffer, chunk.subarray(offset, stop)]);
          offset = end < 0 ? chunk.length : end + 1;
          if (end < 0) break;
          const frame = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(this.buffer)) as Frame;
          this.buffer = Buffer.alloc(0);
          this.receive(frame, dispatch);
        }
      } catch { this.fail(); }
    });
    process.stdin.on('end', () => this.fail());
    process.stdin.on('error', () => this.fail());
    process.stdout.on('error', () => this.fail());
  }

  private receive(frame: Frame, dispatch: (frame: Frame) => Promise<Json>) {
    if (this.dead || !frame || typeof frame !== 'object') throw new Error('invalid envelope');
    if (frame.kind === 'response') {
      checkBinding(frame.binding, this.binding);
      if (frame.protocol !== PROTOCOL) throw new Error('invalid protocol');
      const waiting = this.pendingEffects.get(frame.id);
      if (!waiting || (frame.result === undefined) === (frame.error === undefined)) throw new Error('unknown response');
      // Scoped to effect requests. `request` is the only writer, and it
      // enqueues `execution.request` alone; this check keeps `effectError`
      // from becoming a general response parser.
      if (waiting.method !== 'execution.request') throw new Error('pending response is not an effect');
      const error = frame.error === undefined ? undefined : effectError(frame.error);
      this.pendingEffects.delete(frame.id);
      clearTimeout(waiting.timer);
      if (error) waiting.reject(error);
      else waiting.resolve(frame.result!);
      return;
    }
    if (frame.kind !== 'request' || typeof frame.id !== 'string' || !/^host:[1-9][0-9]*$/.test(frame.id)) {
      throw new Error('invalid request');
    }
    const sequence = Number(frame.id.slice(5));
    if (!Number.isSafeInteger(sequence) || sequence <= this.lastHostId || this.active >= MAX_PENDING) {
      throw new Error('request limit');
    }
    this.lastHostId = sequence;
    this.active++;
    const timer = setTimeout(() => this.fail(), DEADLINE_MS * 2);
    void Promise.resolve().then(async () => {
      if (frame.protocol !== PROTOCOL) throw new ProtocolError('incompatible_protocol');
      checkBinding(frame.binding, this.binding);
      return dispatch(frame);
    }).then(result => this.send({ protocol: PROTOCOL, kind: 'response', id: frame.id, binding: this.binding, result }),
      error => {
        if (!(error instanceof ProtocolError)) { this.diagnostic(error); this.fail(); return; }
        this.send({ protocol: PROTOCOL, kind: 'response', id: frame.id, binding: this.binding,
          error: { category: error.category, admission: error.admission } });
      }).finally(() => { clearTimeout(timer); this.active--; });
  }

  request(method: string, params: Record<string, Json>): Promise<Json> {
    if (method !== 'execution.request') {
      return Promise.reject(new Error('only execution.request may wait for an effect response'));
    }
    if (this.dead || this.pendingEffects.size >= MAX_PENDING) return Promise.reject(new ProtocolError('execution_uncertain', 'uncertain'));
    const id = `bridge:${++this.nextId}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.fail(); }, DEADLINE_MS);
      this.pendingEffects.set(id, { method, resolve, reject, timer });
      this.send({ protocol: PROTOCOL, kind: 'request', id, binding: this.binding, method, params });
    });
  }

  private send(frame: Frame) {
    if (this.dead) return;
    const data = Buffer.from(`${JSON.stringify(frame)}\n`);
    if (data.length > MAX_FRAME || process.stdout.writableLength + data.length > MAX_FRAME * MAX_PENDING) {
      this.fail(); return;
    }
    process.stdout.write(data);
  }

  // One line, flushed before exit. Name and message only: no stack, cause, or payload.
  private diagnostic(error: unknown) {
    try {
      const name = (error instanceof Error && error.name ? error.name : 'Error').slice(0, 40);
      const message = (error instanceof Error ? error.message : '').slice(0, 160);
      const line = `${name}: ${message}`.replace(/[\u0000-\u001f\u007f\u2028\u2029]/g, ' ').replace(/ {2,}/g, ' ').trim();
      writeSync(process.stderr.fd, `pi-bridge: ${line}\n`);
    } catch { /* best-effort; the peer still exits */ }
  }

  // Fatal session termination only. Not a general error path: recoverable
  // protocol errors are responses, and this exits the process.
  failFatal(error: unknown) {
    this.fail(error);
  }

  private fail(error?: unknown) {
    if (this.dead) return;
    if (error !== undefined) this.diagnostic(error);
    this.dead = true;
    for (const wait of this.pendingEffects.values()) {
      clearTimeout(wait.timer);
      wait.reject(new ProtocolError('interrupted', 'uncertain'));
    }
    this.pendingEffects.clear();
    // The owner must reconcile after any framing, timeout, storage or peer failure.
    // Immediate exit also fences this fixture's in-process tools. S3 owns OS process trees.
    process.exit(1);
  }
}
