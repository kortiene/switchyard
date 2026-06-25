/**
 * Phased ADW delivery driver, ported from adw/_orchestrator.py with
 * byte-for-byte-equivalent control-flow semantics (PLAN.md D4): the TS
 * control plane runs a sequence of discrete, single-purpose agent phases
 * (each one runner invocation through the AgentRunner seam), threads
 * AdwState between them, and performs all git/GitHub mechanics itself — the
 * coding agent never sees GH_TOKEN in this mode. Setup, finalize, CI-watch,
 * and the squash-merge gate live here; the agent authors only the commit
 * message and PR body.
 *
 * Differences from the Python driver are exactly the planned ones:
 * - phases run through runner.runPhase via run-phase.ts (not a CLI spawn);
 * - classify runs on the shared Anthropic-SDK structured call by default
 *   (opt-out MX_AGENT_CLASSIFY_ON_RUNNER=1 routes it through the runner);
 * - per-phase cost/usage is accumulated additively into state.
 *
 * Every external effect is injected via OrchestratorDeps (defaulting to the
 * real implementations) — the TS analogue of the module seams the Python
 * tests patch — so the parity suite drives run() with no real agent/git/gh.
 */

import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { shellSplit } from './common.js';
import { DEFAULT_ADW_CONFIG, getAdwConfig, isClosedWorkItemState, type AdwConfig } from './config.js';
import { AdwError } from './errors.js';
import { safeSubprocessEnv } from './env.js';
import { assumeYes, capture, confirm, note, postProgress, type Captured } from './exec.js';
import * as git from './git.js';
import { prNumberFromUrl } from './git.js';
import type { AgentRunner } from './invoker.js';
import { deriveBranch, type WorkItemContext } from './work-item.js';
import {
  commitMessagePath,
  composePhasePrompt,
  gateConditional,
  isConditionalPhase,
  parsePhases,
  prBodyPath,
  validatePhaseChain,
  type AgentPhase,
} from './phases.js';
import { createProvidersFromConfig, providerBackedDeps, type AdwProviders, type ProviderContext } from './providers.js';
import { runAgentPhase, type AgentPhaseOutcome } from './run-phase.js';
import {
  ClassifySchema,
  type ClassifyResult,
  type ImplementResult,
  type PlanResult,
  type ReviewFinding,
  type ReviewResult,
} from './schemas.js';
import { AdwState, makeAdwId } from './state.js';
import { structuredCall, type StructuredCallOptions, type StructuredCallResult } from './structured-call.js';
import type { PhaseUsage } from './invoker.js';

/** Cap failure text fed into prompts/comments (adw/_orchestrator.py:38). */
export const MAX_OUTPUT_CHARS = 8000;

/**
 * How many times to re-poll an empty check rollup before concluding the PR
 * genuinely has no checks (vs. checks merely not registered yet right after
 * `gh pr create`).
 */
const NO_CHECKS_SETTLE_POLLS = 3;

/**
 * Default test gate. Empty in this standalone port — HealthTech has not yet
 * chosen its stack/test command (backlog #1), so nothing is assumed. Configure
 * a real command via `--test-cmd` / `MX_AGENT_TEST_CMD` once the stack lands;
 * an empty gate is skipped (treated as green) rather than run.
 */
export const DEFAULT_TEST_CMD = DEFAULT_ADW_CONFIG.commands.defaultTestCommand;

/**
 * Extra pre-merge verification gates beyond the test gate (e.g. format/lint/
 * build). Empty by default — no toolchain is assumed. Populate at runtime via
 * `MX_AGENT_FINALIZE_GATES` (newline-separated) in finalizeAndMerge.
 */
export const DEFAULT_FINALIZE_GATES: readonly string[] = DEFAULT_ADW_CONFIG.commands.defaultFinalizeGates;

// --- options & injected seams -------------------------------------------------

export interface RunOptions {
  base?: string;
  /** Comma-separated phase subset/order; default: the full chain. */
  phases?: string;
  adwId?: string;
  resume?: boolean;
  noProgress?: boolean;
  /**
   * EXPLICIT OPT-OUT of the D5 secret boundary: forwards the FULL parent
   * environment — including GH_TOKEN and MATRIX_*-/MX_AGENT_*-prefixed
   * secrets — to the runner child. The faithful port of Python's
   * --inherit-env (adw/_orchestrator.py:594, env=None → full inherit),
   * documented there as "less isolated". Never set this in unattended runs.
   */
  inheritEnv?: boolean;
  maxResolve?: number;
  maxPatch?: number;
  maxCiFix?: number;
  ciPollIntervalMs?: number;
  ciMaxPolls?: number;
  testCmd?: string;
  /** --model override; per-phase MX_AGENT_MODEL_<PHASE> still applies under it. */
  model?: string;
  repo?: string;
  /** Per-phase runner timeout in milliseconds (0 = none). */
  timeoutMs?: number;
  verify?: boolean;
  force?: boolean;
  allowDirty?: boolean;
  yes?: boolean;
  dryRun?: boolean;
  maxBudgetUsd?: number;
}

type ResolvedOptions = Required<Omit<RunOptions, 'phases' | 'adwId' | 'repo' | 'maxBudgetUsd'>> &
  Pick<RunOptions, 'phases' | 'adwId' | 'repo' | 'maxBudgetUsd'>;

/** Defaults mirror adw/issue.py build_parser. */
function resolveOptions(options: RunOptions): ResolvedOptions {
  return {
    base: options.base ?? 'main',
    resume: options.resume ?? false,
    noProgress: options.noProgress ?? false,
    inheritEnv: options.inheritEnv ?? false,
    maxResolve: options.maxResolve ?? 3,
    maxPatch: options.maxPatch ?? 2,
    maxCiFix: options.maxCiFix ?? 3,
    ciPollIntervalMs: options.ciPollIntervalMs ?? 30_000,
    ciMaxPolls: options.ciMaxPolls ?? 40,
    testCmd: options.testCmd ?? getAdwConfig().commands.defaultTestCommand,
    model: options.model ?? '',
    timeoutMs: options.timeoutMs ?? 0,
    verify: options.verify ?? true,
    force: options.force ?? false,
    allowDirty: options.allowDirty ?? false,
    yes: options.yes ?? false,
    dryRun: options.dryRun ?? false,
    phases: options.phases,
    adwId: options.adwId,
    repo: options.repo,
    maxBudgetUsd: options.maxBudgetUsd,
  };
}

export interface RunCmdResult {
  rc: number;
  output: string;
}

export type ProgressFn = (phase: string, message: string) => void;

export interface GitOps {
  createOrCheckoutBranch: typeof git.createOrCheckoutBranch;
  commitAll: typeof git.commitAll;
  push: typeof git.push;
  pullRebase: typeof git.pullRebase;
  prForBranch: typeof git.prForBranch;
  createPr: typeof git.createPr;
  ciStatus: typeof git.ciStatus;
  squashMerge: typeof git.squashMerge;
}

/**
 * Every external effect run() touches, injectable for tests (the analogue of
 * the seams adw/test_orchestrator.py patches).
 */
export interface OrchestratorDeps {
  env: Record<string, string | undefined>;
  isatty: () => boolean;
  confirm: (prompt: string) => Promise<boolean>;
  sleep: (ms: number) => Promise<void>;
  /**
   * Run a local gate command with the inherited env; gate commands are build
   * tools (e.g. cargo test), not the coding agent, so they legitimately use
   * the normal environment.
   */
  runCmd: (cmd: readonly string[]) => RunCmdResult;
  capture: (cmd: readonly string[]) => Captured;
  /** First-class provider boundary for universalized runtime effects. */
  providers?: AdwProviders;
  // Legacy seams kept for parity tests and incremental migration.
  workingTreeDirty: () => boolean;
  changedFiles: (base: string) => string[];
  resolveGhBin: () => string | null;
  detectRepo: (ghBin: string | null) => string;
  issueState: (ghBin: string | null, issue: number, repo: string) => string;
  postProgress: typeof postProgress;
  fetchIssue: (ghBin: string | null, issue: number, repo: string) => WorkItemContext | null;
  setStatus: (ghBin: string, owner: string, issue: number, status: string) => void;
  git: GitOps;
  runAgentPhase: typeof runAgentPhase;
  classify: (
    prompt: string,
    options?: StructuredCallOptions,
  ) => Promise<StructuredCallResult<ClassifyResult>>;
}

export function defaultDeps(): OrchestratorDeps {
  const providers = createProvidersFromConfig(getAdwConfig(), changedFiles);
  const providerDeps = providerBackedDeps(providers);
  return {
    env: process.env,
    isatty: () => process.stdin.isTTY === true,
    confirm,
    sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
    runCmd: (cmd) => {
      const result = capture(cmd);
      return { rc: result.returncode, output: (result.stdout || '') + (result.stderr || '') };
    },
    capture,
    providers,
    ...providerDeps,
    runAgentPhase,
    classify: (prompt, options) => structuredCall(prompt, ClassifySchema, options),
  };
}

function numericWorkItemId(id: number | string): number {
  return typeof id === 'number' ? id : Number.parseInt(String(id), 10);
}

/** Adapter for focused tests that still override legacy OrchestratorDeps seams. */
function legacyProvidersFromDeps(deps: OrchestratorDeps): AdwProviders {
  return {
    cli: {
      resolveExecutable: () => deps.resolveGhBin(),
      detectRepository: (ghBin) => deps.detectRepo(ghBin),
    },
    workItems: {
      fetch: (ctx, id) => deps.fetchIssue(ctx.ghBin, numericWorkItemId(id), ctx.repo),
      state: (ctx, id) => deps.issueState(ctx.ghBin, numericWorkItemId(id), ctx.repo),
      postProgress: (ctx, id, adwId, phase, message) =>
        deps.postProgress(ctx.ghBin, id, ctx.repo, adwId, phase, message),
      assignSelf: (ctx, id) => {
        if (!ctx.ghBin) {
          return;
        }
        const edit = [ctx.ghBin, 'issue', 'edit', String(id), '--add-assignee', '@me'];
        if (ctx.repo) {
          edit.push('--repo', ctx.repo);
        }
        deps.capture(edit);
      },
      setStatus: (ctx, id, status) => {
        if (!ctx.ghBin) {
          return;
        }
        const owner = ctx.repo ? (ctx.repo.split('/')[0] ?? '') : '';
        if (owner) {
          deps.setStatus(ctx.ghBin, owner, numericWorkItemId(id), status);
        }
      },
    },
    vcs: {
      workingTreeDirty: () => deps.workingTreeDirty(),
      changedFiles: (base) => deps.changedFiles(base),
      createOrCheckoutBranch: (branch, base) => deps.git.createOrCheckoutBranch(branch, base),
      commitAll: (message) => deps.git.commitAll(message),
      push: (branch) => deps.git.push(branch),
      pullRebase: (base) => deps.git.pullRebase(base),
    },
    changeRequests: {
      findForBranch: (ctx, branch) => (ctx.ghBin ? deps.git.prForBranch(branch, ctx.ghBin, ctx.repo) : null),
      create: (ctx, input) => {
        if (!ctx.ghBin) {
          return { id: null, number: null, url: null, error: 'gh not found' };
        }
        const created = deps.git.createPr(input.branch, input.title, input.body, input.base, ctx.ghBin, ctx.repo);
        return {
          id: created.number !== null ? String(created.number) : (created.url ?? null),
          number: created.number,
          url: created.url,
          error: created.error,
        };
      },
      pipelineStatus: (ctx, id) =>
        ctx.ghBin ? deps.git.ciStatus(id, ctx.ghBin, ctx.repo) : { state: 'unknown', failingJobs: [] },
      squashMerge: (ctx, id) => {
        if (!ctx.ghBin) {
          return { ok: false, error: 'gh not found' };
        }
        return deps.git.squashMerge(id, ctx.ghBin, ctx.repo);
      },
    },
  };
}

function providersForDeps(deps: OrchestratorDeps): AdwProviders {
  return deps.providers ?? legacyProvidersFromDeps(deps);
}

function providerContext(ghBin: string | null, repo: string): ProviderContext {
  return { ghBin, repo };
}

const LEGACY_PROVIDER_OVERRIDE_KEYS = [
  'workingTreeDirty',
  'changedFiles',
  'resolveGhBin',
  'detectRepo',
  'issueState',
  'postProgress',
  'fetchIssue',
  'setStatus',
  'git',
] as const;

function hasLegacyProviderOverrides(overrides: Partial<OrchestratorDeps>): boolean {
  return LEGACY_PROVIDER_OVERRIDE_KEYS.some((key) => Object.prototype.hasOwnProperty.call(overrides, key));
}

/** The per-run agent invocation context threaded into every phase call. */
interface AgentCtx {
  runner: AgentRunner;
  cliModel: string;
  env: Record<string, string>;
  timeoutMs: number;
  maxBudgetUsd?: number;
  /** MX_AGENT_FORCE_FENCED measurement mode — see run-phase RunAgentPhaseOptions. */
  forceFenced?: boolean;
}

// --- helpers (unit-testable) ------------------------------------------------------

/** Tail-truncate noisy output for inclusion in a prompt or comment. */
export function truncate(text: string, limit: number = MAX_OUTPUT_CHARS): string {
  const t = text || '';
  if (t.length <= limit) {
    return t;
  }
  return `…(truncated)…\n${t.slice(t.length - limit)}`;
}

/**
 * Gate the irreversible squash-merge; throws AdwError to abort. When stdin
 * is not a terminal and the run was not pre-authorized (--yes /
 * MX_AGENT_YES=1), refuse rather than silently merge.
 */
export async function confirmMerge(options: {
  yes: boolean;
  isatty: boolean;
  confirm: (prompt: string) => Promise<boolean>;
  changeRequestLabel?: string;
}): Promise<void> {
  if (options.yes) {
    return;
  }
  if (!options.isatty) {
    throw new AdwError('refusing to merge unattended without --yes / MX_AGENT_YES=1');
  }
  const label = options.changeRequestLabel ?? 'PR';
  if (!(await options.confirm(`>> About to squash-merge this ${label} to main. Continue? [y/N] `))) {
    throw new AdwError('aborted');
  }
}

/** Best-effort list of files changed vs origin/<base>. */
export function changedFiles(base: string): string[] {
  const result = capture(['git', 'diff', `origin/${base}`, '--name-only']);
  if (result.returncode !== 0) {
    return [];
  }
  return result.stdout
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

/** Render review findings into a prompt-friendly block. */
export function renderFindings(findings: readonly ReviewFinding[]): string {
  return findings
    .map((f, idx) => {
      const loc = f.location ? ` (${f.location})` : '';
      return `${idx + 1}. [${f.severity}]${loc} ${f.description}`;
    })
    .join('\n');
}

/**
 * Accumulate a phase's dollars into the run's additive total. A null cost
 * means "could not be priced", which poisons the whole accumulation: the
 * total sticks to null rather than silently becoming a false partial sum.
 * An absent (undefined) cost carries no information and is a no-op.
 */
function recordUsage(state: AdwState, usage: PhaseUsage): void {
  if (usage.costUsd === null) {
    state.totalCostUsd = null;
  } else if (usage.costUsd !== undefined && state.totalCostUsd !== null) {
    state.totalCostUsd = (state.totalCostUsd ?? 0) + usage.costUsd;
  }
}

// --- bounded loops -----------------------------------------------------------------

/**
 * Run the test gate, asking the agent to fix failures, until green. Returns
 * true if the gate is green, false if it is still failing after the bound or
 * the agent makes no progress.
 */
export async function resolveLoop(
  state: AdwState,
  agent: AgentCtx,
  config: { testCmd: string; maxAttempts: number; progress: ProgressFn; phase?: string },
  deps: OrchestratorDeps,
): Promise<boolean> {
  // Built-in resolve passes no `phase`; a custom loop phase passes its own name
  // so the loop drives that phase's agent and tags progress under it.
  const phase = config.phase ?? 'resolve';
  if (config.testCmd.trim() === '') {
    config.progress(phase, 'no test command configured; skipping test gate');
    return true;
  }
  const gate = shellSplit(config.testCmd);
  let attempt = 0;
  for (;;) {
    const { rc, output } = deps.runCmd(gate);
    if (rc === 0) {
      config.progress(phase, 'test gate is green');
      return true;
    }
    if (attempt >= config.maxAttempts) {
      config.progress(phase, `test gate still failing after ${config.maxAttempts} attempt(s)`);
      return false;
    }
    attempt += 1;
    config.progress(phase, `test gate failed; ${phase} attempt ${attempt}/${config.maxAttempts}`);
    // Cast: a custom loop name flows to runAgentPhase at runtime (resolving its
    // own template/schema); typing it as 'resolve' here gives `data.resolved`,
    // which the custom schema is validated to declare (validatePhaseChain).
    const outcome = await invokeAgent(deps, phase as 'resolve', [truncate(output)], state, agent);
    recordUsage(state, outcome.usage);
    if (outcome.data.resolved === 0) {
      config.progress(phase, 'agent resolved nothing; stopping');
      return false;
    }
  }
}

/** Patch blocker findings (only) until none remain. Returns true when clear. */
export async function patchLoop(
  state: AdwState,
  findings: readonly ReviewFinding[],
  agent: AgentCtx,
  config: { maxAttempts: number; progress: ProgressFn },
  deps: OrchestratorDeps,
): Promise<boolean> {
  const blockers = findings.filter((f) => f.severity === 'blocker');
  const others = findings.length - blockers.length;
  if (others > 0) {
    config.progress('patch', `${others} non-blocker finding(s) reported, not auto-fixed`);
  }
  if (blockers.length === 0) {
    config.progress('patch', 'no blocker findings');
    return true;
  }

  let remaining = blockers.length;
  const blockersText = renderFindings(blockers);
  // On retries the count, not the list, shrinks; tell the agent the full list
  // may be partly fixed so it re-checks each instead of re-editing fixed ones.
  const retryNote =
    'Some of these may already be resolved by a previous attempt. Re-check each ' +
    'against the current working tree and only fix the ones that still apply.\n\n';
  let attempt = 0;
  while (remaining > 0 && attempt < config.maxAttempts) {
    attempt += 1;
    config.progress('patch', `resolving ${remaining} blocker(s); attempt ${attempt}/${config.maxAttempts}`);
    const promptText = attempt === 1 ? blockersText : retryNote + blockersText;
    const outcome = await invokeAgent(deps, 'patch', [promptText], state, agent);
    recordUsage(state, outcome.usage);
    const result = outcome.data;
    if (result.resolved === 0 || result.remaining >= remaining) {
      remaining = result.remaining;
      break;
    }
    remaining = result.remaining;
  }
  return remaining === 0;
}

/** Watch CI and ask the agent to fix red checks until green. Returns success. */
export async function ciFixLoop(
  state: AdwState,
  pr: number | string,
  agent: AgentCtx,
  config: {
    ghBin: string;
    repo: string;
    maxAttempts: number;
    pollIntervalMs: number;
    maxPolls: number;
    progress: ProgressFn;
  },
  deps: OrchestratorDeps,
): Promise<boolean> {
  let attempt = 0;
  let polls = 0;
  // Tolerate a short window where no checks have registered yet. Like the
  // Python original, this settle counter is deliberately NOT reset after a
  // fix-push (only `polls` is) — fixing that quirk in one engine would break
  // the byte-for-byte semantics parity (D4); change both engines together
  // post-cutover if it ever matters in practice.
  let nonePolls = 0;
  const providers = providersForDeps(deps);
  const ctx = providerContext(config.ghBin, config.repo);
  for (;;) {
    const status = providers.changeRequests.pipelineStatus(ctx, pr);
    if (status.state === 'success') {
      config.progress('ci-fix', 'CI is green');
      return true;
    }
    if (status.state === 'none') {
      // Query succeeded but the PR has no checks. Right after `gh pr create`
      // they may not be registered yet, so settle briefly before concluding
      // there is genuinely nothing to gate on (treated as green).
      nonePolls += 1;
      if (nonePolls > NO_CHECKS_SETTLE_POLLS) {
        config.progress('ci-fix', 'no CI checks registered; treating as green');
        return true;
      }
      if (config.pollIntervalMs > 0) {
        await deps.sleep(config.pollIntervalMs);
      }
      continue;
    }
    if (status.state === 'unknown') {
      config.progress('ci-fix', 'could not determine CI status');
      return false;
    }
    if (status.state === 'pending') {
      polls += 1;
      if (polls > config.maxPolls) {
        config.progress('ci-fix', 'CI still pending after polling budget');
        return false;
      }
      if (config.pollIntervalMs > 0) {
        await deps.sleep(config.pollIntervalMs);
      }
      continue;
    }
    // failure
    if (attempt >= config.maxAttempts) {
      config.progress('ci-fix', `CI still red after ${config.maxAttempts} fix attempt(s)`);
      return false;
    }
    attempt += 1;
    const names = status.failingJobs.map((j) => j.name).join(', ') || 'unknown jobs';
    config.progress('ci-fix', `CI red (${names}); fix attempt ${attempt}/${config.maxAttempts}`);
    const outcome = await invokeAgent(
      deps,
      'resolve',
      [`CI is failing for these checks: ${names}. Fix the cause.`],
      state,
      agent,
    );
    recordUsage(state, outcome.usage);
    if (outcome.data.resolved === 0) {
      config.progress('ci-fix', 'agent resolved nothing; stopping');
      return false;
    }
    // An agent claiming a fix that left no committable change can't move CI;
    // stop instead of re-pushing the same tree and burning the poll budget.
    if (!providers.vcs.workingTreeDirty()) {
      config.progress('ci-fix', 'agent reported a fix but changed nothing; stopping');
      return false;
    }
    const { ok } = providers.vcs.commitAll(`fix: address CI failures (${names})`);
    if (ok) {
      providers.vcs.push(state.branchName ?? '');
      polls = 0; // a new commit kicks off a fresh CI run; reset the budget
    }
  }
}

/** One agent-phase call through the injected run-phase seam. */
function invokeAgent<P extends 'resolve' | 'patch'>(
  deps: OrchestratorDeps,
  phase: P,
  templateArgs: readonly string[],
  state: AdwState,
  agent: AgentCtx,
): Promise<AgentPhaseOutcome<P>> {
  return deps.runAgentPhase({
    phase,
    templateArgs,
    state,
    runner: agent.runner,
    cliModel: agent.cliModel,
    env: agent.env,
    timeoutMs: agent.timeoutMs,
    ...(agent.maxBudgetUsd !== undefined ? { maxBudgetUsd: agent.maxBudgetUsd } : {}),
    ...(agent.forceFenced ? { forceFenced: true } : {}),
  });
}

// --- phase argument assembly -----------------------------------------------------

function workItemLabel(config: AdwConfig): string {
  return config.providers.workItems.type === 'github' ? 'GitHub issue' : 'work item';
}

function changeRequestLabel(config: AdwConfig): string {
  return config.providers.changeRequests.type === 'github' ? 'PR' : 'change request';
}

function changeRequestType(config: AdwConfig): string {
  return config.providers.changeRequests.type === 'github' ? 'pull_request' : 'change_request';
}

function workItemType(config: AdwConfig): string {
  return config.providers.workItems.type === 'github' ? 'issue' : 'work_item';
}

function workItemRef(issue: number, config: AdwConfig): string {
  return `${workItemLabel(config)} #${issue}`;
}

function defaultCommitMessage(config: AdwConfig, issue: number): string {
  return config.providers.workItems.type === 'github'
    ? `feat: implement issue #${issue}\n\ncloses #${issue}`
    : `feat: implement work item #${issue}`;
}

function defaultChangeRequestTitle(config: AdwConfig, issue: number): string {
  return config.providers.workItems.type === 'github' ? `Implement issue #${issue}` : `Implement work item #${issue}`;
}

function defaultChangeRequestBody(config: AdwConfig, issue: number): string {
  return config.providers.workItems.type === 'github' ? `Closes #${issue}` : `Work item #${issue}`;
}

function issueBlob(issue: number, ctx: WorkItemContext, label: string): string {
  return `${label} #${issue}: ${ctx.title}\nLabels: ${ctx.labels.join(' ')}\n\n${ctx.body}`.trim();
}

/** Assemble template arguments, injecting context the token-less agent lacks. */
function phaseArgs(
  phase: string,
  issue: number,
  state: AdwState,
  ctx: WorkItemContext,
  files: readonly string[],
  label: string = workItemLabel(getAdwConfig()),
): string[] {
  const blob = issueBlob(issue, ctx, label);
  switch (phase) {
    case 'classify':
      return [String(issue), blob];
    case 'plan':
      return [blob];
    case 'implement':
      return [state.planFile ?? `(no spec; implement directly from the ${label})`, blob];
    case 'tests':
      return [`${label} #${issue} on branch ${state.branchName}: add focused coverage for this change.\n\n${blob}`];
    case 'e2e':
      return [`${label} #${issue} on branch ${state.branchName}: add e2e coverage if warranted.\n\n${blob}`];
    case 'review':
      // review_phase.md: $1 = spec file (may be empty), ${@:2} = issue/change context.
      return [state.planFile ?? '', blob];
    case 'document':
      return [`Change for ${label} #${issue}; files changed: ${files.join(', ') || 'n/a'}.\n\n${blob}`];
    default:
      return [blob];
  }
}

/**
 * Read the agent-authored commit message / PR body artifacts into state.
 * Free-form text is authored to workspace files (not inlined in JSON) by the
 * review and document phases; document overwrites review, so the last
 * authoring phase wins. Best effort — a missing/unreadable file is ignored.
 */
export function absorbAuthoredText(state: AdwState): void {
  const targets: Array<[string, 'commitMessage' | 'prBody']> = [
    [commitMessagePath(state), 'commitMessage'],
    [prBodyPath(state), 'prBody'],
  ];
  for (const [path, attr] of targets) {
    try {
      const text = readFileSync(path, 'utf8').trim();
      if (text) {
        state[attr] = text;
      }
    } catch {
      // best effort
    }
  }
}

/** Fold a phase result back into run state. */
function applyResult(state: AdwState, phase: string, result: unknown): void {
  if (phase === 'classify') {
    state.issueClass = (result as ClassifyResult).issue_class;
  } else if (phase === 'plan') {
    const plan = result as PlanResult;
    if (plan.plan_file) {
      state.planFile = plan.plan_file;
    }
  }
}

function recordWorkItemMetadata(state: AdwState, config: AdwConfig, issue: number, ctx?: WorkItemContext): void {
  state.workItem = {
    provider: config.providers.workItems.type,
    type: workItemType(config),
    id: String(issue),
    number: issue,
    ...(ctx?.title ? { title: ctx.title } : {}),
  };
}

function recordChangeRequestMetadata(state: AdwState, config: AdwConfig): void {
  state.changeRequest = {
    provider: config.providers.changeRequests.type,
    type: changeRequestType(config),
    id: state.prNumber !== null ? String(state.prNumber) : (state.prUrl ?? null),
    number: state.prNumber,
    url: state.prUrl,
  };
}

// --- setup / finalize -----------------------------------------------------------

/** Setup phase: branch from base, assign, move board to In Progress. */
function setup(
  state: AdwState,
  providerCtx: ProviderContext,
  issue: number,
  ctx: WorkItemContext,
  base: string,
  progress: ProgressFn,
  providers: AdwProviders,
  config: AdwConfig,
): void {
  const branch = deriveBranch(issue, ctx.title, ctx.labels, state.adwId);
  state.branchName = branch;
  const { ok, error } = providers.vcs.createOrCheckoutBranch(branch, base);
  if (!ok) {
    throw new AdwError(`failed to create/checkout branch ${branch}: ${error}`);
  }
  progress('setup', `on branch ${branch}`);
  providers.workItems.assignSelf(providerCtx, issue);
  try {
    providers.workItems.setStatus(providerCtx, issue, config.providers.workItems.inProgressStatus);
  } catch {
    note('could not update board status'); // board update is best effort
  }
}

/**
 * Move the work item to its configured terminal status after a verified merge.
 * Best-effort, mirroring setup's status move: a failed status update must not
 * undo a completed merge, and the verify gate re-reads the real state on its
 * own. No-op unless a project configures `doneStatus` — GitHub already
 * auto-closes the issue via "closes #<n>", so this is for providers (or
 * Projects boards) that need an explicit terminal transition.
 */
function transitionToDone(
  providers: AdwProviders,
  providerCtx: ProviderContext,
  issue: number,
  config: AdwConfig,
  progress: ProgressFn,
): void {
  const doneStatus = config.providers.workItems.doneStatus;
  if (!doneStatus) {
    return;
  }
  try {
    providers.workItems.setStatus(providerCtx, issue, doneStatus);
    progress('report', `work item moved to ${doneStatus}`);
  } catch {
    note('could not update status to done'); // best effort, like setup
  }
}

/**
 * Pre-merge gate commands: the test gate (when configured) followed by any
 * extra quality gates. An empty `testCmd` contributes no test gate (the
 * standalone port assumes no toolchain until one is configured); `extraGates`
 * are additional format/lint/build commands sourced from MX_AGENT_FINALIZE_GATES.
 */
export function finalizeGates(
  testCmd: string,
  extraGates: readonly string[] = [],
  config: AdwConfig = getAdwConfig(),
): string[] {
  const gates: string[] = [...config.commands.defaultFinalizeGates, ...extraGates];
  return testCmd ? [testCmd, ...gates] : [...gates];
}

/** Run gates, commit, push, open PR, watch CI, gate-merge, verify, report. */
async function finalizeAndMerge(
  state: AdwState,
  opts: ResolvedOptions,
  context: {
    providerCtx: ProviderContext;
    issue: number;
    agent: AgentCtx;
    progress: ProgressFn;
    providers: AdwProviders;
    config: AdwConfig;
  },
  deps: OrchestratorDeps,
): Promise<number> {
  const { providerCtx, issue, agent, progress, providers, config } = context;
  const { ghBin, repo } = providerCtx;
  const itemRef = workItemRef(issue, config);
  const crLabel = changeRequestLabel(config);

  // Resume guard: if this run already merged, the branch is gone and the PR is
  // closed — re-running finalize would fail on push or re-merge. Just re-verify.
  if (state.isDone('merge')) {
    progress('report', `merge already completed for ${state.adwId}; nothing to finalize`);
    // Idempotently re-assert the terminal status in case a prior run merged but
    // could not complete the best-effort transition (no-op without doneStatus).
    transitionToDone(providers, providerCtx, issue, config, progress);
    if (opts.verify && ghBin) {
      const st = providers.workItems.state(providerCtx, issue);
      if (!isClosedWorkItemState(st, config)) {
        throw new AdwError(`${itemRef} is ${st} despite a recorded merge; treating as failure`);
      }
    }
    return 0;
  }

  // Final verification gates (orchestrator-owned). Merge only on green.
  // Extra (non-test) gates are configured per-repo via MX_AGENT_FINALIZE_GATES
  // (newline-separated); empty by default so a freshly-ported repo can merge.
  const extraGates = (process.env['MX_AGENT_FINALIZE_GATES'] ?? '')
    .split('\n')
    .map((g) => g.trim())
    .filter((g) => g.length > 0);
  for (const gate of finalizeGates(opts.testCmd, extraGates)) {
    const { rc } = deps.runCmd(shellSplit(gate));
    if (rc !== 0) {
      progress('finalize', `gate failed: ${gate}; not merging`);
      throw new AdwError(`pre-merge gate failed: ${gate}`);
    }
  }
  progress('finalize', 'all pre-merge gates green');

  const commitMessage = state.commitMessage ?? defaultCommitMessage(config, issue);
  const committed = providers.vcs.commitAll(commitMessage);
  if (!committed.ok) {
    throw new AdwError(`commit failed: ${committed.error}`);
  }
  const pushed = providers.vcs.push(state.branchName ?? '');
  if (!pushed.ok) {
    throw new AdwError(`push failed: ${pushed.error}`);
  }

  if (!ghBin) {
    throw new AdwError(`gh not found; cannot open or merge a ${crLabel} (install gh or set GH_BIN)`);
  }

  const prUrl = providers.changeRequests.findForBranch(providerCtx, state.branchName ?? '');
  if (prUrl) {
    state.prUrl = prUrl;
    state.prNumber = prNumberFromUrl(prUrl);
  } else {
    const title = (state.commitMessage ?? defaultChangeRequestTitle(config, issue)).split('\n')[0] ?? '';
    const body = state.prBody ?? defaultChangeRequestBody(config, issue);
    const created = providers.changeRequests.create(providerCtx, {
      branch: state.branchName ?? '',
      title,
      body,
      base: opts.base,
    });
    if (created.error) {
      throw new AdwError(`failed to open ${crLabel}: ${created.error}`);
    }
    state.prNumber = created.number;
    state.prUrl = created.url;
  }
  recordChangeRequestMetadata(state, config);
  state.save();
  progress('finalize', `${crLabel} ready: ${state.prUrl}`);

  // CI watch + fix loop.
  if (state.prNumber !== null) {
    const ciOk = await ciFixLoop(
      state,
      state.prNumber,
      agent,
      {
        ghBin: ghBin ?? '',
        repo,
        maxAttempts: opts.maxCiFix,
        pollIntervalMs: opts.ciPollIntervalMs,
        maxPolls: opts.ciMaxPolls,
        progress,
      },
      deps,
    );
    if (!ciOk) {
      throw new AdwError('CI is not green; refusing to merge');
    }
  }

  // Merge gate — confirmation; non-tty without --yes aborts.
  await confirmMerge({
    yes: assumeYes(opts.yes, deps.env),
    isatty: deps.isatty(),
    confirm: deps.confirm,
    changeRequestLabel: crLabel,
  });
  const merged = providers.changeRequests.squashMerge(providerCtx, state.prNumber ?? state.prUrl ?? '');
  if (!merged.ok) {
    throw new AdwError(`merge failed: ${merged.error}`);
  }
  providers.vcs.pullRebase(opts.base);
  state.markDone('merge');
  state.save();

  // Move the work item to its terminal status before verifying, so a provider
  // whose verify gate reads that status axis sees the post-merge state.
  transitionToDone(providers, providerCtx, issue, config, progress);

  // Verify.
  if (opts.verify) {
    const st = providers.workItems.state(providerCtx, issue);
    if (!isClosedWorkItemState(st, config)) {
      throw new AdwError(`${itemRef} is still ${st} after merge; treating as failure`);
    }
    progress('report', `verified: ${itemRef} is ${st}`);
  }
  progress('report', `phased run ${state.adwId} complete`);
  return 0;
}

// --- plan rendering / entry ---------------------------------------------------------

function printPlan(
  issue: number,
  runner: AgentRunner,
  phases: readonly string[],
  opts: ResolvedOptions,
  config: AdwConfig = getAdwConfig(),
): void {
  const chain = ['setup(ts)', ...phases, 'finalize(ts)', 'ci-fix(ts)', 'merge(ts)', 'report(ts)'];
  console.log(`[dry-run] phased run for ${workItemRef(issue, config)} via ${runner.id}`);
  console.log(`[dry-run] phases: ${chain.join(' -> ')}`);
  console.log(
    `[dry-run] agent env: GH_TOKEN withheld (allowGhToken=false)${opts.inheritEnv ? '; inherited (--inherit-env)' : ''}`,
  );
  console.log(`[dry-run] test gate: ${opts.testCmd || '(none configured)'}`);
}

/**
 * Mint a fresh run state or resume an existing one. --resume requires
 * --adw-id and loads the saved state (starting fresh, with a note, if none
 * is found). A bare --adw-id without --resume must not clobber existing
 * state. A resumed run is bound to its original issue and refuses a
 * mismatched number rather than retargeting the wrong issue onto the
 * existing branch.
 */
function resolveState(opts: ResolvedOptions, issue: number): { state: AdwState; resumed: boolean } {
  if (opts.resume && !opts.adwId) {
    throw new AdwError('--resume requires --adw-id <id>');
  }
  const existing = opts.adwId ? AdwState.load(opts.adwId) : null;
  let state: AdwState | null = null;
  if (opts.resume) {
    state = existing;
    if (state === null) {
      note(`no state for adw_id ${opts.adwId}; starting fresh`);
    }
  } else if (existing !== null) {
    throw new AdwError(`adw_id ${opts.adwId} already has saved state; pass --resume to continue it`);
  }
  const resumed = state !== null;
  if (state === null) {
    state = new AdwState({ adwId: opts.adwId || makeAdwId(), issueNumber: String(issue), base: opts.base });
  }
  if (resumed && state.issueNumber && state.issueNumber !== String(issue)) {
    throw new AdwError(`adw_id ${state.adwId} belongs to work item #${state.issueNumber}, not #${issue}`);
  }
  state.issueNumber = String(issue);
  state.save();
  return { state, resumed };
}

/** Tolerant reconstruction of persisted findings (mirrors the Python reader). */
function findingsFromState(state: AdwState): ReviewFinding[] {
  return state.reviewFindings.map((f) => ({
    severity: String(f['severity'] ?? 'skippable'),
    description: String(f['description'] ?? ''),
    location: String(f['location'] ?? ''),
  }));
}

/** Execute the phased pipeline for one issue through `runner`. */
export async function run(
  issue: number,
  runner: AgentRunner,
  options: RunOptions = {},
  depsOverride: Partial<OrchestratorDeps> = {},
): Promise<number> {
  const opts = resolveOptions(options);
  const config = getAdwConfig();
  const itemLabel = workItemLabel(config);
  const baseDeps = defaultDeps();
  const deps: OrchestratorDeps = {
    ...baseDeps,
    ...depsOverride,
    git: { ...baseDeps.git, ...(depsOverride.git ?? {}) },
  };
  // If a test/consumer overrides the old provider-shaped seams but not
  // `providers` itself, prefer a legacy adapter over the default real
  // providers so no real git/gh leaks into mocked runs.
  if (depsOverride.providers === undefined && hasLegacyProviderOverrides(depsOverride)) {
    delete deps.providers;
  }
  const providers = providersForDeps(deps);

  const phases = parsePhases(opts.phases, config);
  // Preflight the chain (templates + schemas) before any side effects, so a
  // misconfigured custom phase or broken override fails at run start — and
  // a --dry-run doubles as a config check.
  validatePhaseChain(phases, runner.id, config);

  if (opts.dryRun) {
    printPlan(issue, runner, phases, opts, config);
    return 0;
  }

  const ghBin = providers.cli.resolveExecutable(deps.env);
  const repo = opts.repo || providers.cli.detectRepository(ghBin);
  const pctx = providerContext(ghBin, repo);
  const itemRef = workItemRef(issue, config);

  // Preflight: skip already-closed issues; fail fast on unknown numbers.
  if (opts.verify || !opts.force) {
    if (!ghBin) {
      if (opts.verify) {
        throw new AdwError('gh not found but verification is on; install gh, set GH_BIN, or pass --no-verify');
      }
    } else {
      const st = providers.workItems.state(pctx, issue);
      if (isClosedWorkItemState(st, config) && !opts.force) {
        note(`${itemRef} is already CLOSED; skipping (use --force to run anyway)`);
        return 0;
      }
      if (st === 'UNKNOWN') {
        throw new AdwError(`${itemRef} not found in ${repo || 'the current repo'} (is the provider authenticated?)`);
      }
    }
  }

  // State: mint a fresh run or resume an existing one (rules in resolveState).
  const { state, resumed } = resolveState(opts, issue);
  state.engine = 'ts';
  state.runner = runner.id;
  recordWorkItemMetadata(state, config, issue);
  state.save();
  note(`phased run id: ${state.adwId} (workspace: ${state.workspace()})`);

  // A resumed run legitimately carries the prior run's uncommitted edits (the
  // orchestrator only commits at finalize), so the clean-tree precondition
  // applies to fresh runs only.
  if (!opts.allowDirty && !resumed && providers.vcs.workingTreeDirty()) {
    throw new AdwError('working tree is dirty; commit/stash first or pass --allow-dirty');
  }

  const agentEnv = opts.inheritEnv
    ? definedEnv(deps.env)
    : safeSubprocessEnv({ allowGhToken: false, runner: runner.id, source: deps.env });

  const post = !opts.noProgress;
  const progress: ProgressFn = (phase, message) => {
    if (post) {
      providers.workItems.postProgress(pctx, issue, state.adwId, phase, message);
    }
  };

  progress('ops', `starting phased run ${state.adwId}`);

  // Issue context (fetched by the orchestrator; injected into token-less agent phases).
  const ctx = providers.workItems.fetch(pctx, issue) ?? { title: '', body: '', labels: [] };
  recordWorkItemMetadata(state, config, issue, ctx);
  state.save();

  if (!state.isDone('setup')) {
    setup(state, pctx, issue, ctx, opts.base, progress, providers, config);
    state.markDone('setup');
    state.save();
  }

  let files = providers.vcs.changedFiles(opts.base);
  let signal = [ctx.title, ctx.body, ctx.labels.join(' '), files.join(' ')].join(' ');

  const agent: AgentCtx = {
    runner,
    cliModel: opts.model,
    env: agentEnv,
    timeoutMs: opts.timeoutMs,
    ...(opts.maxBudgetUsd !== undefined ? { maxBudgetUsd: opts.maxBudgetUsd } : {}),
    ...(deps.env['MX_AGENT_FORCE_FENCED'] === '1' ? { forceFenced: true } : {}),
  };

  // Runner lifecycle (D6): start/stop are no-ops for the in-process backends;
  // opencode tears down its self-spawned server in stop(), so it runs in a
  // finally — a leaked server child would otherwise outlive the run.
  await runner.start?.();
  try {
    let reviewResult: ReviewResult | null = null;
    for (const phase of phases) {
      if (state.isDone(phase)) {
        note(`skipping ${phase} (already completed)`);
        continue;
      }

      if (isConditionalPhase(phase, config)) {
        const { runIt, reason } = gateConditional(phase, signal, files, config);
        if (!runIt) {
          progress(phase, `skipped: ${reason}`);
          state.markDone(phase);
          state.save();
          continue;
        }
      }

      // Built-in resolve loop, or a custom phase configured as a resolve-style
      // loop (config.loops). A custom loop targets its own agent/command; the
      // built-in passes no `phase`, so resolve is unchanged.
      const customLoop = config.loops[phase];
      if (phase === 'resolve' || customLoop !== undefined) {
        await resolveLoop(
          state,
          agent,
          customLoop !== undefined
            ? { testCmd: customLoop.command, maxAttempts: customLoop.maxAttempts, progress, phase }
            : { testCmd: opts.testCmd, maxAttempts: opts.maxResolve, progress },
          deps,
        );
        state.markDone(phase);
        state.save();
        continue;
      }

      if (phase === 'patch') {
        // On a resume the review phase is skipped, so reconstruct its findings
        // from persisted state rather than silently patching nothing.
        const findings = reviewResult !== null ? reviewResult.findings : findingsFromState(state);
        await patchLoop(state, findings, agent, { maxAttempts: opts.maxPatch, progress }, deps);
        state.markDone(phase);
        state.save();
        continue;
      }

      // D1: classify normally runs on the shared Anthropic-SDK structured call.
      // That path needs a pay-as-you-go ANTHROPIC_API_KEY (the public messages
      // API does not accept a Claude subscription OAuth token), so when no API
      // key is configured we auto-route classify through the selected runner —
      // the Claude Code executable honors a `claude login` / CLAUDE_CODE_OAUTH_TOKEN
      // subscription. MX_AGENT_CLASSIFY_ON_RUNNER=1 forces the runner regardless.
      const classifyOnSharedSdk =
        deps.env['MX_AGENT_CLASSIFY_ON_RUNNER'] !== '1' &&
        (deps.env['ANTHROPIC_API_KEY'] ?? '').trim() !== '';
      if (phase === 'classify' && classifyOnSharedSdk) {
        const prompt = composePhasePrompt(phase, phaseArgs(phase, issue, state, ctx, files, itemLabel), state, runner.id, false);
        const phaseDir = state.phaseDir(phase);
        writeFileSync(join(phaseDir, 'prompt.txt'), prompt, 'utf8');
        const { value, usage } = await deps.classify(prompt, {
          ...(opts.timeoutMs > 0 ? { signal: AbortSignal.timeout(opts.timeoutMs) } : {}),
        });
        writeFileSync(join(phaseDir, 'transcript.log'), `${JSON.stringify(value, null, 2)}\n`, 'utf8');
        recordUsage(state, usage);
        applyResult(state, phase, value);
        state.markDone(phase);
        state.save();
        progress(phase, 'done');
        continue;
      }

      // Normal agent phase (including classify when MX_AGENT_CLASSIFY_ON_RUNNER=1,
      // and any plain project-registered custom phase). The cast carries a custom
      // phase name through the SchemaPhase-typed seam; run-phase resolves its
      // template (<name>.md) and schema (.adw/schemas/<name>.json) by name.
      const outcome = await deps.runAgentPhase({
        phase: phase as AgentPhase,
        templateArgs: phaseArgs(phase, issue, state, ctx, files, itemLabel),
        state,
        runner: agent.runner,
        cliModel: agent.cliModel,
        env: agent.env,
        timeoutMs: agent.timeoutMs,
        ...(agent.maxBudgetUsd !== undefined ? { maxBudgetUsd: agent.maxBudgetUsd } : {}),
        ...(agent.forceFenced ? { forceFenced: true } : {}),
      });
      recordUsage(state, outcome.usage);
      const result = outcome.data;
      applyResult(state, phase, result);
      if (phase === 'review' || phase === 'document') {
        absorbAuthoredText(state);
      }
      if (phase === 'review') {
        const review = result as ReviewResult;
        reviewResult = review;
        // Persist findings so a later --resume can still drive the patch phase.
        state.reviewFindings = review.findings.map((f) => ({
          severity: f.severity,
          description: f.description,
          location: f.location,
        }));
      }
      if (phase === 'implement') {
        const implemented = result as ImplementResult;
        files = implemented.files_changed.length > 0 ? implemented.files_changed : files;
        signal = `${signal} ${files.join(' ')}`;
      }
      state.markDone(phase);
      state.save();
      progress(phase, 'done');
    }

    return await finalizeAndMerge(state, opts, { providerCtx: pctx, issue, agent, progress, providers, config }, deps);
  } finally {
    await runner.stop?.();
  }
}

/** Copy the parent env, dropping undefined values (inherit-env mode only). */
function definedEnv(env: Record<string, string | undefined>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(env)) {
    if (value !== undefined) {
      out[key] = value;
    }
  }
  return out;
}
