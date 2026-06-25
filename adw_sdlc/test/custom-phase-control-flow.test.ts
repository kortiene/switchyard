/**
 * docs/DESIGN-custom-phase-control-flow.md: a registered custom phase may opt
 * into a conditional gate (run only when the change signal/files match) or a
 * resolve-style loop (run a command; fix-and-retry on failure). Built-in phases
 * keep their kernel-owned control flow; control-flow config may only target a
 * registered custom phase, and a loop phase's schema must declare `resolved`.
 */

import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { describe, expect, it } from 'vitest';

import { parseAdwConfig } from '../src/config.js';
import {
  gateConditional,
  gateCustom,
  isConditionalPhase,
  validatePhaseChain,
  type CustomGateRule,
} from '../src/phases.js';

function dirWith(files: Record<string, unknown>): string {
  const dir = mkdtempSync(join(tmpdir(), 'adw-cf-'));
  for (const [name, body] of Object.entries(files)) {
    writeFileSync(join(dir, name), typeof body === 'string' ? body : JSON.stringify(body), 'utf8');
  }
  return dir;
}

const LOOP_SCHEMA = {
  type: 'object',
  properties: { resolved: { type: 'integer' }, remaining: { type: 'integer' } },
  required: ['resolved'],
  additionalProperties: false,
};
// A plain custom-phase schema with no `resolved` field.
const PLAIN_SCHEMA = {
  type: 'object',
  properties: { summary: { type: 'string' } },
  required: ['summary'],
  additionalProperties: false,
};

const rule = (over: Partial<CustomGateRule> = {}): CustomGateRule => ({
  hints: [],
  exactFiles: [],
  pathPrefixes: [],
  fileExtensions: [],
  ...over,
});

describe('custom gates — matching', () => {
  it('runs on a hint, a file rule, or neither', () => {
    const r = rule({ hints: ['payment'], exactFiles: ['LICENSE'], pathPrefixes: ['src/billing/'], fileExtensions: ['.sql'] });
    expect(gateCustom(r, 'touch the payment flow', []).runIt).toBe(true);
    expect(gateCustom(r, 'x', ['src/billing/charge.ts']).runIt).toBe(true);
    expect(gateCustom(r, 'x', ['db/schema.sql']).runIt).toBe(true);
    expect(gateCustom(r, 'x', ['LICENSE']).runIt).toBe(true);
    expect(gateCustom(r, 'unrelated change', ['src/app.ts']).runIt).toBe(false);
  });

  it('matches hints on whole words only', () => {
    const r = rule({ hints: ['auth'] });
    expect(gateCustom(r, 'add auth guard', []).runIt).toBe(true);
    expect(gateCustom(r, 'authorize endpoint', []).runIt).toBe(false); // not a whole-word "auth"
  });
});

describe('custom gates — dispatch', () => {
  const config = parseAdwConfig({
    customPhases: ['audit'],
    gates: { custom: { audit: { pathPrefixes: ['src/billing/'] } } },
  });

  it('isConditionalPhase recognizes built-in and custom gates', () => {
    expect(isConditionalPhase('e2e', config)).toBe(true);
    expect(isConditionalPhase('document', config)).toBe(true);
    expect(isConditionalPhase('audit', config)).toBe(true);
    expect(isConditionalPhase('plan', config)).toBe(false);
  });

  it('gateConditional routes a custom phase through its gate', () => {
    expect(gateConditional('audit', 'x', ['src/billing/charge.ts'], config).runIt).toBe(true);
    expect(gateConditional('audit', 'x', ['src/app.ts'], config).runIt).toBe(false);
    // A non-conditional phase still fails loudly.
    expect(() => gateConditional('plan', 'x', [], config)).toThrow(/not a conditional phase/);
  });
});

describe('control-flow config — startup validation', () => {
  it('accepts a fully-wired gated + looped custom phase', () => {
    const config = parseAdwConfig({
      customPhases: ['verify'],
      prompts: { defaultRoot: dirWith({ 'plan.md': 'P', 'verify.md': 'V' }), runnerRoots: {} },
      schemas: { root: dirWith({ 'verify.json': LOOP_SCHEMA }) },
      gates: { custom: { verify: { hints: ['payment'] } } },
      loops: { verify: { command: 'npm run verify' } },
    });
    expect(() => validatePhaseChain(['plan', 'verify'], 'pi', config)).not.toThrow();
  });

  it('rejects control flow targeting a built-in phase', () => {
    const gate = parseAdwConfig({ gates: { custom: { e2e: { hints: ['x'] } } } });
    expect(() => validatePhaseChain([], 'pi', gate)).toThrow(/built-in phases own their control flow/);
    const loop = parseAdwConfig({ loops: { tests: { command: 'x' } } });
    expect(() => validatePhaseChain([], 'pi', loop)).toThrow(/built-in phases own their control flow/);
  });

  it('rejects control flow naming an unregistered phase', () => {
    const config = parseAdwConfig({ loops: { ghost: { command: 'x' } } });
    expect(() => validatePhaseChain([], 'pi', config)).toThrow(/unregistered phase/);
  });

  it('rejects a custom gate with no matchers', () => {
    const config = parseAdwConfig({ customPhases: ['audit'], gates: { custom: { audit: {} } } });
    expect(() => validatePhaseChain([], 'pi', config)).toThrow(/no matchers/);
  });

  it('rejects a loop phase whose schema omits "resolved"', () => {
    const config = parseAdwConfig({
      customPhases: ['verify'],
      prompts: { defaultRoot: dirWith({ 'verify.md': 'V' }), runnerRoots: {} },
      schemas: { root: dirWith({ 'verify.json': PLAIN_SCHEMA }) },
      loops: { verify: { command: 'npm run verify' } },
    });
    expect(() => validatePhaseChain(['verify'], 'pi', config)).toThrow(/must declare "resolved"/);
  });
});
