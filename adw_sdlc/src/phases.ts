/**
 * Phase catalog, conditional gates, and phased prompt composition, ported
 * from adw/_phases.py. The orchestrator drives this catalog; templates under
 * .pi/prompts/ and .claude/commands/ are shared verbatim with the Python
 * pipeline (PLAN.md D4), so the composed prompt must match adw/ byte for
 * byte — except the fenced-JSON output-contract footer, which is gated off
 * for native-schema backends (PLAN.md Section 7).
 *
 * Model-tier routing lives in models.ts; per-phase result schemas (the
 * to_result analogue) live in schemas.ts.
 */

import { existsSync } from 'node:fs';
import { join } from 'node:path';

import { renderPromptFile } from './common.js';
import { getAdwConfig, resolveRepoPath, type AdwConfig } from './config.js';
import { AdwError } from './errors.js';
import { resolvePhaseSchema } from './schema-registry.js';
import type { AdwState } from './state.js';

// OUTPUT_CONTRACT now lives with PHASE_SCHEMAS in schemas.ts; re-exported here
// so existing importers (and the public index) keep their import path.
export { OUTPUT_CONTRACT } from './schemas.js';

// --- phase catalog -----------------------------------------------------------

/**
 * Configurable agent-phase chain (Python-only setup/finalize/ci-fix/merge/
 * report always wrap this in the orchestrator and are not listed here).
 */
export const AGENT_PHASES = [
  'classify',
  'plan',
  'implement',
  'tests',
  'resolve',
  'e2e',
  'review',
  'patch',
  'document',
] as const;

export type AgentPhase = (typeof AGENT_PHASES)[number];

export const DEFAULT_PHASES: readonly AgentPhase[] = AGENT_PHASES;
// Typed as string sets (not AgentPhase) so the orchestrator can test a chain
// that may include project-registered custom phase names; these only ever hold
// built-in names, and a custom phase is never conditional or looped.
export const CONDITIONAL_PHASES: ReadonlySet<string> = new Set(['e2e', 'document']);
export const LOOP_PHASES: ReadonlySet<string> = new Set(['resolve', 'patch']);

/** Phase -> prompt-template basename (without .md). */
export const TEMPLATE: Record<AgentPhase, string> = {
  classify: 'classify',
  plan: 'plan',
  implement: 'implement',
  tests: 'tests',
  resolve: 'resolve_failed_test',
  e2e: 'e2e_tests',
  // review uses a dedicated phased body (the PR-oriented review.md stays for
  // interactive use).
  review: 'review_phase',
  patch: 'patch',
  document: 'document',
};

/**
 * The phase names a run may use: the built-in catalog plus any project-
 * registered custom phases. A custom phase that collides with a built-in name
 * is a configuration error (the kernel owns the built-in semantics).
 */
export function knownPhaseNames(config: AdwConfig = getAdwConfig()): Set<string> {
  const known = new Set<string>(AGENT_PHASES);
  for (const name of config.customPhases ?? []) {
    if (known.has(name)) {
      throw new AdwError(`custom phase "${name}" collides with a built-in phase name`);
    }
    known.add(name);
  }
  return known;
}

/** Validate that every name is known (built-in or registered custom), else throw loudly. */
function assertKnownPhases(items: readonly string[], known: ReadonlySet<string>, source: string): string[] {
  for (const phase of items) {
    if (!known.has(phase)) {
      throw new AdwError(`unknown phase in ${source}: ${phase} (known: ${[...known].join(', ')})`);
    }
  }
  return [...items];
}

/**
 * Resolve the ordered phase list for a run. Precedence:
 *   1. an explicit `--phases` CSV (per-run override),
 *   2. the project's configured `phases` chain (.adw/config.json),
 *   3. the full built-in catalog (DEFAULT_PHASES).
 *
 * Names must be built-in or project-registered (config.customPhases). Built-in
 * semantics (the resolve/patch loops, the e2e/document conditional gates) are
 * not project-configurable; a project may reorder/drop known phases and append
 * plain custom phases, but cannot give a custom phase loop/gate behavior here.
 */
export function parsePhases(
  csv: string | null | undefined,
  config: AdwConfig = getAdwConfig(),
): string[] {
  const known = knownPhaseNames(config);
  if (csv) {
    const items = csv
      .split(',')
      .map((p) => p.trim())
      .filter((p) => p.length > 0);
    if (items.length === 0) {
      throw new AdwError('no phases given');
    }
    return assertKnownPhases(items, known, '--phases');
  }
  if (config.phases && config.phases.length > 0) {
    return assertKnownPhases(config.phases, known, '.adw/config.json "phases"');
  }
  return [...DEFAULT_PHASES];
}

/**
 * Preflight the resolved phase chain at run start: every phase must have a
 * prompt template that resolves AND a result schema that loads. This runs
 * before any side effects (branch/PR/state mutation) so a misconfigured
 * project — a custom phase missing its `<name>.md` template or
 * `.adw/schemas/<name>.json` schema, or a broken/unsupported schema override —
 * fails loudly up front instead of mid-chain after work has already happened.
 *
 * For a built-in phase with no override the template ships with the package and
 * the schema is wired by construction, so this is a no-op for the stock chain;
 * its value is for custom phases and overrides. Validating the whole chain
 * (rather than just custom names) keeps the check uniform and also catches a
 * stale committed template or a broken override of a safe built-in phase.
 */
export function validatePhaseChain(
  phases: readonly string[],
  runner: string,
  config: AdwConfig = getAdwConfig(),
): void {
  // Validate control-flow config keys first, so a loop/gate on a built-in name
  // gets the precise "built-in phases own their control flow" message rather
  // than tripping the per-phase resolved-field check below.
  validateControlFlowKeys(config);
  for (const phase of phases) {
    // A custom phase has no TEMPLATE entry; its basename defaults to its own
    // name (mirrors composePhasePrompt).
    const basename = (TEMPLATE as Record<string, string | undefined>)[phase] ?? phase;
    const tpath = templatePath(runner, basename, config);
    if (!existsSync(tpath)) {
      throw new AdwError(`phase "${phase}" is missing its prompt template: ${tpath}`);
    }
    // Resolving the handle compiles an override/custom schema eagerly, so a
    // missing custom schema, a broken/unsupported override, or an unknown name
    // throws here rather than when the phase first runs.
    const handle = resolvePhaseSchema(phase, config);
    // A custom loop phase reads `outcome.data.resolved` to detect no-progress,
    // so its result schema must declare `resolved`.
    if (config.loops[phase] !== undefined && !handle.requiredKeys().includes('resolved')) {
      throw new AdwError(
        `custom loop phase "${phase}" must declare "resolved" in its result schema ` +
          `(.adw/schemas/${phase}.json); the loop reads it to detect no-progress`,
      );
    }
  }
}

/**
 * Control-flow config (`gates.custom` / `loops`) may only target a registered
 * custom phase — never a built-in (whose control flow the kernel owns) and never
 * an unregistered name (a typo). A custom gate must also have at least one
 * matcher, else its phase could never run. Fails loudly at startup.
 */
function validateControlFlowKeys(config: AdwConfig): void {
  const builtins = new Set<string>(AGENT_PHASES);
  const custom = new Set(config.customPhases ?? []);
  const entries: ReadonlyArray<[string, 'gate' | 'loop']> = [
    ...Object.keys(config.gates.custom).map((n): [string, 'gate' | 'loop'] => [n, 'gate']),
    ...Object.keys(config.loops).map((n): [string, 'gate' | 'loop'] => [n, 'loop']),
  ];
  for (const [name, kind] of entries) {
    if (builtins.has(name)) {
      throw new AdwError(`custom ${kind} for "${name}" is not allowed: built-in phases own their control flow`);
    }
    if (!custom.has(name)) {
      throw new AdwError(
        `custom ${kind} for "${name}" names an unregistered phase; add it to config.customPhases`,
      );
    }
  }
  for (const [name, rule] of Object.entries(config.gates.custom)) {
    if (rule.hints.length + rule.exactFiles.length + rule.pathPrefixes.length + rule.fileExtensions.length === 0) {
      throw new AdwError(
        `custom gate for "${name}" has no matchers (hints/exactFiles/pathPrefixes/fileExtensions); it could never run`,
      );
    }
  }
}

/** Resolve a phase template path, preferring a runner-specific configured root when present. */
export function templatePath(runner: string, name: string, config: AdwConfig = getAdwConfig()): string {
  const roots = [config.prompts.runnerRoots[runner], config.prompts.defaultRoot].filter(
    (root): root is string => typeof root === 'string' && root.length > 0,
  );
  for (const root of roots) {
    const candidate = join(resolveRepoPath(root), `${name}.md`);
    if (existsSync(candidate)) {
      return candidate;
    }
  }
  return join(resolveRepoPath(config.prompts.defaultRoot), `${name}.md`);
}

// --- conditional gates -----------------------------------------------------------

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Return the first hint that occurs as a whole word in `text`, else null.
 * Word-boundary matching (not bare substring) prevents incidental-substring
 * false positives (adw/_phases.py:151-162): the helper path adw/_exec.py does
 * NOT trip "exec", and "design"/"assignee" do NOT trip a signing hint — which is
 * also why the configured hint lists (config.gates.*.hints) spell ambiguous short
 * stems out as their meaningful forms.
 */
function hintIn(text: string, hints: readonly string[]): string | null {
  for (const hint of hints) {
    if (new RegExp(`\\b${escapeRegExp(hint)}\\b`).test(text)) {
      return hint;
    }
  }
  return null;
}

export interface GateDecision {
  runIt: boolean;
  reason: string;
}

/** Decide whether the e2e phase should run, with a recorded reason. */
export function gateE2e(signal: string, config: AdwConfig = getAdwConfig()): GateDecision {
  const low = (signal || '').toLowerCase();
  const hit = hintIn(low, config.gates.e2e.hints);
  if (hit !== null) {
    return { runIt: true, reason: `change touches cross-boundary flows (${hit})` };
  }
  return { runIt: false, reason: 'no cross-boundary surface detected' };
}

/** Decide whether the document phase should run, with a recorded reason. */
export function gateDocument(
  signal: string,
  changedFiles: readonly string[] = [],
  config: AdwConfig = getAdwConfig(),
): GateDecision {
  const low = (signal || '').toLowerCase();
  const docGate = config.gates.documentation;
  const docLike = changedFiles.some(
    (f) =>
      docGate.exactFiles.includes(f) ||
      docGate.pathPrefixes.some((prefix) => f.startsWith(prefix)) ||
      docGate.fileExtensions.some((ext) => f.endsWith(ext)),
  );
  if (docLike) {
    return { runIt: true, reason: 'documentation files changed' };
  }
  const hit = hintIn(low, docGate.hints);
  if (hit !== null) {
    return { runIt: true, reason: `user-visible/API/protocol surface affected (${hit})` };
  }
  return { runIt: false, reason: 'internal-only change; no docs update needed' };
}

/** A project-supplied conditional-gate predicate for a custom phase. */
export type CustomGateRule = AdwConfig['gates']['custom'][string];

/**
 * Decide a custom phase's gate: run when the change signal matches any hint
 * (whole-word) OR a changed file matches any file rule — the same matching as
 * the built-in `document` gate, with project-supplied lists.
 */
export function gateCustom(
  rule: CustomGateRule,
  signal: string,
  changedFiles: readonly string[] = [],
): GateDecision {
  const fileMatch = changedFiles.some(
    (f) =>
      rule.exactFiles.includes(f) ||
      rule.pathPrefixes.some((prefix) => f.startsWith(prefix)) ||
      rule.fileExtensions.some((ext) => f.endsWith(ext)),
  );
  if (fileMatch) {
    return { runIt: true, reason: 'matched a configured changed-file rule' };
  }
  const hit = hintIn((signal || '').toLowerCase(), rule.hints);
  if (hit !== null) {
    return { runIt: true, reason: `change signal matched a configured hint (${hit})` };
  }
  return { runIt: false, reason: 'no configured signal/file match' };
}

/** True when `phase` runs behind a conditional gate (built-in e2e/document or a custom gate). */
export function isConditionalPhase(phase: string, config: AdwConfig = getAdwConfig()): boolean {
  return CONDITIONAL_PHASES.has(phase) || config.gates.custom[phase] !== undefined;
}

/**
 * Decide a conditional phase via its gate. Throws AdwError for any
 * non-conditional phase so a miswired caller fails loudly instead of
 * silently running it.
 */
export function gateConditional(
  phase: string,
  signal: string,
  changedFiles: readonly string[] = [],
  config: AdwConfig = getAdwConfig(),
): GateDecision {
  if (phase === 'e2e') {
    return gateE2e(signal, config);
  }
  if (phase === 'document') {
    return gateDocument(signal, changedFiles, config);
  }
  const custom = config.gates.custom[phase];
  if (custom !== undefined) {
    return gateCustom(custom, signal, changedFiles);
  }
  throw new AdwError(`not a conditional phase: ${phase}`);
}

// --- phased envelope -----------------------------------------------------------
//
// The reused templates were written for interactive/one-shot use and are also
// consumed by the Python pipeline, so they must not be edited for phased mode.
// Instead the orchestrator composes each phase prompt as:
//
//     [shared preamble] + [per-phase reframing] + [domain template body] + [footer]
//
// The preamble/footer (owned here, in code) supply the phased rules — the
// orchestrator owns git/gh; no GitHub access; emit a trailing JSON contract —
// and override stale framing in the reused bodies. The preamble is the
// prompt-level half of the D5 secret boundary (engine-neutral wording: the
// orchestrator, not "Python", owns git/gh in this standalone port).

export const PHASE_PREAMBLE_SHARED =
  'You are running as a single automated phase of the ADW pipeline.\n' +
  'The orchestrator performs ALL git and GitHub work for this run: do NOT run git or gh, do NOT ' +
  'create/switch/commit/push branches, and do NOT open, merge, or comment on pull requests. ' +
  'If the task section below tells you to do any of that, skip those steps.\n' +
  'You have no GitHub access in this phase; all issue context you need is provided inline.\n';

// Per-phase reframing prepended after the shared preamble; overrides stale
// framing carried by the reused interactive templates.
export const PHASE_CONTEXT: Partial<Record<AgentPhase, string>> = {
  implement:
    'Scope for this phase: make the code change only. Focused tests are added in a ' +
    'separate `tests` phase — do not do broad test work here. If $1 names a spec file that exists, ' +
    'treat it as the source of truth; otherwise (e.g. $1 is a placeholder note, not a path) treat ' +
    'the inline issue context as the spec and implement directly — do NOT stop merely because no ' +
    'spec file path was provided.\n',
  tests: 'Scope for this phase: add or strengthen focused, non-e2e tests for the change.\n',
  e2e:
    'The orchestrator already decided this phase should run; do the work rather than ' +
    're-deciding whether e2e coverage is warranted.\n',
  // review uses a dedicated phased template (review_phase.md) that is already
  // working-tree-oriented, so it needs no reframing here.
  document:
    'The orchestrator already decided documentation is warranted; update the existing ' +
    'docs surface (README/docs/wiki/help) only. Do not create an app_docs/ tree.\n',
};

/** Phases that author free-form text to workspace files instead of inlining it in JSON. */
export const ARTIFACT_PHASES: ReadonlySet<string> = new Set(['review', 'document']);

/** Workspace path where the authoring phase writes the commit message. */
export function commitMessagePath(state: AdwState): string {
  return join(state.workspace(), 'commit_message.txt');
}

/** Workspace path where the authoring phase writes the PR body. */
export function prBodyPath(state: AdwState): string {
  return join(state.workspace(), 'pr_body.md');
}

/**
 * Build the per-phase footer. Artifact-file instructions are independent of
 * the output mechanism and always emitted for artifact phases; the fenced-
 * JSON contract block exists solely for stdout parsing, so it is gated off
 * when the backend constrains output to a schema natively (PLAN.md Section 7
 * — the footer and a native outputFormat must never both be active).
 */
export function buildFooter(phase: string, state: AdwState, emitJsonContract: boolean): string {
  const lines: string[] = [];
  if (ARTIFACT_PHASES.has(phase)) {
    lines.push(
      'Author these files first (this keeps large free-form text out of the JSON, which',
      'the pipeline parses mechanically):',
      '- Write the full commit message (subject + body, ending with a line `closes #<issue>`) to: ' +
        commitMessagePath(state),
      `- Write the complete PR body (Markdown) to: ${prBodyPath(state)}`,
      'Set the matching wrote_* booleans to true once each file is written.',
      '',
    );
  }
  if (emitJsonContract) {
    lines.push(
      '## Required output',
      '',
      'End your reply with EXACTLY one fenced ```json block matching this shape, and nothing after it:',
      '',
      '```json',
      resolvePhaseSchema(phase).outputContract(),
      '```',
    );
  }
  return lines.join('\n');
}

/**
 * Compose the full phased prompt for `phase` (pure): shared preamble +
 * per-phase reframing + the (reused or new) domain template body + the
 * footer. With emitJsonContract=false (native-schema backends) the JSON
 * contract block is omitted; an entirely empty footer drops its separator.
 */
export function composePhasePrompt(
  phase: string,
  templateArgs: readonly string[],
  state: AdwState,
  runner = 'pi',
  emitJsonContract = true,
): string {
  // A custom phase has no TEMPLATE/PHASE_CONTEXT entry: its template basename
  // defaults to the phase name (`<name>.md`) and it carries no extra reframing.
  const basename = (TEMPLATE as Record<string, string | undefined>)[phase] ?? phase;
  const context = (PHASE_CONTEXT as Record<string, string | undefined>)[phase] ?? '';
  const tpath = templatePath(runner, basename);
  if (!existsSync(tpath)) {
    throw new AdwError(`prompt template not found for phase ${phase}: ${tpath}`);
  }
  const body = renderPromptFile(tpath, templateArgs);
  const preamble = PHASE_PREAMBLE_SHARED + context;
  const footer = buildFooter(phase, state, emitJsonContract);
  if (!footer) {
    return `${preamble}\n---\n\n${body}\n`;
  }
  return `${preamble}\n---\n\n${body}\n\n---\n\n${footer}\n`;
}
