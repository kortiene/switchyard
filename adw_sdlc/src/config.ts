/**
 * Project-level ADW configuration.
 *
 * The universalization seam starts here: the deterministic control plane keeps
 * its safety guarantees in code, while project/domain policy moves into a
 * validated .adw/config.json. Missing config falls back to the current
 * HealthTech standalone-port defaults so this is behavior-preserving.
 */

import { existsSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

import { z } from 'zod';

import { REPO_ROOT } from './common.js';
import { AdwError } from './errors.js';

export const ADW_CONFIG_PATH = join(REPO_ROOT, '.adw', 'config.json');

const TierSchema = z.enum(['cheap', 'mid', 'capable']);
const RunnerModelMapSchema = z
  .object({
    claude: z.string().min(1),
    codex: z.string().min(1),
    opencode: z.string().min(1),
    pi: z.string().min(1),
  })
  .catchall(z.string().min(1));

export const AdwConfigSchema = z.object({
  version: z.literal(1),
  project: z.object({
    id: z.string().min(1),
    name: z.string().min(1),
  }),
  prompts: z.object({
    /** Fallback template root, relative to the repository root unless absolute. */
    defaultRoot: z.string().min(1),
    /** Runner-specific template roots (e.g. claude -> .claude/commands). */
    runnerRoots: z.record(z.string(), z.string().min(1)),
  }),
  /**
   * Ordered agent-phase chain for the run. Optional: when omitted the kernel
   * runs the full built-in catalog (classify..document). Validated here for
   * shape only (a non-empty list of non-empty names); membership is checked by
   * `parsePhases` against the kernel's `AGENT_PHASES`, because each phase's
   * loop/gate/structured-output semantics live in the kernel and are NOT
   * project-configurable. Projects may reorder or drop known phases; inventing
   * a genuinely new phase name is a separate concern (per-phase schema
   * overrides), not this field.
   */
  phases: z.array(z.string().min(1)).min(1).optional(),
  /**
   * Per-phase structured-output schema overrides (see
   * docs/DESIGN-schema-overrides.md). Optional. `root` is a directory of
   * `<phase>.json` JSON Schema files (convention); `overrides` maps a phase to
   * an explicit file path. Resolution: `overrides[phase]` > `root/<phase>.json`
   * > built-in. Only phases without load-bearing result fields are overridable;
   * the kernel rejects the rest loudly (validated at resolve time, not here).
   */
  schemas: z
    .object({
      root: z.string().min(1).default('.adw/schemas'),
      overrides: z.record(z.string(), z.string().min(1)).default({}),
    })
    .optional(),
  /**
   * Project-registered NEW phase names (beyond the built-in catalog). A custom
   * phase may appear in the `phases` chain; it runs as a plain, sequential
   * agent phase (no loop, no conditional gate). Each requires a prompt template
   * (`<name>.md` under the prompt roots) and a result schema
   * (`.adw/schemas/<name>.json`); its model tier comes from
   * `models.phaseTiers[name]` (else the default tier). A name colliding with a
   * built-in phase is rejected. See docs/DESIGN-schema-overrides.md (capability B).
   */
  customPhases: z.array(z.string().min(1)).optional(),
  providers: z.object({
    cli: z.object({ type: z.literal('github') }),
    workItems: z.object({
      type: z.literal('github'),
      /** Provider state values that count as terminal/closed for the verify gate. */
      closedStates: z.array(z.string().min(1)).default(['CLOSED']),
      /** Status applied to a work item when its phased run starts (provider-neutral name). */
      inProgressStatus: z.string().min(1).default('In Progress'),
      /**
       * Status applied to a work item once its run is merged AND verified.
       * Optional and unset by default: GitHub auto-closes the issue via
       * "closes #<n>", so this exists for providers (or Projects boards) that
       * need an explicit terminal transition. Best-effort — a failed status
       * update never undoes a completed merge. For non-GitHub providers whose
       * verify gate reads this same status axis, include `doneStatus` in
       * `closedStates` so the post-merge verification recognises it as terminal.
       */
      doneStatus: z.string().min(1).optional(),
      /** Provider field name carrying the workflow status (GitHub Projects default: 'Status'). */
      statusFieldName: z.string().min(1).default('Status'),
    }),
    vcs: z.object({ type: z.literal('git') }),
    changeRequests: z.object({ type: z.literal('github') }),
  }),
  progress: z.object({
    /** Marker used on orchestrator-authored progress comments. */
    tag: z.string().min(1),
  }),
  branching: z.object({
    defaultPrefix: z.string().min(1),
    labelPrefixes: z.record(z.string(), z.string().min(1)),
    slug: z.object({
      maxLength: z.number().int().positive(),
      stripDiacritics: z.boolean(),
      stripPhaseIssuePrefix: z.boolean(),
    }),
  }),
  gates: z.object({
    e2e: z.object({
      hints: z.array(z.string().min(1)),
    }),
    documentation: z.object({
      hints: z.array(z.string().min(1)),
      exactFiles: z.array(z.string().min(1)),
      pathPrefixes: z.array(z.string().min(1)),
      fileExtensions: z.array(z.string().min(1)),
    }),
  }),
  models: z.object({
    classifyModel: z.string().min(1),
    defaultTier: TierSchema,
    phaseTiers: z.record(z.string(), TierSchema),
    tiers: z.object({
      cheap: RunnerModelMapSchema,
      mid: RunnerModelMapSchema,
      capable: RunnerModelMapSchema,
    }),
  }),
  commands: z.object({
    defaultTestCommand: z.string(),
    defaultFinalizeGates: z.array(z.string()),
  }),
});

export type AdwConfig = z.infer<typeof AdwConfigSchema>;

export const DEFAULT_ADW_CONFIG: AdwConfig = {
  version: 1,
  project: { id: 'healthtech', name: 'HealthTech' },
  prompts: {
    defaultRoot: '.pi/prompts',
    runnerRoots: {
      claude: '.claude/commands',
    },
  },
  providers: {
    cli: { type: 'github' },
    workItems: {
      type: 'github',
      closedStates: ['CLOSED'],
      inProgressStatus: 'In Progress',
      statusFieldName: 'Status',
    },
    vcs: { type: 'git' },
    changeRequests: { type: 'github' },
  },
  progress: { tag: '[MX-ADW]' },
  branching: {
    defaultPrefix: 'feat',
    labelPrefixes: {
      'type:bug': 'fix',
      'type:docs': 'docs',
      'type:ci': 'ci',
      'type:testing': 'test',
      bug: 'fix',
      docs: 'docs',
      documentation: 'docs',
      'tech-debt': 'refactor',
      infra: 'ci',
      ci: 'ci',
      test: 'test',
      testing: 'test',
      feature: 'feat',
    },
    slug: {
      maxLength: 40,
      stripDiacritics: true,
      stripPhaseIssuePrefix: true,
    },
  },
  gates: {
    e2e: {
      hints: [
        'ipc',
        'daemon',
        'matrix',
        'signing',
        'signed',
        'signature',
        'trust',
        'policy',
        'sandbox',
        'pty',
        'stream',
        'artifact',
        'exec',
        'login',
        'sync',
        'scheduler',
        'crypto',
        'encryption',
        'decryption',
        'auth',
        'authentication',
        'consent',
        'qr',
        'offline',
        'backup',
        'recovery',
      ],
    },
    documentation: {
      hints: [
        'cli',
        'help',
        'public api',
        'protocol',
        'schema',
        'user-visible',
        'user facing',
        'user-facing',
        'config',
        'command',
        'flag',
        'endpoint',
        'migration',
      ],
      exactFiles: ['README.md'],
      pathPrefixes: ['docs/', 'wiki/'],
      fileExtensions: ['.md'],
    },
  },
  models: {
    classifyModel: 'claude-haiku-4-5',
    defaultTier: 'mid',
    phaseTiers: {
      classify: 'cheap',
      plan: 'capable',
      implement: 'capable',
      tests: 'mid',
      resolve: 'mid',
      e2e: 'mid',
      review: 'capable',
      patch: 'capable',
      document: 'mid',
    },
    tiers: {
      cheap: {
        claude: 'claude-haiku-4-5',
        pi: 'haiku',
        codex: 'gpt-5.4-mini',
        opencode: 'anthropic/claude-haiku-4-5',
      },
      mid: {
        claude: 'claude-sonnet-4-6',
        pi: 'sonnet',
        codex: 'gpt-5.4',
        opencode: 'anthropic/claude-sonnet-4-6',
      },
      capable: {
        claude: 'claude-opus-4-8',
        pi: 'opus',
        codex: 'gpt-5.5',
        opencode: 'anthropic/claude-opus-4-8',
      },
    },
  },
  commands: {
    defaultTestCommand: '',
    defaultFinalizeGates: [],
  },
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function deepMerge(base: unknown, override: unknown): unknown {
  if (override === undefined) {
    return structuredClone(base);
  }
  if (!isRecord(base) || !isRecord(override)) {
    return structuredClone(override);
  }
  const out: Record<string, unknown> = structuredClone(base) as Record<string, unknown>;
  for (const [key, value] of Object.entries(override)) {
    out[key] = deepMerge(out[key], value);
  }
  return out;
}

function normalizeConfig(config: AdwConfig): AdwConfig {
  return {
    ...config,
    branching: {
      ...config.branching,
      labelPrefixes: Object.fromEntries(
        Object.entries(config.branching.labelPrefixes).map(([label, prefix]) => [label.toLowerCase(), prefix]),
      ),
    },
  };
}

/** Parse a raw partial config, layering it over the built-in default config. */
export function parseAdwConfig(raw: unknown, source: string = 'ADW config'): AdwConfig {
  const merged = deepMerge(DEFAULT_ADW_CONFIG, raw);
  const parsed = AdwConfigSchema.safeParse(merged);
  if (!parsed.success) {
    throw new AdwError(`invalid ${source}: ${parsed.error.issues.map((i) => `${i.path.join('.')}: ${i.message}`).join('; ')}`);
  }
  return normalizeConfig(parsed.data);
}

/** Load .adw/config.json if present; otherwise return the behavior-preserving defaults. */
export function loadAdwConfig(path: string = ADW_CONFIG_PATH): AdwConfig {
  if (!existsSync(path)) {
    return parseAdwConfig({}, 'default ADW config');
  }
  let raw: unknown;
  try {
    raw = JSON.parse(readFileSync(path, 'utf8')) as unknown;
  } catch (err) {
    throw new AdwError(`could not read ADW config: ${path}`, { cause: err });
  }
  return parseAdwConfig(raw, path);
}

let cachedConfig: AdwConfig | null = null;
let testOverride: AdwConfig | null = null;

/** Cached project config for production call sites. */
export function getAdwConfig(): AdwConfig {
  if (testOverride !== null) {
    return testOverride;
  }
  if (cachedConfig === null) {
    cachedConfig = loadAdwConfig();
  }
  return cachedConfig;
}

/** Test-only override; pass null to restore the on-disk/default config. */
export function setAdwConfigForTests(config: AdwConfig | null): void {
  testOverride = config;
  cachedConfig = null;
}

/** Resolve a config path relative to the repository root unless already absolute. */
export function resolveRepoPath(path: string): string {
  return resolve(REPO_ROOT, path);
}

/** Whether a provider-reported work-item state counts as closed for the verify gate. */
export function isClosedWorkItemState(state: string, config: AdwConfig = getAdwConfig()): boolean {
  return config.providers.workItems.closedStates.includes(state);
}
