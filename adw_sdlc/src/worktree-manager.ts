/** Git-specific provisioning and ownership validation for managed worktrees. */

import { randomUUID } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, realpathSync, rmSync } from 'node:fs';
import { isAbsolute, join, relative, resolve, sep } from 'node:path';
import { spawnSync } from 'node:child_process';

import { AdwError } from './errors.js';
import {
  assertManagedPath,
  createManagedRunContext,
  defaultManagedWorktreeRoot,
  discoverRepository,
  type RepositoryLayout,
  type RunContext,
} from './run-context.js';
import type { ManagedRunRecord } from './run-registry.js';
import { validateAdwId } from './state.js';

export interface GitWorktreeEntry {
  path: string;
  head: string | null;
  branch: string | null;
  detached: boolean;
  bare: boolean;
  locked: boolean;
  lockReason: string | null;
  prunable: boolean;
  pruneReason: string | null;
}

export interface WorktreeValidation {
  entry: GitWorktreeEntry;
  headOid: string;
  branch: string | null;
  clean: boolean;
  operationInProgress: string | null;
}

interface CommandResult {
  code: number;
  stdout: string;
  stderr: string;
}

function command(cwd: string, args: readonly string[]): CommandResult {
  const result = spawnSync('git', ['-C', cwd, ...args], { encoding: 'utf8' });
  return {
    code: result.error ? 127 : (result.status ?? 127),
    stdout: result.stdout ?? '',
    stderr: result.error ? String(result.error) : (result.stderr ?? ''),
  };
}

function requireGit(cwd: string, args: readonly string[], action: string): string {
  const result = command(cwd, args);
  if (result.code !== 0) {
    throw new AdwError(`${action}: ${result.stderr.trim() || result.stdout.trim() || 'git command failed'}`);
  }
  return result.stdout.trim();
}

function canonicalIfPresent(path: string): string {
  try {
    return realpathSync(path);
  } catch {
    return resolve(path);
  }
}

function contained(parent: string, candidate: string): boolean {
  const rel = relative(parent, candidate);
  return rel !== '' && rel !== '..' && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);
}

/** Parse `git worktree list --porcelain` output without splitting paths on spaces. */
export function parseWorktreePorcelain(output: string): GitWorktreeEntry[] {
  const normalized = output.includes('\0') ? output.replaceAll('\0', '\n') : output;
  const records = normalized.split(/\n\s*\n/);
  const entries: GitWorktreeEntry[] = [];
  for (const record of records) {
    const lines = record.split('\n').filter((line) => line.length > 0);
    const first = lines[0];
    if (!first?.startsWith('worktree ')) continue;
    const entry: GitWorktreeEntry = {
      path: first.slice('worktree '.length),
      head: null,
      branch: null,
      detached: false,
      bare: false,
      locked: false,
      lockReason: null,
      prunable: false,
      pruneReason: null,
    };
    for (const line of lines.slice(1)) {
      const space = line.indexOf(' ');
      const key = space === -1 ? line : line.slice(0, space);
      const value = space === -1 ? '' : line.slice(space + 1);
      switch (key) {
        case 'HEAD': entry.head = value || null; break;
        case 'branch': entry.branch = value.startsWith('refs/heads/') ? value.slice('refs/heads/'.length) : value; break;
        case 'detached': entry.detached = true; break;
        case 'bare': entry.bare = true; break;
        case 'locked': entry.locked = true; entry.lockReason = value || null; break;
        case 'prunable': entry.prunable = true; entry.pruneReason = value || null; break;
      }
    }
    entries.push(entry);
  }
  return entries;
}

export class WorktreeManager {
  readonly layout: RepositoryLayout;
  readonly managedRoot: string;

  constructor(sourceRoot: string, options: { managedRoot?: string } = {}) {
    this.layout = discoverRepository(sourceRoot);
    this.managedRoot = resolve(options.managedRoot ?? defaultManagedWorktreeRoot(this.layout));
    if (this.managedRoot === this.layout.sourceWorktreeRoot || this.managedRoot === resolve(sep)) {
      throw new AdwError(`unsafe managed worktree parent: ${this.managedRoot}`);
    }
    const relativeToSource = relative(this.layout.sourceWorktreeRoot, this.managedRoot);
    if (
      relativeToSource === '' ||
      (relativeToSource !== '..' && !relativeToSource.startsWith(`..${sep}`) && !isAbsolute(relativeToSource))
    ) {
      throw new AdwError(`managed worktree parent must be outside the source checkout: ${this.managedRoot}`);
    }
  }

  context(adwId: string): RunContext {
    return createManagedRunContext({ layout: this.layout, adwId, managedRoot: this.managedRoot });
  }

  listGitWorktrees(): GitWorktreeEntry[] {
    return parseWorktreePorcelain(
      requireGit(this.layout.sourceRoot, ['worktree', 'list', '--porcelain'], 'could not list Git worktrees'),
    );
  }

  /** Fetch the selected base, then allocate a detached, clean linked worktree. */
  allocateDetached(
    adwId: string,
    base: string,
    generationId: string = randomUUID(),
  ): { context: RunContext; generationId: string; allocationOid: string } {
    validateAdwId(adwId);
    if (!base || base.startsWith('-') || /[\u0000-\u001f\u007f]/.test(base)) {
      throw new AdwError(`invalid base branch: ${JSON.stringify(base)}`);
    }
    const context = this.context(adwId);
    mkdirSync(this.managedRoot, { recursive: true });
    assertManagedPath(this.managedRoot, context.worktreeRoot);
    if (existsSync(context.worktreeRoot)) {
      throw new AdwError(`managed worktree path already exists: ${context.worktreeRoot}`);
    }
    requireGit(this.layout.sourceRoot, ['fetch', 'origin', '--quiet', base], `could not fetch origin/${base}`);
    const allocationOid = requireGit(
      this.layout.sourceRoot,
      ['rev-parse', '--verify', `origin/${base}^{commit}`],
      `origin/${base} does not resolve to a commit`,
    );
    requireGit(
      this.layout.sourceRoot,
      ['worktree', 'add', '--detach', context.worktreeRoot, allocationOid],
      'could not add managed worktree',
    );
    const validation = this.validatePath(context.worktreeRoot, { allowDetached: true });
    if (!validation.entry.detached || validation.headOid !== allocationOid || !validation.clean) {
      throw new AdwError(`new managed worktree failed detached/HEAD/clean validation: ${context.worktreeRoot}`);
    }
    if (!existsSync(context.projectRoot) || !lstatSync(context.projectRoot).isDirectory()) {
      throw new AdwError(`managed project path is missing from the linked worktree: ${context.projectRoot}`);
    }
    return { context, generationId, allocationOid };
  }

  /** Attach a fresh, unowned local branch. Existing local or remote refs always collide. */
  attachBranch(context: RunContext, branch: string): string {
    if (!branch || branch.startsWith('-') || /[\u0000-\u001f\u007f]/.test(branch)) {
      throw new AdwError(`invalid managed branch: ${JSON.stringify(branch)}`);
    }
    for (const ref of [`refs/heads/${branch}`, `refs/remotes/origin/${branch}`]) {
      if (command(this.layout.sourceRoot, ['show-ref', '--verify', '--quiet', ref]).code === 0) {
        throw new AdwError(`managed branch collision: ${ref} already exists without matching ownership`);
      }
    }
    for (const entry of this.listGitWorktrees()) {
      if (entry.branch === branch) {
        throw new AdwError(`branch ${branch} is already checked out in worktree ${entry.path}`);
      }
    }
    requireGit(context.projectRoot, ['switch', '-c', branch], `could not attach managed branch ${branch}`);
    const head = requireGit(context.projectRoot, ['rev-parse', 'HEAD'], 'could not resolve managed branch HEAD');
    this.lock(context.worktreeRoot, context.artifactRoot.split(sep).at(-1) ?? 'unknown');
    return head;
  }

  lock(worktreePath: string, adwId: string): void {
    const reason = `switchyard:${validateAdwId(adwId)}`;
    const result = command(this.layout.sourceRoot, ['worktree', 'lock', '--reason', reason, worktreePath]);
    if (result.code !== 0 && !result.stderr.includes('already locked')) {
      throw new AdwError(`could not lock managed worktree: ${result.stderr.trim() || 'git worktree lock failed'}`);
    }
  }

  /** Prove the artifact boundary is contained, untracked, and ignored. */
  prepareArtifactRoot(context: RunContext): void {
    const worktree = canonicalIfPresent(context.worktreeRoot);
    const artifact = resolve(context.artifactRoot);
    if (!contained(worktree, artifact)) {
      throw new AdwError(`managed artifact root escapes its worktree: ${artifact}`);
    }
    mkdirSync(artifact, { recursive: true });
    const targets = [join(artifact, 'commit_message.txt'), join(artifact, 'pr_body.md')];
    for (const target of targets) {
      const rel = relative(context.projectRoot, target);
      if (command(context.projectRoot, ['ls-files', '--error-unmatch', '--', rel]).code === 0) {
        throw new AdwError(`managed artifact path is tracked by Git: ${target}`);
      }
      if (command(context.projectRoot, ['check-ignore', '--no-index', '--quiet', '--', rel]).code !== 0) {
        throw new AdwError(
          `managed artifact path is not ignored: ${target}; add an ignore rule for agents/ before using --worktree`,
        );
      }
    }
  }

  validateRecord(record: ManagedRunRecord, options: { allowDetached?: boolean } = {}): WorktreeValidation {
    if (realpathSync(record.gitCommonDir) !== this.layout.gitCommonDir) {
      throw new AdwError(`managed run ${record.adwId} belongs to a different Git repository`);
    }
    if (resolve(record.managedRoot) !== this.managedRoot) {
      throw new AdwError(`managed run ${record.adwId} has a different configured worktree parent`);
    }
    assertManagedPath(this.managedRoot, record.worktreePath);
    if (realpathSync(record.worktreePath) === this.layout.sourceWorktreeRoot) {
      throw new AdwError(`managed worktree record aliases the source checkout: ${record.worktreePath}`);
    }
    const validation = this.validatePath(record.worktreePath, options);
    if (record.branch !== null && validation.branch !== record.branch) {
      throw new AdwError(
        `managed run ${record.adwId} expected branch ${record.branch}, found ${validation.branch ?? 'detached HEAD'}`,
      );
    }
    if (record.expectedHeadOid !== null && validation.headOid !== record.expectedHeadOid) {
      throw new AdwError(
        `managed run ${record.adwId} HEAD changed outside a recorded transition ` +
          `(${record.expectedHeadOid} -> ${validation.headOid})`,
      );
    }
    const sameBranch = this.listGitWorktrees().filter((entry) => entry.branch === record.branch);
    if (record.branch !== null && sameBranch.some((entry) => canonicalIfPresent(entry.path) !== canonicalIfPresent(record.worktreePath))) {
      const foreign = sameBranch.find((entry) => canonicalIfPresent(entry.path) !== canonicalIfPresent(record.worktreePath));
      throw new AdwError(`branch ${record.branch} is checked out in another worktree: ${foreign?.path}`);
    }
    return validation;
  }

  validatePath(worktreePath: string, options: { allowDetached?: boolean } = {}): WorktreeValidation {
    const expected = canonicalIfPresent(worktreePath);
    const entry = this.listGitWorktrees().find((candidate) => canonicalIfPresent(candidate.path) === expected);
    if (!entry) {
      throw new AdwError(`path is not registered by git worktree list: ${worktreePath}`);
    }
    if (!existsSync(worktreePath)) {
      throw new AdwError(`registered managed worktree directory is missing: ${worktreePath}`);
    }
    const common = requireGit(worktreePath, ['rev-parse', '--path-format=absolute', '--git-common-dir'], 'could not validate worktree repository');
    if (realpathSync(common) !== this.layout.gitCommonDir) {
      throw new AdwError(`worktree common Git directory does not match its registry: ${worktreePath}`);
    }
    const headOid = requireGit(worktreePath, ['rev-parse', 'HEAD'], 'could not resolve worktree HEAD');
    const branchRaw = requireGit(worktreePath, ['branch', '--show-current'], 'could not inspect worktree branch');
    const branch = branchRaw || null;
    if (branch === null && options.allowDetached !== true) {
      throw new AdwError(`managed worktree is unexpectedly detached: ${worktreePath}`);
    }
    const operationInProgress = this.operationInProgress(worktreePath);
    if (operationInProgress !== null) {
      throw new AdwError(`managed worktree has ${operationInProgress} in progress: ${worktreePath}`);
    }
    const clean = command(worktreePath, ['status', '--porcelain', '--untracked-files=all']).stdout.trim() === '';
    return { entry, headOid, branch, clean, operationInProgress };
  }

  operationInProgress(worktreePath: string): string | null {
    const probes: Array<[string, string]> = [
      ['rebase-merge', 'a rebase'],
      ['rebase-apply', 'a rebase'],
      ['MERGE_HEAD', 'a merge'],
      ['CHERRY_PICK_HEAD', 'a cherry-pick'],
      ['REVERT_HEAD', 'a revert'],
    ];
    for (const [gitPath, label] of probes) {
      const path = command(worktreePath, ['rev-parse', '--git-path', gitPath]);
      if (path.code === 0 && existsSync(path.stdout.trim())) return label;
    }
    return null;
  }

  /** Remove only a clean, exactly owned, Switchyard-locked worktree; never force. */
  remove(record: ManagedRunRecord, options: { removeArtifacts?: boolean } = {}): void {
    const validation = this.validateRecord(record, { allowDetached: record.branch === null });
    if (!validation.clean) {
      throw new AdwError(`managed worktree is dirty; refusing removal: ${record.worktreePath}`);
    }
    if (!validation.entry.locked || validation.entry.lockReason !== `switchyard:${record.adwId}`) {
      throw new AdwError(`managed worktree does not have the expected Switchyard lock: ${record.worktreePath}`);
    }
    if (options.removeArtifacts === true && existsSync(record.artifactRoot)) {
      const artifact = canonicalIfPresent(record.artifactRoot);
      const worktree = realpathSync(record.worktreePath);
      if (!contained(worktree, artifact)) {
        throw new AdwError(`refusing to remove artifact path outside owned worktree: ${artifact}`);
      }
      rmSync(artifact, { recursive: true });
    }
    requireGit(this.layout.sourceRoot, ['worktree', 'unlock', record.worktreePath], 'could not unlock managed worktree');
    const removed = command(this.layout.sourceRoot, ['worktree', 'remove', record.worktreePath]);
    if (removed.code !== 0) {
      try {
        this.lock(record.worktreePath, record.adwId);
      } catch {
        // Preserve the primary removal error; reconciliation reports lock loss.
      }
      throw new AdwError(`could not remove managed worktree without force: ${removed.stderr.trim() || removed.stdout.trim()}`);
    }
    if (record.branch !== null) {
      const deleted = command(this.layout.sourceRoot, ['branch', '-D', record.branch]);
      if (deleted.code !== 0) {
        throw new AdwError(`worktree removed, but local branch cleanup failed: ${deleted.stderr.trim()}`);
      }
    }
  }
}
