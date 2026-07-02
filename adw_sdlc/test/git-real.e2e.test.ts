/**
 * Real-git coverage for the merge tail (design-review finding: it was
 * mock-only). Exercises createOrCheckoutBranch → commitAll → syncWithBase
 * (clean / conflicted / diverged) → push against actual local repositories:
 * a bare `origin` plus two clones — the "lane" under test and a "sibling"
 * that moves main underneath it, exactly like a concurrent batch lane.
 * gh is never touched; the PR/merge half stays covered by the mocked tests.
 */

import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { setProjectRoot } from '../src/common.js';
import { commitAll, createOrCheckoutBranch, push, syncWithBase } from '../src/git.js';

let tmp: string;
let origin: string;
let lane: string;
let sibling: string;
let savedGitEnv: { global: string | undefined; system: string | undefined };

function git(cwd: string, ...args: string[]): string {
  return execFileSync('git', args, { cwd, encoding: 'utf8' });
}

/** Pin the committer identity locally so the suite is machine-independent. */
function identify(repo: string): void {
  git(repo, 'config', 'user.name', 'adw-test');
  git(repo, 'config', 'user.email', 'adw-test@example.invalid');
}

/** Commit `content` to `file` on the sibling's main and push — main moves. */
function moveMain(file: string, content: string, message: string): void {
  git(sibling, 'pull', '--rebase', '--quiet', 'origin', 'main');
  writeFileSync(join(sibling, file), content);
  git(sibling, 'add', '-A');
  git(sibling, 'commit', '--quiet', '-m', message);
  git(sibling, 'push', '--quiet', 'origin', 'main');
}

beforeEach(() => {
  // Hermetic against the machine's git config: init.templateDir hooks are
  // copied at repo-creation time (a global gitleaks pre-commit hook was
  // observed running inside these temp repos), and commit.gpgsign or
  // init.defaultBranch would break commits/refs on other machines. Nulling
  // the global/system config BEFORE any init/clone removes all three; the
  // src/git.ts child processes inherit process.env, so they are covered too.
  savedGitEnv = { global: process.env['GIT_CONFIG_GLOBAL'], system: process.env['GIT_CONFIG_SYSTEM'] };
  process.env['GIT_CONFIG_GLOBAL'] = '/dev/null';
  process.env['GIT_CONFIG_SYSTEM'] = '/dev/null';
  tmp = mkdtempSync(join(tmpdir(), 'adw-git-real-'));
  origin = join(tmp, 'origin.git');
  lane = join(tmp, 'lane');
  sibling = join(tmp, 'sibling');

  git(tmp, 'init', '--bare', '--initial-branch=main', origin);
  git(tmp, 'clone', '--quiet', origin, lane);
  identify(lane);
  writeFileSync(join(lane, 'README.md'), 'line one\nline two\nline three\n');
  git(lane, 'add', '-A');
  git(lane, 'commit', '--quiet', '-m', 'seed');
  git(lane, 'push', '--quiet', '-u', 'origin', 'main');
  git(tmp, 'clone', '--quiet', origin, sibling);
  identify(sibling);

  // git.ts commands run with cwd = the project root override, like a live run.
  setProjectRoot(lane);
});

afterEach(() => {
  if (savedGitEnv.global === undefined) {
    delete process.env['GIT_CONFIG_GLOBAL'];
  } else {
    process.env['GIT_CONFIG_GLOBAL'] = savedGitEnv.global;
  }
  if (savedGitEnv.system === undefined) {
    delete process.env['GIT_CONFIG_SYSTEM'];
  } else {
    process.env['GIT_CONFIG_SYSTEM'] = savedGitEnv.system;
  }
  setProjectRoot(null);
  rmSync(tmp, { recursive: true, force: true });
});

describe('merge tail against real repositories', () => {
  it('cuts a branch from origin/<base>, commits, and pushes it', () => {
    expect(createOrCheckoutBranch('feat/1-abc-x', 'main').ok).toBe(true);
    writeFileSync(join(lane, 'new.txt'), 'work\n');
    expect(commitAll('feat: add work').ok).toBe(true);
    expect(push('feat/1-abc-x').ok).toBe(true);
    expect(git(origin, 'show-ref', 'refs/heads/feat/1-abc-x')).toContain('feat/1-abc-x');
  });

  it('reports not-behind (and no force needed) when the base has not moved', () => {
    createOrCheckoutBranch('feat/1-abc-x', 'main');
    writeFileSync(join(lane, 'new.txt'), 'work\n');
    commitAll('feat: add work');
    expect(syncWithBase('main')).toEqual({ ok: true, rebased: false, forcePushNeeded: false, error: null });
  });

  it('rebases over a moved base; the pushed branch then needs (and survives) a lease force-push', () => {
    createOrCheckoutBranch('feat/1-abc-x', 'main');
    writeFileSync(join(lane, 'new.txt'), 'work\n');
    commitAll('feat: add work');
    expect(push('feat/1-abc-x').ok).toBe(true); // pre-rebase history is on origin

    moveMain('other.txt', 'sibling work\n', 'sibling: lands first');

    const synced = syncWithBase('main');
    expect(synced.ok).toBe(true);
    expect(synced.rebased).toBe(true);
    expect(synced.forcePushNeeded).toBe(true); // origin/<branch> holds the pre-rebase commits

    expect(push('feat/1-abc-x').ok).toBe(false); // plain push must be rejected (non-fast-forward)
    expect(push('feat/1-abc-x', true).ok).toBe(true); // --force-with-lease lands it

    // Settled: current with base, remote is an ancestor again.
    expect(syncWithBase('main')).toEqual({ ok: true, rebased: false, forcePushNeeded: false, error: null });
    // The rebased branch contains the sibling's commit — gates/CI now validate
    // against the base the merge will actually land on.
    expect(git(lane, 'log', '--oneline')).toContain('sibling: lands first');
  });

  it('detects divergence on a later invocation even without a fresh rebase (resume after a dead force-push)', () => {
    createOrCheckoutBranch('feat/1-abc-x', 'main');
    writeFileSync(join(lane, 'new.txt'), 'work\n');
    commitAll('feat: add work');
    push('feat/1-abc-x');
    moveMain('other.txt', 'sibling work\n', 'sibling: lands first');

    expect(syncWithBase('main').rebased).toBe(true); // rebase happened, force-push "died" (never ran)
    const resumed = syncWithBase('main'); // next attempt: not behind, but diverged
    expect(resumed.rebased).toBe(false);
    expect(resumed.forcePushNeeded).toBe(true);
    expect(push('feat/1-abc-x', true).ok).toBe(true);
  });

  it('aborts a conflicted rebase, leaving the branch tip and a clean tree for salvage', () => {
    createOrCheckoutBranch('feat/1-abc-x', 'main');
    writeFileSync(join(lane, 'README.md'), 'line one CHANGED BY LANE\nline two\nline three\n');
    commitAll('feat: lane edit');
    const tipBefore = git(lane, 'rev-parse', 'HEAD').trim();

    moveMain('README.md', 'line one CHANGED BY SIBLING\nline two\nline three\n', 'sibling: conflicting edit');

    const synced = syncWithBase('main');
    expect(synced.ok).toBe(false);
    expect(synced.stage).toBe('rebase');
    expect(synced.error).toMatch(/conflict|could not apply/i);
    // The abort left everything salvageable: same tip, clean tree, no rebase in progress.
    expect(git(lane, 'rev-parse', 'HEAD').trim()).toBe(tipBefore);
    expect(git(lane, 'status', '--porcelain').trim()).toBe('');
  });
});
