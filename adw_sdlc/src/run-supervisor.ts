/** High-level lifecycle for one opt-in managed worktree run. */

import { createHash, randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';

import { setProjectRoot } from './common.js';
import { getAdwConfig, isClosedWorkItemState } from './config.js';
import { AdwError } from './errors.js';
import { note } from './exec.js';
import type { AgentRunner } from './invoker.js';
import { run, runDetailed, type RunOptions } from './orchestrator.js';
import { createProvidersFromConfig, type AdwProviders, type ProviderContext } from './providers.js';
import {
  contextFromRecord,
  RunRegistry,
  type ManagedLifecycleState,
  type ManagedRunRecord,
} from './run-registry.js';
import type { RunOutcome } from './run-outcome.js';
import { makeAdwId, AdwState, validateAdwId } from './state.js';
import { deriveBranch, type WorkItemId } from './work-item.js';
import { WorktreeManager } from './worktree-manager.js';

export interface ManagedRunOptions extends RunOptions {
  worktreeRoot?: string;
}

const ACTIVE_STATES: ReadonlySet<ManagedLifecycleState> = new Set(['provisioning', 'ready', 'running']);
const KERNEL_VERSION = '0.0.1';

function selectedSourceRoot(options: ManagedRunOptions): string {
  return resolve(options.projectRoot ?? process.cwd());
}

export interface ManagedProviderSetup {
  providers: AdwProviders;
  providerCtx: ProviderContext;
  workItemProvider: string;
}

export type ManagedProviderResolver = (projectRoot: string) => ManagedProviderSetup;

export interface ManagedSupervisorDeps {
  resolveProviders: ManagedProviderResolver;
  runDetailed: typeof runDetailed;
  runDry: typeof run;
}

function defaultProvidersAt(projectRoot: string): ManagedProviderSetup {
  setProjectRoot(projectRoot);
  const config = getAdwConfig();
  const providers = createProvidersFromConfig(config, () => []);
  const ghBin = providers.cli.resolveExecutable(process.env);
  const repo = providers.cli.detectRepository(ghBin);
  return {
    providers,
    providerCtx: { ghBin, repo },
    workItemProvider: config.providers.workItems.type,
  };
}

function recordForProvisioning(options: {
  adwId: string;
  generationId: string;
  issue: WorkItemId;
  runner: AgentRunner;
  manager: WorktreeManager;
  base: string;
}): ManagedRunRecord {
  const context = options.manager.context(options.adwId);
  const now = new Date().toISOString();
  return {
    schemaVersion: 1,
    adwId: options.adwId,
    repositoryId: options.manager.layout.repositoryId,
    gitCommonDir: options.manager.layout.gitCommonDir,
    providerRepo: '(pending)',
    workItemProvider: '(pending)',
    workItemId: String(options.issue),
    sourceRoot: options.manager.layout.sourceRoot,
    projectRelativePath: options.manager.layout.projectRelativePath,
    managedRoot: options.manager.managedRoot,
    worktreePath: context.worktreeRoot,
    projectRoot: context.projectRoot,
    stateRoot: context.stateRoot,
    artifactRoot: context.artifactRoot,
    generationId: options.generationId,
    branch: null,
    branchIntent: null,
    base: options.base,
    allocationOid: null,
    expectedHeadOid: null,
    remoteHeadOid: null,
    lifecycle: 'provisioning',
    outcome: null,
    runner: options.runner.id,
    kernelVersion: KERNEL_VERSION,
    configDigest: null,
    lease: null,
    workItemSnapshot: null,
    changeRequestId: null,
    changeRequestUrl: null,
    changeRequestHeadOid: null,
    mergeIntent: null,
    cleanupDisposition: null,
    error: null,
    createdAt: now,
    updatedAt: now,
  };
}

function stateForRecord(record: ManagedRunRecord): AdwState | null {
  return AdwState.loadManaged(record.adwId, {
    controlRoot: record.stateRoot,
    artifactRoot: record.artifactRoot,
    strictPersistence: true,
  });
}

function lifecycleFor(outcome: RunOutcome): ManagedLifecycleState {
  switch (outcome.kind) {
    case 'merged':
      return 'merged';
    case 'pr_ready':
      return 'pr-ready';
    case 'skipped_closed':
      return 'skipped-closed';
    case 'interrupted':
      return 'interrupted';
    case 'failed':
      return 'failed';
  }
}

function statusContext(record: ManagedRunRecord, resolveProviders: ManagedProviderResolver): {
  providers: AdwProviders;
  providerCtx: ProviderContext;
} {
  const resolved = resolveProviders(record.projectRoot);
  return {
    providers: resolved.providers,
    providerCtx: {
      ghBin: resolved.providerCtx.ghBin,
      repo: record.providerRepo === '(pending)' ? resolved.providerCtx.repo : record.providerRepo,
    },
  };
}

function reconcileMergeIntent(
  record: ManagedRunRecord,
  localHeadOid: string,
  resolveProviders: ManagedProviderResolver,
): RunOutcome | null {
  if (record.mergeIntent === null) return null;
  const { providers, providerCtx } = statusContext(record, resolveProviders);
  const statusReader = providers.changeRequests.status;
  if (statusReader === undefined) {
    throw new AdwError(`cannot reconcile merge intent for ${record.adwId}: provider has no status query`);
  }
  const status = statusReader(providerCtx, record.mergeIntent.changeRequestId);
  if (status.state === 'unknown') {
    throw new AdwError(`cannot reconcile merge intent for ${record.adwId}: remote state is unknown`);
  }
  if (!status.headOid || status.headOid !== record.mergeIntent.headOid || localHeadOid !== record.mergeIntent.headOid) {
    throw new AdwError(`cannot reconcile merge intent for ${record.adwId}: change-request head identity changed`);
  }
  if (status.state === 'merged') {
    return {
      kind: 'merged',
      adwId: record.adwId,
      workItemId: record.workItemId,
      ...(record.branch ? { branch: record.branch } : {}),
      changeRequestId: record.mergeIntent.changeRequestId,
      ...(status.url ? { changeRequestUrl: status.url } : {}),
    };
  }
  if (status.state === 'closed') {
    throw new AdwError(`change request from merge intent for ${record.adwId} was closed without merging`);
  }
  return null; // still open at the exact recorded head; normal finalize may continue
}

async function withManagedSignalForwarding<T>(
  outerSignal: AbortSignal | undefined,
  execute: (signal: AbortSignal) => Promise<T>,
): Promise<T> {
  const controller = new AbortController();
  let signals = 0;
  const forward = (name: 'SIGINT' | 'SIGTERM'): void => {
    signals += 1;
    if (signals === 1) {
      controller.abort(new Error(`managed run interrupted by ${name}`));
      return;
    }
    // A second signal is an explicit immediate escalation. Restore the
    // platform default handler before re-sending it to this process.
    process.off('SIGINT', onSigint);
    process.off('SIGTERM', onSigterm);
    process.kill(process.pid, name);
  };
  const onSigint = (): void => forward('SIGINT');
  const onSigterm = (): void => forward('SIGTERM');
  process.on('SIGINT', onSigint);
  process.on('SIGTERM', onSigterm);
  try {
    const signal = outerSignal
      ? AbortSignal.any([outerSignal, controller.signal])
      : controller.signal;
    return await execute(signal);
  } finally {
    process.off('SIGINT', onSigint);
    process.off('SIGTERM', onSigterm);
  }
}

/**
 * Apply the design's two automatic cleanup proofs. Unknown provider state,
 * dirt, or any ownership mismatch retains the lane.
 */
export function cleanupManagedRun(
  registry: RunRegistry,
  manager: WorktreeManager,
  adwId: string,
  resolveProviders: ManagedProviderResolver = defaultProvidersAt,
): { cleaned: boolean; reason: string } {
  let record = registry.read(adwId);
  if (record === null) throw new AdwError(`unknown managed run: ${adwId}`);
  if (record.lifecycle === 'cleaned' && !existsSync(record.worktreePath)) {
    return { cleaned: true, reason: 'worktree was already removed; durable state retained' };
  }
  const validation = manager.validateRecord(record, { allowDetached: record.branch === null });
  if (!validation.clean) return { cleaned: false, reason: 'worktree is dirty' };

  const { providers, providerCtx } = statusContext(record, resolveProviders);
  if (record.outcome?.kind === 'merged') {
    const requestId = record.changeRequestId ?? record.changeRequestUrl;
    if (!requestId || providers.changeRequests.status === undefined) {
      return { cleaned: false, reason: 'provider cannot authoritatively reconcile the merged change request' };
    }
    const status = providers.changeRequests.status(providerCtx, requestId);
    if (status.state !== 'merged') return { cleaned: false, reason: `change request is ${status.state}` };
    if (!status.headOid || status.headOid !== validation.headOid) {
      return { cleaned: false, reason: 'provider/local change-request head identity does not match' };
    }
    record = registry.update(adwId, (current) => ({
      ...current,
      changeRequestHeadOid: status.headOid,
      cleanupDisposition: 'merged-head-proven',
    }));
  } else if (record.outcome?.kind === 'skipped_closed') {
    const config = getAdwConfig();
    const workItemState = providers.workItems.state(providerCtx, record.workItemId);
    const state = stateForRecord(record);
    if (!isClosedWorkItemState(workItemState, config)) {
      return { cleaned: false, reason: `work item is ${workItemState}` };
    }
    if (
      record.branch !== null ||
      record.changeRequestId !== null ||
      record.changeRequestUrl !== null ||
      record.allocationOid === null ||
      validation.headOid !== record.allocationOid ||
      (state !== null && state.completedPhases.some((phase) => phase !== 'setup'))
    ) {
      return { cleaned: false, reason: 'skipped-closed lane is not pristine' };
    }
    record = registry.update(adwId, (current) => ({ ...current, cleanupDisposition: 'pristine-closed-proven' }));
  } else {
    return { cleaned: false, reason: 'only merged or pristine skipped-closed runs are removable' };
  }

  const lifecycle = registry.acquireLifecycleLock();
  try {
    const current = registry.read(adwId);
    if (current === null || current.generationId !== record.generationId) {
      throw new AdwError(`managed run ${adwId} changed before cleanup`);
    }
    manager.remove(current, { removeArtifacts: true });
    registry.update(adwId, (owned) => ({
      ...owned,
      lifecycle: 'cleaned',
      cleanupDisposition: 'worktree-removed',
      lease: null,
    }));
  } catch (error) {
    if (existsSync(record.worktreePath)) {
      registry.update(adwId, (current) => ({
        ...current,
        lifecycle: 'cleanup-needed',
        cleanupDisposition: error instanceof Error ? error.message : String(error),
      }));
    }
    throw error;
  } finally {
    lifecycle.release();
  }
  return { cleaned: true, reason: 'worktree removed; durable state retained' };
}

async function executeRegisteredRun(
  issue: WorkItemId,
  runner: AgentRunner,
  options: ManagedRunOptions,
  registry: RunRegistry,
  manager: WorktreeManager,
  record: ManagedRunRecord,
  resume: boolean,
  deps: ManagedSupervisorDeps,
): Promise<RunOutcome> {
  const validation = manager.validateRecord(record);
  const savedState = stateForRecord(record);
  if (resume && savedState === null && !validation.clean) {
    throw new AdwError(
      `managed run ${record.adwId} has no durable state but its worktree contains edits; refusing unsafe resume`,
    );
  }
  const executionLease = registry.acquireRunLease(record.adwId);
  registry.update(record.adwId, (current) => ({
    ...current,
    lifecycle: 'running',
    runner: runner.id,
    lease: executionLease.metadata,
    error: null,
  }));

  let outcome: RunOutcome;
  try {
    const reconciled = resume ? reconcileMergeIntent(record, validation.headOid, deps.resolveProviders) : null;
    outcome =
      reconciled ??
      (await withManagedSignalForwarding(options.signal, (signal) =>
        deps.runDetailed(issue, runner, {
          ...options,
          projectRoot: record.projectRoot,
          repo: record.providerRepo,
          adwId: record.adwId,
          resume,
          signal,
          runContext: contextFromRecord(record),
          managed: {
            branch: record.branch ?? '',
            generationId: record.generationId,
            mergeLockPath: registry.lockPath('merge'),
            checkpointMergeIntent: (intent) => {
              registry.update(record.adwId, (current) => ({
                ...current,
                changeRequestId: intent.changeRequestId,
                expectedHeadOid: intent.headOid,
                mergeIntent: { ...intent, startedAt: new Date().toISOString() },
              }));
            },
            checkpointMergeOutcome: (merged) => {
              registry.update(record.adwId, (current) => ({
                ...current,
                changeRequestId: merged.changeRequestId,
                changeRequestHeadOid: merged.headOid,
              }));
            },
            ...(record.workItemSnapshot !== null &&
            typeof record.workItemSnapshot['title'] === 'string' &&
            typeof record.workItemSnapshot['body'] === 'string' &&
            Array.isArray(record.workItemSnapshot['labels'])
              ? {
                  workItemSnapshot: {
                    title: record.workItemSnapshot['title'],
                    body: record.workItemSnapshot['body'],
                    labels: record.workItemSnapshot['labels'].filter(
                      (label): label is string => typeof label === 'string',
                    ),
                  },
                }
              : {}),
          },
        }),
      ));
    let headOid = record.expectedHeadOid;
    try {
      headOid = manager.validatePath(record.worktreePath).headOid;
    } catch {
      // Preserve the prior checkpoint; reconciliation will report the mismatch.
    }
    const state = stateForRecord(record);
    registry.update(record.adwId, (current) => ({
      ...current,
      lifecycle: lifecycleFor(outcome),
      outcome,
      expectedHeadOid: headOid,
      lease: null,
      changeRequestId:
        outcome.changeRequestId ??
        (state?.prNumber !== null && state?.prNumber !== undefined ? String(state.prNumber) : current.changeRequestId),
      changeRequestUrl: outcome.changeRequestUrl ?? state?.prUrl ?? current.changeRequestUrl,
      error: outcome.error ?? null,
      cleanupDisposition:
        outcome.kind === 'merged' || outcome.kind === 'skipped_closed'
          ? 'cleanup-proof-pending'
          : 'retained-by-policy',
    }));
  } finally {
    executionLease.release();
    const current = registry.read(record.adwId);
    if (current?.lease?.leaseId === executionLease.metadata.leaseId) {
      registry.update(record.adwId, (owned) => ({ ...owned, lease: null, lifecycle: 'interrupted' }));
    }
  }

  if (outcome.kind === 'merged' || outcome.kind === 'skipped_closed') {
    try {
      const result = cleanupManagedRun(registry, manager, record.adwId, deps.resolveProviders);
      if (!result.cleaned) {
        registry.update(record.adwId, (current) => ({ ...current, cleanupDisposition: result.reason }));
      }
    } catch (error) {
      note(`managed cleanup retained ${record.adwId}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  return outcome;
}

async function freshManagedRun(
  issue: WorkItemId,
  runner: AgentRunner,
  options: ManagedRunOptions,
  manager: WorktreeManager,
  registry: RunRegistry,
  deps: ManagedSupervisorDeps,
): Promise<RunOutcome> {
  const adwId = validateAdwId(options.adwId ?? makeAdwId());
  const generationId = randomUUID();
  const base = options.base ?? 'main';
  const lifecycle = registry.acquireLifecycleLock();
  try {
    if (registry.read(adwId) !== null) {
      throw new AdwError(`managed run ${adwId} already exists; pass --resume to continue it`);
    }
    const duplicate = registry.list().find(
      (record) => record.workItemId === String(issue) && ACTIVE_STATES.has(record.lifecycle),
    );
    if (duplicate) {
      throw new AdwError(
        `work item ${String(issue)} already has active managed run ${duplicate.adwId} at ${duplicate.worktreePath}`,
      );
    }
    registry.create(recordForProvisioning({ adwId, generationId, issue, runner, manager, base }));
  } finally {
    lifecycle.release();
  }

  try {
    const allocated = manager.allocateDetached(adwId, base, generationId);
    let record = registry.update(adwId, (current) => ({
      ...current,
      worktreePath: allocated.context.worktreeRoot,
      projectRoot: allocated.context.projectRoot,
      stateRoot: allocated.context.stateRoot,
      artifactRoot: allocated.context.artifactRoot,
      allocationOid: allocated.allocationOid,
      expectedHeadOid: allocated.allocationOid,
    }));

    const config = (() => {
      setProjectRoot(record.projectRoot);
      return getAdwConfig();
    })();
    if (config.providers.vcs.type !== 'git') {
      throw new AdwError('--worktree requires the git VCS provider');
    }
    if (base !== (options.base ?? 'main')) {
      throw new AdwError(`managed base changed while loading lane configuration: ${base}`);
    }
    const providerSetup = deps.resolveProviders(record.projectRoot);
    const providerRepo = options.repo ?? providerSetup.providerCtx.repo;
    if (config.providers.changeRequests.type === 'github' && !providerRepo) {
      throw new AdwError('managed GitHub runs require an explicit or detected --repo owner/name locator');
    }
    const providerCtx = { ...providerSetup.providerCtx, repo: providerRepo };
    const status =
      providerCtx.ghBin || providerSetup.workItemProvider !== 'github'
        ? providerSetup.providers.workItems.state(providerCtx, issue)
        : 'UNKNOWN';
    if (isClosedWorkItemState(status, config) && !options.force) {
      manager.lock(record.worktreePath, adwId);
      const skipped: RunOutcome = { kind: 'skipped_closed', adwId, workItemId: String(issue) };
      registry.update(adwId, (current) => ({
        ...current,
        providerRepo,
        workItemProvider: providerSetup.workItemProvider,
        lifecycle: 'skipped-closed',
        outcome: skipped,
        cleanupDisposition: 'cleanup-proof-pending',
      }));
      const result = cleanupManagedRun(registry, manager, adwId, deps.resolveProviders);
      if (!result.cleaned) registry.update(adwId, (current) => ({ ...current, cleanupDisposition: result.reason }));
      return skipped;
    }
    if (status === 'UNKNOWN' && (options.verify ?? true)) {
      throw new AdwError(`work item ${String(issue)} could not be verified before managed allocation`);
    }
    const workItem = providerSetup.providers.workItems.fetch(providerCtx, issue) ?? { title: '', body: '', labels: [] };
    const branch = deriveBranch(issue, workItem.title, workItem.labels, adwId);
    const configDigest = createHash('sha256').update(JSON.stringify(config)).digest('hex');
    record = registry.update(adwId, (current) => ({
      ...current,
      providerRepo,
      workItemProvider: providerSetup.workItemProvider,
      branchIntent: branch,
      configDigest,
      workItemSnapshot: {
        title: workItem.title,
        body: workItem.body,
        labels: workItem.labels,
        configDigest,
      },
    }));
    const attachLock = registry.acquireLifecycleLock();
    try {
      const collision = registry.list().find(
        (candidate) => candidate.adwId !== adwId && candidate.branch === branch && candidate.lifecycle !== 'cleaned',
      );
      if (collision) throw new AdwError(`managed branch ${branch} is owned by run ${collision.adwId}`);
      const head = manager.attachBranch(allocated.context, branch);
      manager.prepareArtifactRoot(allocated.context);
      record = registry.update(adwId, (current) => ({
        ...current,
        providerRepo,
        workItemProvider: providerSetup.workItemProvider,
        branch,
        branchIntent: branch,
        expectedHeadOid: head,
        lifecycle: 'ready',
      }));
    } finally {
      attachLock.release();
    }
    return await executeRegisteredRun(issue, runner, options, registry, manager, record, false, deps);
  } catch (error) {
    const current = registry.read(adwId);
    if (current !== null && current.lifecycle !== 'cleaned') {
      registry.update(adwId, (record) => ({
        ...record,
        lifecycle: 'failed',
        error: error instanceof Error ? error.message : String(error),
        cleanupDisposition: 'retained-by-policy',
      }));
    }
    throw error;
  }
}

/** Structured managed-run API. One CLI process owns exactly one lane/lease. */
export async function runManagedDetailed(
  issue: WorkItemId,
  runner: AgentRunner,
  options: ManagedRunOptions = {},
  depsOverride: Partial<ManagedSupervisorDeps> = {},
): Promise<RunOutcome> {
  const deps: ManagedSupervisorDeps = {
    resolveProviders: defaultProvidersAt,
    runDetailed,
    runDry: run,
    ...depsOverride,
  };
  const sourceRoot = selectedSourceRoot(options);
  const manager = new WorktreeManager(sourceRoot, { managedRoot: options.worktreeRoot });
  const registry = new RunRegistry(manager.layout.gitCommonDir);

  if (options.dryRun) {
    const providerSetup = deps.resolveProviders(manager.layout.sourceRoot);
    const providerRepo = options.repo ?? providerSetup.providerCtx.repo;
    const snapshot = providerSetup.providers.workItems.fetch(
      { ...providerSetup.providerCtx, repo: providerRepo },
      issue,
    ) ?? { title: '', body: '', labels: [] };
    const previewBranch = deriveBranch(issue, snapshot.title, snapshot.labels, '00000000').replace(
      '00000000',
      '<adw-id>',
    );
    console.log(`[dry-run] managed source root: ${manager.layout.sourceRoot}`);
    console.log(`[dry-run] managed worktree parent: ${manager.managedRoot}`);
    console.log(`[dry-run] managed worktree path: ${manager.managedRoot}/<adw-id>`);
    console.log(`[dry-run] managed branch: ${previewBranch}`);
    console.log(`[dry-run] managed state root: ${registry.root}`);
    console.log('[dry-run] retention: failures/interruptions/PR-ready/unknown state are retained; cleanup never forces');
    await deps.runDry(issue, runner, { ...options, projectRoot: manager.layout.sourceRoot, dryRun: true });
    return { kind: 'pr_ready', workItemId: String(issue) };
  }

  if (options.resume) {
    if (!options.adwId) throw new AdwError('--worktree --resume requires --adw-id <id>');
    let record = registry.read(validateAdwId(options.adwId));
    if (record === null) throw new AdwError(`unknown managed run: ${options.adwId}`);
    if (record.repositoryId !== manager.layout.repositoryId) {
      throw new AdwError(`managed run ${record.adwId} belongs to a different repository`);
    }
    const ownedManager = new WorktreeManager(sourceRoot, { managedRoot: record.managedRoot });
    if (!record.branch) {
      if (!record.branchIntent || record.allocationOid === null) {
        throw new AdwError(`managed run ${record.adwId} never completed recoverable branch provisioning`);
      }
      const repairLock = registry.acquireLifecycleLock();
      try {
        const validation = ownedManager.validateRecord(record, { allowDetached: true });
        let head = validation.headOid;
        if (validation.branch === null) {
          head = ownedManager.attachBranch(contextFromRecord(record), record.branchIntent);
        } else if (validation.branch !== record.branchIntent) {
          throw new AdwError(
            `managed provisioning expected branch ${record.branchIntent}, found ${validation.branch}`,
          );
        } else {
          ownedManager.lock(record.worktreePath, record.adwId);
        }
        ownedManager.prepareArtifactRoot(contextFromRecord(record));
        record = registry.update(record.adwId, (current) => ({
          ...current,
          branch: current.branchIntent,
          expectedHeadOid: head,
          lifecycle: 'ready',
        }));
      } finally {
        repairLock.release();
      }
    }
    return await executeRegisteredRun(issue, runner, options, registry, ownedManager, record, true, deps);
  }
  return await freshManagedRun(issue, runner, options, manager, registry, deps);
}

/** Numeric CLI wrapper: successful terminal outcomes map to zero. */
export async function runManagedWorktree(
  issue: WorkItemId,
  runner: AgentRunner,
  options: ManagedRunOptions = {},
): Promise<number> {
  const outcome = await runManagedDetailed(issue, runner, options);
  if (outcome.kind === 'failed' || outcome.kind === 'interrupted') {
    if (outcome.error) note(`managed run ${outcome.adwId ?? '(unallocated)'}: ${outcome.error}`);
    return 1;
  }
  return 0;
}

export interface ManagedRunInspection {
  record: ManagedRunRecord;
  git: {
    registered: boolean;
    directoryPresent: boolean;
    branch: string | null;
    headOid: string | null;
    clean: boolean | null;
    error: string | null;
  };
}

export function inspectManagedRuns(sourceRoot: string): ManagedRunInspection[] {
  const baseManager = new WorktreeManager(sourceRoot);
  const registry = new RunRegistry(baseManager.layout.gitCommonDir);
  return registry.list().map((record) => {
    const manager = new WorktreeManager(sourceRoot, { managedRoot: record.managedRoot });
    if (record.lifecycle === 'cleaned' && !existsSync(record.worktreePath)) {
      return {
        record,
        git: {
          registered: false,
          directoryPresent: false,
          branch: null,
          headOid: null,
          clean: null,
          error: null,
        },
      };
    }
    try {
      const validation = manager.validateRecord(record, { allowDetached: record.branch === null });
      return {
        record,
        git: {
          registered: true,
          directoryPresent: true,
          branch: validation.branch,
          headOid: validation.headOid,
          clean: validation.clean,
          error: null,
        },
      };
    } catch (error) {
      const registered = manager.listGitWorktrees().some(
        (entry) => resolve(entry.path) === resolve(record.worktreePath),
      );
      return {
        record,
        git: {
          registered,
          directoryPresent: existsSync(record.worktreePath),
          branch: null,
          headOid: null,
          clean: null,
          error: error instanceof Error ? error.message : String(error),
        },
      };
    }
  });
}

export function removeManagedRun(sourceRoot: string, adwId: string): { cleaned: boolean; reason: string } {
  const initial = new WorktreeManager(sourceRoot);
  const registry = new RunRegistry(initial.layout.gitCommonDir);
  const record = registry.read(validateAdwId(adwId));
  if (record === null) throw new AdwError(`unknown managed run: ${adwId}`);
  const manager = new WorktreeManager(sourceRoot, { managedRoot: record.managedRoot });
  return cleanupManagedRun(registry, manager, adwId);
}

/** Reconciliation preview only; it never invokes global `git worktree prune`. */
export function previewManagedPrune(sourceRoot: string): ManagedRunInspection[] {
  return inspectManagedRuns(sourceRoot).filter(
    ({ record, git }) =>
      record.lifecycle !== 'cleaned' &&
      (record.lifecycle === 'cleanup-needed' || !git.registered || !git.directoryPresent || git.error !== null),
  );
}
