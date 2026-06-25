/**
 * The schema registry is the single seam every per-phase schema touchpoint
 * resolves through. Step 1 of docs/DESIGN-schema-overrides.md is pure
 * indirection: every handle must delegate to the built-in, byte-identically.
 * These tests pin that so a later override slice cannot silently change the
 * built-in path.
 */

import { describe, expect, it } from 'vitest';

import { AdwError } from '../src/errors.js';
import { AGENT_PHASES } from '../src/phases.js';
import { resolvePhaseSchema } from '../src/schema-registry.js';
import { OUTPUT_CONTRACT, parsePhaseResult, phaseJsonSchema, PHASE_SCHEMAS } from '../src/schemas.js';

describe('schema registry', () => {
  it('delegates jsonSchema/outputContract/requiredKeys to the built-in for every catalog phase', () => {
    for (const phase of AGENT_PHASES) {
      const handle = resolvePhaseSchema(phase);
      expect(handle.jsonSchema(), phase).toEqual(phaseJsonSchema(phase));
      expect(handle.outputContract(), phase).toBe(OUTPUT_CONTRACT[phase]);
      expect(handle.requiredKeys(), phase).toEqual(Object.keys(PHASE_SCHEMAS[phase].shape));
    }
  });

  it('validate() matches parsePhaseResult, coercion included', () => {
    // resolve coerces a numeric string and truncates a float, like parsePhaseResult.
    const payload = { resolved: '2', remaining: 0.9, summary: 'x' };
    expect(resolvePhaseSchema('resolve').validate(payload)).toEqual(parsePhaseResult('resolve', payload));
  });

  it('validate() rejects a non-object payload loudly, like the built-in', () => {
    expect(() => resolvePhaseSchema('plan').validate(['not', 'an', 'object'])).toThrow(AdwError);
  });
});
