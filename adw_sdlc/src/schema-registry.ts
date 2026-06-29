/**
 * The per-phase schema seam.
 *
 * Four call sites need a phase's structured-output schema: the native-schema
 * channel (the JSON Schema handed to a runner), the parent-side validate/coerce
 * step, the fenced-JSON output contract rendered into the prompt footer, and —
 * for the load-bearing-field check — the set of keys a phase declares. Each one
 * resolves through `resolvePhaseSchema` here, so a project-supplied override can
 * be returned for a phase without touching the call sites.
 *
 * Built-in phases return the Zod schema in schemas.ts (with its Python-parity
 * coercion). A project may override the schema of a phase whose result fields
 * the orchestrator does NOT read — see OVERRIDABLE_PHASES; overriding any other
 * phase is rejected loudly (docs/DESIGN-schema-overrides.md §5/§6.6), because
 * the kernel's control flow depends on those built-in shapes.
 */

import { existsSync } from 'node:fs';
import { join } from 'node:path';

import { getAdwConfig, resolvePackagePath, resolveRepoPath, type AdwConfig } from './config.js';
import { AdwError } from './errors.js';
import type { JsonSchema } from './invoker.js';
import {
  compileOverride,
  formatOverrideErrors,
  jsonSchemaExample,
  loadOverrideSchema,
  overrideRequiredKeys,
} from './schema-override.js';
import { OUTPUT_CONTRACT, parsePhaseResult, phaseJsonSchema, PHASE_SCHEMAS, type SchemaPhase } from './schemas.js';
import type { z } from 'zod';

/**
 * Phases a project may override: those with NO control-flow-bearing result
 * fields (the orchestrator records but never branches on `tests`/`e2e`/
 * `document` results). Every other built-in phase is load-bearing
 * (classify/plan/implement/review/resolve/patch — see the design §5 table) and
 * classify additionally runs on a Zod-only in-process path, so they stay
 * built-in until a later slice designs the interaction.
 */
export const OVERRIDABLE_PHASES: ReadonlySet<SchemaPhase> = new Set(['tests', 'e2e', 'document']);

/** Everything a call site needs to drive structured output for one phase. */
export interface PhaseSchemaHandle<P extends SchemaPhase> {
  /** JSON Schema for backends with native schema output. */
  jsonSchema(): JsonSchema;
  /** Parse + validate a raw runner payload into the phase's result shape. */
  validate(payload: unknown): z.infer<(typeof PHASE_SCHEMAS)[P]>;
  /** Example JSON shape rendered into the fenced-JSON prompt footer. */
  outputContract(): string;
  /** Keys the phase declares — the basis for the load-bearing-field check. */
  requiredKeys(): readonly string[];
}

/** The built-in handle: today's exact behavior for a catalog phase. */
function builtinHandle<P extends SchemaPhase>(phase: P): PhaseSchemaHandle<P> {
  return {
    jsonSchema: () => phaseJsonSchema(phase),
    validate: (payload) => parsePhaseResult(phase, payload),
    outputContract: () => OUTPUT_CONTRACT[phase],
    requiredKeys: () => Object.keys(PHASE_SCHEMAS[phase].shape),
  };
}

/** A handle backed by a project-supplied JSON Schema, validated with ajv. */
function overrideHandle<P extends SchemaPhase>(phase: P, schema: JsonSchema, path: string): PhaseSchemaHandle<P> {
  const validate = compileOverride(schema, path);
  return {
    jsonSchema: () => schema,
    validate: (payload) => {
      if (typeof payload !== 'object' || payload === null || Array.isArray(payload)) {
        throw new AdwError(`${phase} phase output must be a JSON object`);
      }
      if (!validate(payload)) {
        throw new AdwError(
          `${phase} phase output does not match its override schema (${path}): ${formatOverrideErrors(validate)}`,
        );
      }
      // Safe cast: overridable phases carry no control-flow-bearing fields, so
      // the orchestrator records this value but never reads typed fields off it.
      return payload as z.infer<(typeof PHASE_SCHEMAS)[P]>;
    },
    outputContract: () => JSON.stringify(jsonSchemaExample(schema)),
    requiredKeys: () => overrideRequiredKeys(schema),
  };
}

/**
 * Resolve the override file for `phase`, or null when none is configured. An
 * explicit `overrides[phase]` is a deliberate project choice and stays
 * project-root-resolved — a missing one fails loudly (no silent fallback). The
 * convention path `root/<phase>.json` resolves from the PROJECT root first,
 * then falls back to the PACKAGE root (bundled kernel defaults), so a target
 * that customizes only `.adw/config.json` and ships no `.adw/schemas` still
 * resolves a phase's schema. In-repo both tiers are the same directory.
 */
function overridePath(phase: string, config: AdwConfig): string | null {
  const explicit = config.schemas?.overrides[phase];
  if (explicit !== undefined) {
    const resolved = resolveRepoPath(explicit);
    if (!existsSync(resolved)) {
      throw new AdwError(`configured schema override for "${phase}" not found: ${resolved}`);
    }
    return resolved;
  }
  const root = config.schemas?.root ?? '.adw/schemas';
  const projectCandidate = join(resolveRepoPath(root), `${phase}.json`);
  if (existsSync(projectCandidate)) {
    return projectCandidate;
  }
  const packageCandidate = join(resolvePackagePath(root), `${phase}.json`);
  return existsSync(packageCandidate) ? packageCandidate : null;
}

/**
 * Resolve the schema handle for `phase`:
 * - a built-in phase returns its Zod handle, unless the project supplies an
 *   override file for an OVERRIDABLE phase (an override of a load-bearing or
 *   excluded phase is a loud error);
 * - a project-registered custom phase (config.customPhases) returns an
 *   ajv-backed handle from its required schema file;
 * - any other name is unknown and rejected.
 */
export function resolvePhaseSchema<P extends SchemaPhase>(phase: P, config?: AdwConfig): PhaseSchemaHandle<P>;
export function resolvePhaseSchema(phase: string, config?: AdwConfig): PhaseSchemaHandle<SchemaPhase>;
export function resolvePhaseSchema(
  phase: string,
  config: AdwConfig = getAdwConfig(),
): PhaseSchemaHandle<SchemaPhase> {
  const path = overridePath(phase, config);
  if (phase in PHASE_SCHEMAS) {
    const builtin = phase as SchemaPhase;
    if (path === null) {
      return builtinHandle(builtin);
    }
    if (!OVERRIDABLE_PHASES.has(builtin)) {
      throw new AdwError(
        `schema override for "${phase}" is not supported: it is load-bearing or excluded (classify). ` +
          `Overridable phases: ${[...OVERRIDABLE_PHASES].join(', ')}. See docs/DESIGN-schema-overrides.md.`,
      );
    }
    return overrideHandle(builtin, loadOverrideSchema(path), path);
  }
  // Custom (project-registered) phase: requires its own result schema.
  if (!(config.customPhases ?? []).includes(phase)) {
    throw new AdwError(`unknown phase "${phase}": not a built-in phase and not listed in config.customPhases`);
  }
  if (path === null) {
    const root = config.schemas?.root ?? '.adw/schemas';
    throw new AdwError(
      `custom phase "${phase}" requires a result schema; none found at ` +
        `${join(resolveRepoPath(root), `${phase}.json`)} (or config.schemas.overrides["${phase}"])`,
    );
  }
  return overrideHandle(phase as SchemaPhase, loadOverrideSchema(path), path);
}
