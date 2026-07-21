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

import { projectRoot, REPO_ROOT } from './common.js';
import { ENV_DENY_PREFIXES, RUNNER_ENV_ALLOW } from './env.js';
import { AdwError } from './errors.js';

/** Path to the project's .adw/config.json, resolved under the current project root. */
export function adwConfigPath(): string {
  return join(projectRoot(), '.adw', 'config.json');
}

/**
 * @deprecated Use adwConfigPath(); kept for back-compat with existing importers.
 * Evaluated at import = the package root (today's default value); runtime reads
 * that must follow an explicit project root call adwConfigPath() instead.
 */
export const ADW_CONFIG_PATH = adwConfigPath();

const TierSchema = z.enum(['cheap', 'mid', 'capable']);
const RunnerModelMapSchema = z
  .object({
    claude: z.string().min(1),
    codex: z.string().min(1),
    opencode: z.string().min(1),
    pi: z.string().min(1),
  })
  .catchall(z.string().min(1));

/**
 * One or more explicitly named credentials may be forwarded to the OpenCode
 * server for use through OpenCode's `{env:NAME}` config substitution. Keep
 * GitHub authority and control-plane secrets outside that opt-in channel, and
 * reject OpenCode's own config selectors so an auth indirection cannot replace
 * the runner-authored permission policy through a second config source.
 */
const OPENCODE_FIXED_ENV = new Set<string>(RUNNER_ENV_ALLOW.opencode);
const RESERVED_OPENCODE_AUTH_ENV = new Set<string>([
  'GH_TOKEN',
  'GITHUB_TOKEN',
  'GH_ENTERPRISE_TOKEN',
  'GITHUB_ENTERPRISE_TOKEN',
  'GH_BIN',
  'OPENCODE_CONFIG',
  'OPENCODE_CONFIG_CONTENT',
  'OPENCODE_CONFIG_DIR',
  // Do not turn authEnv into a route for another runner's known credential or
  // config knob. Names already in OpenCode's fixed row remain valid/no-op.
  ...Object.values(RUNNER_ENV_ALLOW).flat().filter((name) => !OPENCODE_FIXED_ENV.has(name)),
]);

const OpencodeAuthEnvSchema = z
  .string()
  .min(1)
  .regex(/^[A-Za-z_][A-Za-z0-9_]*$/, 'must be an environment-variable name')
  .superRefine((name, ctx) => {
    if (ENV_DENY_PREFIXES.some((prefix) => name.startsWith(prefix))) {
      ctx.addIssue({
        code: 'custom',
        message: `matches denied prefix ${ENV_DENY_PREFIXES.find((prefix) => name.startsWith(prefix))}`,
      });
    }
    if (RESERVED_OPENCODE_AUTH_ENV.has(name)) {
      ctx.addIssue({ code: 'custom', message: 'is reserved and cannot be used as an OpenCode provider credential' });
    }
  });

const OpencodeRunnerConfigSchema = z.object({
  /** Operator-owned OpenCode server config; the adapter always replaces `permission`. */
  config: z.record(z.string(), z.unknown()).default({}),
  /** Optional provider credential env name(s) referenced as `{env:NAME}` in `config`. */
  authEnv: z.union([OpencodeAuthEnvSchema, z.array(OpencodeAuthEnvSchema).min(1)]).optional(),
});

/**
 * A conditional-gate predicate for a custom phase: the phase runs when the
 * change signal matches any `hints` (whole-word) OR a changed file matches any
 * of the file rules — the same matching as the built-in `documentation` gate.
 * All lists default empty; an entirely empty predicate is rejected at startup
 * (the phase could never run). Only custom phases may carry one.
 */
const CustomGateSchema = z.object({
  hints: z.array(z.string().min(1)).default([]),
  exactFiles: z.array(z.string().min(1)).default([]),
  pathPrefixes: z.array(z.string().min(1)).default([]),
  fileExtensions: z.array(z.string().min(1)).default([]),
});

/**
 * A resolve-style loop for a custom phase: the orchestrator runs `command`;
 * a non-zero exit invokes the phase's agent to fix it and retries up to
 * `maxAttempts`. The phase's result schema must declare `resolved` (checked at
 * startup). Only custom phases may carry one.
 */
const CustomLoopSchema = z.object({
  command: z.string().min(1),
  maxAttempts: z.number().int().positive().default(3),
});

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
  /** Runner-specific server configuration that does not belong in model routing. */
  runners: z.object({
    opencode: OpencodeRunnerConfigSchema,
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
  /**
   * Resolve-style loops for custom phases (see
   * docs/DESIGN-custom-phase-control-flow.md). Keyed by a registered custom
   * phase name. Optional; default empty (no custom loops). A key naming a
   * built-in or unregistered phase is rejected at startup, as is a loop phase
   * whose result schema omits `resolved`.
   */
  loops: z.record(z.string(), CustomLoopSchema).default({}),
  /**
   * Provider selection. Each role's `type` is validated here for SHAPE only (a
   * non-empty string); which kinds actually exist is the provider registry's
   * job (`createProvidersFromConfig` in providers.ts), which fails closed with a
   * loud AdwError on an unknown kind. This is the same shape/membership split
   * the `phases` chain uses (config shape vs. kernel `AGENT_PHASES` membership)
   * and it deliberately avoids a config.ts ⇄ providers.ts import cycle: a future
   * in-tree provider (e.g. gitlab/glab) registers in the kernel without any
   * change to this schema. Built-in kinds: `github` (cli/workItems/
   * changeRequests) and `git` (vcs).
   */
  providers: z.object({
    cli: z.object({ type: z.string().min(1) }),
    workItems: z.object({
      type: z.string().min(1),
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
      /**
       * Declarative-provider descriptor fields (for `type: "cli"` and `"rest"`).
       * OPTIONAL and preserved here as loose shape only — the route/map grammar,
       * the placeholder check, the host allowlist / https guard, and the
       * one-named-credential guard are validated in provider-descriptor.ts at
       * provider construction (run start, fail-closed), keeping descriptor
       * semantics out of this schema the way schema-override.ts owns JSON-Schema
       * overrides. Ignored by the `github` built-in. See
       * docs/DESIGN-declarative-providers.md.
       */
      authEnv: z.string().min(1).optional(),
      routes: z.record(z.string(), z.unknown()).optional(),
      // rest-only descriptor fields (loose shape; validated by the rest loader).
      baseUrl: z.string().min(1).optional(),
      allowedHosts: z.array(z.string().min(1)).optional(),
      authHeader: z.string().min(1).optional(),
      authScheme: z.string().optional(),
    }),
    vcs: z.object({ type: z.string().min(1) }),
    changeRequests: z.object({
      type: z.string().min(1),
      // Declarative `rest` change-request descriptor (loose shape; the routes,
      // body templating, host allowlist / https guard, and one-named-credential
      // guard are validated in provider-descriptor.ts at construction). Ignored
      // by the `github` built-in. See docs/DESIGN-declarative-providers.md.
      baseUrl: z.string().min(1).optional(),
      allowedHosts: z.array(z.string().min(1)).optional(),
      authEnv: z.string().min(1).optional(),
      authHeader: z.string().min(1).optional(),
      authScheme: z.string().optional(),
      routes: z.record(z.string(), z.unknown()).optional(),
    }),
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
    /**
     * Per-custom-phase conditional gates (see
     * docs/DESIGN-custom-phase-control-flow.md). Keyed by a registered custom
     * phase name. Optional; default empty. A key naming a built-in or
     * unregistered phase, or a predicate with no matchers, is rejected at startup.
     */
    custom: z.record(z.string(), CustomGateSchema).default({}),
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
    defaultRoot: '.adw/prompts',
    runnerRoots: {},
  },
  runners: {
    opencode: { config: {} },
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
    custom: {},
  },
  loops: {},
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
export function loadAdwConfig(path: string = adwConfigPath()): AdwConfig {
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
let cachedRoot: string | null = null;
let testOverride: AdwConfig | null = null;

/**
 * Cached project config for production call sites. The cache is root-aware: it
 * reloads when projectRoot() changes (e.g. after a late setProjectRoot()), so a
 * run that targets an external repo always reads that repo's config without any
 * manual cache busting — and the dependency graph stays acyclic (setProjectRoot
 * never reaches into this module).
 */
export function getAdwConfig(): AdwConfig {
  if (testOverride !== null) {
    return testOverride;
  }
  const root = projectRoot();
  if (cachedConfig === null || cachedRoot !== root) {
    cachedConfig = loadAdwConfig();
    cachedRoot = root;
  }
  return cachedConfig;
}

/** Test-only override; pass null to restore the on-disk/default config. */
export function setAdwConfigForTests(config: AdwConfig | null): void {
  testOverride = config;
  cachedConfig = null;
  cachedRoot = null;
}

/** Resolve a config path relative to the PROJECT root unless already absolute. */
export function resolveRepoPath(path: string): string {
  return resolve(projectRoot(), path);
}

/**
 * Resolve a path relative to the PACKAGE root (the kernel/code location) unless
 * already absolute. The second tier of prompt/schema resolution (bundled
 * kernel defaults) and the pin for the build-time pack generator, which authors
 * the package's own prompts and must never follow a project-root override.
 */
export function resolvePackagePath(path: string): string {
  return resolve(REPO_ROOT, path);
}

/** Whether a provider-reported work-item state counts as closed for the verify gate. */
export function isClosedWorkItemState(state: string, config: AdwConfig = getAdwConfig()): boolean {
  return config.providers.workItems.closedStates.includes(state);
}
