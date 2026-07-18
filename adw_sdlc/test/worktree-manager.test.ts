import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { setProjectRoot } from '../src/common.js';
import { commitAll } from '../src/git.js';
import { RunRegistry, type ManagedRunRecord } from '../src/run-registry.js';
import { AdwState } from '../src/state.js';
import { parseWorktreePorcelain, WorktreeManager } from '../src/worktree-manager.js';

let tmp: string;
let origin: string;
let primary: string;
let managedRoot: string;
let savedGitEnv: { global: string | undefined; system: string | undefined };

function git(cwd: string, ...args: string[]): string {
  return execFileSync('git', args, { cwd, encoding: 'utf8' });
}

function recordFor(
  manager: WorktreeManager,
  values: {
    adwId: string;
    generationId: string;
    branch: string | null;
    allocationOid: string;
    expectedHeadOid: string;
  },
): ManagedRunRecord {
  const context = manager.context(values.adwId);
  const now = new Date().toISOString();
  return {
    schemaVersion: 1,
    adwId: values.adwId,
    repositoryId: manager.layout.repositoryId,
    gitCommonDir: manager.layout.gitCommonDir,
    providerRepo: 'owner/repo',
    workItemProvider: 'github',
    workItemId: '1',
    sourceRoot: manager.layout.sourceRoot,
    projectRelativePath: manager.layout.projectRelativePath,
    managedRoot: manager.managedRoot,
    worktreePath: context.worktreeRoot,
    projectRoot: context.projectRoot,
    stateRoot: context.stateRoot,
    artifactRoot: context.artifactRoot,
    generationId: values.generationId,
    branch: values.branch,
    branchIntent: values.branch,
    base: 'main',
    allocationOid: values.allocationOid,
    expectedHeadOid: values.expectedHeadOid,
    remoteHeadOid: null,
    lifecycle: 'ready',
    outcome: null,
    runner: 'mock',
    kernelVersion: 'test',
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

beforeEach(() => {
  savedGitEnv = { global: process.env['GIT_CONFIG_GLOBAL'], system: process.env['GIT_CONFIG_SYSTEM'] };
  process.env['GIT_CONFIG_GLOBAL'] = '/dev/null';
  process.env['GIT_CONFIG_SYSTEM'] = '/dev/null';
  tmp = mkdtempSync(join(tmpdir(), 'adw-managed-worktrees-'));
  origin = join(tmp, 'origin.git');
  primary = join(tmp, 'primary');
  managedRoot = join(tmp, 'managed lanes with spaces');
  git(tmp, 'init', '--bare', '--initial-branch=main', origin);
  git(tmp, 'clone', '--quiet', origin, primary);
  git(primary, 'config', 'user.name', 'adw-test');
  git(primary, 'config', 'user.email', 'adw-test@example.invalid');
  mkdirSync(join(primary, 'app'));
  writeFileSync(join(primary, '.gitignore'), 'agents/\n');
  writeFileSync(join(primary, 'README.md'), 'seed\n');
  writeFileSync(join(primary, 'app', 'app.txt'), 'app\n');
  git(primary, 'add', '-A');
  git(primary, 'commit', '--quiet', '-m', 'seed');
  git(primary, 'push', '--quiet', '-u', 'origin', 'main');
});

afterEach(() => {
  setProjectRoot(null);
  if (savedGitEnv.global === undefined) delete process.env['GIT_CONFIG_GLOBAL'];
  else process.env['GIT_CONFIG_GLOBAL'] = savedGitEnv.global;
  if (savedGitEnv.system === undefined) delete process.env['GIT_CONFIG_SYSTEM'];
  else process.env['GIT_CONFIG_SYSTEM'] = savedGitEnv.system;
  rmSync(tmp, { recursive: true, force: true });
});

describe('parseWorktreePorcelain', () => {
  it('parses attached, detached, locked, prunable, and spaced paths', () => {
    const parsed = parseWorktreePorcelain(
      'worktree /tmp/primary checkout\nHEAD aaaa\nbranch refs/heads/main\n\n' +
        'worktree /tmp/lane ü\nHEAD bbbb\ndetached\nlocked switchyard:a1b2c3d4\nprunable gitdir file points to non-existent location\n\n',
    );
    expect(parsed).toEqual([
      expect.objectContaining({ path: '/tmp/primary checkout', head: 'aaaa', branch: 'main', detached: false }),
      expect.objectContaining({
        path: '/tmp/lane ü',
        head: 'bbbb',
        branch: null,
        detached: true,
        locked: true,
        lockReason: 'switchyard:a1b2c3d4',
        prunable: true,
      }),
    ]);
  });
});

describe('WorktreeManager against a real shared Git repository', () => {
  it('allocates an isolated monorepo lane without touching a dirty primary checkout', () => {
    const sourceProject = join(primary, 'app');
    writeFileSync(join(primary, 'primary-only.txt'), 'untracked dirt\n');
    const branchBefore = git(primary, 'branch', '--show-current').trim();
    const statusBefore = git(primary, 'status', '--porcelain');
    const manager = new WorktreeManager(sourceProject, { managedRoot });
    const allocated = manager.allocateDetached('a1b2c3d4', 'main', 'generation-one');

    expect(allocated.context.projectRoot).toBe(join(allocated.context.worktreeRoot, 'app'));
    expect(git(primary, 'branch', '--show-current').trim()).toBe(branchBefore);
    expect(git(primary, 'status', '--porcelain')).toBe(statusBefore);

    const head = manager.attachBranch(allocated.context, 'feat/1-a1b2c3d4-managed');
    manager.prepareArtifactRoot(allocated.context);
    writeFileSync(join(allocated.context.artifactRoot, 'commit_message.txt'), 'feat: managed\n');
    expect(git(allocated.context.worktreeRoot, 'status', '--porcelain')).toBe('');

    // Even if another tool force-stages an ignored manager artifact, commit
    // staging detects and removes it from the index rather than committing it.
    git(allocated.context.projectRoot, 'add', '-f', 'agents/a1b2c3d4/commit_message.txt');
    setProjectRoot(allocated.context.projectRoot);
    expect(commitAll('must refuse', false, 'agents/a1b2c3d4').error).toMatch(/artifact root entered the Git index/);
    expect(git(allocated.context.projectRoot, 'diff', '--cached', '--name-only')).toBe('');

    const record = recordFor(manager, {
      adwId: 'a1b2c3d4',
      generationId: allocated.generationId,
      branch: 'feat/1-a1b2c3d4-managed',
      allocationOid: allocated.allocationOid,
      expectedHeadOid: head,
    });
    expect(manager.validateRecord(record)).toEqual(expect.objectContaining({ branch: record.branch, clean: true }));
    expect(manager.listGitWorktrees().find((entry) => entry.path === record.worktreePath)).toEqual(
      expect.objectContaining({ locked: true, lockReason: 'switchyard:a1b2c3d4' }),
    );

    manager.remove(record, { removeArtifacts: true });
    expect(manager.listGitWorktrees().some((entry) => entry.path === record.worktreePath)).toBe(false);
    expect(() => git(primary, 'show-ref', '--verify', `refs/heads/${record.branch}`)).toThrow();
    expect(git(primary, 'branch', '--show-current').trim()).toBe('main');
  });

  it('rejects detached/wrong-head resume and a foreign branch collision', () => {
    const manager = new WorktreeManager(primary, { managedRoot });
    const first = manager.allocateDetached('11111111', 'main', 'generation-one');
    const head = manager.attachBranch(first.context, 'feat/one-11111111-owned');
    manager.prepareArtifactRoot(first.context);
    const record = recordFor(manager, {
      adwId: '11111111',
      generationId: first.generationId,
      branch: 'feat/one-11111111-owned',
      allocationOid: first.allocationOid,
      expectedHeadOid: head,
    });
    git(first.context.worktreeRoot, 'switch', '--detach', '--quiet');
    expect(() => manager.validateRecord(record)).toThrow(/unexpectedly detached|expected branch/);

    const second = manager.allocateDetached('22222222', 'main', 'generation-two');
    expect(() => manager.attachBranch(second.context, 'feat/one-11111111-owned')).toThrow(/collision|already checked out/);
  });

  it('rejects managed roots inside the source and symlink substitution before cleanup', () => {
    expect(() => new WorktreeManager(primary, { managedRoot: join(primary, 'lanes') })).toThrow(
      /must be outside the source checkout/,
    );
    const manager = new WorktreeManager(primary, { managedRoot });
    const allocated = manager.allocateDetached('33333333', 'main', 'generation-three');
    const head = manager.attachBranch(allocated.context, 'feat/three-33333333-owned');
    const record = recordFor(manager, {
      adwId: '33333333',
      generationId: allocated.generationId,
      branch: 'feat/three-33333333-owned',
      allocationOid: allocated.allocationOid,
      expectedHeadOid: head,
    });
    rmSync(record.worktreePath, { recursive: true });
    symlinkSync(primary, record.worktreePath, 'dir');
    expect(() => manager.remove(record)).toThrow(/symlinked managed worktree path/);
    expect(git(primary, 'branch', '--show-current').trim()).toBe('main');
  });
});

describe('RunRegistry', () => {
  it('persists atomically, fails closed on corrupt JSON, and enforces one lease holder', () => {
    const manager = new WorktreeManager(primary, { managedRoot });
    const allocated = manager.allocateDetached('abcdef12', 'main', 'generation-one');
    const registry = new RunRegistry(manager.layout.gitCommonDir);
    const record = recordFor(manager, {
      adwId: 'abcdef12',
      generationId: allocated.generationId,
      branch: null,
      allocationOid: allocated.allocationOid,
      expectedHeadOid: allocated.allocationOid,
    });
    registry.create(record);
    expect(registry.read(record.adwId)).toEqual(record);
    expect(registry.list().map((item) => item.adwId)).toEqual(['abcdef12']);

    const lease = registry.acquireRunLease(record.adwId);
    expect(() => registry.acquireRunLease(record.adwId)).toThrow(/already held/);
    lease.release();
    expect(() => registry.acquireRunLease(record.adwId).release()).not.toThrow();

    writeFileSync(registry.recordPath(record.adwId), '{not-json', 'utf8');
    expect(() => registry.read(record.adwId)).toThrow(/corrupt managed run record/);
    expect(readFileSync(registry.recordPath(record.adwId), 'utf8')).toBe('{not-json');
  });

  it('keeps durable state separate from worktree-local authored artifacts and rejects corruption', () => {
    const controlRoot = join(tmp, 'durable', '1234abcd');
    const artifactRoot = join(tmp, 'lane', 'agents', '1234abcd');
    const state = new AdwState({
      adwId: '1234abcd',
      controlRoot,
      artifactRoot,
      strictPersistence: true,
    });
    state.markDone('plan');
    state.save();
    expect(state.workspace()).toBe(controlRoot);
    expect(state.artifactWorkspace()).toBe(artifactRoot);
    expect(AdwState.loadManaged('1234abcd', { controlRoot, artifactRoot })?.isDone('plan')).toBe(true);

    writeFileSync(join(controlRoot, 'state.json'), '{broken', 'utf8');
    expect(() => AdwState.loadManaged('1234abcd', { controlRoot, artifactRoot })).toThrow(
      /corrupt managed run state/,
    );
  });
});
