/**
 * Provider interfaces for the universal ADW boundary.
 *
 * The orchestration kernel should reason in terms of work items, VCS changes,
 * change requests, CI, and progress updates — not GitHub/gh directly. This
 * file defines those provider seams and ships behavior-preserving Git/GitHub
 * adapters over the existing functions. The current OrchestratorDeps surface
 * still exists for test parity, but defaultDeps() is now assembled from these
 * providers so runtime effects already pass through the provider boundary.
 */

import {
  capture,
  detectRepo,
  issueState,
  postProgress,
  resolveGhBin,
  workingTreeDirty,
} from './exec.js';
import { DEFAULT_ADW_CONFIG, getAdwConfig, type AdwConfig } from './config.js';
import { AdwError } from './errors.js';
import * as git from './git.js';
import {
  assertStatusTransitionRoutable,
  parseCliChangeRequestDescriptor,
  parseCliWorkItemDescriptor,
  parseRestChangeRequestDescriptor,
  parseRestWorkItemDescriptor,
} from './provider-descriptor.js';
import {
  createCliChangeRequestProvider,
  createCliWorkItemProvider,
  createRestChangeRequestProvider,
  createRestWorkItemProvider,
} from './providers-rest-cli.js';
import { fetchWorkItem, setStatus, type WorkItemContext } from './work-item.js';

export interface ProviderContext {
  /** Provider executable, if one is needed (GitHub uses gh). */
  ghBin: string | null;
  /** Repository slug or provider-specific locator (GitHub: owner/repo). */
  repo: string;
}

export interface ProviderCli {
  resolveExecutable(env?: Record<string, string | undefined>): string | null;
  detectRepository(executable: string | null): string;
}

export interface WorkItemProvider {
  fetch(ctx: ProviderContext, id: number | string): WorkItemContext | null;
  state(ctx: ProviderContext, id: number | string): string;
  postProgress(ctx: ProviderContext, id: number | string, adwId: string, phase: string, message: string): void;
  /** Best-effort assignment to the current provider user, when supported. */
  assignSelf(ctx: ProviderContext, id: number | string): void;
  /** Best-effort workflow/status update, when supported. */
  setStatus(ctx: ProviderContext, id: number | string, status: string): void;
}

export interface OperationResult {
  ok: boolean;
  error: string | null;
}

/** Backward-compatible alias for the old git-operation result wording. */
export type GitOperationResult = OperationResult;

export interface SyncWithBaseResult extends OperationResult {
  /** True when the branch was actually behind the base and got rebased. */
  rebased: boolean;
  /** True when the remote branch exists but diverged — the next push needs force. */
  forcePushNeeded?: boolean;
  /** Which step failed when ok=false; a fetch failure is retryable in place. */
  stage?: 'fetch' | 'rebase';
}

export interface VcsProvider {
  workingTreeDirty(): boolean;
  changedFiles(base: string): string[];
  createOrCheckoutBranch(branch: string, base: string): OperationResult;
  commitAll(message: string): OperationResult;
  push(branch: string, force?: boolean): OperationResult;
  pullRebase(base: string): OperationResult;
  /** Rebase the current branch onto origin/<base> when the base has moved. */
  syncWithBase(base: string): SyncWithBaseResult;
}

export interface ChangeRequest {
  /** Provider-neutral identifier (GitHub: PR number as string when known, else URL). */
  id: string | null;
  /** Numeric id when the provider has one (GitHub PR number), else null. */
  number: number | null;
  url: string | null;
}

export type PipelineState = git.CiState;

export interface PipelineJob {
  name: string;
  logExcerpt: string;
}

export interface PipelineStatus {
  state: PipelineState;
  failingJobs: PipelineJob[];
}

/** Backward-compatible aliases for the previous CI/check terminology. */
export type FailingJob = PipelineJob;
export type CiStatus = PipelineStatus;

export interface CreateChangeRequestInput {
  branch: string;
  title: string;
  body: string;
  base: string;
}

export interface CreateChangeRequestResult extends ChangeRequest {
  error: string | null;
}

/** Backward-compatible alias for the old GitHub PR-shaped create result. */
export type CreatePrResult = CreateChangeRequestResult;

export interface ChangeRequestProvider {
  findForBranch(ctx: ProviderContext, branch: string): string | null;
  create(ctx: ProviderContext, input: CreateChangeRequestInput): CreateChangeRequestResult;
  pipelineStatus(ctx: ProviderContext, id: number | string): PipelineStatus;
  /** Compatibility alias for older callers/tests. Prefer pipelineStatus(). */
  ciStatus?(ctx: ProviderContext, id: number | string): PipelineStatus;
  squashMerge(ctx: ProviderContext, id: number | string): OperationResult;
  /**
   * Bounded, error-focused log excerpt of the failing pipeline run, or ''.
   * Optional: providers without log access simply yield no excerpt. Callers
   * must keep it out of public comments (CI logs can echo secrets).
   */
  failingLogExcerpt?(ctx: ProviderContext, id: number | string): string;
}

export interface AdwProviders {
  cli: ProviderCli;
  workItems: WorkItemProvider;
  vcs: VcsProvider;
  changeRequests: ChangeRequestProvider;
}

function numericId(id: number | string): number {
  return typeof id === 'number' ? id : Number.parseInt(String(id), 10);
}

export function createGitHubCliProvider(): ProviderCli {
  return {
    resolveExecutable: (env) => resolveGhBin(env),
    detectRepository: (executable) => detectRepo(executable),
  };
}

export function createGitHubWorkItemProvider(): WorkItemProvider {
  return {
    fetch: (ctx, id) => fetchWorkItem(ctx.ghBin, numericId(id), ctx.repo),
    state: (ctx, id) => issueState(ctx.ghBin, numericId(id), ctx.repo),
    postProgress: (ctx, id, adwId, phase, message) =>
      postProgress(ctx.ghBin, id, ctx.repo, adwId, phase, message),
    assignSelf: (ctx, id) => {
      if (!ctx.ghBin) {
        return;
      }
      const args = [ctx.ghBin, 'issue', 'edit', String(id), '--add-assignee', '@me'];
      if (ctx.repo) {
        args.push('--repo', ctx.repo);
      }
      capture(args);
    },
    setStatus: (ctx, id, status) => {
      if (!ctx.ghBin) {
        return;
      }
      const owner = ctx.repo ? (ctx.repo.split('/')[0] ?? '') : '';
      if (!owner) {
        return;
      }
      setStatus(ctx.ghBin, owner, numericId(id), status, getAdwConfig().providers.workItems.statusFieldName);
    },
  };
}

export function createGitVcsProvider(captureChangedFiles: (base: string) => string[]): VcsProvider {
  return {
    workingTreeDirty,
    changedFiles: captureChangedFiles,
    createOrCheckoutBranch: (branch, base) => git.createOrCheckoutBranch(branch, base),
    commitAll: (message) => git.commitAll(message),
    push: (branch, force) => git.push(branch, force ?? false),
    pullRebase: (base) => git.pullRebase(base),
    syncWithBase: (base) => git.syncWithBase(base),
  };
}

function changeRequestResultFromPr(result: git.CreatePrResult): CreateChangeRequestResult {
  return {
    id: result.number !== null ? String(result.number) : (result.url ?? null),
    number: result.number,
    url: result.url,
    error: result.error,
  };
}

function legacyPrResult(result: CreateChangeRequestResult): git.CreatePrResult {
  return { number: result.number, url: result.url, error: result.error };
}

function githubPipelineStatus(ctx: ProviderContext, id: number | string): PipelineStatus {
  return ctx.ghBin ? git.ciStatus(id, ctx.ghBin, ctx.repo) : { state: 'unknown', failingJobs: [] };
}

export function createGitHubChangeRequestProvider(): ChangeRequestProvider {
  return {
    findForBranch: (ctx, branch) => (ctx.ghBin ? git.prForBranch(branch, ctx.ghBin, ctx.repo) : null),
    create: (ctx, input) => {
      if (!ctx.ghBin) {
        return { id: null, number: null, url: null, error: 'gh not found' };
      }
      return changeRequestResultFromPr(
        git.createPr(input.branch, input.title, input.body, input.base, ctx.ghBin, ctx.repo),
      );
    },
    pipelineStatus: githubPipelineStatus,
    ciStatus: githubPipelineStatus,
    failingLogExcerpt: (ctx, id) => (ctx.ghBin ? git.failingCiLogExcerpt(id, ctx.ghBin, ctx.repo) : ''),
    squashMerge: (ctx, id) => {
      if (!ctx.ghBin) {
        return { ok: false, error: 'gh not found' };
      }
      return git.squashMerge(id, ctx.ghBin, ctx.repo);
    },
  };
}

// ── Provider registry ──────────────────────────────────────────────────────
// Provider *kind* (config `type`) dispatches through these per-role tables
// instead of a closed switch, so adding an in-tree provider (e.g. gitlab/glab)
// is a one-line registration here — no config-schema change. The registry is
// the single source of truth for which kinds exist and fails CLOSED (a loud
// AdwError) on an unknown kind. This is the kernel half of the same
// shape/membership split the phase chain uses: `config.ts` validates the
// type's shape (a non-empty string), and the registry validates membership —
// keeping config.ts ⇄ providers.ts acyclic (cf. parsePhases vs. AGENT_PHASES).
//
// VCS factories take the changed-files capture; the others are nullary.

type CliFactory = () => ProviderCli;
// Work-item factories receive the resolved config so descriptor-driven kinds
// (cli/rest) can read providers.workItems; the github factory ignores it.
type WorkItemFactory = (config: AdwConfig) => WorkItemProvider;
type VcsFactory = (captureChangedFiles: (base: string) => string[]) => VcsProvider;
// Like work items, change-request factories receive the resolved config so the
// descriptor-driven `rest` kind can read providers.changeRequests; github ignores it.
type ChangeRequestFactory = (config: AdwConfig) => ChangeRequestProvider;

const CLI_PROVIDERS: Record<string, CliFactory> = {
  github: createGitHubCliProvider,
};
const WORK_ITEM_PROVIDERS: Record<string, WorkItemFactory> = {
  github: createGitHubWorkItemProvider,
  cli: (config) => {
    const d = parseCliWorkItemDescriptor(config.providers.workItems);
    assertStatusTransitionRoutable(config.providers.workItems.doneStatus, d.routes.setStatus !== undefined, 'cli');
    return createCliWorkItemProvider(d);
  },
  rest: (config) => {
    const d = parseRestWorkItemDescriptor(config.providers.workItems);
    assertStatusTransitionRoutable(config.providers.workItems.doneStatus, d.routes.setStatus !== undefined, 'rest');
    return createRestWorkItemProvider(d);
  },
};
const VCS_PROVIDERS: Record<string, VcsFactory> = {
  git: createGitVcsProvider,
};
const CHANGE_REQUEST_PROVIDERS: Record<string, ChangeRequestFactory> = {
  github: createGitHubChangeRequestProvider,
  cli: (config) => createCliChangeRequestProvider(parseCliChangeRequestDescriptor(config.providers.changeRequests)),
  rest: (config) => createRestChangeRequestProvider(parseRestChangeRequestDescriptor(config.providers.changeRequests)),
};

function resolveProviderFactory<T>(role: string, table: Record<string, T>, type: string): T {
  const factory = table[type];
  if (!factory) {
    const supported = Object.keys(table).sort().join(', ');
    throw new AdwError(`unsupported ${role} provider type "${type}" (supported: ${supported})`);
  }
  return factory;
}

/** Provider kinds the kernel can build for each role (sorted; used in tests/diagnostics). */
export function supportedProviderTypes(): {
  cli: string[];
  workItems: string[];
  vcs: string[];
  changeRequests: string[];
} {
  return {
    cli: Object.keys(CLI_PROVIDERS).sort(),
    workItems: Object.keys(WORK_ITEM_PROVIDERS).sort(),
    vcs: Object.keys(VCS_PROVIDERS).sort(),
    changeRequests: Object.keys(CHANGE_REQUEST_PROVIDERS).sort(),
  };
}

export function createProvidersFromConfig(
  config: AdwConfig,
  captureChangedFiles: (base: string) => string[],
): AdwProviders {
  return {
    cli: resolveProviderFactory('cli', CLI_PROVIDERS, config.providers.cli.type)(),
    workItems: resolveProviderFactory('workItems', WORK_ITEM_PROVIDERS, config.providers.workItems.type)(config),
    vcs: resolveProviderFactory('vcs', VCS_PROVIDERS, config.providers.vcs.type)(captureChangedFiles),
    changeRequests: resolveProviderFactory(
      'changeRequests',
      CHANGE_REQUEST_PROVIDERS,
      config.providers.changeRequests.type,
    )(config),
  };
}

export function createDefaultProviders(captureChangedFiles: (base: string) => string[]): AdwProviders {
  return createProvidersFromConfig(DEFAULT_ADW_CONFIG, captureChangedFiles);
}

/** Legacy OrchestratorDeps adapter for the provider-backed runtime path. */
export function providerBackedDeps(providers: AdwProviders): {
  resolveGhBin: () => string | null;
  detectRepo: (ghBin: string | null) => string;
  issueState: (ghBin: string | null, issue: number, repo: string) => string;
  postProgress: (ghBin: string | null, issue: number | string, repo: string, adwId: string, phase: string, message: string) => void;
  fetchIssue: (ghBin: string | null, issue: number, repo: string) => WorkItemContext | null;
  setStatus: (ghBin: string, owner: string, issue: number, status: string) => void;
  workingTreeDirty: () => boolean;
  changedFiles: (base: string) => string[];
  git: {
    createOrCheckoutBranch: typeof git.createOrCheckoutBranch;
    commitAll: typeof git.commitAll;
    push: typeof git.push;
    pullRebase: typeof git.pullRebase;
    syncWithBase: typeof git.syncWithBase;
    prForBranch: typeof git.prForBranch;
    createPr: typeof git.createPr;
    ciStatus: typeof git.ciStatus;
    squashMerge: typeof git.squashMerge;
    failingCiLogExcerpt: typeof git.failingCiLogExcerpt;
  };
} {
  return {
    resolveGhBin: () => providers.cli.resolveExecutable(),
    detectRepo: (ghBin) => providers.cli.detectRepository(ghBin),
    issueState: (ghBin, issue, repo) => providers.workItems.state({ ghBin, repo }, issue),
    postProgress: (ghBin, issue, repo, adwId, phase, message) =>
      providers.workItems.postProgress({ ghBin, repo }, issue, adwId, phase, message),
    fetchIssue: (ghBin, issue, repo) => providers.workItems.fetch({ ghBin, repo }, issue),
    setStatus: (ghBin, owner, issue, status) => providers.workItems.setStatus({ ghBin, repo: owner }, issue, status),
    workingTreeDirty: () => providers.vcs.workingTreeDirty(),
    changedFiles: (base) => providers.vcs.changedFiles(base),
    git: {
      createOrCheckoutBranch: (branch, base) => providers.vcs.createOrCheckoutBranch(branch, base),
      commitAll: (message) => providers.vcs.commitAll(message),
      push: (branch, force) => providers.vcs.push(branch, force),
      pullRebase: (base) => providers.vcs.pullRebase(base),
      syncWithBase: (base) => providers.vcs.syncWithBase(base),
      prForBranch: (branch, ghBin, repo) => providers.changeRequests.findForBranch({ ghBin, repo }, branch),
      createPr: (branch, title, body, base, ghBin, repo) =>
        legacyPrResult(providers.changeRequests.create({ ghBin, repo }, { branch, title, body, base })),
      ciStatus: (pr, ghBin, repo) => providers.changeRequests.pipelineStatus({ ghBin, repo }, pr),
      squashMerge: (pr, ghBin, repo) => providers.changeRequests.squashMerge({ ghBin, repo }, pr),
      failingCiLogExcerpt: (pr, ghBin, repo) =>
        providers.changeRequests.failingLogExcerpt?.({ ghBin, repo }, pr) ?? '',
    },
  };
}
