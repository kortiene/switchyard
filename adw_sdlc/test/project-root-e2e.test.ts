/**
 * End-to-end integration tests for issue #56: explicit external project root.
 *
 * These tests drive run() through the complete pipeline (non-dry-run) with a
 * real temp directory as the external project root. They verify system-level
 * integration that the unit/integration tests in project-root.test.ts do not:
 *
 *  E1  State files are persisted under the external project root's agents/ dir
 *      (AC4 across the full pipeline, not just agentsDir() in isolation).
 *  E2  The external config's defaultTestCommand flows through resolveOptions
 *      into the resolve loop gate command (AC2/AC7 in a real run, not dry-run).
 *  E3  Nothing is written under REPO_ROOT/agents/ for a run targeting an
 *      external project root (AC5 regression guard for state location).
 *  E4  run() with an invalid project root fails closed before any side effects,
 *      propagating an actionable AdwError (AC6 at the full-pipeline boundary).
 *
 * These tests are complementary to project-root.test.ts (unit/integration);
 * they share the same afterEach teardown contract (reset all process-global
 * overrides) but run the full orchestrator pipeline with mocked deps.
 */

import { existsSync, mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { REPO_ROOT } from '../src/common.js';
import { setAdwConfigForTests } from '../src/config.js';
import { setProjectRoot } from '../src/common.js';
import { AdwError } from '../src/errors.js';
import { run, type OrchestratorDeps } from '../src/orchestrator.js';
import { commitMessagePath, prBodyPath } from '../src/phases.js';
import { runAgentPhase } from '../src/run-phase.js';
import { createMockRunner } from '../src/runners/runner-mock.js';
import { AdwState, agentsDir, setAgentsDir } from '../src/state.js';

// ---------------------------------------------------------------------------
// Shared cleanup — reset all process-global overrides after every test.
// ---------------------------------------------------------------------------

afterEach(() => {
  setProjectRoot(null);
  setAgentsDir(null);
  setAdwConfigForTests(null);
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function realpath(p: string): string {
  return realpathSync(p);
}

type DepsOverride = Partial<Omit<OrchestratorDeps, 'git'>> & { git?: Partial<OrchestratorDeps['git']> };

/** Full deps with inert stubs; override only the seams under test. */
function testDeps(overrides: DepsOverride = {}): OrchestratorDeps {
  const base: OrchestratorDeps = {
    // ANTHROPIC_API_KEY present → classify uses the shared SDK path (deps.classify).
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
    postProgress: () => {},
    fetchIssue: () => ({ title: 'T', body: 'B', labels: [] }),
    setStatus: () => {},
    git: {
      createOrCheckoutBranch: () => ({ ok: true, error: null }),
      commitAll: () => ({ ok: true, error: null }),
      push: () => ({ ok: true, error: null }),
      pullRebase: () => ({ ok: true, error: null }),
      syncWithBase: () => ({ ok: true, rebased: false, error: null }),
      prForBranch: () => null,
      createPr: () => ({ number: 42, url: 'https://x/pull/42', error: null }),
      ciStatus: () => ({ state: 'success', failingJobs: [] }),
      squashMerge: () => ({ ok: true, error: null }),
    },
    runAgentPhase: (async () => {
      throw new Error('runAgentPhase not stubbed for this test');
    }) as typeof runAgentPhase,
    classify: async () => ({ value: { issue_class: 'feat' as const, reason: 'r' }, usage: {} }),
    fileExists: () => true,
  };
  return { ...base, ...overrides, git: { ...base.git, ...(overrides.git ?? {}) } };
}

/**
 * Scripted runAgentPhase stub that returns canned results per phase.
 * The `onCall` callback is invoked before each result is returned, giving
 * tests a hook to inspect the call (e.g., write workspace artifacts).
 */
function agentStub(
  results: Record<string, unknown>,
  onCall?: (opts: Parameters<typeof runAgentPhase>[0]) => void,
): typeof runAgentPhase {
  return (async (opts) => {
    onCall?.(opts);
    const data = results[opts.phase];
    if (data === undefined) {
      throw new Error(`unexpected phase in agentStub: ${opts.phase}`);
    }
    return { data, usage: {}, attempts: 1 };
  }) as typeof runAgentPhase;
}

/**
 * Minimal phase results for the default chain
 * (classify via SDK → plan → implement → tests → resolve [gate only] → review → [merge]).
 * e2e and document are gated off for an internal 'src/lib.rs' change.
 * patch is a no-op when review findings are empty.
 */
const PHASE_RESULTS: Record<string, unknown> = {
  plan: { plan_file: 'specs/x.md', spec_created: true, summary: '' },
  implement: { summary: 'did it', files_changed: ['src/lib.rs'] },
  tests: { tests_added: true, summary: '' },
  review: { findings: [], wrote_commit_message: true, wrote_pr_body: true },
};

/** Write a minimal .adw/config.json into dir/.adw/. */
function writeExtConfig(dir: string, opts: { testCommand?: string } = {}): void {
  mkdirSync(join(dir, '.adw'), { recursive: true });
  writeFileSync(
    join(dir, '.adw', 'config.json'),
    JSON.stringify({
      version: 1,
      project: { id: 'ext-proj', name: 'External Project' },
      commands: { defaultTestCommand: opts.testCommand ?? '' },
    }),
  );
}

/** Return the single sub-directory name under dir, or '' if none. */
function singleSubdir(dir: string): string {
  if (!existsSync(dir)) {
    return '';
  }
  const entries = readdirSync(dir, { withFileTypes: true })
    .filter((e) => e.isDirectory())
    .map((e) => e.name);
  return entries[0] ?? '';
}

// ---------------------------------------------------------------------------
// E1/E3: State persistence under the external project root
// ---------------------------------------------------------------------------

describe('full pipeline — state persistence (E1/E3)', () => {
  it('persists run state under external-root/agents/ and nowhere else (AC4)', async () => {
    const ext = mkdtempSync(join(tmpdir(), 'adw-e2e-state-'));
    const rext = realpath(ext);
    try {
      writeExtConfig(ext);

      const deps = testDeps({
        runAgentPhase: agentStub(PHASE_RESULTS, (opts) => {
          if (opts.phase === 'review') {
            // Simulate the review agent authoring commit/PR text artifacts.
            mkdirSync(opts.state.workspace(), { recursive: true });
            writeFileSync(commitMessagePath(opts.state), 'feat: x\n\ncloses #5', 'utf8');
            writeFileSync(prBodyPath(opts.state), 'Closes #5', 'utf8');
          }
        }),
      });

      const rc = await run(
        5,
        createMockRunner(),
        { projectRoot: ext, yes: true, verify: false, noProgress: true, allowDirty: true },
        deps,
      );
      expect(rc).toBe(0);

      // E1: state must be under the external project root's agents/ directory.
      const extAgentsDir = join(rext, 'agents');
      expect(existsSync(extAgentsDir)).toBe(true);
      const adwId = singleSubdir(extAgentsDir);
      expect(adwId).toBeTruthy();

      // State loads correctly (agentsDir() still points at ext/agents because
      // setProjectRoot(ext) was called at the top of run() and has not been
      // reset yet — afterEach does that).
      const state = AdwState.load(adwId);
      expect(state).not.toBeNull();
      // The run reached merge (setup → review → finalize → merge are all done).
      expect(state?.completedPhases).toContain('setup');
      expect(state?.completedPhases).toContain('review');

      // E3: nothing was written under REPO_ROOT/agents/ for this external run.
      expect(existsSync(join(REPO_ROOT, 'agents', adwId))).toBe(false);
    } finally {
      rmSync(ext, { recursive: true, force: true });
    }
  });

  it('agentsDir() during the run points at external-root/agents/ (AC4)', async () => {
    const ext = mkdtempSync(join(tmpdir(), 'adw-e2e-agentsdir-'));
    const rext = realpath(ext);
    let capturedAgentsDir: string | undefined;
    try {
      writeExtConfig(ext);

      const deps = testDeps({
        runAgentPhase: agentStub(PHASE_RESULTS, (opts) => {
          // Capture agentsDir() at phase-run time (inside the run() call, after
          // setProjectRoot was invoked at its top).
          capturedAgentsDir = agentsDir();
          if (opts.phase === 'review') {
            mkdirSync(opts.state.workspace(), { recursive: true });
            writeFileSync(commitMessagePath(opts.state), 'feat: x\n\ncloses #5', 'utf8');
            writeFileSync(prBodyPath(opts.state), 'Closes #5', 'utf8');
          }
        }),
      });

      await run(
        5,
        createMockRunner(),
        { projectRoot: ext, yes: true, verify: false, noProgress: true, allowDirty: true },
        deps,
      );

      // The live agentsDir() during the run pointed at the external root.
      expect(capturedAgentsDir).toBe(join(rext, 'agents'));
    } finally {
      rmSync(ext, { recursive: true, force: true });
    }
  });
});

// ---------------------------------------------------------------------------
// E2: External config testCmd flows through resolveOptions into the resolve loop
// ---------------------------------------------------------------------------

describe('full pipeline — external testCmd in resolve loop (E2)', () => {
  it('uses the external config defaultTestCommand as the resolve loop gate (AC2/AC7)', async () => {
    const ext = mkdtempSync(join(tmpdir(), 'adw-e2e-gate-'));
    try {
      // External project uses a project-specific test command.
      writeExtConfig(ext, { testCommand: 'scripts/verify.sh' });

      const gateCalls: string[][] = [];
      const deps = testDeps({
        // Track every runCmd call to see what gate command is used.
        runCmd: (cmd: readonly string[]) => {
          gateCalls.push([...cmd]);
          return { rc: 0, output: '' };
        },
        runAgentPhase: agentStub(PHASE_RESULTS, (opts) => {
          if (opts.phase === 'review') {
            mkdirSync(opts.state.workspace(), { recursive: true });
            writeFileSync(commitMessagePath(opts.state), 'feat: x\n\ncloses #5', 'utf8');
            writeFileSync(prBodyPath(opts.state), 'Closes #5', 'utf8');
          }
        }),
      });

      const rc = await run(
        5,
        createMockRunner(),
        {
          projectRoot: ext,
          yes: true,
          verify: false,
          noProgress: true,
          allowDirty: true,
          // Deliberately omit testCmd — must come from the external config.
        },
        deps,
      );
      expect(rc).toBe(0);

      // The resolve loop must have invoked runCmd with the external config's gate.
      const scriptCalls = gateCalls.filter((cmd) => cmd[0] === 'scripts/verify.sh');
      expect(scriptCalls.length).toBeGreaterThan(0);
    } finally {
      rmSync(ext, { recursive: true, force: true });
    }
  });

  it('Switchyard own testCmd is NOT used when targeting an external project (AC5 guard)', async () => {
    // The Switchyard package config has defaultTestCommand: 'npm run verify'
    // (see adw_sdlc/.adw/config.json or the committed DEFAULT_ADW_CONFIG).
    // When projectRoot points at an external repo, we must NOT use the package config's gate.
    const ext = mkdtempSync(join(tmpdir(), 'adw-e2e-gate-guard-'));
    const stateDir = mkdtempSync(join(tmpdir(), 'adw-e2e-gate-guard-state-'));
    try {
      // External project has an empty test command — gate is skipped entirely.
      writeExtConfig(ext, { testCommand: '' });
      setAgentsDir(stateDir); // keep state out of ext for this test

      const gateCalls: string[][] = [];
      const deps = testDeps({
        runCmd: (cmd: readonly string[]) => {
          gateCalls.push([...cmd]);
          return { rc: 0, output: '' };
        },
        runAgentPhase: agentStub(PHASE_RESULTS, (opts) => {
          if (opts.phase === 'review') {
            mkdirSync(opts.state.workspace(), { recursive: true });
            writeFileSync(commitMessagePath(opts.state), 'feat: x\n\ncloses #5', 'utf8');
            writeFileSync(prBodyPath(opts.state), 'Closes #5', 'utf8');
          }
        }),
      });

      const rc = await run(
        5,
        createMockRunner(),
        { projectRoot: ext, yes: true, verify: false, noProgress: true, allowDirty: true },
        deps,
      );
      expect(rc).toBe(0);

      // With the external config's testCommand: '', the resolve loop skips the gate.
      // The package config's 'npm run verify' must NOT have been invoked.
      const npmCalls = gateCalls.filter((cmd) => cmd.some((arg) => arg.includes('npm')));
      expect(npmCalls).toHaveLength(0);
      // And no runCmd calls at all from the resolve gate (it was an empty command).
      // (finalize gates from finalizeGates() are also empty for the external config.)
      expect(gateCalls).toHaveLength(0);
    } finally {
      setAgentsDir(null);
      rmSync(ext, { recursive: true, force: true });
      rmSync(stateDir, { recursive: true, force: true });
    }
  });
});

// ---------------------------------------------------------------------------
// E4: Invalid project root fails closed before any side effects (AC6)
// ---------------------------------------------------------------------------

describe('full pipeline — invalid project root fails closed (E4/AC6)', () => {
  it('run() with a non-existent project root rejects with AdwError before touching the filesystem', async () => {
    // Use a parent tmpdir that exists, with a subdir name that never does.
    const base = mkdtempSync(join(tmpdir(), 'adw-e2e-ac6-base-'));
    const missing = join(base, 'ghost-subdir');
    const stateDir = mkdtempSync(join(tmpdir(), 'adw-e2e-ac6-state-'));
    try {
      setAgentsDir(stateDir);
      const initialEntries = existsSync(stateDir) ? readdirSync(stateDir) : [];
      await expect(
        run(5, createMockRunner(), { projectRoot: missing, yes: true, noProgress: true, allowDirty: true }, testDeps()),
      ).rejects.toThrow(AdwError);
      await expect(
        run(5, createMockRunner(), { projectRoot: missing, yes: true, noProgress: true, allowDirty: true }, testDeps()),
      ).rejects.toThrow(/project root does not exist/);
      // No state was written — run() failed before any side effects.
      expect(readdirSync(stateDir)).toEqual(initialEntries);
    } finally {
      setAgentsDir(null);
      rmSync(base, { recursive: true, force: true });
      rmSync(stateDir, { recursive: true, force: true });
    }
  });

  it('run() with a file (not a directory) as project root rejects with AdwError (AC6)', async () => {
    const base = mkdtempSync(join(tmpdir(), 'adw-e2e-ac6-file-'));
    const fileRoot = join(base, 'not-a-dir.txt');
    writeFileSync(fileRoot, 'content');
    const stateDir = mkdtempSync(join(tmpdir(), 'adw-e2e-ac6-file-state-'));
    try {
      setAgentsDir(stateDir);
      await expect(
        run(5, createMockRunner(), { projectRoot: fileRoot, yes: true, noProgress: true, allowDirty: true }, testDeps()),
      ).rejects.toThrow(AdwError);
      await expect(
        run(5, createMockRunner(), { projectRoot: fileRoot, yes: true, noProgress: true, allowDirty: true }, testDeps()),
      ).rejects.toThrow(/project root is not a directory/);
      // No state was written.
      expect(readdirSync(stateDir)).toHaveLength(0);
    } finally {
      setAgentsDir(null);
      rmSync(base, { recursive: true, force: true });
      rmSync(stateDir, { recursive: true, force: true });
    }
  });
});
