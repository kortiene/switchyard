/**
 * Tier→model routing per runner (PLAN.md Section 6), ported from
 * adw/_phases.py:55-71 (PHASE_TIER / TIER_MODELS) and model_for_phase
 * (adw/_phases.py:88-97). Override precedence is preserved verbatim:
 * --model > MX_AGENT_MODEL_<PHASE> > tier default.
 */

import { DEFAULT_ADW_CONFIG, getAdwConfig, type AdwConfig } from './config.js';
import type { RunnerId } from './invoker.js';

export type Tier = 'cheap' | 'mid' | 'capable';

/** Phase → model tier (adw/_phases.py:55-65). Unknown phases resolve as 'mid'. */
export const PHASE_TIER: Record<string, Tier> = DEFAULT_ADW_CONFIG.models.phaseTiers;

/**
 * Tier → concrete model id, per runner.
 *
 * - claude: exact current Claude IDs (PLAN.md Section 6).
 * - pi: bare model names, matching the Python TIER_MODELS verbatim — pi
 *   accepts them and users override via --model / MX_AGENT_MODEL_<PHASE>.
 * - codex: verified current in roadmap step 7 (Codex models endpoint cache
 *   of 2026-05-31 + the OpenAI pricing docs): gpt-5.4-mini / gpt-5.4 /
 *   gpt-5.5, all supported_in_api with effort low|medium|high|xhigh. The
 *   newest generations dropped the `-codex` suffix (last was gpt-5.3-codex).
 * - opencode: provider/model strings; Anthropic models by default.
 */
export const TIER_MODELS: Record<RunnerId, Record<Tier, string>> = {
  claude: {
    cheap: DEFAULT_ADW_CONFIG.models.tiers.cheap.claude,
    mid: DEFAULT_ADW_CONFIG.models.tiers.mid.claude,
    capable: DEFAULT_ADW_CONFIG.models.tiers.capable.claude,
  },
  pi: {
    cheap: DEFAULT_ADW_CONFIG.models.tiers.cheap.pi,
    mid: DEFAULT_ADW_CONFIG.models.tiers.mid.pi,
    capable: DEFAULT_ADW_CONFIG.models.tiers.capable.pi,
  },
  codex: {
    cheap: DEFAULT_ADW_CONFIG.models.tiers.cheap.codex,
    mid: DEFAULT_ADW_CONFIG.models.tiers.mid.codex,
    capable: DEFAULT_ADW_CONFIG.models.tiers.capable.codex,
  },
  opencode: {
    cheap: DEFAULT_ADW_CONFIG.models.tiers.cheap.opencode,
    mid: DEFAULT_ADW_CONFIG.models.tiers.mid.opencode,
    capable: DEFAULT_ADW_CONFIG.models.tiers.capable.opencode,
  },
};

/**
 * The classify phase runs on the shared Anthropic SDK structured call with
 * this model regardless of the selected runner (PLAN.md D1).
 */
export const CLASSIFY_MODEL = DEFAULT_ADW_CONFIG.models.classifyModel;

/** Configurable default classify model; CLASSIFY_MODEL is the built-in default snapshot. */
export function classifyModel(config: AdwConfig = getAdwConfig()): string {
  return config.models.classifyModel;
}

export interface ModelOverrides {
  /** --model: applies to every phase when set. */
  cliModel?: string;
  /** Environment for MX_AGENT_MODEL_<PHASE> lookups; defaults to process.env. */
  env?: Record<string, string | undefined>;
  /** Project config; defaults to the loaded .adw/config.json/defaults. */
  config?: AdwConfig;
}

/** Resolve the model for `phase` on `runner`: --model > MX_AGENT_MODEL_<PHASE> > tier default. */
export function modelForPhase(phase: string, runner: RunnerId, overrides: ModelOverrides = {}): string {
  if (overrides.cliModel) {
    return overrides.cliModel;
  }
  const env = overrides.env ?? process.env;
  const envOverride = env[`MX_AGENT_MODEL_${phase.toUpperCase()}`];
  if (envOverride) {
    return envOverride;
  }
  const config = overrides.config ?? getAdwConfig();
  const tier = config.models.phaseTiers[phase] ?? config.models.defaultTier;
  return config.models.tiers[tier][runner] ?? TIER_MODELS[runner][tier];
}
