/** Immutable filesystem and repository identity for one ADW invocation. */

import { createHash } from 'node:crypto';
import { existsSync, lstatSync, realpathSync, statSync } from 'node:fs';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { spawnSync } from 'node:child_process';

import { REPO_ROOT } from './common.js';
import { AdwError } from './errors.js';
import { validateAdwId } from './state.js';

export type RunMode = 'primary' | 'external' | 'managed';

export interface RunContext {
  packageRoot: string;
  sourceRoot: string;
  worktreeRoot: string;
  projectRoot: string;
  stateRoot: string;
  artifactRoot: string;
  gitCommonDir: string;
  mode: RunMode;
}

export interface RepositoryLayout {
  sourceRoot: string;
  sourceWorktreeRoot: string;
  gitCommonDir: string;
  projectRelativePath: string;
  repositoryId: string;
}

function existingDirectory(path: string, label: string): string {
  let canonical: string;
  try {
    canonical = realpathSync(path);
  } catch (error) {
    throw new AdwError(`${label} does not exist: ${path}`, { cause: error });
  }
  if (!statSync(canonical).isDirectory()) {
    throw new AdwError(`${label} is not a directory: ${canonical}`);
  }
  return canonical;
}

function gitValue(cwd: string, args: readonly string[], label: string): string {
  const result = spawnSync('git', ['-C', cwd, ...args], { encoding: 'utf8' });
  if (result.error || result.status !== 0) {
    const detail = (result.stderr ?? '').trim() || String(result.error ?? 'git command failed');
    throw new AdwError(`${label}: ${detail}`);
  }
  const value = (result.stdout ?? '').trim();
  if (!value) {
    throw new AdwError(`${label}: git returned an empty path`);
  }
  return value;
}

function isContained(parent: string, candidate: string): boolean {
  const rel = relative(parent, candidate);
  return rel === '' || (rel !== '..' && !rel.startsWith(`..${sep}`) && !isAbsolute(rel));
}

/** Discover the selected project, enclosing worktree, and canonical common Git directory. */
export function discoverRepository(sourceRoot: string): RepositoryLayout {
  const source = existingDirectory(resolve(sourceRoot), 'source project root');
  const topRaw = gitValue(source, ['rev-parse', '--show-toplevel'], 'source is not a Git worktree');
  const sourceWorktreeRoot = existingDirectory(resolve(source, topRaw), 'source Git top-level');
  if (!isContained(sourceWorktreeRoot, source)) {
    throw new AdwError(`source project root escapes its Git worktree: ${source}`);
  }

  let commonRaw: string;
  const absolute = spawnSync(
    'git',
    ['-C', source, 'rev-parse', '--path-format=absolute', '--git-common-dir'],
    { encoding: 'utf8' },
  );
  if (!absolute.error && absolute.status === 0 && (absolute.stdout ?? '').trim()) {
    commonRaw = (absolute.stdout ?? '').trim();
  } else {
    commonRaw = gitValue(source, ['rev-parse', '--git-common-dir'], 'could not resolve Git common directory');
  }
  const gitCommonDir = existingDirectory(
    isAbsolute(commonRaw) ? commonRaw : resolve(sourceWorktreeRoot, commonRaw),
    'Git common directory',
  );
  const projectRelativePath = relative(sourceWorktreeRoot, source);
  const repositoryId = createHash('sha256').update(gitCommonDir).digest('hex').slice(0, 16);
  return { sourceRoot: source, sourceWorktreeRoot, gitCommonDir, projectRelativePath, repositoryId };
}

/** Default managed-worktree parent: a repository-keyed sibling of the source checkout. */
export function defaultManagedWorktreeRoot(layout: RepositoryLayout): string {
  return join(
    dirname(layout.sourceWorktreeRoot),
    '.switchyard-worktrees',
    `${basename(layout.sourceWorktreeRoot)}-${layout.repositoryId}`,
  );
}

export function managedControlRoot(gitCommonDir: string): string {
  return join(gitCommonDir, 'switchyard');
}

/** Build the exact paths for an allocated managed lane. */
export function createManagedRunContext(options: {
  layout: RepositoryLayout;
  adwId: string;
  managedRoot?: string;
  worktreeRoot?: string;
}): RunContext {
  const adwId = validateAdwId(options.adwId);
  const managedRoot = resolve(options.managedRoot ?? defaultManagedWorktreeRoot(options.layout));
  const worktreeRoot = resolve(options.worktreeRoot ?? join(managedRoot, adwId));
  if (!isContained(managedRoot, worktreeRoot) || worktreeRoot === managedRoot) {
    throw new AdwError(`managed worktree path escapes its configured parent: ${worktreeRoot}`);
  }
  const projectRoot = join(worktreeRoot, options.layout.projectRelativePath);
  return Object.freeze({
    packageRoot: REPO_ROOT,
    sourceRoot: options.layout.sourceRoot,
    worktreeRoot,
    projectRoot,
    stateRoot: join(managedControlRoot(options.layout.gitCommonDir), 'runs', adwId),
    artifactRoot: join(projectRoot, 'agents', adwId),
    gitCommonDir: options.layout.gitCommonDir,
    mode: 'managed' as const,
  });
}

/** Canonical containment check used before cleanup of manager-owned paths. */
export function assertManagedPath(parent: string, candidate: string): void {
  const canonicalParent = existingDirectory(resolve(parent), 'managed worktree parent');
  const resolvedCandidate = resolve(candidate);
  if (resolvedCandidate === canonicalParent || !isContained(canonicalParent, resolvedCandidate)) {
    throw new AdwError(`refusing path outside managed worktree parent: ${resolvedCandidate}`);
  }
  if (existsSync(resolvedCandidate)) {
    if (lstatSync(resolvedCandidate).isSymbolicLink()) {
      throw new AdwError(`refusing symlinked managed worktree path: ${resolvedCandidate}`);
    }
    const canonicalCandidate = realpathSync(resolvedCandidate);
    if (!isContained(canonicalParent, canonicalCandidate) || canonicalCandidate === canonicalParent) {
      throw new AdwError(`managed worktree resolves outside its configured parent: ${resolvedCandidate}`);
    }
  }
}
