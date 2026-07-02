/**
 * Orchestrator-owned git/GitHub operations, ported from adw/_git.py.
 *
 * Under the phased model the coding agent never touches git/gh; the TS
 * control plane performs all branch/commit/push/PR/CI-watch/merge work here,
 * in the parent process whose environment may legitimately hold GH_TOKEN
 * (PLAN.md Section 3.3). No runner is granted git/gh tools, and none receives
 * GH_TOKEN in phased mode, so any git/gh the agent's shell could still invoke
 * fails closed.
 *
 * Every mutating operation honours dryRun by printing the command instead of
 * running it.
 */

import { capture, ghJson, note } from './exec.js';

/** Mirrors the Python (ok, error) tuples. */
export interface GitResult {
  ok: boolean;
  error: string | null;
}

export interface FailingJob {
  name: string;
  logExcerpt: string;
}

export type CiState = 'success' | 'failure' | 'pending' | 'none' | 'unknown';

export interface CiStatus {
  state: CiState;
  failingJobs: FailingJob[];
}

// Treat these PR-check conclusions/states as red (adw/_git.py:19-21).
const FAIL_CONCLUSIONS = new Set(['FAILURE', 'CANCELLED', 'TIMED_OUT', 'ACTION_REQUIRED', 'STARTUP_FAILURE']);
const FAIL_STATES = new Set(['FAILURE', 'ERROR']);
const PENDING_STATES = new Set(['PENDING', 'EXPECTED']);

/** Print a command under dry-run and return a success result; else null. */
function emit(cmd: readonly string[], dryRun: boolean): GitResult | null {
  if (dryRun) {
    console.log(`[dry-run] ${cmd.join(' ')}`);
    return { ok: true, error: null };
  }
  note(cmd.join(' '));
  return null;
}

/** Fetch origin and switch to `branch`, creating it from origin/<base>. */
export function createOrCheckoutBranch(branch: string, base: string, dryRun = false): GitResult {
  if (dryRun) {
    console.log('[dry-run] git fetch origin --quiet');
    console.log(`[dry-run] git switch -c ${branch} origin/${base}`);
    return { ok: true, error: null };
  }

  capture(['git', 'fetch', 'origin', '--quiet']); // best effort; offline is tolerable
  const existsLocally =
    capture(['git', 'show-ref', '--verify', '--quiet', `refs/heads/${branch}`]).returncode === 0;
  const cmd = existsLocally
    ? ['git', 'switch', branch]
    : ['git', 'switch', '-c', branch, `origin/${base}`];

  note(cmd.join(' '));
  const result = capture(cmd);
  if (result.returncode !== 0) {
    return { ok: false, error: result.stderr.trim() || 'git switch failed' };
  }
  return { ok: true, error: null };
}

export interface SyncWithBaseResult extends GitResult {
  /** True when the branch was actually behind origin/<base> and got rebased. */
  rebased: boolean;
  /**
   * True when origin/<branch> exists but is not an ancestor of HEAD — a prior
   * process rebased (or the remote holds a stale same-named branch) and the
   * next push must carry force to land at all.
   */
  forcePushNeeded?: boolean;
  /** Which step failed when ok=false; a fetch failure is retryable in place. */
  stage?: 'fetch' | 'rebase';
}

const SYNC_FETCH_ATTEMPTS = 3;

/**
 * Bring the current branch up to date with origin/<base> before merging.
 *
 * The branch is cut from origin/<base> once at setup and never updated, so
 * when other runs merge while this one is in flight (worktree-per-run
 * batches), its gates and CI verdicts were computed against a base that no
 * longer exists. Rebasing is a no-op when the base has not moved; a
 * conflicted rebase is aborted and reported — it cannot be healed here and
 * needs a fresh run cut from the moved base. A fetch that keeps failing is
 * reported as stage:'fetch' — on the merge path "cannot prove currency" must
 * fail loudly, never silently pass as "current" (parallel lanes contend on
 * the shared .git, so single fetch attempts do fail in practice).
 */
export function syncWithBase(base: string, dryRun = false): SyncWithBaseResult {
  if (dryRun) {
    console.log('[dry-run] git fetch origin --quiet');
    console.log(`[dry-run] git rebase --empty=drop origin/${base}`);
    return { ok: true, rebased: false, forcePushNeeded: false, error: null };
  }

  let fetchError = '';
  let fetched = false;
  for (let i = 0; i < SYNC_FETCH_ATTEMPTS; i += 1) {
    const fetch = capture(['git', 'fetch', 'origin', '--quiet']);
    if (fetch.returncode === 0) {
      fetched = true;
      break;
    }
    fetchError = fetch.stderr.trim() || 'git fetch failed';
    if (i < SYNC_FETCH_ATTEMPTS - 1) {
      capture(['sleep', String(i + 1)]); // outlast ref-lock contention from sibling lanes
    }
  }
  if (!fetched) {
    return { ok: false, rebased: false, stage: 'fetch', error: `git fetch origin failed: ${fetchError}` };
  }
  const behind = capture(['git', 'rev-list', '--count', `HEAD..origin/${base}`]);
  if (behind.returncode !== 0) {
    return {
      ok: false,
      rebased: false,
      stage: 'fetch',
      error: `behind-probe failed: ${behind.stderr.trim() || 'git rev-list failed'}`,
    };
  }

  let rebased = false;
  if (Number.parseInt(behind.stdout.trim(), 10) > 0) {
    // --empty=drop: a lane that duplicated work already upstream should shrink,
    // not halt the rebase on a commit that became empty.
    const cmd = ['git', 'rebase', '--empty=drop', `origin/${base}`];
    note(cmd.join(' '));
    const result = capture(cmd);
    if (result.returncode !== 0) {
      capture(['git', 'rebase', '--abort']); // best effort: leave the tree usable
      return {
        ok: false,
        rebased: false,
        stage: 'rebase',
        error: result.stderr.trim() || result.stdout.trim() || 'git rebase failed',
      };
    }
    rebased = true;
  }

  // Divergence is checked against the remote branch, not this invocation's
  // rebase: a resumed run whose predecessor rebased but died before its
  // force-push must still push with force or it non-fast-forward-fails forever.
  let forcePushNeeded = false;
  const branch = capture(['git', 'branch', '--show-current']).stdout.trim();
  if (branch) {
    const upstream = `refs/remotes/origin/${branch}`;
    const upstreamExists = capture(['git', 'rev-parse', '--verify', '--quiet', upstream]).returncode === 0;
    if (upstreamExists) {
      forcePushNeeded = capture(['git', 'merge-base', '--is-ancestor', upstream, 'HEAD']).returncode !== 0;
    }
  }
  return { ok: true, rebased, forcePushNeeded, error: null };
}

/** Stage all changes and commit `message`; a clean tree is a no-op success. */
export function commitAll(message: string, dryRun = false): GitResult {
  if (!dryRun && !capture(['git', 'status', '--porcelain']).stdout.trim()) {
    return { ok: true, error: null }; // nothing to commit
  }

  for (const cmd of [
    ['git', 'add', '-A'],
    ['git', 'commit', '-m', message],
  ]) {
    const emitted = emit(cmd, dryRun);
    if (emitted !== null) {
      continue;
    }
    const result = capture(cmd);
    if (result.returncode !== 0) {
      return { ok: false, error: result.stderr.trim() || 'git commit failed' };
    }
  }
  return { ok: true, error: null };
}

/**
 * Push `branch` to origin, setting upstream. `force` uses --force-with-lease
 * for the post-rebase push. Note the lease is thin here: syncWithBase fetches
 * immediately before, so the remote-tracking ref the lease compares against
 * is already refreshed — the real guard is that each branch has exactly one
 * writer (the adw-id is embedded in the branch name).
 */
export function push(branch: string, force = false, dryRun = false): GitResult {
  const cmd = force
    ? ['git', 'push', '--force-with-lease', '-u', 'origin', branch]
    : ['git', 'push', '-u', 'origin', branch];
  const emitted = emit(cmd, dryRun);
  if (emitted !== null) {
    return emitted;
  }
  const result = capture(cmd);
  if (result.returncode !== 0) {
    return { ok: false, error: result.stderr.trim() || 'git push failed' };
  }
  return { ok: true, error: null };
}

/** Switch back to `base` and rebase-pull it. */
export function pullRebase(base: string, dryRun = false): GitResult {
  for (const cmd of [
    ['git', 'switch', base],
    ['git', 'pull', '--rebase', 'origin', base],
  ]) {
    const emitted = emit(cmd, dryRun);
    if (emitted !== null) {
      continue;
    }
    const result = capture(cmd);
    if (result.returncode !== 0) {
      return { ok: false, error: result.stderr.trim() || 'git pull --rebase failed' };
    }
  }
  return { ok: true, error: null };
}

/** Return the URL of an existing open PR for `branch`, or null. */
export function prForBranch(branch: string, ghBin: string, repo: string): string | null {
  const args = [ghBin, 'pr', 'list', '--head', branch, '--json', 'url', '--state', 'open'];
  if (repo) {
    args.push('--repo', repo);
  }
  const prs = ghJson(args);
  if (Array.isArray(prs) && prs.length > 0) {
    const url = (prs[0] as Record<string, unknown>)['url'];
    return typeof url === 'string' ? url : null;
  }
  return null;
}

export interface CreatePrResult {
  number: number | null;
  url: string | null;
  error: string | null;
}

/** Open a PR for `branch`; return its number/url or an error. */
export function createPr(
  branch: string,
  title: string,
  body: string,
  base: string,
  ghBin: string,
  repo: string,
  dryRun = false,
): CreatePrResult {
  const args = [ghBin, 'pr', 'create', '--base', base, '--head', branch, '--title', title, '--body', body];
  if (repo) {
    args.push('--repo', repo);
  }
  if (dryRun) {
    console.log(`[dry-run] ${args.join(' ')}`);
    return { number: null, url: null, error: null };
  }
  note(`${args.slice(0, 6).join(' ')} …`);
  const result = capture(args);
  if (result.returncode !== 0) {
    return { number: null, url: null, error: result.stderr.trim() || 'gh pr create failed' };
  }
  const out = result.stdout.trim();
  const lines = out ? out.split('\n') : [];
  const url = lines.length > 0 ? (lines[lines.length - 1] ?? '') : '';
  return { number: prNumberFromUrl(url), url: url || null, error: null };
}

/**
 * Return {state, failingJobs} for a PR's checks (adw/_git.py:142-180).
 *
 * `none` means the query succeeded but the PR has no checks (yet), distinct
 * from `unknown` (the gh query itself failed / was unparseable); callers
 * settle on `none` before concluding there is nothing to gate on.
 * logExcerpt stays empty — fetching per-job logs is out of scope and would
 * risk leaking secrets.
 */
export function ciStatus(pr: number | string, ghBin: string, repo: string): CiStatus {
  const args = [ghBin, 'pr', 'view', String(pr), '--json', 'statusCheckRollup'];
  if (repo) {
    args.push('--repo', repo);
  }
  const data = ghJson(args);
  if (typeof data !== 'object' || data === null || Array.isArray(data)) {
    return { state: 'unknown', failingJobs: [] };
  }

  const rollup = (data as Record<string, unknown>)['statusCheckRollup'];
  if (!Array.isArray(rollup) || rollup.length === 0) {
    return { state: 'none', failingJobs: [] };
  }

  const failing: FailingJob[] = [];
  let pending = false;
  for (const entry of rollup) {
    const check = (typeof entry === 'object' && entry !== null ? entry : {}) as Record<string, unknown>;
    const name = String(check['name'] || check['context'] || 'check');
    const status = String(check['status'] ?? '').toUpperCase(); // CheckRun: QUEUED/IN_PROGRESS/COMPLETED
    const conclusion = String(check['conclusion'] ?? '').toUpperCase(); // CheckRun
    const state = String(check['state'] ?? '').toUpperCase(); // StatusContext
    if (FAIL_CONCLUSIONS.has(conclusion) || FAIL_STATES.has(state)) {
      failing.push({ name, logExcerpt: '' });
    } else if ((status && status !== 'COMPLETED') || PENDING_STATES.has(state)) {
      pending = true;
    }
  }

  if (failing.length > 0) {
    return { state: 'failure', failingJobs: failing };
  }
  if (pending) {
    return { state: 'pending', failingJobs: [] };
  }
  return { state: 'success', failingJobs: [] };
}

/** Hard cap for CI log excerpts fed to the fix agent (prompt-budget bound). */
export const CI_LOG_EXCERPT_MAX_CHARS = 4000;

const ANSI_ESCAPES = /\u001b\[[0-9;]*m/g;
// `\]` covers GitHub `##[error]` annotations; `\s+[a-z]+\d+:` covers
// compiler-code verdicts like `error TS2304:` (the /i flag widens the class).
const ERROR_SHAPED_LINE = /error(\[|:|\]|\s+[a-z]+\d+:)|FAILED|panicked at|warning:.*-D warnings|Diff in|assertion/i;
// Verdicts print last: always keep the raw tail, filtered or not, so one
// matching noise line ("npm ERR! Failed at…") can never crowd out the real
// verdict that happened to be phrased in words the shapes miss.
const RAW_TAIL_LINES = 40;

/**
 * A bounded, error-focused excerpt of the failing CI run's logs for `pr`'s
 * head branch, or '' when anything is unavailable (best effort, never throws
 * through gh failures). The verdict lines a CI failure carries often cannot be
 * reproduced locally (toolchain skew between the runner image and this
 * machine was observed live: CI clippy fired lints local clippy does not), so
 * a fix agent given only check NAMES has nothing actionable to work from.
 *
 * Secret hygiene: callers must feed this ONLY to the local fix agent's prompt
 * (persisted to the gitignored agents/ workspace), never into issue/PR
 * comments — CI logs can echo secrets on private repos.
 */
export function failingCiLogExcerpt(
  pr: number | string,
  ghBin: string,
  repo: string,
  maxChars = CI_LOG_EXCERPT_MAX_CHARS,
): string {
  const prArgs = [ghBin, 'pr', 'view', String(pr), '--json', 'headRefName'];
  if (repo) {
    prArgs.push('--repo', repo);
  }
  const prData = ghJson(prArgs);
  const branch =
    typeof prData === 'object' && prData !== null && !Array.isArray(prData)
      ? String((prData as Record<string, unknown>)['headRefName'] ?? '')
      : '';
  if (!branch) {
    return '';
  }

  const runArgs = [ghBin, 'run', 'list', '--branch', branch, '--limit', '1', '--json', 'databaseId'];
  if (repo) {
    runArgs.push('--repo', repo);
  }
  const runs = ghJson(runArgs);
  const runId =
    Array.isArray(runs) && runs.length > 0
      ? String((runs[0] as Record<string, unknown>)['databaseId'] ?? '')
      : '';
  if (!runId) {
    return '';
  }

  const logArgs = [ghBin, 'run', 'view', runId, '--log-failed'];
  if (repo) {
    logArgs.push('--repo', repo);
  }
  const log = capture(logArgs);
  if (log.returncode !== 0 || !log.stdout.trim()) {
    return '';
  }

  const lines = log.stdout.replace(ANSI_ESCAPES, '').split('\n');
  const text = lines
    .filter((line, i) => i >= lines.length - RAW_TAIL_LINES || ERROR_SHAPED_LINE.test(line))
    .join('\n')
    .trim();
  return text.length > maxChars ? `…${text.slice(-maxChars)}` : text;
}

/** Squash-merge `pr` and delete its branch. */
export function squashMerge(pr: number | string, ghBin: string, repo: string, dryRun = false): GitResult {
  const args = [ghBin, 'pr', 'merge', String(pr), '--squash', '--delete-branch'];
  if (repo) {
    args.push('--repo', repo);
  }
  const emitted = emit(args, dryRun);
  if (emitted !== null) {
    return emitted;
  }
  const result = capture(args);
  if (result.returncode !== 0) {
    return { ok: false, error: result.stderr.trim() || 'gh pr merge failed' };
  }
  return { ok: true, error: null };
}

/** Extract the trailing PR number from a GitHub PR URL. */
export function prNumberFromUrl(url: string): number | null {
  if (!url) {
    return null;
  }
  const parts = url.replace(/\/+$/, '').split('/');
  const tail = parts[parts.length - 1] ?? '';
  return /^\d+$/.test(tail) ? Number.parseInt(tail, 10) : null;
}
