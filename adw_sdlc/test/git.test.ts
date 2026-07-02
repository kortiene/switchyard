/**
 * Tests for the orchestrator-owned git/gh layer (port of adw/test_git.py).
 * The exec layer is mocked; no real git/gh runs.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../src/exec.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../src/exec.js')>();
  return { ...actual, capture: vi.fn(), ghJson: vi.fn(), note: vi.fn() };
});

import { capture, ghJson } from '../src/exec.js';
import {
  ciStatus,
  commitAll,
  createOrCheckoutBranch,
  createPr,
  failingCiLogExcerpt,
  prForBranch,
  prNumberFromUrl,
  push,
  squashMerge,
  syncWithBase,
} from '../src/git.js';

const captureMock = vi.mocked(capture);
const ghJsonMock = vi.mocked(ghJson);

const ok = { returncode: 0, stdout: '', stderr: '' };
const fail = { returncode: 1, stdout: '', stderr: 'boom' };

beforeEach(() => {
  captureMock.mockReset();
  ghJsonMock.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('createOrCheckoutBranch', () => {
  it('switches to an existing local branch', () => {
    captureMock.mockImplementation((cmd) => {
      if (cmd[1] === 'show-ref') {
        return ok; // branch exists locally
      }
      return ok;
    });
    expect(createOrCheckoutBranch('feat/5-x', 'main')).toEqual({ ok: true, error: null });
    const switchCall = captureMock.mock.calls.find((c) => c[0][1] === 'switch');
    expect(switchCall?.[0]).toEqual(['git', 'switch', 'feat/5-x']);
  });

  it('creates a missing branch from origin/<base>', () => {
    captureMock.mockImplementation((cmd) => (cmd[1] === 'show-ref' ? fail : ok));
    expect(createOrCheckoutBranch('feat/5-x', 'main').ok).toBe(true);
    const switchCall = captureMock.mock.calls.find((c) => c[0][1] === 'switch');
    expect(switchCall?.[0]).toEqual(['git', 'switch', '-c', 'feat/5-x', 'origin/main']);
  });

  it('reports a failed switch', () => {
    captureMock.mockImplementation((cmd) => (cmd[1] === 'switch' ? fail : ok));
    expect(createOrCheckoutBranch('feat/5-x', 'main')).toEqual({ ok: false, error: 'boom' });
  });

  it('prints instead of running under dry-run', () => {
    const log = vi.spyOn(console, 'log').mockImplementation(() => {});
    expect(createOrCheckoutBranch('feat/5-x', 'main', true).ok).toBe(true);
    expect(captureMock).not.toHaveBeenCalled();
    expect(log).toHaveBeenCalledWith('[dry-run] git switch -c feat/5-x origin/main');
  });
});

describe('commitAll', () => {
  it('treats a clean tree as a no-op success', () => {
    captureMock.mockReturnValue({ returncode: 0, stdout: '', stderr: '' });
    expect(commitAll('msg')).toEqual({ ok: true, error: null });
    expect(captureMock).toHaveBeenCalledTimes(1); // only the status probe
  });

  it('stages and commits when the tree is dirty', () => {
    captureMock.mockImplementation((cmd) =>
      cmd[1] === 'status' ? { returncode: 0, stdout: ' M x.rs\n', stderr: '' } : ok,
    );
    expect(commitAll('msg').ok).toBe(true);
    const cmds = captureMock.mock.calls.map((c) => c[0][1]);
    expect(cmds).toContain('add');
    expect(cmds).toContain('commit');
  });

  it('surfaces a commit failure', () => {
    captureMock.mockImplementation((cmd) => {
      if (cmd[1] === 'status') {
        return { returncode: 0, stdout: ' M x.rs\n', stderr: '' };
      }
      return cmd[1] === 'commit' ? fail : ok;
    });
    expect(commitAll('msg')).toEqual({ ok: false, error: 'boom' });
  });
});

describe('push', () => {
  it('pushes with upstream and reports failures', () => {
    captureMock.mockReturnValueOnce(ok);
    expect(push('feat/5-x').ok).toBe(true);
    expect(captureMock.mock.calls[0]?.[0]).toEqual(['git', 'push', '-u', 'origin', 'feat/5-x']);
    captureMock.mockReturnValueOnce(fail);
    expect(push('feat/5-x')).toEqual({ ok: false, error: 'boom' });
  });

  it('force-pushes with lease after a rebase', () => {
    captureMock.mockReturnValueOnce(ok);
    expect(push('feat/5-x', true).ok).toBe(true);
    expect(captureMock.mock.calls[0]?.[0]).toEqual([
      'git', 'push', '--force-with-lease', '-u', 'origin', 'feat/5-x',
    ]);
  });
});

describe('syncWithBase', () => {
  it('is a no-op when the branch is not behind origin/<base>', () => {
    captureMock.mockImplementation((cmd) =>
      cmd[1] === 'rev-list' ? { returncode: 0, stdout: '0\n', stderr: '' } : ok,
    );
    expect(syncWithBase('main')).toEqual({ ok: true, rebased: false, forcePushNeeded: false, error: null });
    expect(captureMock.mock.calls.some((c) => c[0][1] === 'rebase')).toBe(false);
  });

  it('rebases (dropping now-empty commits) when the base has moved', () => {
    captureMock.mockImplementation((cmd) =>
      cmd[1] === 'rev-list' ? { returncode: 0, stdout: '3\n', stderr: '' } : ok,
    );
    expect(syncWithBase('main')).toEqual({ ok: true, rebased: true, forcePushNeeded: false, error: null });
    const rebaseCall = captureMock.mock.calls.find((c) => c[0][1] === 'rebase');
    expect(rebaseCall?.[0]).toEqual(['git', 'rebase', '--empty=drop', 'origin/main']);
  });

  it('aborts and reports a conflicted rebase', () => {
    captureMock.mockImplementation((cmd) => {
      if (cmd[1] === 'rev-list') {
        return { returncode: 0, stdout: '2\n', stderr: '' };
      }
      if (cmd[1] === 'rebase' && cmd[2] !== '--abort') {
        return { returncode: 1, stdout: '', stderr: 'CONFLICT (content): x.rs' };
      }
      return ok;
    });
    expect(syncWithBase('main')).toEqual({
      ok: false,
      rebased: false,
      stage: 'rebase',
      error: 'CONFLICT (content): x.rs',
    });
    expect(captureMock.mock.calls.some((c) => c[0][1] === 'rebase' && c[0][2] === '--abort')).toBe(true);
  });

  it('retries a flaky fetch (sibling-lane ref-lock contention) then proceeds', () => {
    let fetches = 0;
    captureMock.mockImplementation((cmd) => {
      if (cmd[1] === 'fetch') {
        fetches += 1;
        return fetches < 3 ? fail : ok;
      }
      return cmd[1] === 'rev-list' ? { returncode: 0, stdout: '0\n', stderr: '' } : ok;
    });
    expect(syncWithBase('main').ok).toBe(true);
    expect(fetches).toBe(3);
    expect(captureMock.mock.calls.some((c) => c[0][0] === 'sleep')).toBe(true);
  });

  it('fails loud (stage fetch, retryable message) after persistent fetch failures', () => {
    captureMock.mockImplementation((cmd) => (cmd[1] === 'fetch' ? fail : ok));
    expect(syncWithBase('main')).toEqual({
      ok: false,
      rebased: false,
      stage: 'fetch',
      error: 'git fetch origin failed: boom',
    });
    expect(captureMock.mock.calls.some((c) => c[0][1] === 'rebase')).toBe(false);
  });

  it('fails loud when the behind-probe fails — "cannot prove currency" must not merge', () => {
    captureMock.mockImplementation((cmd) => (cmd[1] === 'rev-list' ? fail : ok));
    expect(syncWithBase('main')).toEqual({
      ok: false,
      rebased: false,
      stage: 'fetch',
      error: 'behind-probe failed: boom',
    });
    expect(captureMock.mock.calls.some((c) => c[0][1] === 'rebase')).toBe(false);
  });

  it('flags forcePushNeeded when origin/<branch> exists but diverged (resume after a dead force-push)', () => {
    captureMock.mockImplementation((cmd) => {
      if (cmd[1] === 'rev-list') {
        return { returncode: 0, stdout: '0\n', stderr: '' };
      }
      if (cmd[1] === 'branch') {
        return { returncode: 0, stdout: 'feat/5-x\n', stderr: '' };
      }
      if (cmd[1] === 'merge-base') {
        return fail; // remote tip is NOT an ancestor of HEAD
      }
      return ok;
    });
    expect(syncWithBase('main')).toEqual({ ok: true, rebased: false, forcePushNeeded: true, error: null });
  });

  it('prints instead of running under dry-run', () => {
    const log = vi.spyOn(console, 'log').mockImplementation(() => {});
    expect(syncWithBase('main', true)).toEqual({ ok: true, rebased: false, forcePushNeeded: false, error: null });
    expect(captureMock).not.toHaveBeenCalled();
    expect(log).toHaveBeenCalledWith('[dry-run] git rebase --empty=drop origin/main');
  });
});

describe('prForBranch / createPr', () => {
  it('returns the first open PR url, else null', () => {
    ghJsonMock.mockReturnValueOnce([{ url: 'https://x/pull/7' }]);
    expect(prForBranch('b', '/bin/gh', 'o/r')).toBe('https://x/pull/7');
    ghJsonMock.mockReturnValueOnce([]);
    expect(prForBranch('b', '/bin/gh', 'o/r')).toBeNull();
  });

  it('parses the created PR number from the trailing url line', () => {
    captureMock.mockReturnValueOnce({
      returncode: 0,
      stdout: 'some banner\nhttps://github.com/o/r/pull/42\n',
      stderr: '',
    });
    expect(createPr('b', 't', 'body', 'main', '/bin/gh', 'o/r')).toEqual({
      number: 42,
      url: 'https://github.com/o/r/pull/42',
      error: null,
    });
  });

  it('reports gh failures', () => {
    captureMock.mockReturnValueOnce(fail);
    expect(createPr('b', 't', 'body', 'main', '/bin/gh', 'o/r').error).toBe('boom');
  });
});

describe('ciStatus', () => {
  it('maps an unparseable query to unknown and an empty rollup to none', () => {
    ghJsonMock.mockReturnValueOnce(null);
    expect(ciStatus(7, '/bin/gh', 'o/r').state).toBe('unknown');
    ghJsonMock.mockReturnValueOnce({ statusCheckRollup: [] });
    expect(ciStatus(7, '/bin/gh', 'o/r').state).toBe('none');
  });

  it('reports failure with the failing job names', () => {
    ghJsonMock.mockReturnValueOnce({
      statusCheckRollup: [
        { name: 'build', status: 'COMPLETED', conclusion: 'SUCCESS' },
        { name: 'test', status: 'COMPLETED', conclusion: 'FAILURE' },
        { context: 'lint', state: 'ERROR' },
      ],
    });
    const status = ciStatus(7, '/bin/gh', 'o/r');
    expect(status.state).toBe('failure');
    expect(status.failingJobs.map((j) => j.name)).toEqual(['test', 'lint']);
  });

  it('reports pending for queued/in-progress/expected checks', () => {
    ghJsonMock.mockReturnValueOnce({
      statusCheckRollup: [
        { name: 'build', status: 'IN_PROGRESS', conclusion: '' },
        { name: 'done', status: 'COMPLETED', conclusion: 'SUCCESS' },
      ],
    });
    expect(ciStatus(7, '/bin/gh', 'o/r').state).toBe('pending');
    ghJsonMock.mockReturnValueOnce({ statusCheckRollup: [{ context: 'ext', state: 'EXPECTED' }] });
    expect(ciStatus(7, '/bin/gh', 'o/r').state).toBe('pending');
  });

  it('reports success when every check concluded green', () => {
    ghJsonMock.mockReturnValueOnce({
      statusCheckRollup: [
        { name: 'build', status: 'COMPLETED', conclusion: 'SUCCESS' },
        { context: 'ext', state: 'SUCCESS' },
      ],
    });
    expect(ciStatus(7, '/bin/gh', 'o/r').state).toBe('success');
  });
});

describe('squashMerge', () => {
  it('merges with --squash --delete-branch and honors dry-run', () => {
    captureMock.mockReturnValueOnce(ok);
    expect(squashMerge(42, '/bin/gh', 'o/r').ok).toBe(true);
    expect(captureMock.mock.calls[0]?.[0]).toEqual([
      '/bin/gh', 'pr', 'merge', '42', '--squash', '--delete-branch', '--repo', 'o/r',
    ]);
    const log = vi.spyOn(console, 'log').mockImplementation(() => {});
    expect(squashMerge(42, '/bin/gh', 'o/r', true).ok).toBe(true);
    expect(log).toHaveBeenCalled();
    expect(captureMock).toHaveBeenCalledTimes(1); // dry-run ran nothing new
  });
});

describe('failingCiLogExcerpt', () => {
  it('keeps error-shaped lines from beyond the tail window, strips ANSI, drops far noise', () => {
    ghJsonMock
      .mockReturnValueOnce({ headRefName: 'feat/5-x' })
      .mockReturnValueOnce([{ databaseId: 123 }]);
    // Error shapes appear FIRST, then >40 lines of noise: the shapes must
    // survive from beyond the always-kept raw tail; the early noise must not.
    const noise = Array.from({ length: 45 }, (_, i) => `verify\tVerify\tcompiling dep ${i}`).join('\n');
    captureMock.mockReturnValueOnce({
      returncode: 0,
      stdout:
        'verify\tVerify\tDownloading crates\n' +
        'verify\tVerify\t\u001b[1m\u001b[91merror\u001b[0m[E0308]: mismatched types\n' +
        'verify\tVerify\tsrc/foo.ts(3,1): error TS2304: Cannot find name x\n' +
        'verify\tVerify\t##[error]Process completed with exit code 2.\n' +
        'verify\tVerify\ttest foo ... FAILED\n' +
        `${noise}\n`,
      stderr: '',
    });
    const out = failingCiLogExcerpt(7, '/bin/gh', 'o/r');
    expect(out).toContain('error[E0308]: mismatched types');
    expect(out).toContain('error TS2304'); // compiler-code shape
    expect(out).toContain('##[error]Process completed'); // forge annotation shape
    expect(out).toContain('FAILED');
    expect(out).not.toContain('Downloading crates'); // pre-tail noise filtered
    expect(out).not.toContain('\u001b'); // ANSI stripped
    const logCall = captureMock.mock.calls.find((c) => c[0][1] === 'run');
    expect(logCall?.[0]).toEqual(['/bin/gh', 'run', 'view', '123', '--log-failed', '--repo', 'o/r']);
  });

  it('always keeps the raw tail (verdicts print last), capped from the end', () => {
    ghJsonMock.mockReturnValueOnce({ headRefName: 'b' }).mockReturnValueOnce([{ databaseId: 9 }]);
    // One early noise line matches /FAILED/i — it must NOT crowd out the
    // unshaped verdict at the very end of the log.
    const filler = Array.from({ length: 60 }, (_, i) => `step ${i}`).join('\n');
    captureMock.mockReturnValueOnce({
      returncode: 0,
      stdout: `npm ERR! Failed at the build script\n${filler}\n${'x'.repeat(5000)}\nthe verdict line`,
      stderr: '',
    });
    const out = failingCiLogExcerpt(7, '/bin/gh', 'o/r', 100);
    expect(out.startsWith('…')).toBe(true);
    expect(out).toContain('the verdict line'); // the tail survives the cap
    expect(out.length).toBeLessThanOrEqual(101);
  });

  it('returns empty when the branch, run, or log is unavailable (best effort)', () => {
    ghJsonMock.mockReturnValueOnce(null); // pr view failed
    expect(failingCiLogExcerpt(7, '/bin/gh', 'o/r')).toBe('');
    ghJsonMock.mockReturnValueOnce({ headRefName: 'b' }).mockReturnValueOnce([]); // no runs
    expect(failingCiLogExcerpt(7, '/bin/gh', 'o/r')).toBe('');
    ghJsonMock.mockReturnValueOnce({ headRefName: 'b' }).mockReturnValueOnce([{ databaseId: 9 }]);
    captureMock.mockReturnValueOnce(fail); // log fetch failed
    expect(failingCiLogExcerpt(7, '/bin/gh', 'o/r')).toBe('');
  });
});

describe('prNumberFromUrl', () => {
  it('extracts trailing numbers and rejects everything else', () => {
    expect(prNumberFromUrl('https://github.com/o/r/pull/42')).toBe(42);
    expect(prNumberFromUrl('https://github.com/o/r/pull/42/')).toBe(42);
    expect(prNumberFromUrl('')).toBeNull();
    expect(prNumberFromUrl('https://github.com/o/r/pulls')).toBeNull();
  });
});
