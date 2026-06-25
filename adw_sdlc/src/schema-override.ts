/**
 * Loading, validating, and exemplifying project-supplied per-phase JSON Schema
 * overrides (docs/DESIGN-schema-overrides.md, capability A). Kept separate from
 * the registry so the registry stays a thin resolver; this module owns the only
 * use of ajv in the package.
 *
 * Overrides are DATA (JSON Schema), never code. ajv compiles them for
 * parent-side validation; the native-schema channel consumes the raw schema
 * directly. Remote `$ref`s are rejected — ajv would not fetch them, but on a
 * secrets-owning CLI we fail loudly rather than depend on that default.
 */

import { readFileSync } from 'node:fs';

import { Ajv } from 'ajv';
import type { ValidateFunction } from 'ajv';

import { AdwError } from './errors.js';
import type { JsonSchema } from './invoker.js';

/** Reject any `$ref` that points off-document (http/https/protocol-relative). */
function assertNoRemoteRefs(node: unknown, path: string): void {
  if (Array.isArray(node)) {
    for (const child of node) {
      assertNoRemoteRefs(child, path);
    }
    return;
  }
  if (node !== null && typeof node === 'object') {
    for (const [key, value] of Object.entries(node)) {
      if (key === '$ref' && typeof value === 'string' && /^(https?:)?\/\//i.test(value)) {
        throw new AdwError(`schema override may not use a remote $ref (${value}): ${path}`);
      }
      assertNoRemoteRefs(value, path);
    }
  }
}

/**
 * Read + parse + sanity-check an override schema file. A phase result is always
 * a JSON object, so the override must describe an object (explicit
 * `"type":"object"` or a `properties` map). Throws AdwError on any problem.
 */
export function loadOverrideSchema(path: string): JsonSchema {
  let raw: string;
  try {
    raw = readFileSync(path, 'utf8');
  } catch (err) {
    throw new AdwError(`could not read schema override: ${path}`, { cause: err });
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw) as unknown;
  } catch (err) {
    throw new AdwError(`schema override is not valid JSON: ${path}`, { cause: err });
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    throw new AdwError(`schema override must be a JSON object: ${path}`);
  }
  const schema = parsed as Record<string, unknown>;
  if (schema['type'] !== undefined && schema['type'] !== 'object') {
    throw new AdwError(`schema override for a phase result must be "type":"object": ${path}`);
  }
  if (schema['type'] === undefined && schema['properties'] === undefined) {
    throw new AdwError(`schema override must describe an object (set "type":"object" and "properties"): ${path}`);
  }
  assertNoRemoteRefs(schema, path);
  return schema as JsonSchema;
}

/** Compile a loaded override schema into an ajv validator. Throws on a bad schema. */
export function compileOverride(schema: JsonSchema, path: string): ValidateFunction {
  // Fresh instance per compile: avoids ajv's duplicate-`$id` cache error when
  // several overrides (or repeated resolves) share an `$id`, and keeps this
  // path free of cross-call state.
  const ajv = new Ajv({ allErrors: true, strict: false });
  try {
    return ajv.compile(schema);
  } catch (err) {
    throw new AdwError(`invalid JSON Schema in override: ${path}`, { cause: err });
  }
}

/** Format ajv errors into a single human-readable line. */
export function formatOverrideErrors(validate: ValidateFunction): string {
  return (validate.errors ?? [])
    .map((e) => `${e.instancePath || '/'} ${e.message ?? 'is invalid'}`.trim())
    .join('; ');
}

/** The keys an override declares — `required` if present, else its property names. */
export function overrideRequiredKeys(schema: JsonSchema): string[] {
  const s = schema as Record<string, unknown>;
  if (Array.isArray(s['required'])) {
    return s['required'].filter((k): k is string => typeof k === 'string');
  }
  const props = s['properties'];
  return props !== null && typeof props === 'object' ? Object.keys(props) : [];
}

/**
 * A minimal example value for a JSON Schema, bounded to the shapes ADW phase
 * results actually use (flat objects, arrays of those, primitives, enums). Used
 * to render the fenced-JSON output contract for non-native backends. Not a
 * general generator — it covers the documented subset and emits placeholders
 * (`"<string>"`, `0`, `true`) elsewhere.
 */
export function jsonSchemaExample(schema: JsonSchema): unknown {
  const s = schema as Record<string, unknown>;
  if (Array.isArray(s['enum']) && s['enum'].length > 0) {
    return s['enum'][0];
  }
  if (s['const'] !== undefined) {
    return s['const'];
  }
  switch (s['type']) {
    case 'string':
      return '<string>';
    case 'integer':
    case 'number':
      return 0;
    case 'boolean':
      return true;
    case 'null':
      return null;
    case 'array': {
      const items = s['items'];
      return items !== null && typeof items === 'object' && !Array.isArray(items)
        ? [jsonSchemaExample(items as JsonSchema)]
        : [];
    }
    default: {
      const props = s['properties'];
      const out: Record<string, unknown> = {};
      if (props !== null && typeof props === 'object') {
        for (const [key, value] of Object.entries(props)) {
          if (value !== null && typeof value === 'object') {
            out[key] = jsonSchemaExample(value as JsonSchema);
          }
        }
      }
      return out;
    }
  }
}
