// The bridge accepts the shared safe-integer JSON subset; it never grants mission authority.
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';

const pins = JSON.parse(readFileSync(new URL('../pins.json', import.meta.url), 'utf8'));
export const PROTOCOL: number = pins.bridge.protocol;
export const MAX_FRAME: number = pins.bridge.maxFrame;
export const MAX_PENDING: number = pins.bridge.maxPending;
export const DEADLINE_MS: number = pins.bridge.deadlineMs;
export const CAPABILITIES: string[] = pins.bridge.capabilities;
export const SOURCE: string = pins.commit;
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
  // Unlabeled errors are not definite refusals. Callers that mean "Pi will not
  // store this command" pass admission 'rejected' explicitly.
  constructor(category: string, admission = 'unspecified') { super(category); this.category = category; this.admission = admission; }
}

// Only the Rust execution owner supplies these dispositions, and only as the
// error body of an `execution.request` response. Do not reuse this for any
// other Pi-to-Rust method: an unknown or contradictory body terminates the
// transport rather than becoming a recoverable tool error (or success) inside Pi.
export function effectError(value: unknown): ProtocolError {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('invalid effect error');
  const fields = value as Record<string, unknown>;
  if (Object.keys(fields).sort().join(',') !== 'admission,category') throw new Error('invalid effect error');
  const valid = (fields.category === 'execution_rejected' && fields.admission === 'rejected')
    || (fields.category === 'operation_conflict' && fields.admission === 'rejected')
    || (fields.category === 'execution_uncertain' && fields.admission === 'uncertain');
  if (!valid) throw new Error('invalid effect disposition');
  return new ProtocolError(fields.category as string, fields.admission as string);
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
