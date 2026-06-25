/**
 * Capability B of docs/DESIGN-schema-overrides.md: a project can register a
 * NEW plain phase (config.customPhases), put it in the `phases` chain, and give
 * it a template (`<name>.md`) and a result schema (`.adw/schemas/<name>.json`).
 * It runs as a plain sequential phase — no loop, no conditional gate. Loop/gated
 * custom phases and built-in-name collisions are rejected.
 */

import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { afterEach, describe, expect, it } from 'vitest';

import { parseAdwConfig, setAdwConfigForTests } from '../src/config.js';
import { AdwError } from '../src/errors.js';
import { composePhasePrompt, knownPhaseNames, parsePhases } from '../src/phases.js';
import { resolvePhaseSchema } from '../src/schema-registry.js';
import { AdwState } from '../src/state.js';

const AUDIT_SCHEMA = {
  type: 'object',
  properties: { summary: { type: 'string' }, risk: { type: 'string' } },
  required: ['summary'],
  additionalProperties: false,
};

function dirWith(files: Record<string, unknown>): string {
  const dir = mkdtempSync(join(tmpdir(), 'adw-custom-'));
  for (const [name, body] of Object.entries(files)) {
    writeFileSync(join(dir, name), typeof body === 'string' ? body : JSON.stringify(body), 'utf8');
  }
  return dir;
}

afterEach(() => setAdwConfigForTests(null));

describe('custom phases — chain membership', () => {
  it('accepts a registered custom phase in the chain', () => {
    const config = parseAdwConfig({ customPhases: ['audit'], phases: ['plan', 'implement', 'audit'] });
    expect(parsePhases(undefined, config)).toEqual(['plan', 'implement', 'audit']);
    expect([...knownPhaseNames(config)]).toContain('audit');
  });

  it('rejects an unregistered phase name', () => {
    const config = parseAdwConfig({ customPhases: ['audit'] });
    expect(() => parsePhases('plan,ghost', config)).toThrow(/unknown phase/);
  });

  it('rejects a custom phase that collides with a built-in name', () => {
    const config = parseAdwConfig({ customPhases: ['review'] });
    expect(() => parsePhases(undefined, config)).toThrow(/collides/);
    expect(() => knownPhaseNames(config)).toThrow(/collides/);
  });
});

describe('custom phases — schema resolution', () => {
  it('returns an ajv handle for a registered custom phase with a schema', () => {
    const config = parseAdwConfig({ customPhases: ['audit'], schemas: { root: dirWith({ 'audit.json': AUDIT_SCHEMA }) } });
    const handle = resolvePhaseSchema('audit', config);
    expect(handle.requiredKeys()).toEqual(['summary']);
    expect(handle.validate({ summary: 'ok' })).toEqual({ summary: 'ok' });
    expect(() => handle.validate({ risk: 'high' })).toThrow(AdwError); // missing required summary
    expect(JSON.parse(handle.outputContract())).toEqual({ summary: '<string>', risk: '<string>' });
  });

  it('rejects a registered custom phase with no schema file', () => {
    const config = parseAdwConfig({ customPhases: ['audit'], schemas: { root: dirWith({}) } });
    expect(() => resolvePhaseSchema('audit', config)).toThrow(/requires a result schema/);
  });

  it('rejects an unregistered, non-built-in phase name', () => {
    const config = parseAdwConfig({ schemas: { root: dirWith({ 'ghost.json': AUDIT_SCHEMA }) } });
    expect(() => resolvePhaseSchema('ghost', config)).toThrow(/unknown phase/);
  });
});

describe('custom phases — prompt composition', () => {
  it('composes a full prompt from the custom template and generated contract', () => {
    const promptRoot = dirWith({ 'audit.md': 'Audit the change: $1' });
    const schemaRoot = dirWith({ 'audit.json': AUDIT_SCHEMA });
    setAdwConfigForTests(
      parseAdwConfig({
        customPhases: ['audit'],
        prompts: { defaultRoot: promptRoot, runnerRoots: {} },
        schemas: { root: schemaRoot },
      }),
    );
    const prompt = composePhasePrompt('audit', ['payment flow'], new AdwState({ adwId: 'a1b2c3d4' }), 'pi', true);
    expect(prompt).toContain('Audit the change: payment flow');
    expect(prompt).toContain('{"summary":"<string>","risk":"<string>"}'); // generated fenced-JSON contract
  });

  it('fails loudly when a custom phase has a schema but no template', () => {
    const schemaRoot = dirWith({ 'audit.json': AUDIT_SCHEMA });
    setAdwConfigForTests(
      parseAdwConfig({
        customPhases: ['audit'],
        prompts: { defaultRoot: dirWith({}), runnerRoots: {} },
        schemas: { root: schemaRoot },
      }),
    );
    expect(() => composePhasePrompt('audit', ['x'], new AdwState({ adwId: 'a1b2c3d4' }), 'pi', true)).toThrow(
      /prompt template not found/,
    );
  });
});
