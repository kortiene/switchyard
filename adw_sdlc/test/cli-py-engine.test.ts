/**
 * Pins that `--engine py` fails CLOSED in this standalone port (issue #27): the
 * Python sibling (`adw/issue.py`) is not bundled here, so selecting py must
 * raise an explicit AdwError at dispatch — rc 1, the load-bearing message
 * substring `not available in this standalone distribution`, and (the strongest
 * regression pin) NO subprocess attempted at all. We still mock
 * node:child_process so a regression that reintroduces the dead
 * `python3 adw/issue.py` spawn would trip the spawn-never-called guard.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';

const { spawnMock } = vi.hoisted(() => ({ spawnMock: vi.fn() }));

vi.mock('node:child_process', async (importOriginal) => ({
  ...(await importOriginal<typeof import('node:child_process')>()),
  spawn: spawnMock,
}));

import { main } from '../src/cli.js';

afterEach(() => {
  spawnMock.mockReset();
  vi.restoreAllMocks();
});

const UNAVAILABLE = 'not available in this standalone distribution';

describe('--engine py fails closed (Python sibling not bundled)', () => {
  it('rejects the --engine py flag with rc 1 and never spawns a subprocess', async () => {
    const stderr = vi.spyOn(console, 'error').mockImplementation(() => {});
    const rc = await main(['--engine', 'py', '5', '--yes'], { env: {} });
    expect(rc).toBe(1);
    expect(stderr).toHaveBeenCalledWith(expect.stringContaining(UNAVAILABLE));
    expect(spawnMock).not.toHaveBeenCalled();
  });

  it('rejects ADW_ENGINE=py from the environment identically', async () => {
    const stderr = vi.spyOn(console, 'error').mockImplementation(() => {});
    const rc = await main(['5', '--yes'], { env: { ADW_ENGINE: 'py' } });
    expect(rc).toBe(1);
    expect(stderr).toHaveBeenCalledWith(expect.stringContaining(UNAVAILABLE));
    expect(spawnMock).not.toHaveBeenCalled();
  });

  it('fails closed regardless of forwarded args or post-`--` passthru', async () => {
    const stderr = vi.spyOn(console, 'error').mockImplementation(() => {});
    // A TS-invalid runner and a passthru chunk are both irrelevant on a dead
    // path: nothing is parsed, nothing is forwarded, nothing is spawned.
    const rc = await main(['--engine', 'py', '5', '--runner', 'gemini', '--', '--x'], { env: {} });
    expect(rc).toBe(1);
    expect(stderr).toHaveBeenCalledWith(expect.stringContaining(UNAVAILABLE));
    expect(spawnMock).not.toHaveBeenCalled();
  });

  it('fails closed for the --engine=py (equals) spelling (distinct extractEngineFlag branch)', async () => {
    const stderr = vi.spyOn(console, 'error').mockImplementation(() => {});
    // extractEngineFlag has two code paths: `--engine <value>` (space) and
    // `--engine=<value>` (equals). The three tests above cover the space form
    // and the env knob; this pins the equals branch against the same contract.
    const rc = await main(['--engine=py', '5', '--yes'], { env: {} });
    expect(rc).toBe(1);
    expect(stderr).toHaveBeenCalledWith(expect.stringContaining(UNAVAILABLE));
    expect(spawnMock).not.toHaveBeenCalled();
  });
});
