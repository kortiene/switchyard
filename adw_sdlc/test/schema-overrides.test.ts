/**
 * Capability A of docs/DESIGN-schema-overrides.md: a project may override the
 * structured-output schema of a phase WITHOUT load-bearing result fields
 * (tests/e2e/document). Overriding a load-bearing or excluded phase is a loud
 * error. With no override file, every phase keeps its built-in behavior.
 */

import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { describe, expect, it } from 'vitest';

import { parseAdwConfig } from '../src/config.js';
import { AdwError } from '../src/errors.js';
import { jsonSchemaExample, loadOverrideSchema } from '../src/schema-override.js';
import { OVERRIDABLE_PHASES, resolvePhaseSchema } from '../src/schema-registry.js';
import { OUTPUT_CONTRACT, parsePhaseResult } from '../src/schemas.js';

/** A schema dir with the given <phase>.json files, plus a config pointing at it. */
function schemaDir(files: Record<string, unknown>) {
  const dir = mkdtempSync(join(tmpdir(), 'adw-schemas-'));
  for (const [name, schema] of Object.entries(files)) {
    writeFileSync(join(dir, name), JSON.stringify(schema), 'utf8');
  }
  return { dir, config: parseAdwConfig({ schemas: { root: dir } }) };
}

const TESTS_OVERRIDE = {
  type: 'object',
  properties: {
    tests_added: { type: 'boolean' },
    summary: { type: 'string' },
    coverage_pct: { type: 'integer' },
  },
  required: ['tests_added', 'summary'],
  additionalProperties: false,
};

describe('per-phase schema overrides (capability A)', () => {
  it('overridable phases are exactly the ones with no load-bearing result fields', () => {
    expect([...OVERRIDABLE_PHASES].sort()).toEqual(['document', 'e2e', 'tests']);
  });

  it('routes an overridable phase through the project schema (convention path)', () => {
    const { config } = schemaDir({ 'tests.json': TESTS_OVERRIDE });
    const handle = resolvePhaseSchema('tests', config);

    expect(handle.jsonSchema()).toEqual(TESTS_OVERRIDE);
    expect(handle.requiredKeys()).toEqual(['tests_added', 'summary']);
    // A conforming payload passes through untouched.
    expect(handle.validate({ tests_added: true, summary: 'added cases' })).toEqual({
      tests_added: true,
      summary: 'added cases',
    });
    // A non-conforming payload fails loudly (no built-in coercion masks it).
    expect(() => handle.validate({ tests_added: 'yes', summary: 'x' })).toThrow(AdwError);
    expect(() => handle.validate(['not', 'an', 'object'])).toThrow(AdwError);
  });

  it('renders an output contract example that validates against the override', () => {
    const { config } = schemaDir({ 'tests.json': TESTS_OVERRIDE });
    const handle = resolvePhaseSchema('tests', config);
    const example = JSON.parse(handle.outputContract());
    expect(example).toEqual({ tests_added: true, summary: '<string>', coverage_pct: 0 });
    // The generated example must itself satisfy the override.
    expect(() => handle.validate(example)).not.toThrow();
  });

  it('honors an explicit overrides[phase] path', () => {
    const dir = mkdtempSync(join(tmpdir(), 'adw-schemas-'));
    const path = join(dir, 'e2e.custom.json');
    const e2eSchema = { type: 'object', properties: { e2e_added: { type: 'boolean' } }, required: ['e2e_added'] };
    writeFileSync(path, JSON.stringify(e2eSchema), 'utf8');
    const config = parseAdwConfig({ schemas: { overrides: { e2e: path } } });
    expect(resolvePhaseSchema('e2e', config).jsonSchema()).toEqual(e2eSchema);
  });

  it('rejects an override of a load-bearing or excluded phase loudly', () => {
    const { config } = schemaDir({
      'implement.json': TESTS_OVERRIDE,
      'classify.json': TESTS_OVERRIDE,
      'review.json': TESTS_OVERRIDE,
    });
    expect(() => resolvePhaseSchema('implement', config)).toThrow(/not supported/);
    expect(() => resolvePhaseSchema('classify', config)).toThrow(/not supported/);
    expect(() => resolvePhaseSchema('review', config)).toThrow(/not supported/);
  });

  it('fails loudly when an explicit override path is missing', () => {
    const config = parseAdwConfig({ schemas: { overrides: { tests: '/no/such/schema.json' } } });
    expect(() => resolvePhaseSchema('tests', config)).toThrow(/not found/);
  });

  it('keeps built-in behavior when no override file is present', () => {
    const { config } = schemaDir({}); // empty dir
    const handle = resolvePhaseSchema('tests', config);
    expect(handle.outputContract()).toBe(OUTPUT_CONTRACT['tests']);
    // Built-in path still coerces like parsePhaseResult (Python parity).
    expect(handle.validate({ tests_added: 'truthy' })).toEqual(parsePhaseResult('tests', { tests_added: 'truthy' }));
  });
});

describe('override loader guards', () => {
  function write(content: unknown): string {
    const dir = mkdtempSync(join(tmpdir(), 'adw-schema-file-'));
    const path = join(dir, 's.json');
    writeFileSync(path, typeof content === 'string' ? content : JSON.stringify(content), 'utf8');
    return path;
  }

  it('rejects non-JSON, non-object, non-object-typed, and remote-$ref schemas', () => {
    expect(() => loadOverrideSchema(write('{not json'))).toThrow(/not valid JSON/);
    expect(() => loadOverrideSchema(write([1, 2, 3]))).toThrow(/must be a JSON object/);
    expect(() => loadOverrideSchema(write({ type: 'array' }))).toThrow(/"type":"object"/);
    expect(() => loadOverrideSchema(write({ type: 'object', properties: { x: { $ref: 'https://evil/x.json' } } }))).toThrow(
      /remote \$ref/,
    );
  });

  it('accepts a bare properties object (implicit object type)', () => {
    expect(loadOverrideSchema(write({ properties: { ok: { type: 'boolean' } } }))).toEqual({
      properties: { ok: { type: 'boolean' } },
    });
  });

  it('generates bounded examples (enum, array, nested object, primitives)', () => {
    expect(jsonSchemaExample({ enum: ['a', 'b'] })).toBe('a');
    expect(jsonSchemaExample({ type: 'array', items: { type: 'string' } })).toEqual(['<string>']);
    expect(
      jsonSchemaExample({
        type: 'object',
        properties: { n: { type: 'integer' }, flag: { type: 'boolean' }, nested: { type: 'object', properties: { s: { type: 'string' } } } },
      }),
    ).toEqual({ n: 0, flag: true, nested: { s: '<string>' } });
  });
});
