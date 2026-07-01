/**
 * Parity tests for the TS control plane (port of adw/test_orchestrator.py):
 * the runner, git, and gh layers are injected via OrchestratorDeps; no real
 * agent/git/gh/cargo runs. run() is driven end to end with the mock-runner
 * seam to assert phase ordering, gating, resume semantics, secret
 * withholding, and the merge gate.
 */

import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { parseAdwConfig, setAdwConfigForTests } from '../src/config.js';
import { AdwError, RunnerAuthError, RunnerTransientError } from '../src/errors.js';
import type { AgentRunner } from '../src/invoker.js';
import type { AdwProviders } from '../src/providers.js';
import {
  ciFixLoop,
  confirmMerge,
  finalizeGates,
  patchLoop,
  renderFindings,
  resolveLoop,
  run,
  truncate,
  type OrchestratorDeps,
} from '../src/orchestrator.js';
import { commitMessagePath, prBodyPath } from '../src/phases.js';
import { runAgentPhase } from '../src/run-phase.js';
import { createMockRunner } from '../src/runners/runner-mock.js';
import { AdwState, setAgentsDir } from '../src/state.js';
import { StructuredCallApiError } from '../src/structured-call.js';

let tmp: string;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), 'adw-orch-'));
  setAgentsDir(tmp);
});

afterEach(() => {
  setAgentsDir(null);
  setAdwConfigForTests(null); // ensure no per-test config override leaks
  rmSync(tmp, { recursive: true, force: true });
});

const noop = () => {};

type DepsOverride = Partial<Omit<OrchestratorDeps, 'git'>> & { git?: Partial<OrchestratorDeps['git']> };

/** Full deps with inert stubs; tests override the seams they exercise. */
function testDeps(overrides: DepsOverride = {}): OrchestratorDeps {
  const base: OrchestratorDeps = {
    // ANTHROPIC_API_KEY present by default → classify uses the shared SDK path.
    // (Subscription mode = no key → classify routes through the runner; tested below.)
    env: { PATH: '/bin', ANTHROPIC_API_KEY: 'sk-ant-test' },
    isatty: () => false,
    confirm: async () => false,
    sleep: async () => {},
    runCmd: () => ({ rc: 0, output: '' }),
    capture: () => ({ returncode: 0, stdout: '', stderr: '' }),
    workingTreeDirty: () => false,
    changedFiles: () => ['src/lib.rs'],
    resolveGhBin: () => '/bin/gh',
    detectRepo: () => 'o/r',
    issueState: () => 'OPEN',
    postProgress: noop,
    fetchIssue: () => ({ title: 'T', body: 'B', labels: [] }),
    setStatus: noop,
    git: {
      createOrCheckoutBranch: () => ({ ok: true, error: null }),
      commitAll: () => ({ ok: true, error: null }),
      push: () => ({ ok: true, error: null }),
      pullRebase: () => ({ ok: true, error: null }),
      prForBranch: () => null,
      createPr: () => ({ number: 42, url: 'https://x/pull/42', error: null }),
      ciStatus: () => ({ state: 'success', failingJobs: [] }),
      squashMerge: () => ({ ok: true, error: null }),
    },
    runAgentPhase: (async () => {
      throw new Error('runAgentPhase not stubbed for this test');
    }) as typeof runAgentPhase,
    classify: async () => ({ value: { issue_class: 'feat', reason: 'r' }, usage: {} }),
  };
  return {
    ...base,
    ...overrides,
    git: { ...base.git, ...(overrides.git ?? {}) },
  };
}

/** A scripted runAgentPhase stub returning per-phase canned results. */
function agentStub(
  results: Record<string, unknown>,
  onCall?: (opts: Parameters<typeof runAgentPhase>[0]) => void,
  usage: Record<string, unknown> = {},
): typeof runAgentPhase {
  return (async (opts: Parameters<typeof runAgentPhase>[0]) => {
    onCall?.(opts);
    const data = results[opts.phase];
    if (data === undefined) {
      throw new Error(`unexpected phase: ${opts.phase}`);
    }
    return { data, usage, attempts: 1 };
  }) as typeof runAgentPhase;
}

function agentCtx(runner: AgentRunner) {
  return { runner, cliModel: '', env: { PATH: '/bin' }, timeoutMs: 0 };
}

const state5 = () => new AdwState({ adwId: 'a1b2c3d4' });

describe('truncate', () => {
  it('keeps short text unchanged', () => {
    expect(truncate('abc', 10)).toBe('abc');
  });

  it('keeps the tail of long text and marks the cut', () => {
    const out = truncate('x'.repeat(100), 10);
    expect(out.endsWith('x'.repeat(10))).toBe(true);
    expect(out).toContain('truncated');
  });
});

describe('confirmMerge', () => {
  it('passes with --yes', async () => {
    await confirmMerge({ yes: true, isatty: false, confirm: async () => false });
  });

  it('aborts unattended without --yes', async () => {
    await expect(confirmMerge({ yes: false, isatty: false, confirm: async () => true })).rejects.toThrow(
      AdwError,
    );
  });

  it('honors an interactive yes/no', async () => {
    await confirmMerge({ yes: false, isatty: true, confirm: async () => true });
    await expect(confirmMerge({ yes: false, isatty: true, confirm: async () => false })).rejects.toThrow(
      AdwError,
    );
  });
});

describe('finalizeGates', () => {
  it('puts the test gate first, then the extra gates', () => {
    const gates = finalizeGates('pytest -q', ['lint', 'build']);
    expect(gates).toEqual(['pytest -q', 'lint', 'build']);
  });

  it('contributes no test gate when the test command is empty', () => {
    expect(finalizeGates('')).toEqual([]);
    expect(finalizeGates('', ['check'])).toEqual(['check']);
  });
});

describe('renderFindings', () => {
  it('renders numbered severity-tagged lines with optional locations', () => {
    const text = renderFindings([
      { severity: 'blocker', description: 'bug', location: 'a.rs:1' },
      { severity: 'tech_debt', description: 'later', location: '' },
    ]);
    expect(text).toBe('1. [blocker] (a.rs:1) bug\n2. [tech_debt] later');
  });
});

describe('resolveLoop', () => {
  it('returns immediately when the gate is green', async () => {
    const runCmd = vi.fn(() => ({ rc: 0, output: '' }));
    const agent = vi.fn();
    const deps = testDeps({ runCmd, runAgentPhase: agent as unknown as typeof runAgentPhase });
    const ok = await resolveLoop(
      state5(),
      agentCtx(createMockRunner()),
      { testCmd: 'cargo test', maxAttempts: 3, progress: noop },
      deps,
    );
    expect(ok).toBe(true);
    expect(agent).not.toHaveBeenCalled();
    expect(runCmd).toHaveBeenCalledTimes(1);
  });

  it('fixes then goes green', async () => {
    const runCmd = vi
      .fn()
      .mockReturnValueOnce({ rc: 1, output: 'fail' })
      .mockReturnValueOnce({ rc: 0, output: '' });
    const calls: string[] = [];
    const deps = testDeps({
      runCmd,
      runAgentPhase: agentStub({ resolve: { resolved: 1, remaining: 0, summary: '' } }, (o) =>
        calls.push(o.phase),
      ),
    });
    const ok = await resolveLoop(
      state5(),
      agentCtx(createMockRunner()),
      { testCmd: 'cargo test', maxAttempts: 3, progress: noop },
      deps,
    );
    expect(ok).toBe(true);
    expect(calls).toEqual(['resolve']);
  });

  it('stops when the agent makes no progress', async () => {
    const deps = testDeps({
      runCmd: () => ({ rc: 1, output: 'fail' }),
      runAgentPhase: agentStub({ resolve: { resolved: 0, remaining: 2, summary: '' } }),
    });
    const ok = await resolveLoop(
      state5(),
      agentCtx(createMockRunner()),
      { testCmd: 'cargo test', maxAttempts: 3, progress: noop },
      deps,
    );
    expect(ok).toBe(false);
  });

  it('drives a custom phase agent (and reads its resolved) when given a phase override', async () => {
    const runCmd = vi
      .fn()
      .mockReturnValueOnce({ rc: 1, output: 'fail' })
      .mockReturnValueOnce({ rc: 0, output: '' });
    const calls: string[] = [];
    const deps = testDeps({
      runCmd,
      runAgentPhase: agentStub({ verify: { resolved: 1, remaining: 0 } }, (o) => calls.push(o.phase)),
    });
    const ok = await resolveLoop(
      state5(),
      agentCtx(createMockRunner()),
      { testCmd: 'npm run verify', maxAttempts: 3, progress: noop, phase: 'verify' },
      deps,
    );
    expect(ok).toBe(true);
    expect(calls).toEqual(['verify']); // the custom phase's agent, not 'resolve'
  });

  it('caps attempts (initial gate + one per resolve)', async () => {
    const runCmd = vi.fn(() => ({ rc: 1, output: 'fail' }));
    let agentCalls = 0;
    const deps = testDeps({
      runCmd,
      runAgentPhase: agentStub({ resolve: { resolved: 1, remaining: 1, summary: '' } }, () => {
        agentCalls += 1;
      }),
    });
    const ok = await resolveLoop(
      state5(),
      agentCtx(createMockRunner()),
      { testCmd: 'cargo test', maxAttempts: 2, progress: noop },
      deps,
    );
    expect(ok).toBe(false);
    expect(agentCalls).toBe(2);
    expect(runCmd).toHaveBeenCalledTimes(3); // initial + after each resolve
  });
});

describe('patchLoop', () => {
  it('skips when there are no blockers', async () => {
    const agent = vi.fn();
    const deps = testDeps({ runAgentPhase: agent as unknown as typeof runAgentPhase });
    const ok = await patchLoop(
      state5(),
      [
        { severity: 'skippable', description: 'nit', location: '' },
        { severity: 'tech_debt', description: 'later', location: '' },
      ],
      agentCtx(createMockRunner()),
      { maxAttempts: 2, progress: noop },
      deps,
    );
    expect(ok).toBe(true);
    expect(agent).not.toHaveBeenCalled();
  });

  it('resolves blockers', async () => {
    let calls = 0;
    const deps = testDeps({
      runAgentPhase: agentStub({ patch: { resolved: 1, remaining: 0, summary: '' } }, () => {
        calls += 1;
      }),
    });
    const ok = await patchLoop(
      state5(),
      [{ severity: 'blocker', description: 'bug', location: '' }],
      agentCtx(createMockRunner()),
      { maxAttempts: 2, progress: noop },
      deps,
    );
    expect(ok).toBe(true);
    expect(calls).toBe(1);
  });

  it('breaks on no progress', async () => {
    const deps = testDeps({
      runAgentPhase: agentStub({ patch: { resolved: 0, remaining: 1, summary: '' } }),
    });
    const ok = await patchLoop(
      state5(),
      [{ severity: 'blocker', description: 'bug', location: '' }],
      agentCtx(createMockRunner()),
      { maxAttempts: 3, progress: noop },
      deps,
    );
    expect(ok).toBe(false);
  });
});

describe('ciFixLoop', () => {
  const cfg = { ghBin: '/bin/gh', repo: 'o/r', maxAttempts: 2, pollIntervalMs: 0, maxPolls: 3, progress: noop };
  const st = () => new AdwState({ adwId: 'a1b2c3d4', branchName: 'feat/5-x' });

  it('returns true immediately on success', async () => {
    const deps = testDeps({ git: { ciStatus: () => ({ state: 'success', failingJobs: [] }) } });
    expect(await ciFixLoop(st(), 7, agentCtx(createMockRunner()), cfg, deps)).toBe(true);
  });

  it('returns false only after a sustained unknown streak, not on the first transient read', async () => {
    const ciStatus = vi.fn(() => ({ state: 'unknown' as const, failingJobs: [] }));
    const deps = testDeps({ git: { ciStatus } });
    expect(await ciFixLoop(st(), 7, agentCtx(createMockRunner()), cfg, deps)).toBe(false);
    expect(ciStatus.mock.calls.length).toBeGreaterThan(3); // settled, not an instant bail
  });

  it('recovers when a transient unknown read clears to success', async () => {
    const ciStatus = vi
      .fn()
      .mockReturnValueOnce({ state: 'unknown' as const, failingJobs: [] })
      .mockReturnValueOnce({ state: 'unknown' as const, failingJobs: [] })
      .mockReturnValue({ state: 'success' as const, failingJobs: [] });
    const deps = testDeps({ git: { ciStatus } });
    expect(await ciFixLoop(st(), 7, agentCtx(createMockRunner()), cfg, deps)).toBe(true);
  });

  it('settles a persistent empty rollup to success', async () => {
    const ciStatus = vi.fn(() => ({ state: 'none' as const, failingJobs: [] }));
    const deps = testDeps({ git: { ciStatus } });
    expect(await ciFixLoop(st(), 7, agentCtx(createMockRunner()), cfg, deps)).toBe(true);
    expect(ciStatus.mock.calls.length).toBeGreaterThan(3); // settled, not instant
  });

  it('exhausts the poll budget on persistent pending', async () => {
    const deps = testDeps({ git: { ciStatus: () => ({ state: 'pending', failingJobs: [] }) } });
    expect(await ciFixLoop(st(), 7, agentCtx(createMockRunner()), cfg, deps)).toBe(false);
  });

  it('fixes a red check, commits, pushes, and goes green', async () => {
    const ciStatus = vi
      .fn()
      .mockReturnValueOnce({ state: 'failure', failingJobs: [{ name: 'ci', logExcerpt: '' }] })
      .mockReturnValueOnce({ state: 'success', failingJobs: [] });
    const commitAll = vi.fn(() => ({ ok: true, error: null }));
    const push = vi.fn(() => ({ ok: true, error: null }));
    const deps = testDeps({
      workingTreeDirty: () => true,
      git: { ciStatus, commitAll, push },
      runAgentPhase: agentStub({ resolve: { resolved: 1, remaining: 0, summary: '' } }),
    });
    expect(await ciFixLoop(st(), 7, agentCtx(createMockRunner()), cfg, deps)).toBe(true);
    expect(commitAll).toHaveBeenCalledWith('fix: address CI failures (ci)');
    expect(push).toHaveBeenCalledWith('feat/5-x');
  });

  it('enforces the soft parent-side budget after a ci-fix agent attempt', async () => {
    const commitAll = vi.fn(() => ({ ok: true, error: null }));
    const deps = testDeps({
      workingTreeDirty: () => true,
      git: { ciStatus: () => ({ state: 'failure', failingJobs: [{ name: 'ci', logExcerpt: '' }] }), commitAll },
      runAgentPhase: agentStub({ resolve: { resolved: 1, remaining: 0, summary: '' } }, undefined, { costUsd: 0.5 }),
    });
    await expect(
      ciFixLoop(st(), 7, agentCtx(createMockRunner()), { ...cfg, maxBudgetUsd: 0.4 }, deps),
    ).rejects.toThrow(/exceeded the budget cap/);
    expect(commitAll).not.toHaveBeenCalled(); // stopped before committing/pushing another CI fix
  });

  it('stops when the agent claims a fix but changed nothing', async () => {
    const deps = testDeps({
      workingTreeDirty: () => false,
      git: { ciStatus: () => ({ state: 'failure', failingJobs: [{ name: 'ci', logExcerpt: '' }] }) },
      runAgentPhase: agentStub({ resolve: { resolved: 1, remaining: 0, summary: '' } }),
    });
    expect(await ciFixLoop(st(), 7, agentCtx(createMockRunner()), cfg, deps)).toBe(false);
  });
});

describe('run() integration', () => {
  const PHASE_RESULTS: Record<string, unknown> = {
    plan: { plan_file: 'specs/x.md', spec_created: true, summary: '' },
    implement: { summary: 'did it', files_changed: ['src/lib.rs'] },
    tests: { tests_added: true, summary: '' },
    review: { findings: [], wrote_commit_message: true, wrote_pr_body: true },
  };

  function loadedId(): string {
    const ids = readdirSync(tmp, { withFileTypes: true })
      .filter((e) => e.isDirectory())
      .map((e) => e.name);
    expect(ids).toHaveLength(1);
    return ids[0]!;
  }

  it('runs phases in order, withholds GH_TOKEN, absorbs artifacts, and merges', async () => {
    const order: string[] = [];
    const poisoned = { GH_TOKEN: 'ghp_secret', PATH: '/bin', MATRIX_TOKEN: 'x', ADW_FOO: 'x', MX_AGENT_FOO: 'x', ANTHROPIC_API_KEY: 'sk-ant-x' };
    const issueStates = vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED');
    const classify = vi.fn(async (prompt: string) => {
      order.push('classify');
      expect(prompt).toContain('GitHub issue #5');
      return { value: { issue_class: 'feat' as const, reason: 'r' }, usage: { costUsd: 0.01 } };
    });
    const deps = testDeps({
      env: poisoned,
      issueState: issueStates,
      fetchIssue: () => ({ title: 'T', body: 'B', labels: ['type:feature'] }),
      classify,
      runAgentPhase: agentStub(PHASE_RESULTS, (opts) => {
        order.push(opts.phase);
        // The phased agent env must never carry GH_TOKEN or denied prefixes.
        expect(opts.env).not.toHaveProperty('GH_TOKEN');
        expect(Object.keys(opts.env).some((k) => k.startsWith('MATRIX_') || k.startsWith('ADW_') || k.startsWith('MX_AGENT_'))).toBe(false);
        if (opts.phase === 'review') {
          // Simulate the agent authoring commit/PR text to workspace files.
          mkdirSync(opts.state.workspace(), { recursive: true });
          writeFileSync(commitMessagePath(opts.state), 'feat: phased pipeline\n\ncloses #5', 'utf8');
          writeFileSync(prBodyPath(opts.state), 'Closes #5\n\nImplements the thing.', 'utf8');
        }
      }),
    });

    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);
    // e2e and document are gated off for an internal feature touching src/lib.rs.
    expect(order).toEqual(['classify', 'plan', 'implement', 'tests', 'review']);

    const state = AdwState.load(loadedId());
    expect(state).not.toBeNull();
    expect(state?.completedPhases).toContain('merge');
    // The agent-authored commit message (artifact file) was absorbed into state.
    expect(state?.commitMessage).toBe('feat: phased pipeline\n\ncloses #5');
    expect(state?.prBody ?? '').toContain('Implements the thing.');
    // TS-additive observability fields are recorded.
    expect(state?.engine).toBe('ts');
    expect(state?.runner).toBe('claude');
    expect(state?.workItem).toEqual({ provider: 'github', type: 'issue', id: '5', number: 5, title: 'T' });
    expect(state?.prNumber).toBe(42);
    expect(state?.changeRequest).toEqual({
      provider: 'github',
      type: 'pull_request',
      id: '42',
      number: 42,
      url: 'https://x/pull/42',
    });
    expect(state?.totalCostUsd).toBeCloseTo(0.01);
  });

  it('threads ADW_PARITY_FORCE_FENCED_JSON=1 to runAgentPhase as forceFenced (measurement mode)', async () => {
    const seen: boolean[] = [];
    const deps = testDeps({
      env: { PATH: '/bin', ANTHROPIC_API_KEY: 'sk-ant-x', ADW_PARITY_FORCE_FENCED_JSON: '1' },
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      fetchIssue: () => ({ title: 'T', body: 'B', labels: ['type:feature'] }),
      classify: vi.fn(async () => ({ value: { issue_class: 'feat' as const, reason: 'r' }, usage: { costUsd: 0 } })),
      runAgentPhase: agentStub(PHASE_RESULTS, (opts) => {
        seen.push(opts.forceFenced === true);
        if (opts.phase === 'review') {
          mkdirSync(opts.state.workspace(), { recursive: true });
          writeFileSync(commitMessagePath(opts.state), 'feat: x\n\ncloses #5', 'utf8');
          writeFileSync(prBodyPath(opts.state), 'b', 'utf8');
        }
      }),
    });
    await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);
    expect(seen.length).toBeGreaterThan(0);
    expect(seen.every(Boolean)).toBe(true); // every agent phase got forceFenced:true
  });

  it('moves the work item to the configured doneStatus after a verified merge', async () => {
    // GitHub auto-closes the issue via "closes #<n>", but a project may also
    // want its Projects board moved to a terminal column on merge. Opt-in via
    // doneStatus; unset by default, so existing runs are unaffected.
    setAdwConfigForTests(parseAdwConfig({ providers: { workItems: { type: 'github', doneStatus: 'Done' } } }));
    const setStatus = vi.fn();
    const deps = testDeps({
      setStatus,
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      runAgentPhase: agentStub(PHASE_RESULTS),
    });
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);
    // setup moves it to In Progress; the verified merge moves it to Done.
    expect(setStatus).toHaveBeenCalledWith('/bin/gh', 'o', 5, 'In Progress');
    expect(setStatus).toHaveBeenCalledWith('/bin/gh', 'o', 5, 'Done');
  });

  it('does not move work-item status post-merge when no doneStatus is configured', async () => {
    // Default config: only the setup In-Progress move happens — no terminal
    // transition (GitHub's auto-close is the terminal signal).
    const setStatus = vi.fn();
    const deps = testDeps({
      setStatus,
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      runAgentPhase: agentStub(PHASE_RESULTS),
    });
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);
    expect(setStatus).toHaveBeenCalledTimes(1);
    expect(setStatus).toHaveBeenCalledWith('/bin/gh', 'o', 5, 'In Progress');
  });

  it('runs a registered plain custom phase through the generic path and records it', async () => {
    // 'audit' is a project-registered custom phase placed in the chain. The
    // startup preflight (validatePhaseChain) requires it to be fully wired, so
    // supply a template (resolved via the claude runner root) and a schema;
    // built-in phases still resolve from the configured default prompt root.
    const promptDir = mkdtempSync(join(tmpdir(), 'adw-audit-prompt-'));
    writeFileSync(join(promptDir, 'audit.md'), 'Audit the change: $1', 'utf8');
    const schemaDir = mkdtempSync(join(tmpdir(), 'adw-audit-schema-'));
    writeFileSync(
      join(schemaDir, 'audit.json'),
      JSON.stringify({ type: 'object', properties: { summary: { type: 'string' }, risk: { type: 'string' } }, required: ['summary'] }),
      'utf8',
    );
    setAdwConfigForTests(
      parseAdwConfig({
        customPhases: ['audit'],
        phases: ['classify', 'plan', 'implement', 'audit'],
        prompts: { defaultRoot: '.pi/prompts', runnerRoots: { claude: promptDir } },
        schemas: { root: schemaDir },
      }),
    );
    const order: string[] = [];
    const deps = testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      runAgentPhase: agentStub({ ...PHASE_RESULTS, audit: { summary: 'audited', risk: 'low' } }, (opts) =>
        order.push(opts.phase),
      ),
    });
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);
    // classify runs on the shared SDK path; plan/implement/audit through the runner.
    expect(order).toEqual(['plan', 'implement', 'audit']);
    expect(AdwState.load(loadedId())?.completedPhases).toContain('audit');
  });

  /** Wire a custom phase's template (claude runner root) + schema for run() tests. */
  function customPhaseDirs(name: string, schema: Record<string, unknown>): { promptDir: string; schemaDir: string } {
    const promptDir = mkdtempSync(join(tmpdir(), 'adw-cf-prompt-'));
    writeFileSync(join(promptDir, `${name}.md`), `${name}: $1`, 'utf8');
    const schemaDir = mkdtempSync(join(tmpdir(), 'adw-cf-schema-'));
    writeFileSync(join(schemaDir, `${name}.json`), JSON.stringify(schema), 'utf8');
    return { promptDir, schemaDir };
  }

  it('skips a gated custom phase when the change signal misses, runs it when it hits', async () => {
    const { promptDir, schemaDir } = customPhaseDirs('audit', {
      type: 'object',
      properties: { summary: { type: 'string' } },
      required: ['summary'],
    });
    const mkConfig = () =>
      parseAdwConfig({
        customPhases: ['audit'],
        phases: ['classify', 'plan', 'implement', 'audit'],
        prompts: { defaultRoot: '.pi/prompts', runnerRoots: { claude: promptDir } },
        schemas: { root: schemaDir },
        gates: { custom: { audit: { hints: ['payment'] } } },
      });

    // (1) signal misses → audit gated out (recorded done, agent never called).
    setAdwConfigForTests(mkConfig());
    const missOrder: string[] = [];
    const rcMiss = await run(5, createMockRunner(), { yes: true, noProgress: true }, testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      fetchIssue: () => ({ title: 'unrelated change', body: 'b', labels: [] }),
      runAgentPhase: agentStub({ ...PHASE_RESULTS, audit: { summary: 'x' } }, (o) => missOrder.push(o.phase)),
    }));
    expect(rcMiss).toBe(0);
    expect(missOrder).not.toContain('audit');
    expect(AdwState.load(loadedId())?.completedPhases).toContain('audit'); // recorded as skipped

    // (2) signal hits ("payment" in the title) → audit runs. (loadedId() above
    // already asserted exactly one run dir before this second run is minted.)
    setAdwConfigForTests(mkConfig());
    const hitOrder: string[] = [];
    const rcHit = await run(7, createMockRunner(), { yes: true, noProgress: true }, testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      fetchIssue: () => ({ title: 'payment flow update', body: 'b', labels: [] }),
      runAgentPhase: agentStub({ ...PHASE_RESULTS, audit: { summary: 'x' } }, (o) => hitOrder.push(o.phase)),
    }));
    expect(rcHit).toBe(0);
    expect(hitOrder).toContain('audit');
  });

  it('runs a looped custom phase: red command then green', async () => {
    const { promptDir, schemaDir } = customPhaseDirs('verify', {
      type: 'object',
      properties: { resolved: { type: 'integer' }, remaining: { type: 'integer' } },
      required: ['resolved'],
    });
    setAdwConfigForTests(
      parseAdwConfig({
        customPhases: ['verify'],
        phases: ['classify', 'plan', 'implement', 'verify'],
        prompts: { defaultRoot: '.pi/prompts', runnerRoots: { claude: promptDir } },
        schemas: { root: schemaDir },
        loops: { verify: { command: 'npm run verify', maxAttempts: 3 } },
      }),
    );
    // First run of the verify command fails, the agent "fixes", the rerun is green.
    const runCmd = vi.fn().mockReturnValueOnce({ rc: 1, output: 'verify failed' }).mockReturnValue({ rc: 0, output: '' });
    const order: string[] = [];
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true }, testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      runCmd,
      runAgentPhase: agentStub({ ...PHASE_RESULTS, verify: { resolved: 1, remaining: 0 } }, (o) => order.push(o.phase)),
    }));
    expect(rc).toBe(0);
    expect(order).toContain('verify'); // the loop invoked the verify agent to fix the red command
    expect(runCmd).toHaveBeenCalledWith(['npm', 'run', 'verify']);
    expect(AdwState.load(loadedId())?.completedPhases).toContain('verify');
  });

  it('uses first-class providers when supplied, not the legacy provider-shaped seams', async () => {
    const fail = (name: string) => () => {
      throw new Error(`legacy seam called: ${name}`);
    };
    const issueStates = vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED');
    const providers: AdwProviders = {
      cli: {
        resolveExecutable: vi.fn(() => '/bin/gh'),
        detectRepository: vi.fn(() => 'o/r'),
      },
      workItems: {
        fetch: vi.fn(() => ({ title: 'T', body: 'B', labels: ['type:feature'] })),
        state: issueStates,
        postProgress: vi.fn(),
        assignSelf: vi.fn(),
        setStatus: vi.fn(),
      },
      vcs: {
        workingTreeDirty: vi.fn(() => false),
        changedFiles: vi.fn(() => ['src/lib.rs']),
        createOrCheckoutBranch: vi.fn(() => ({ ok: true, error: null })),
        commitAll: vi.fn(() => ({ ok: true, error: null })),
        push: vi.fn(() => ({ ok: true, error: null })),
        pullRebase: vi.fn(() => ({ ok: true, error: null })),
      },
      changeRequests: {
        findForBranch: vi.fn(() => null),
        create: vi.fn(() => ({ id: '42', number: 42, url: 'https://x/pull/42', error: null })),
        pipelineStatus: vi.fn(() => ({ state: 'success' as const, failingJobs: [] })),
        squashMerge: vi.fn(() => ({ ok: true, error: null })),
      },
    };
    const deps = testDeps({
      providers,
      resolveGhBin: fail('resolveGhBin'),
      detectRepo: fail('detectRepo'),
      issueState: fail('issueState'),
      postProgress: fail('postProgress'),
      fetchIssue: fail('fetchIssue'),
      setStatus: fail('setStatus'),
      workingTreeDirty: fail('workingTreeDirty'),
      changedFiles: fail('changedFiles'),
      git: {
        createOrCheckoutBranch: fail('git.createOrCheckoutBranch'),
        commitAll: fail('git.commitAll'),
        push: fail('git.push'),
        pullRebase: fail('git.pullRebase'),
        prForBranch: fail('git.prForBranch'),
        createPr: fail('git.createPr'),
        ciStatus: fail('git.ciStatus'),
        squashMerge: fail('git.squashMerge'),
      },
      runAgentPhase: agentStub(PHASE_RESULTS),
    });

    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);
    expect(providers.workItems.assignSelf).toHaveBeenCalledWith({ ghBin: '/bin/gh', repo: 'o/r' }, 5);
    expect(providers.workItems.setStatus).toHaveBeenCalledWith({ ghBin: '/bin/gh', repo: 'o/r' }, 5, 'In Progress');
    expect(providers.changeRequests.create).toHaveBeenCalledWith(
      { ghBin: '/bin/gh', repo: 'o/r' },
      { branch: expect.stringMatching(/^feat\/5-/), title: 'Implement issue #5', body: 'Closes #5', base: 'main' },
    );
    expect(providers.changeRequests.squashMerge).toHaveBeenCalledWith({ ghBin: '/bin/gh', repo: 'o/r' }, 42);
    const state = AdwState.load(loadedId());
    expect(state?.issueNumber).toBe('5');
    expect(state?.workItem).toEqual({ provider: 'github', type: 'issue', id: '5', number: 5, title: 'T' });
    expect(state?.prNumber).toBe(42);
    expect(state?.prUrl).toBe('https://x/pull/42');
    expect(state?.changeRequest).toEqual({
      provider: 'github',
      type: 'pull_request',
      id: '42',
      number: 42,
      url: 'https://x/pull/42',
    });
  });

  it('calls runner.start before the phases and runner.stop in a finally, even when a phase throws (D6 lifecycle)', async () => {
    const calls: string[] = [];
    const lifecycleRunner = (): AgentRunner => ({
      ...createMockRunner(),
      start: async () => {
        calls.push('start');
      },
      stop: async () => {
        calls.push('stop');
      },
    });

    const okDeps = testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      runAgentPhase: agentStub(PHASE_RESULTS, () => {}),
    });
    await run(5, lifecycleRunner(), { yes: true, noProgress: true }, okDeps);
    expect(calls).toEqual(['start', 'stop']);

    calls.length = 0;
    const failingDeps = testDeps({
      issueState: vi.fn().mockReturnValue('OPEN'),
      runAgentPhase: (() => {
        throw new AdwError('phase exploded');
      }) as unknown as typeof runAgentPhase,
    });
    await expect(run(6, lifecycleRunner(), { yes: true, noProgress: true }, failingDeps)).rejects.toThrow(
      'phase exploded',
    );
    expect(calls).toEqual(['start', 'stop']);
  });

  it('resumes by skipping completed phases', async () => {
    const pre = new AdwState({ adwId: 'a1b2c3d4', issueNumber: '5', branchName: 'feat/5-x' });
    for (const ph of ['setup', 'classify', 'plan', 'implement', 'tests', 'resolve', 'e2e', 'review', 'patch', 'document']) {
      pre.markDone(ph);
    }
    pre.commitMessage = 'feat: x\n\ncloses #5';
    pre.prNumber = 42;
    pre.save();

    const deps = testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      git: { prForBranch: () => 'https://x/pull/42' },
      classify: async () => {
        throw new Error('no phase should run on resume');
      },
    });
    const rc = await run(5, createMockRunner(), { adwId: 'a1b2c3d4', resume: true, yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);
  });

  it('short-circuits finalize after a recorded merge (no re-merge, no commit)', async () => {
    const pre = new AdwState({ adwId: 'a1b2c3d4', issueNumber: '5', branchName: 'feat/5-x' });
    for (const ph of ['setup', 'classify', 'plan', 'implement', 'tests', 'resolve', 'e2e', 'review', 'patch', 'document', 'merge']) {
      pre.markDone(ph);
    }
    pre.prNumber = 42;
    pre.save();

    const squashMerge = vi.fn(() => ({ ok: true, error: null }));
    const commitAll = vi.fn(() => ({ ok: true, error: null }));
    const deps = testDeps({
      // OPEN at preflight (so the run proceeds), CLOSED at the re-verify.
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValue('CLOSED'),
      git: { squashMerge, commitAll },
    });
    const rc = await run(5, createMockRunner(), { adwId: 'a1b2c3d4', resume: true, yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);
    expect(squashMerge).not.toHaveBeenCalled();
    expect(commitAll).not.toHaveBeenCalled();
  });

  it('requires --adw-id with --resume', async () => {
    await expect(run(5, createMockRunner(), { resume: true, yes: true, noProgress: true }, testDeps())).rejects.toThrow(
      /--resume requires --adw-id/,
    );
  });

  it('refuses a bare --adw-id that would clobber existing state', async () => {
    new AdwState({ adwId: 'a1b2c3d4', issueNumber: '5' }).save();
    await expect(
      run(5, createMockRunner(), { adwId: 'a1b2c3d4', yes: true, noProgress: true }, testDeps()),
    ).rejects.toThrow(/already has saved state/);
  });

  it('rejects resuming a run that belongs to a different issue', async () => {
    const pre = new AdwState({ adwId: 'a1b2c3d4', issueNumber: '5' });
    pre.markDone('setup');
    pre.save();
    await expect(
      run(9, createMockRunner(), { adwId: 'a1b2c3d4', resume: true, yes: true, noProgress: true }, testDeps()),
    ).rejects.toThrow(/belongs to work item #5/);
  });

  it('rejects a dirty tree on a fresh run but tolerates it on resume', async () => {
    const deps = testDeps({ workingTreeDirty: () => true });
    await expect(run(5, createMockRunner(), { yes: true, noProgress: true }, deps)).rejects.toThrow(
      /working tree is dirty/,
    );
  });

  it('recovers persisted review findings for the patch phase on resume', async () => {
    // review done with a persisted blocker, patch NOT done -> on resume the
    // patch phase must still see the blocker (regression: findings were lost).
    const pre = new AdwState({ adwId: 'a1b2c3d4', issueNumber: '5', branchName: 'feat/5-x' });
    for (const ph of ['setup', 'classify', 'plan', 'implement', 'tests', 'resolve', 'e2e', 'review']) {
      pre.markDone(ph);
    }
    pre.reviewFindings = [{ severity: 'blocker', description: 'bug', location: 'a.rs:1' }];
    pre.commitMessage = 'feat: x\n\ncloses #5';
    pre.prNumber = 42;
    pre.save();

    const patchPrompts: string[] = [];
    const deps = testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      git: { prForBranch: () => 'https://x/pull/42' },
      runAgentPhase: agentStub(
        {
          patch: { resolved: 1, remaining: 0, summary: '' },
          document: { docs_updated: false, files: [], summary: '', wrote_commit_message: false, wrote_pr_body: false },
        },
        (opts) => {
          if (opts.phase === 'patch') {
            patchPrompts.push(String(opts.templateArgs[0]));
          }
        },
      ),
    });
    const rc = await run(5, createMockRunner(), { adwId: 'a1b2c3d4', resume: true, yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);
    expect(patchPrompts).toHaveLength(1);
    expect(patchPrompts[0]).toContain('[blocker] (a.rs:1) bug');
  });

  it('tolerates additive/junk finding entries from a foreign writer', async () => {
    // state.json is the cross-language contract: finding objects may carry
    // additive keys, omit optional ones, or be junk. Resume must coerce,
    // never crash — write the document by hand to include a non-dict entry.
    const ws = join(tmp, 'a1b2c3d4');
    mkdirSync(ws, { recursive: true });
    writeFileSync(
      join(ws, 'state.json'),
      JSON.stringify({
        adw_id: 'a1b2c3d4',
        schema_version: 1,
        issue_number: '5',
        branch_name: 'feat/5-x',
        base: 'main',
        commit_message: 'feat: x\n\ncloses #5',
        pr_number: 42,
        review_findings: [
          { severity: 'blocker', description: 'bug', location: 'a.rs:1', file: 'a.rs' },
          { description: 'no severity recorded' },
          'not-a-dict',
        ],
        completed_phases: ['setup', 'classify', 'plan', 'implement', 'tests', 'resolve', 'e2e', 'review'],
      }),
      'utf8',
    );

    const patchPrompts: string[] = [];
    const deps = testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      git: { prForBranch: () => 'https://x/pull/42' },
      runAgentPhase: agentStub(
        {
          patch: { resolved: 1, remaining: 0, summary: '' },
          document: { docs_updated: false, files: [], summary: '', wrote_commit_message: false, wrote_pr_body: false },
        },
        (opts) => {
          if (opts.phase === 'patch') {
            patchPrompts.push(String(opts.templateArgs[0]));
          }
        },
      ),
    });
    const rc = await run(5, createMockRunner(), { adwId: 'a1b2c3d4', resume: true, yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);
    // The blocker (additive key ignored) is patched; the severity-less
    // finding coerces to skippable; the non-dict entry is dropped.
    expect(patchPrompts).toHaveLength(1);
    expect(patchPrompts[0]).toContain('[blocker] (a.rs:1) bug');
    expect(patchPrompts[0]).not.toContain('no severity recorded');
  });

  it('refuses to merge when CI cannot go green', async () => {
    const deps = testDeps({
      issueState: vi.fn().mockReturnValue('OPEN'),
      classify: async () => ({ value: { issue_class: 'feat', reason: 'r' }, usage: {} }),
      runAgentPhase: agentStub(PHASE_RESULTS),
      git: { ciStatus: () => ({ state: 'unknown', failingJobs: [] }) },
    });
    await expect(run(5, createMockRunner(), { yes: true, noProgress: true }, deps)).rejects.toThrow(
      /CI is not green/,
    );
  });

  it('surfaces the failing output and aborts before merge when a finalize gate cannot be healed', async () => {
    // Extra (non-test) pre-merge gates are configured via ADW_FINALIZE_GATES;
    // one that returns non-zero must block the merge. The orchestrator now
    // surfaces the failing output and gives the agent a bounded chance to fix
    // it, but when the agent changes nothing it still aborts (never merges red)
    // — and the thrown error carries the gate output, not a bare "gate failed".
    const runCmd = vi.fn((cmd: readonly string[]) =>
      cmd.join(' ') === 'check:fmt' ? { rc: 1, output: 'fmt diff' } : { rc: 0, output: '' },
    );
    const squashMerge = vi.fn(() => ({ ok: true, error: null }));
    const deps = testDeps({
      env: { PATH: '/bin', ANTHROPIC_API_KEY: 'sk-ant-test', ADW_FINALIZE_GATES: 'check:fmt' },
      issueState: vi.fn().mockReturnValue('OPEN'),
      runCmd,
      workingTreeDirty: () => false,
      git: { squashMerge },
      runAgentPhase: agentStub({ ...PHASE_RESULTS, resolve: { resolved: 0, remaining: 0, summary: '' } }),
    });
    await expect(run(5, createMockRunner(), { yes: true, noProgress: true }, deps)).rejects.toThrow(
      /pre-merge gate failed[\s\S]*check:fmt[\s\S]*fmt diff/,
    );
    expect(squashMerge).not.toHaveBeenCalled();
  });

  it('heals a fixable finalize gate then merges', async () => {
    // The lone-formatting-nit case: the gate fails once, the agent fixes the
    // cause (dirty tree, resolved > 0), the re-run passes, and the merge
    // proceeds — instead of a whole passing run being thrown away by a gate
    // that dies silently on the first non-zero exit.
    let fmtCalls = 0;
    const runCmd = vi.fn((cmd: readonly string[]) => {
      if (cmd.join(' ') === 'check:fmt') {
        fmtCalls += 1;
        return fmtCalls === 1 ? { rc: 1, output: 'fmt diff' } : { rc: 0, output: '' };
      }
      return { rc: 0, output: '' };
    });
    const squashMerge = vi.fn(() => ({ ok: true, error: null }));
    const resolvePrompts: string[] = [];
    const deps = testDeps({
      env: { PATH: '/bin', ANTHROPIC_API_KEY: 'sk-ant-test', ADW_FINALIZE_GATES: 'check:fmt' },
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValue('CLOSED'),
      runCmd,
      workingTreeDirty: () => true,
      git: { squashMerge },
      runAgentPhase: agentStub(
        { ...PHASE_RESULTS, resolve: { resolved: 1, remaining: 0, summary: '' } },
        (o) => {
          if (o.phase === 'resolve') {
            resolvePrompts.push(String(o.templateArgs[0]));
          }
        },
      ),
    });
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true, allowDirty: true }, deps);
    expect(rc).toBe(0);
    expect(squashMerge).toHaveBeenCalled();
    expect(fmtCalls).toBeGreaterThanOrEqual(2); // failed, healed, re-checked green
    expect(resolvePrompts[0]).toContain('check:fmt'); // agent got the gate + its output
  });

  it('retries a phase that hits a transient provider error, then continues to merge', async () => {
    // A momentary API 5xx in one phase (review, historically) must not throw
    // away a passing run: the phase is retried with backoff and the pipeline
    // continues. Backoff sleep is the injected no-op stub, so this is instant.
    let reviewCalls = 0;
    const runAgentPhaseStub = (async (opts: Parameters<typeof runAgentPhase>[0]) => {
      if (opts.phase === 'review') {
        reviewCalls += 1;
        if (reviewCalls === 1) {
          throw new RunnerTransientError('claude', 'review', 'Internal server error');
        }
      }
      const data = PHASE_RESULTS[opts.phase];
      if (data === undefined) {
        throw new Error(`unexpected phase: ${opts.phase}`);
      }
      return { data, usage: {}, attempts: 1 };
    }) as typeof runAgentPhase;
    const squashMerge = vi.fn(() => ({ ok: true, error: null }));
    const deps = testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValue('CLOSED'),
      git: { squashMerge },
      runAgentPhase: runAgentPhaseStub,
    });
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true, transientBackoffMs: 0 }, deps);
    expect(rc).toBe(0);
    expect(reviewCalls).toBe(2); // failed transiently once, retried, then succeeded
    expect(squashMerge).toHaveBeenCalled();
  });

  it('persists classify prompt.txt and transcript.log on the structured-call path', async () => {
    const deps = testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      runAgentPhase: agentStub(PHASE_RESULTS),
    });
    await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);
    const dir = join(tmp, loadedId(), 'classify');
    expect(readFileSync(join(dir, 'prompt.txt'), 'utf8')).toContain('GitHub issue #5');
    expect(JSON.parse(readFileSync(join(dir, 'transcript.log'), 'utf8'))).toEqual({
      issue_class: 'feat',
      reason: 'r',
    });
  });

  it('retries a normal claude runner phase without pay-as-you-go Anthropic auth after runner auth failure', async () => {
    const order: string[] = [];
    const seenEnvs: Array<Record<string, string>> = [];
    let planAttempts = 0;
    const deps = testDeps({
      env: {
        PATH: '/bin',
        ANTHROPIC_API_KEY: 'sk-ant-empty',
        ANTHROPIC_AUTH_TOKEN: 'payg-token-empty',
        CLAUDE_CODE_OAUTH_TOKEN: 'subscription-token',
      },
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      runAgentPhase: (async (opts: Parameters<typeof runAgentPhase>[0]) => {
        order.push(opts.phase);
        seenEnvs.push(opts.env);
        if (opts.phase === 'plan') {
          planAttempts += 1;
          if (planAttempts === 1) {
            throw new RunnerAuthError(
              'claude',
              'plan',
              'Claude Code used ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN and the Anthropic API credit balance is too low',
            );
          }
        }
        const data = PHASE_RESULTS[opts.phase];
        if (data === undefined) {
          throw new Error(`unexpected phase: ${opts.phase}`);
        }
        return { data, usage: {}, attempts: 1 };
      }) as typeof runAgentPhase,
    });

    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);

    expect(rc).toBe(0);
    expect(order).toEqual(['plan', 'plan', 'implement', 'tests', 'review']);
    expect(seenEnvs[0]).toMatchObject({
      ANTHROPIC_API_KEY: 'sk-ant-empty',
      ANTHROPIC_AUTH_TOKEN: 'payg-token-empty',
      CLAUDE_CODE_OAUTH_TOKEN: 'subscription-token',
    });
    for (const env of seenEnvs.slice(1)) {
      expect(env).not.toHaveProperty('ANTHROPIC_API_KEY');
      expect(env).not.toHaveProperty('ANTHROPIC_AUTH_TOKEN');
      expect(env).toHaveProperty('CLAUDE_CODE_OAUTH_TOKEN', 'subscription-token');
    }
  });

  it('falls back to the runner when shared-SDK classify hits an Anthropic API error', async () => {
    const order: string[] = [];
    const seenEnvs: Array<Record<string, string>> = [];
    const classify = vi.fn(async () => {
      throw new StructuredCallApiError(400, 'Your credit balance is too low to access the Anthropic API.');
    });
    const deps = testDeps({
      env: {
        PATH: '/bin',
        ANTHROPIC_API_KEY: 'sk-ant-test',
        ANTHROPIC_AUTH_TOKEN: 'payg-token-empty',
        CLAUDE_CODE_OAUTH_TOKEN: 'subscription-token',
      },
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      classify,
      runAgentPhase: agentStub(
        { ...PHASE_RESULTS, classify: { issue_class: 'fix', reason: 'runner fallback' } },
        (opts) => {
          order.push(opts.phase);
          seenEnvs.push(opts.env);
        },
      ),
    });

    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);

    expect(rc).toBe(0);
    expect(classify).toHaveBeenCalledTimes(1);
    expect(order).toEqual(['classify', 'plan', 'implement', 'tests', 'review']);
    expect(seenEnvs).toHaveLength(5);
    for (const env of seenEnvs) {
      expect(env).not.toHaveProperty('ANTHROPIC_API_KEY');
      expect(env).not.toHaveProperty('ANTHROPIC_AUTH_TOKEN');
      expect(env).toHaveProperty('CLAUDE_CODE_OAUTH_TOKEN', 'subscription-token');
    }
    expect(AdwState.load(loadedId())?.issueClass).toBe('fix');
  });

  it('routes classify through the runner when ADW_CLASSIFY_ON_RUNNER=1', async () => {
    const order: string[] = [];
    const classify = vi.fn(async () => ({ value: { issue_class: 'feat' as const, reason: 'r' }, usage: {} }));
    const deps = testDeps({
      env: { PATH: '/bin', ADW_CLASSIFY_ON_RUNNER: '1' },
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      classify,
      runAgentPhase: agentStub(
        { ...PHASE_RESULTS, classify: { issue_class: 'feat', reason: 'r' } },
        (opts) => order.push(opts.phase),
      ),
    });
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);
    expect(classify).not.toHaveBeenCalled();
    expect(order).toEqual(['classify', 'plan', 'implement', 'tests', 'review']);
    expect(AdwState.load(loadedId())?.issueClass).toBe('feat');
  });

  it('routes classify through the runner when no ANTHROPIC_API_KEY (subscription mode)', async () => {
    const order: string[] = [];
    const classify = vi.fn(async () => ({ value: { issue_class: 'feat' as const, reason: 'r' }, usage: {} }));
    const deps = testDeps({
      // No ANTHROPIC_API_KEY → the shared-SDK classify path is unavailable, so
      // classify must fall back to the selected runner (the subscription path),
      // without the operator needing to set ADW_CLASSIFY_ON_RUNNER=1.
      env: { PATH: '/bin' },
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      classify,
      runAgentPhase: agentStub(
        { ...PHASE_RESULTS, classify: { issue_class: 'feat', reason: 'r' } },
        (opts) => order.push(opts.phase),
      ),
    });
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);
    expect(classify).not.toHaveBeenCalled();
    expect(order).toEqual(['classify', 'plan', 'implement', 'tests', 'review']);
    expect(AdwState.load(loadedId())?.issueClass).toBe('feat');
  });

  it('inheritEnv is an explicit opt-out that forwards the full parent env (Python --inherit-env parity)', async () => {
    const poisoned = { GH_TOKEN: 'ghp_secret', PATH: '/bin', ADW_FOO: 'x', MX_AGENT_FOO: 'x', ANTHROPIC_API_KEY: 'sk-ant-x' };
    const seenEnvs: Array<Record<string, string>> = [];
    const deps = testDeps({
      env: poisoned,
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      runAgentPhase: agentStub(PHASE_RESULTS, (opts) => seenEnvs.push(opts.env)),
    });
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true, inheritEnv: true }, deps);
    expect(rc).toBe(0);
    // Opt-OUT: the documented less-isolated mode forwards everything…
    expect(seenEnvs[0]).toEqual(poisoned);
    // …and remains strictly opt-in: the same run without the flag is covered
    // by the 'runs phases in order' test, which asserts GH_TOKEN is absent.
  });

  it('poisons total_cost_usd to null once any phase cost is unknown', async () => {
    const costs: Array<number | null | undefined> = [0.02, null, 0.01, undefined];
    let call = 0;
    const deps = testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      classify: async () => ({ value: { issue_class: 'feat', reason: 'r' }, usage: { costUsd: 0.05 } }),
      runAgentPhase: (async (opts: Parameters<typeof runAgentPhase>[0]) => ({
        data: PHASE_RESULTS[opts.phase],
        usage: { costUsd: costs[call++ % costs.length] },
      })) as typeof runAgentPhase,
    });
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);
    // 0.05 (classify) + 0.02 (plan) accumulate, then null (implement) poisons
    // the total for good — never a false partial sum.
    expect(AdwState.load(loadedId())?.totalCostUsd).toBeNull();
  });

  it('previews the plan under dry-run without touching anything', async () => {
    const log = vi.spyOn(console, 'log').mockImplementation(() => {});
    const resolveGhBin = vi.fn(() => '/bin/gh');
    const rc = await run(5, createMockRunner(), { dryRun: true }, testDeps({ resolveGhBin }));
    expect(rc).toBe(0);
    expect(resolveGhBin).not.toHaveBeenCalled();
    expect(log.mock.calls.flat().join('\n')).toContain('GH_TOKEN withheld');
    expect(readdirSync(tmp)).toEqual([]); // no workspace minted
  });

  it('threads timeoutMs from RunOptions into every runAgentPhase call (Drill 1 path)', async () => {
    const seen: number[] = [];
    const deps = testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      runAgentPhase: agentStub(PHASE_RESULTS, (opts) => {
        seen.push(opts.timeoutMs ?? -1);
        if (opts.phase === 'review') {
          mkdirSync(opts.state.workspace(), { recursive: true });
          writeFileSync(commitMessagePath(opts.state), 'feat: x\n\ncloses #5', 'utf8');
          writeFileSync(prBodyPath(opts.state), 'b', 'utf8');
        }
      }),
    });
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true, timeoutMs: 30_000 }, deps);
    expect(rc).toBe(0);
    expect(seen.length).toBeGreaterThan(0);
    expect(seen.every((ms) => ms === 30_000)).toBe(true);
  });

  it('threads maxBudgetUsd from RunOptions into every runAgentPhase call (Drill 2 path)', async () => {
    const seen: Array<number | undefined> = [];
    const deps = testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      runAgentPhase: agentStub(PHASE_RESULTS, (opts) => {
        seen.push(opts.maxBudgetUsd);
        if (opts.phase === 'review') {
          mkdirSync(opts.state.workspace(), { recursive: true });
          writeFileSync(commitMessagePath(opts.state), 'feat: x\n\ncloses #5', 'utf8');
          writeFileSync(prBodyPath(opts.state), 'b', 'utf8');
        }
      }),
    });
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true, maxBudgetUsd: 2.5 }, deps);
    expect(rc).toBe(0);
    expect(seen.length).toBeGreaterThan(0);
    expect(seen.every((usd) => usd === 2.5)).toBe(true);
  });

  it('writes per-phase metrics.json (cost/duration/attempts) alongside state.json', async () => {
    // A deterministic stepped clock so durations are stable: each start()/record()
    // pair advances 1000ms.
    let t = 0;
    const now = () => (t += 1000);
    const deps = testDeps({
      now,
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      classify: async () => ({ value: { issue_class: 'feat' as const, reason: 'r' }, usage: { costUsd: 0.05 } }),
      runAgentPhase: (async (opts: Parameters<typeof runAgentPhase>[0]) => {
        if (opts.phase === 'review') {
          mkdirSync(opts.state.workspace(), { recursive: true });
          writeFileSync(commitMessagePath(opts.state), 'feat: x\n\ncloses #5', 'utf8');
          writeFileSync(prBodyPath(opts.state), 'b', 'utf8');
        }
        // implement "nudged" (2 attempts); the rest clean.
        return {
          data: PHASE_RESULTS[opts.phase],
          usage: { costUsd: 0.1, inputTokens: 20, outputTokens: 4 },
          attempts: opts.phase === 'implement' ? 2 : 1,
        };
      }) as typeof runAgentPhase,
    });
    const rc = await run(5, createMockRunner(), { yes: true, noProgress: true }, deps);
    expect(rc).toBe(0);

    const doc = JSON.parse(readFileSync(join(tmp, loadedId(), 'metrics.json'), 'utf8'));
    // classify + plan + implement + tests + review = 5 recorded phases.
    expect(doc.summary.phases).toBe(5);
    expect(doc.summary.attempts).toBe(6); // one phase nudged (2), four clean (1)
    expect(doc.summary.nudged_phases).toBe(1);
    expect(doc.summary.total_cost_usd).toBeCloseTo(0.05 + 0.1 * 4, 10);
    const byPhase = Object.fromEntries(doc.phases.map((p: { phase: string }) => [p.phase, p]));
    expect(byPhase.implement.attempts).toBe(2);
    expect(byPhase.plan.duration_ms).toBe(1000); // one clock step per phase
    expect(byPhase.plan.model).toBe('claude-opus-4-8'); // capable tier on claude
    expect(byPhase.classify.model).toBe('claude-haiku-4-5'); // classify model
  });

  it('aborts mid-chain when the accumulated cost exceeds --max-budget-usd (soft parent gate)', async () => {
    // pi has no native budget cap, so the parent-side gate is the only stop.
    const order: string[] = [];
    const deps = testDeps({
      issueState: vi.fn().mockReturnValueOnce('OPEN').mockReturnValueOnce('CLOSED'),
      classify: async () => ({ value: { issue_class: 'feat' as const, reason: 'r' }, usage: { costUsd: 0.5 } }),
      runAgentPhase: agentStub(PHASE_RESULTS, (opts) => {
        order.push(opts.phase);
      }, { costUsd: 0.5 }),
    });
    // classify (0.5) + plan (0.5) = 1.0 > cap 0.9 -> abort before implement.
    await expect(
      run(5, createMockRunner({ id: 'pi', caps: { nativeBudget: false } }), {
        yes: true,
        noProgress: true,
        maxBudgetUsd: 0.9,
      }, deps),
    ).rejects.toThrow(/exceeded the budget cap/);
    expect(order).toEqual(['plan']); // implement never started
    // The partial run still left a metrics.json for the phases that did run.
    expect(existsSync(join(tmp, loadedId(), 'metrics.json'))).toBe(true);
  });

  it('rejects a misconfigured custom phase at startup, before the dry-run plan prints', async () => {
    // 'audit' is registered and in the chain but has no template/schema. The
    // startup preflight runs even under --dry-run (which doubles as a config
    // check), so the chain is rejected up front rather than a plan being
    // printed for an unwireable pipeline.
    const log = vi.spyOn(console, 'log').mockImplementation(() => {});
    log.mockClear(); // a prior dry-run test may have left calls on a shared spy
    setAdwConfigForTests(parseAdwConfig({ customPhases: ['audit'], phases: ['plan', 'implement', 'audit'] }));
    await expect(run(5, createMockRunner(), { dryRun: true }, testDeps())).rejects.toThrow(
      /phase "audit" is missing its prompt template/,
    );
    expect(log.mock.calls.flat().join('\n')).not.toContain('[dry-run]'); // preflight threw before printPlan
    expect(readdirSync(tmp)).toEqual([]); // nothing minted
  });
});
