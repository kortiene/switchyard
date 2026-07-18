import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { setProjectRoot } from '../src/common.js';
import type { AgentRunner } from '../src/invoker.js';
import type { AdwProviders } from '../src/providers.js';
import { RunRegistry } from '../src/run-registry.js';
import type { RunOutcome } from '../src/run-outcome.js';
import {
  inspectManagedRuns,
  previewManagedPrune,
  removeManagedRun,
  runManagedDetailed,
  type ManagedProviderResolver,
  type ManagedSupervisorDeps,
} from '../src/run-supervisor.js';
import { createMockRunner } from '../src/runners/runner-mock.js';
import { WorktreeManager } from '../src/worktree-manager.js';

let tmp: string;
let origin: string;
let primary: string;
let sourceProject: string;
let managedRoot: string;
let runner: AgentRunner;
let savedGitEnv: { global: string | undefined; system: string | undefined };

function git(cwd: string, ...args: string[]): string {
  return execFileSync('git', args, { cwd, encoding: 'utf8' });
}

function providerResolver(options: {
  workItemState?: string;
  requestState?: 'open' | 'closed' | 'merged' | 'unknown';
} = {}): ManagedProviderResolver {
  return (projectRoot) => {
    const providers: AdwProviders = {
      cli: {
        resolveExecutable: () => '/bin/fake-forge',
        detectRepository: () => 'owner/repo',
      },
      workItems: {
        fetch: () => ({ title: 'Managed lane', body: 'Do the work', labels: ['feature'] }),
        state: () => options.workItemState ?? 'OPEN',
        postProgress: () => {},
        assignSelf: () => {},
        setStatus: () => {},
      },
      vcs: {
        workingTreeDirty: () => false,
        changedFiles: () => [],
        createOrCheckoutBranch: () => ({ ok: true, error: null }),
        commitAll: () => ({ ok: true, error: null }),
        push: () => ({ ok: true, error: null }),
        pullRebase: () => ({ ok: true, error: null }),
        fetchRemote: () => ({ ok: true, error: null }),
        syncWithBase: () => ({ ok: true, rebased: false, error: null }),
      },
      changeRequests: {
        findForBranch: () => null,
        create: () => ({ id: '17', number: 17, url: 'https://example.invalid/17', error: null }),
        pipelineStatus: () => ({ state: 'success', failingJobs: [] }),
        squashMerge: () => ({ ok: true, error: null }),
        status: (_ctx, id) => ({
          state: options.requestState ?? 'merged',
          id: String(id),
          number: 17,
          url: 'https://example.invalid/17',
          headBranch: git(projectRoot, 'branch', '--show-current').trim(),
          headOid: git(projectRoot, 'rev-parse', 'HEAD').trim(),
          baseOid: git(projectRoot, 'rev-parse', 'origin/main').trim(),
        }),
      },
    };
    return {
      providers,
      providerCtx: { ghBin: '/bin/fake-forge', repo: 'owner/repo' },
      workItemProvider: 'github',
    };
  };
}

function supervisorDeps(
  outcome: (options: NonNullable<Parameters<ManagedSupervisorDeps['runDetailed']>[2]>) => RunOutcome,
  resolveProviders: ManagedProviderResolver = providerResolver(),
): ManagedSupervisorDeps {
  return {
    resolveProviders,
    runDetailed: vi.fn(async (_issue, _runner, options = {}) => outcome(options)),
    runDry: vi.fn(async () => 0),
  };
}

beforeEach(() => {
  savedGitEnv = { global: process.env['GIT_CONFIG_GLOBAL'], system: process.env['GIT_CONFIG_SYSTEM'] };
  process.env['GIT_CONFIG_GLOBAL'] = '/dev/null';
  process.env['GIT_CONFIG_SYSTEM'] = '/dev/null';
  tmp = mkdtempSync(join(tmpdir(), 'adw-managed-supervisor-'));
  origin = join(tmp, 'origin.git');
  primary = join(tmp, 'primary');
  sourceProject = join(primary, 'app');
  managedRoot = join(tmp, 'managed');
  git(tmp, 'init', '--bare', '--initial-branch=main', origin);
  git(tmp, 'clone', '--quiet', origin, primary);
  git(primary, 'config', 'user.name', 'adw-test');
  git(primary, 'config', 'user.email', 'adw-test@example.invalid');
  mkdirSync(sourceProject);
  writeFileSync(join(primary, '.gitignore'), 'agents/\n');
  writeFileSync(join(sourceProject, 'app.txt'), 'seed\n');
  git(primary, 'add', '-A');
  git(primary, 'commit', '--quiet', '-m', 'seed');
  git(primary, 'push', '--quiet', '-u', 'origin', 'main');
  runner = createMockRunner();
});

afterEach(() => {
  setProjectRoot(null);
  vi.restoreAllMocks();
  if (savedGitEnv.global === undefined) delete process.env['GIT_CONFIG_GLOBAL'];
  else process.env['GIT_CONFIG_GLOBAL'] = savedGitEnv.global;
  if (savedGitEnv.system === undefined) delete process.env['GIT_CONFIG_SYSTEM'];
  else process.env['GIT_CONFIG_SYSTEM'] = savedGitEnv.system;
  rmSync(tmp, { recursive: true, force: true });
});

describe('managed run supervisor', () => {
  it('provisions and retains PR-ready work, then resumes the exact owned lane', async () => {
    const deps = supervisorDeps((options) => ({
      kind: options.resume ? 'failed' : 'pr_ready',
      adwId: options.adwId,
      workItemId: '41',
      branch: options.managed?.branch,
      changeRequestId: '17',
      changeRequestUrl: 'https://example.invalid/17',
      ...(options.resume ? { error: 'gate still red' } : {}),
    }));
    const first = await runManagedDetailed(
      '41',
      runner,
      { projectRoot: sourceProject, worktreeRoot: managedRoot, adwId: 'a1b2c3d4', noMerge: true, verify: false },
      deps,
    );
    expect(first.kind).toBe('pr_ready');
    expect(git(primary, 'branch', '--show-current').trim()).toBe('main');
    const manager = new WorktreeManager(sourceProject, { managedRoot });
    const registry = new RunRegistry(manager.layout.gitCommonDir);
    const retained = registry.read('a1b2c3d4')!;
    expect(retained.lifecycle).toBe('pr-ready');
    expect(retained.lease).toBeNull();
    expect(retained.branch).toMatch(/a1b2c3d4/);
    expect(existsSync(retained.worktreePath)).toBe(true);
    expect(manager.validateRecord(retained).entry.lockReason).toBe('switchyard:a1b2c3d4');

    const resumed = await runManagedDetailed(
      '41',
      runner,
      { projectRoot: sourceProject, adwId: 'a1b2c3d4', resume: true, verify: false },
      deps,
    );
    expect(resumed).toEqual(expect.objectContaining({ kind: 'failed', error: 'gate still red' }));
    expect(registry.read('a1b2c3d4')).toEqual(
      expect.objectContaining({ lifecycle: 'failed', lease: null, cleanupDisposition: 'retained-by-policy' }),
    );
    expect(existsSync(retained.worktreePath)).toBe(true);
  });

  it('authoritatively cleans a merged lane while retaining its registry record', async () => {
    const deps = supervisorDeps((options) => ({
      kind: 'merged',
      adwId: options.adwId,
      workItemId: '42',
      branch: options.managed?.branch,
      changeRequestId: '17',
      changeRequestUrl: 'https://example.invalid/17',
    }));
    const outcome = await runManagedDetailed(
      '42',
      runner,
      { projectRoot: sourceProject, worktreeRoot: managedRoot, adwId: 'b1b2c3d4', verify: false, yes: true },
      deps,
    );
    expect(outcome.kind).toBe('merged');
    const manager = new WorktreeManager(sourceProject, { managedRoot });
    const registry = new RunRegistry(manager.layout.gitCommonDir);
    const record = registry.read('b1b2c3d4')!;
    expect(record.lifecycle).toBe('cleaned');
    expect(record.changeRequestHeadOid).toBe(record.expectedHeadOid);
    expect(existsSync(record.worktreePath)).toBe(false);
    expect(existsSync(registry.recordPath(record.adwId))).toBe(true);
    expect(inspectManagedRuns(sourceProject)[0]).toEqual(
      expect.objectContaining({ git: expect.objectContaining({ registered: false, error: null }) }),
    );
    expect(previewManagedPrune(sourceProject)).toEqual([]);
    expect(removeManagedRun(sourceProject, record.adwId)).toEqual(
      expect.objectContaining({ cleaned: true, reason: expect.stringContaining('already removed') }),
    );
    expect(git(primary, 'branch', '--show-current').trim()).toBe('main');
  });

  it('cleans an allocated-but-pristine already-closed work item without attaching a branch', async () => {
    const deps = supervisorDeps(
      () => {
        throw new Error('worker must not run for an already-closed item');
      },
      providerResolver({ workItemState: 'CLOSED' }),
    );
    const outcome = await runManagedDetailed(
      '43',
      runner,
      { projectRoot: sourceProject, worktreeRoot: managedRoot, adwId: 'c1b2c3d4', verify: false },
      deps,
    );
    expect(outcome.kind).toBe('skipped_closed');
    const registry = new RunRegistry(new WorktreeManager(sourceProject, { managedRoot }).layout.gitCommonDir);
    expect(registry.read('c1b2c3d4')).toEqual(
      expect.objectContaining({ lifecycle: 'cleaned', branch: null, cleanupDisposition: 'worktree-removed' }),
    );
    expect(deps.runDetailed).not.toHaveBeenCalled();
  });

  it('dry-runs without minting a record or creating the managed parent', async () => {
    const deps = supervisorDeps(() => ({ kind: 'failed', workItemId: '44' }));
    const stdout = vi.spyOn(console, 'log').mockImplementation(() => {});
    const outcome = await runManagedDetailed(
      '44',
      runner,
      { projectRoot: sourceProject, worktreeRoot: managedRoot, dryRun: true },
      deps,
    );
    expect(outcome).toEqual({ kind: 'pr_ready', workItemId: '44' });
    expect(existsSync(managedRoot)).toBe(false);
    expect(deps.runDry).toHaveBeenCalledOnce();
    expect(stdout).toHaveBeenCalledWith(expect.stringContaining('retention'));
  });
});
