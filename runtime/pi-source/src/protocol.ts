// The bridge accepts the shared safe-integer JSON subset; it never grants mission authority.
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';

export const MAX_FRAME = 65_536;
export const MAX_PENDING = 16;
export const DEADLINE_MS = 5_000;
export const SOURCE = 'e65b3b88c08412bb8ffbd21fb5dc5756064a6e5a';
export const ADAPTER = hash(['protocol.ts', 'transport.ts', 'runtime.ts', 'forwarders.ts',
  '../test/bridge-child.ts', '../pins.json', '../package-lock.json']
  .map(path => readFileSync(new URL(path, import.meta.url), 'utf8')));
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type Binding = { missionId: string; attemptId: string; sessionId: string; generation: number };
export type Frame = { protocol: number; kind: string; id: string; binding: Binding; method?: string;
  commandId?: string; params?: Record<string, Json>; result?: Json; error?: { category: string; admission: string } };

export class ProtocolError extends Error {
  category: string;
  admission: string;
  constructor(category: string, admission = 'rejected') { super(category); this.category = category; this.admission = admission; }
}

export function canonical(value: unknown): string {
  if (value === null || typeof value === 'boolean') return JSON.stringify(value);
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value)) throw new ProtocolError('invalid_number');
    return JSON.stringify(value);
  }
  if (typeof value === 'string') {
    if (!value.isWellFormed()) throw new ProtocolError('invalid_unicode');
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (typeof value === 'object' && value !== null) {
    return `{${Object.keys(value).sort().map(key => `${canonical(key)}:${canonical((value as Record<string, unknown>)[key])}`).join(',')}}`;
  }
  throw new ProtocolError('invalid_json');
}

export function hash(value: unknown): string {
  return `sha256:${createHash('sha256').update(canonical(value)).digest('hex')}`;
}

export function operationId(binding: Binding, task: string, key: string): string {
  return `op-v1:${hash(['switchyard.exec.v1', binding.attemptId, binding.sessionId, task, key]).slice(7)}`;
}

export function checkBinding(actual: Binding, expected: Binding): void {
  if (!actual || actual.missionId !== expected.missionId || actual.attemptId !== expected.attemptId
    || actual.sessionId !== expected.sessionId) throw new ProtocolError('binding_mismatch');
  if (actual.generation !== expected.generation) throw new ProtocolError('stale_generation');
}
