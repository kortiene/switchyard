/**
 * Engine/runner selection wiring (PLAN.md roadmap step 10, D4): the CLI must
 * resolve ADW_ENGINE / --engine (default ts in this standalone port), reject
 * `py` (the Python sibling is not bundled — issue #27), and on the ts engine
 * validate ADW_RUNNER / --runner over the four-runner registry and bind the
 * loaded adapter into orchestrator.run. Unknown engine/runner values throw,
 * mirroring adw/_orchestrator.py:557-559.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  CLI_USAGE,
  DEFAULT_ENGINE,
  extractEngineFlag,
  main,
  parseCliArgs,
  resolveEngineId,
  splitPassthru,
  type CliDeps,
} from '../src/cli.js';
import { AdwError, RunnerNotInstalledError } from '../src/errors.js';
import type { AgentRunner } from '../src/invoker.js';
import { createMockRunner } from '../src/runners/runner-mock.js';
import type { WorkItemId } from '../src/work-item.js';

afterEach(() => {
  vi.restoreAllMocks();
});

function cliDeps(over: Partial<CliDeps> = {}): CliDeps {
  return {
    env: { PATH: '/bin' },
    loadRunner: vi.fn(async (id) => createMockRunner({ id }) as AgentRunner),
    runIssue: vi.fn(async () => 0),
    ...over,
  };
}

/** Silence the expected `error: …` line and capture it for assertions. */
function muteStderr(): ReturnType<typeof vi.spyOn> {
  return vi.spyOn(console, 'error').mockImplementation(() => {});
}

describe('resolveEngineId', () => {
  it('defaults to py until cutover', () => {
    expect(DEFAULT_ENGINE).toBe('ts');
    expect(resolveEngineId(undefined)).toBe('ts');
    expect(resolveEngineId(null)).toBe('ts');
    expect(resolveEngineId('')).toBe('ts');
  });

  it('accepts the two engines', () => {
    expect(resolveEngineId('py')).toBe('py');
    expect(resolveEngineId('ts')).toBe('ts');
  });

  it('throws on unknown values (fail loud, never guess)', () => {
    expect(() => resolveEngineId('rust')).toThrow(AdwError);
    expect(() => resolveEngineId('TS')).toThrow(/unknown engine: 'TS'/);
  });
});

describe('splitPassthru / extractEngineFlag', () => {
  it('splits at the first --', () => {
    expect(splitPassthru(['5', '--yes', '--', '--model', 'x'])).toEqual([
      ['5', '--yes'],
      ['--model', 'x'],
    ]);
    expect(splitPassthru(['5'])).toEqual([['5'], []]);
  });

  it('removes --engine in both spellings; the last occurrence wins', () => {
    expect(extractEngineFlag(['5', '--engine', 'ts', '--yes'])).toEqual({
      engine: 'ts',
      rest: ['5', '--yes'],
    });
    expect(extractEngineFlag(['--engine=py', '5'])).toEqual({ engine: 'py', rest: ['5'] });
    expect(extractEngineFlag(['--engine=py', '--engine', 'ts', '5'])).toEqual({
      engine: 'ts',
      rest: ['5'],
    });
    expect(extractEngineFlag(['5'])).toEqual({ rest: ['5'] });
  });

  it('rejects a dangling or empty --engine (must not mask ADW_ENGINE)', () => {
    expect(() => extractEngineFlag(['5', '--engine'])).toThrow(/--engine requires a value/);
    expect(() => extractEngineFlag(['5', '--engine='])).toThrow(/--engine requires a value/);
    expect(() => extractEngineFlag(['5', '--engine', ''])).toThrow(/--engine requires a value/);
  });
});

describe('CLI usage', () => {
  it('describes the universal work-item workflow while keeping the issue command alias', () => {
    expect(CLI_USAGE).toContain('<work-item-id>');
    expect(CLI_USAGE).toContain('work-item delivery workflow');
    expect(CLI_USAGE).toContain('"issue" command name is kept as a');
  });

  it('describes --engine py as NOT available, not as a working delegation (issue #27)', () => {
    // Pin the corrected help text so a revert to the old "delegates to python3
    // adw/issue.py" wording is caught before it ships. "NOT" and "available"
    // are split across two lines in the template literal, so use a regex.
    expect(CLI_USAGE).toMatch(/engine py is NOT\s+available in this distribution/);
    expect(CLI_USAGE).not.toContain('delegates');
  });

  it('documents the successful PR-only --no-merge mode', () => {
    expect(CLI_USAGE).toContain('--no-merge');
    expect(CLI_USAGE).toContain('leave the green change request open');
  });
});

describe('parseCliArgs', () => {
  it('parses the issue number and accepts free-form notes', () => {
    const parsed = parseCliArgs(['5', 'fix', 'the', 'thing']);
    expect(parsed.issue).toBe('5');
    expect(parsed.workItem).toBe('5');
    expect(parsed.notes).toEqual(['fix', 'the', 'thing']);
    expect(parsed.runner).toBeUndefined();
    expect(parsed.options).toEqual({});
  });

  it('accepts provider-native string ids and rejects only missing or empty ids', () => {
    expect(() => parseCliArgs([])).toThrow(/missing work item id/);
    expect(() => parseCliArgs([''])).toThrow(/work item id must be non-empty/);
    expect(() => parseCliArgs(['   '])).toThrow(/work item id must be non-empty/);
    expect(parseCliArgs(['PROJ-123']).workItem).toBe('PROJ-123');
    expect(parseCliArgs(['550e8400-e29b-41d4-a716-446655440000']).workItem).toBe(
      '550e8400-e29b-41d4-a716-446655440000',
    );
    expect(parseCliArgs(['123456789012345678901234567890']).workItem).toBe(
      '123456789012345678901234567890',
    );
    expect(parseCliArgs(['000123']).workItem).toBe('000123');
  });

  it('maps every phased flag onto RunOptions (seconds become milliseconds)', () => {
    const parsed = parseCliArgs([
      '7',
      '--runner', 'codex',
      '--phases', 'plan,implement',
      '--adw-id', 'a1b2c3d4',
      '--resume',
      '--no-progress',
      '--inherit-env',
      '--max-resolve', '5',
      '--max-patch', '1',
      '--max-ci-fix', '9',
      '--ci-poll-interval', '10',
      '--ci-max-polls', '7',
      '--test-cmd', 'cargo test -p x',
      '--model', 'm-1',
      '--repo', 'o/r',
      '--base', 'dev',
      '--timeout', '60',
      '--no-verify',
      '--force',
      '--allow-dirty',
      '-y',
      '--dry-run',
      '--max-budget-usd', '2.5',
    ]);
    expect(parsed.issue).toBe('7');
    expect(parsed.workItem).toBe('7');
    expect(parsed.runner).toBe('codex');
    expect(parsed.options).toEqual({
      phases: 'plan,implement',
      adwId: 'a1b2c3d4',
      resume: true,
      noProgress: true,
      inheritEnv: true,
      maxResolve: 5,
      maxPatch: 1,
      maxCiFix: 9,
      ciPollIntervalMs: 10_000,
      ciMaxPolls: 7,
      testCmd: 'cargo test -p x',
      model: 'm-1',
      repo: 'o/r',
      base: 'dev',
      timeoutMs: 60_000,
      verify: false,
      force: true,
      allowDirty: true,
      yes: true,
      dryRun: true,
      maxBudgetUsd: 2.5,
    });
  });

  it('accepts --flag=value spellings', () => {
    const parsed = parseCliArgs(['5', '--runner=opencode', '--timeout=30', '--yes']);
    expect(parsed.runner).toBe('opencode');
    expect(parsed.options.timeoutMs).toBe(30_000);
    expect(parsed.options.yes).toBe(true);
  });

  it('parses --no-merge and rejects the opposite --yes intent', () => {
    expect(parseCliArgs(['5', '--no-merge']).options).toEqual({ noMerge: true });
    expect(() => parseCliArgs(['5', '--no-merge', '--yes'])).toThrow(
      /--yes and --no-merge are mutually exclusive/,
    );
    expect(() => parseCliArgs(['5', '-y', '--no-merge'])).toThrow(
      /--yes and --no-merge are mutually exclusive/,
    );
  });

  it('defaults --test-cmd and --repo from the environment like adw/issue.py', () => {
    const parsed = parseCliArgs(['5'], { ADW_TEST_CMD: 'cargo test -p y', REPO: 'a/b' });
    expect(parsed.options.testCmd).toBe('cargo test -p y');
    expect(parsed.options.repo).toBe('a/b');
    // An explicit flag still wins over the env default.
    const explicit = parseCliArgs(['5', '--test-cmd', 'x', '--repo', 'c/d'], {
      ADW_TEST_CMD: 'cargo test -p y',
      REPO: 'a/b',
    });
    expect(explicit.options.testCmd).toBe('x');
    expect(explicit.options.repo).toBe('c/d');
  });

  it('rejects malformed values, missing values, and unknown flags', () => {
    expect(() => parseCliArgs(['5', '--timeout', 'soon'])).toThrow(/--timeout expects an integer/);
    expect(() => parseCliArgs(['5', '--max-budget-usd', 'lots'])).toThrow(/expects a number/);
    expect(() => parseCliArgs(['5', '--model'])).toThrow(/--model requires a value/);
    expect(() => parseCliArgs(['5', '--resume=please'])).toThrow(/does not take a value/);
    expect(() => parseCliArgs(['5', '--frobnicate'])).toThrow(/unknown flag: --frobnicate/);
  });

  it('never swallows an option-looking token as a flag value (argparse parity)', () => {
    // `--model --yes` must fail loud, not silently consume the user's --yes
    // and later die at the merge confirmation gate.
    expect(() => parseCliArgs(['5', '--model', '--yes'])).toThrow(/--model requires a value/);
    expect(() => parseCliArgs(['5', '--runner', '--dry-run'])).toThrow(/--runner requires a value/);
    // …but negative numbers are legitimate values, as in argparse.
    expect(parseCliArgs(['5', '--max-resolve', '-1']).options.maxResolve).toBe(-1);
  });

  it('rejects an explicit empty --runner (must not mask ADW_RUNNER)', () => {
    expect(() => parseCliArgs(['5', '--runner='])).toThrow(/--runner requires a non-empty value/);
    expect(() => parseCliArgs(['5', '--runner', ''])).toThrow(/--runner requires a non-empty value/);
  });

  it('accepts ONE contiguous positional chunk anywhere, like argparse nargs=*', () => {
    // The chunk may follow options…
    const parsed = parseCliArgs(['--yes', '999', 'a', 'note', '--force']);
    expect(parsed.issue).toBe('999');
    expect(parsed.workItem).toBe('999');
    expect(parsed.notes).toEqual(['a', 'note']);
    // …but a second positional run is an error there and here.
    expect(() => parseCliArgs(['999', '--yes', 'somenote'])).toThrow(/unrecognized argument: somenote/);
    expect(() => parseCliArgs(['999', '--phases', 'plan', 'implement'])).toThrow(
      /unrecognized argument: implement/,
    );
  });

  it('handles -h/--help instead of treating it as an unknown flag', () => {
    expect(parseCliArgs(['-h']).help).toBe(true);
    expect(parseCliArgs(['--help']).help).toBe(true);
    expect(parseCliArgs(['5', '--help']).help).toBe(true); // no issue required
  });

  it('rejects py-only flags with a pointer back to the py engine', () => {
    for (const flag of ['--one-shot', '--print-prompt', '--json']) {
      expect(() => parseCliArgs(['5', flag])).toThrow(/py-engine option/);
    }
    expect(() => parseCliArgs(['5', '--template', 'x.md'])).toThrow(/py-engine option/);
    expect(() => parseCliArgs(['5', '--log-dir=/tmp/x'])).toThrow(/py-engine option/);
    expect(() => parseCliArgs(['5', '--thinking', 'high'])).toThrow(/py-engine option/);
  });
});

describe('runWorkItem dispatch', () => {
  it('prefers runWorkItem when supplied, otherwise calls runIssue', async () => {
    const runIssue: CliDeps['runIssue'] = vi.fn(async (_issue: WorkItemId) => 0);
    const runWorkItem: NonNullable<CliDeps['runWorkItem']> = vi.fn(async (_issue: WorkItemId) => 0);
    const deps = cliDeps({ runIssue, runWorkItem });
    const rc = await main(['5', '--runner', 'claude', '--yes'], deps);
    expect(rc).toBe(0);
    expect(runWorkItem).toHaveBeenCalledTimes(1);
    expect(runWorkItem).toHaveBeenCalledWith('5', expect.anything(), expect.anything());
    expect(runIssue).not.toHaveBeenCalled();

    const legacyRun: CliDeps['runIssue'] = vi.fn(async (_issue: WorkItemId) => 0);
    await main(['5', '--runner', 'claude', '--yes'], cliDeps({ runIssue: legacyRun }));
    expect(legacyRun).toHaveBeenCalledTimes(1);
  });

  it('dispatches a string work-item id unchanged', async () => {
    const runWorkItem: NonNullable<CliDeps['runWorkItem']> = vi.fn(async () => 0);
    const deps = cliDeps({ runWorkItem });

    expect(await main(['PROJ-123', '--runner', 'claude', '--no-merge'], deps)).toBe(0);
    expect(runWorkItem).toHaveBeenCalledWith('PROJ-123', expect.anything(), { noMerge: true });
  });
});

describe('main — engine dispatch', () => {
  it('defaults to the ts engine (post-cutover): binds the runner', async () => {
    const deps = cliDeps();
    const rc = await main(['5', '--runner', 'claude', '--yes'], deps);
    expect(rc).toBe(0);
    expect(deps.loadRunner).toHaveBeenCalledWith('claude');
    expect(deps.runIssue).toHaveBeenCalledTimes(1);
  });

  it('fails closed when --engine py is selected: explicit AdwError, no runner', async () => {
    const stderr = muteStderr();
    const deps = cliDeps();
    // argv content is irrelevant on a dead path; nothing is forwarded.
    const rc = await main(['--engine', 'py', '5', '--runner', 'gemini', '--yes', '--', '--model', 'x'], deps);
    expect(rc).toBe(1);
    expect(stderr).toHaveBeenCalledWith(
      expect.stringContaining('not available in this standalone distribution'),
    );
    expect(deps.loadRunner).not.toHaveBeenCalled();
    expect(deps.runIssue).not.toHaveBeenCalled();
  });

  it('fails closed identically when py comes from ADW_ENGINE', async () => {
    const stderr = muteStderr();
    const deps = cliDeps({ env: { ADW_ENGINE: 'py' } });
    expect(await main(['5', '--yes'], deps)).toBe(1);
    expect(stderr).toHaveBeenCalledWith(
      expect.stringContaining('not available in this standalone distribution'),
    );
    expect(deps.loadRunner).not.toHaveBeenCalled();
    expect(deps.runIssue).not.toHaveBeenCalled();
  });

  it('binds the selected runner into orchestrator.run on the ts engine', async () => {
    const runner = createMockRunner({ id: 'opencode' });
    const loadRunner = vi.fn(async () => runner as AgentRunner);
    const runIssue = vi.fn(async () => 0);
    const deps = cliDeps({ loadRunner: loadRunner as unknown as CliDeps['loadRunner'], runIssue });

    const rc = await main(['--engine', 'ts', '9', '--runner', 'opencode', '--yes', '--timeout', '30'], deps);
    expect(rc).toBe(0);
    expect(loadRunner).toHaveBeenCalledWith('opencode');
    expect(runIssue).toHaveBeenCalledTimes(1);
    const [issue, boundRunner, options] = runIssue.mock.calls[0] as unknown as [WorkItemId, AgentRunner, object];
    expect(issue).toBe('9');
    expect(boundRunner).toBe(runner);
    expect(options).toEqual({ yes: true, timeoutMs: 30_000 });
  });

  it('honors ADW_ENGINE and ADW_RUNNER from the environment', async () => {
    const deps = cliDeps({ env: { ADW_ENGINE: 'ts', ADW_RUNNER: 'codex' } });
    await main(['5', '--yes'], deps);
    expect(deps.loadRunner).toHaveBeenCalledWith('codex');
  });

  it('lets flags win over the environment for both selectors', async () => {
    // ADW_ENGINE=py would fail closed, but the --engine ts flag wins.
    const deps = cliDeps({ env: { ADW_ENGINE: 'py', ADW_RUNNER: 'codex' } });
    await main(['--engine', 'ts', '5', '--runner', 'pi', '--yes'], deps);
    expect(deps.loadRunner).toHaveBeenCalledWith('pi');
  });

  it('defaults the ts runner to claude (the cutover-gate runner)', async () => {
    const deps = cliDeps({ env: { ADW_ENGINE: 'ts' } });
    await main(['5', '--yes'], deps);
    expect(deps.loadRunner).toHaveBeenCalledWith('claude');
  });

  it('fails loud on an unknown engine', async () => {
    const stderr = muteStderr();
    const deps = cliDeps();
    expect(await main(['--engine', 'rust', '5'], deps)).toBe(1);
    expect(stderr).toHaveBeenCalledWith(expect.stringContaining("unknown engine: 'rust'"));
    expect(await main(['5'], cliDeps({ env: { ADW_ENGINE: 'go' } }))).toBe(1);
    expect(deps.runIssue).not.toHaveBeenCalled();
  });

  it('fails loud on an unknown runner under the ts engine', async () => {
    const stderr = muteStderr();
    const deps = cliDeps({ env: { ADW_ENGINE: 'ts' } });
    expect(await main(['5', '--runner', 'gemini'], deps)).toBe(1);
    expect(stderr).toHaveBeenCalledWith(expect.stringContaining("unknown runner: 'gemini'"));
    expect(deps.loadRunner).not.toHaveBeenCalled();
  });

  it('rejects runner passthru flags on the ts engine (no runner command line)', async () => {
    muteStderr();
    const deps = cliDeps({ env: { ADW_ENGINE: 'ts' } });
    expect(await main(['5', '--yes', '--', '--permission-mode', 'acceptEdits'], deps)).toBe(1);
    expect(deps.runIssue).not.toHaveBeenCalled();
  });

  it('surfaces RunnerNotInstalledError as a friendly rc-1 failure', async () => {
    const stderr = muteStderr();
    const deps = cliDeps({
      env: { ADW_ENGINE: 'ts' },
      loadRunner: vi.fn(async () => {
        throw new RunnerNotInstalledError('codex', '@openai/codex-sdk');
      }),
    });
    expect(await main(['5', '--runner', 'codex'], deps)).toBe(1);
    expect(stderr).toHaveBeenCalledWith(expect.stringContaining('not installed'));
  });

  it('maps AdwError from the run itself to rc 1 but lets bugs propagate', async () => {
    muteStderr();
    const adwFail = cliDeps({
      env: { ADW_ENGINE: 'ts' },
      runIssue: vi.fn(async () => {
        throw new AdwError('working tree is dirty');
      }),
    });
    expect(await main(['5', '--yes'], adwFail)).toBe(1);

    const bug = cliDeps({
      env: { ADW_ENGINE: 'ts' },
      runIssue: vi.fn(async () => {
        throw new TypeError('boom');
      }),
    });
    await expect(main(['5', '--yes'], bug)).rejects.toThrow(TypeError);
  });

  it('returns the orchestrator rc unchanged', async () => {
    const deps = cliDeps({ env: { ADW_ENGINE: 'ts' }, runIssue: vi.fn(async () => 2) });
    expect(await main(['5', '--yes'], deps)).toBe(2);
  });

  it('prints usage and exits 0 on --help under the ts engine (argparse parity)', async () => {
    const stdout = vi.spyOn(console, 'log').mockImplementation(() => {});
    const deps = cliDeps({ env: { ADW_ENGINE: 'ts' } });
    expect(await main(['--help'], deps)).toBe(0);
    expect(stdout).toHaveBeenCalledWith(expect.stringContaining('usage: adw-sdlc issue'));
    expect(deps.loadRunner).not.toHaveBeenCalled();
    expect(deps.runIssue).not.toHaveBeenCalled();
    // …while py is unavailable here, so even --help fails closed at dispatch
    // (the engine is rejected before argv is parsed for --help).
    const stderr = muteStderr();
    const py = cliDeps({ env: { ADW_ENGINE: 'py' } });
    expect(await main(['--help'], py)).toBe(1);
    expect(stderr).toHaveBeenCalledWith(
      expect.stringContaining('not available in this standalone distribution'),
    );
    expect(py.loadRunner).not.toHaveBeenCalled();
  });

  it('previews --dry-run without loading the optional runner SDK (py parity)', async () => {
    const loadRunner = vi.fn(async () => {
      throw new RunnerNotInstalledError('codex', '@openai/codex-sdk');
    });
    const runIssue = vi.fn(async () => 0);
    const deps = cliDeps({
      env: { ADW_ENGINE: 'ts' },
      loadRunner: loadRunner as unknown as CliDeps['loadRunner'],
      runIssue,
    });
    expect(await main(['5', '--runner', 'codex', '--dry-run'], deps)).toBe(0);
    expect(loadRunner).not.toHaveBeenCalled();
    const [, runner, options] = runIssue.mock.calls[0] as unknown as [WorkItemId, AgentRunner, { dryRun?: boolean }];
    expect(runner.id).toBe('codex');
    expect(options.dryRun).toBe(true);
    // The stub is preview-only: it must never be able to execute a phase.
    await expect(
      runner.runPhase({} as unknown as Parameters<AgentRunner['runPhase']>[0]),
    ).rejects.toThrow(/dry-run runner cannot execute phases/);
    // An unknown runner still fails loud even on a dry run.
    muteStderr();
    expect(await main(['5', '--runner', 'gemini', '--dry-run'], deps)).toBe(1);
  });

  it('notes (rather than silently drops) PI_THINKING on the ts engine', async () => {
    const stderr = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
    const deps = cliDeps({ env: { ADW_ENGINE: 'ts', PI_THINKING: 'high' } });
    expect(await main(['5', '--yes'], deps)).toBe(0);
    expect(stderr).toHaveBeenCalledWith(expect.stringContaining('PI_THINKING is ignored by the ts engine'));
    // No note when the knob is not set.
    stderr.mockClear();
    await main(['5', '--yes'], cliDeps({ env: { ADW_ENGINE: 'ts' } }));
    expect(stderr).not.toHaveBeenCalled();
  });
});
