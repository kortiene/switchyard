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
import * as git from './git.js';
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

export interface VcsProvider {
  workingTreeDirty(): boolean;
  changedFiles(base: string): string[];
  createOrCheckoutBranch(branch: string, base: string): OperationResult;
  commitAll(message: string): OperationResult;
  push(branch: string): OperationResult;
  pullRebase(base: string): OperationResult;
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
    push: (branch) => git.push(branch),
    pullRebase: (base) => git.pullRebase(base),
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
    squashMerge: (ctx, id) => {
      if (!ctx.ghBin) {
        return { ok: false, error: 'gh not found' };
      }
      return git.squashMerge(id, ctx.ghBin, ctx.repo);
    },
  };
}

export function createProvidersFromConfig(
  config: AdwConfig,
  captureChangedFiles: (base: string) => string[],
): AdwProviders {
  // The schema currently allows only these built-ins. Keep the switch shape so
  // adding gitlab/linear/jira providers later is localized here.
  const cli = config.providers.cli.type === 'github' ? createGitHubCliProvider() : neverProvider('cli');
  const workItems =
    config.providers.workItems.type === 'github' ? createGitHubWorkItemProvider() : neverProvider('workItems');
  const vcs = config.providers.vcs.type === 'git' ? createGitVcsProvider(captureChangedFiles) : neverProvider('vcs');
  const changeRequests =
    config.providers.changeRequests.type === 'github'
      ? createGitHubChangeRequestProvider()
      : neverProvider('changeRequests');
  return { cli, workItems, vcs, changeRequests };
}

function neverProvider(name: string): never {
  throw new Error(`unsupported provider configured for ${name}`);
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
    prForBranch: typeof git.prForBranch;
    createPr: typeof git.createPr;
    ciStatus: typeof git.ciStatus;
    squashMerge: typeof git.squashMerge;
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
      push: (branch) => providers.vcs.push(branch),
      pullRebase: (base) => providers.vcs.pullRebase(base),
      prForBranch: (branch, ghBin, repo) => providers.changeRequests.findForBranch({ ghBin, repo }, branch),
      createPr: (branch, title, body, base, ghBin, repo) =>
        legacyPrResult(providers.changeRequests.create({ ghBin, repo }, { branch, title, body, base })),
      ciStatus: (pr, ghBin, repo) => providers.changeRequests.pipelineStatus({ ghBin, repo }, pr),
      squashMerge: (pr, ghBin, repo) => providers.changeRequests.squashMerge({ ghBin, repo }, pr),
    },
  };
}
