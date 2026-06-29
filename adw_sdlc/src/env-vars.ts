import { AdwError } from './errors.js';

export interface EnvAlias {
  /** Canonical ADW control-plane env var. */
  canonical: string;
  /** Deprecated compatibility alias inherited from mx-agent. */
  legacy: string;
}

export const ENV_ALIASES = {
  engine: { canonical: 'ADW_ENGINE', legacy: 'MX_AGENT_ENGINE' },
  runner: { canonical: 'ADW_RUNNER', legacy: 'MX_AGENT_RUNNER' },
  testCmd: { canonical: 'ADW_TEST_CMD', legacy: 'MX_AGENT_TEST_CMD' },
  finalizeGates: { canonical: 'ADW_FINALIZE_GATES', legacy: 'MX_AGENT_FINALIZE_GATES' },
  classifyOnRunner: { canonical: 'ADW_CLASSIFY_ON_RUNNER', legacy: 'MX_AGENT_CLASSIFY_ON_RUNNER' },
  assumeYes: { canonical: 'ADW_ASSUME_YES', legacy: 'MX_AGENT_YES' },
  forceFenced: { canonical: 'ADW_PARITY_FORCE_FENCED_JSON', legacy: 'MX_AGENT_FORCE_FENCED' },
  // The legacy MX_AGENT_PROJECT_ROOT twin never existed in mx-agent; it is
  // included only for table uniformity (so readEnvAlias's precedence/conflict
  // handling applies). ADW_PROJECT_ROOT is covered by the ADW_ deny prefix, so
  // it is withheld from runner children with no allowlist change.
  projectRoot: { canonical: 'ADW_PROJECT_ROOT', legacy: 'MX_AGENT_PROJECT_ROOT' },
} as const;

const warnedLegacy = new Set<string>();

function warnLegacyEnv(alias: EnvAlias): void {
  if (warnedLegacy.has(alias.legacy)) {
    return;
  }
  warnedLegacy.add(alias.legacy);
  process.stderr.write(`>> ${alias.legacy} is deprecated; use ${alias.canonical} instead.\n`);
}

/** Return the canonical per-phase model override env names for a phase. */
export function modelEnvAlias(phase: string): EnvAlias {
  const suffix = phase.toUpperCase();
  return { canonical: `ADW_MODEL_${suffix}`, legacy: `MX_AGENT_MODEL_${suffix}` };
}

/**
 * Read a canonical ADW env var with an mx-agent compatibility alias.
 *
 * The canonical name is preferred. If the legacy alias is also set, it must
 * agree; different values fail loudly rather than silently choosing one. This
 * keeps migrations deterministic and prevents two scripts from disagreeing
 * about a control-plane knob.
 */
export function readEnvAlias(
  env: Record<string, string | undefined>,
  alias: EnvAlias,
  opts: { warnLegacy?: boolean } = {},
): string | undefined {
  const canonicalValue = env[alias.canonical];
  const legacyValue = env[alias.legacy];

  if (canonicalValue !== undefined && legacyValue !== undefined && canonicalValue !== legacyValue) {
    throw new AdwError(
      `conflicting env vars: ${alias.canonical} and deprecated ${alias.legacy} are both set with different values`,
    );
  }

  if (canonicalValue === undefined && legacyValue !== undefined && opts.warnLegacy !== false) {
    warnLegacyEnv(alias);
  }

  return canonicalValue ?? legacyValue;
}

/** Convenience for boolean ADW env toggles that are enabled only by the string "1". */
export function readEnvFlag(env: Record<string, string | undefined>, alias: EnvAlias): boolean {
  return readEnvAlias(env, alias) === '1';
}
