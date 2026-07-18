/**
 * The invoker layer over runPhase: single nudge-retry, no-retry-on-timeout,
 * budget fail-fast, footer gating by caps.nativeSchema, and prompt/transcript
 * persistence — the seams adw/_phases.py:482-517 pins, driven through
 * runner-mock.ts (PLAN.md Section 10).
 */

import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { AdwError, RunnerAuthError, RunnerTransientError } from '../src/errors.js';
import { PHASE_TIMEOUT_ABORT_REASON } from '../src/invoker.js';
import { NUDGE, runAgentPhase } from '../src/run-phase.js';
import { createMockRunner } from '../src/runners/runner-mock.js';
import { AdwState, setAgentsDir } from '../src/state.js';

let tmp: string;
let state: AdwState;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), 'adw-runphase-'));
  setAgentsDir(tmp);
  state = new AdwState({ adwId: 'a1b2c3d4' });
});

afterEach(() => {
  setAgentsDir(null);
  rmSync(tmp, { recursive: true, force: true });
});

const GOOD_RESOLVE = { resolved: 1, remaining: 0, summary: 's' };

describe('runAgentPhase', () => {
  it('uses native structured output and persists prompt.txt', async () => {
    const runner = createMockRunner({ script: () => ({ structured: GOOD_RESOLVE }) });
    const outcome = await runAgentPhase({
      phase: 'resolve',
      templateArgs: ['test output'],
      state,
      runner,
      env: { PATH: '/bin' },
    });
    expect(outcome.data).toEqual(GOOD_RESOLVE);
    expect(outcome.attempts).toBe(1); // clean first parse, no nudge
    expect(runner.requests).toHaveLength(1);
    const req = runner.requests[0]!;
    // Native-schema backend: schema attached, fenced-JSON footer gated OFF.
    expect(req.schema).toBeDefined();
    expect(req.prompt).not.toContain('## Required output');
    expect(req.env).toEqual({ PATH: '/bin' });
    expect(req.model).toBe('claude-sonnet-4-6'); // resolve = mid tier on claude
    expect(readFileSync(join(tmp, 'a1b2c3d4', 'resolve', 'prompt.txt'), 'utf8')).toBe(req.prompt);
  });

  it('forceFenced routes a native-schema backend through the fenced-JSON path (measurement mode)', async () => {
    // Default mock caps are nativeSchema:true; forceFenced must flip it to the
    // fenced path: contract footer ON, NO native schema handed to the SDK — so a
    // parity-rate run can harvest a fenced-path baseline from claude.
    const runner = createMockRunner({
      script: () => ({ transcriptText: 'ok\n```json\n{"resolved": 1, "remaining": 0}\n```\n' }),
    });
    const outcome = await runAgentPhase({
      phase: 'resolve',
      templateArgs: ['test output'],
      state,
      runner,
      env: {},
      forceFenced: true,
    });
    expect(outcome.data.resolved).toBe(1);
    const req = runner.requests[0]!;
    expect(req.schema).toBeUndefined(); // native schema withheld
    expect(req.prompt).toContain('## Required output');
    expect(req.prompt).toContain('End your reply with EXACTLY one fenced'); // tools/parity-rate FENCED_MARKER
  });

  it('parses fenced JSON from the transcript for non-native-schema backends', async () => {
    const runner = createMockRunner({
      id: 'pi',
      caps: { nativeSchema: false },
      script: () => ({ transcriptText: 'done!\n```json\n{"resolved": 1, "remaining": 0}\n```\n' }),
    });
    const outcome = await runAgentPhase({
      phase: 'resolve',
      templateArgs: ['test output'],
      state,
      runner,
      env: {},
    });
    expect(outcome.data.resolved).toBe(1);
    const req = runner.requests[0]!;
    expect(req.schema).toBeUndefined();
    expect(req.prompt).toContain('## Required output'); // fenced-JSON contract ON
  });

  it('nudges exactly once on a parse failure, then succeeds', async () => {
    const runner = createMockRunner({
      caps: { nativeSchema: false },
      script: (_req, call) =>
        call === 0
          ? { transcriptText: 'no json here', usage: { inputTokens: 10, outputTokens: 1 } }
          : {
              transcriptText: '```json\n{"resolved": 2, "remaining": 0}\n```',
              usage: { inputTokens: 11, outputTokens: 2 },
            },
    });
    const outcome = await runAgentPhase({
      phase: 'resolve',
      templateArgs: ['x'],
      state,
      runner,
      env: {},
    });
    expect(outcome.data.resolved).toBe(2);
    expect(outcome.attempts).toBe(2); // nudge-retry fired (double-charge signal)
    expect(runner.requests).toHaveLength(2);
    expect(runner.requests[1]!.prompt.endsWith(NUDGE)).toBe(true);
    expect(runner.requests[1]!.transcriptPath.endsWith('transcript-2.log')).toBe(true);
    // Both attempts consumed tokens; usage is the pair's sum.
    expect(outcome.usage).toMatchObject({ inputTokens: 21, outputTokens: 3 });
  });

  it('retries a native-schema backend WITH the fenced-JSON contract it never saw', async () => {
    // A native-schema success can come back without structured_output AND
    // without parseable text; the retry prompt must then carry the contract
    // footer the first prompt deliberately omitted, or the nudge demands a
    // JSON shape the agent was never shown.
    const runner = createMockRunner({
      script: (_req, call) =>
        call === 0
          ? { transcriptText: 'prose report, no JSON anywhere' }
          : { transcriptText: '```json\n{"tests_added": true, "summary": "s"}\n```' },
    });
    const outcome = await runAgentPhase({
      phase: 'tests',
      templateArgs: ['x'],
      state,
      runner,
      env: {},
    });
    expect(outcome.data).toEqual({ tests_added: true, summary: 's' });
    expect(runner.requests).toHaveLength(2);
    expect(runner.requests[0]!.prompt).not.toContain('## Required output');
    expect(runner.requests[1]!.prompt).toContain('## Required output');
    expect(runner.requests[1]!.prompt).toContain('"tests_added"');
    expect(runner.requests[1]!.prompt.endsWith(NUDGE)).toBe(true);
  });

  it('nudges once when a native-schema backend returns a non-conforming payload', async () => {
    const runner = createMockRunner({
      script: (_req, call) =>
        call === 0
          ? { structured: { issue_class: 'bogus-class' } }
          : { structured: { issue_class: 'feat', reason: 'r' } },
    });
    const outcome = await runAgentPhase({
      phase: 'classify',
      templateArgs: ['5', 'ctx'],
      state,
      runner,
      env: {},
    });
    expect(outcome.data).toEqual({ issue_class: 'feat', reason: 'r' });
    expect(runner.requests).toHaveLength(2);
  });

  it('classifies Claude credit-balance output as auth failure with NO nudge retry', async () => {
    const runner = createMockRunner({
      id: 'claude',
      script: () => ({ ok: false, rc: 1, transcriptText: 'Credit balance is too low' }),
    });
    const promise = runAgentPhase({
      phase: 'plan',
      templateArgs: ['56', 'T', 'B', '', 'src/x.ts', 'GitHub issue'],
      state,
      runner,
      env: { PATH: '/bin', ANTHROPIC_API_KEY: 'sk-ant-empty' },
    });
    await expect(promise).rejects.toThrow(RunnerAuthError);
    await expect(promise).rejects.toThrow(/ANTHROPIC_API_KEY/);
    expect(runner.requests).toHaveLength(1); // auth/account failures do not benefit from a JSON nudge
    expect(existsSync(join(tmp, 'a1b2c3d4', 'plan', 'transcript-2.log'))).toBe(false);
  });

  it('names ANTHROPIC_AUTH_TOKEN when that pay-as-you-go auth source shadows claude login', async () => {
    const runner = createMockRunner({
      id: 'claude',
      script: () => ({ ok: false, rc: 1, transcriptText: 'Credit balance is too low' }),
    });
    const promise = runAgentPhase({
      phase: 'plan',
      templateArgs: ['56', 'T', 'B', '', 'src/x.ts', 'GitHub issue'],
      state,
      runner,
      env: { PATH: '/bin', ANTHROPIC_AUTH_TOKEN: 'payg-token-empty' },
    });
    await expect(promise).rejects.toThrow(RunnerAuthError);
    await expect(promise).rejects.toThrow(/ANTHROPIC_AUTH_TOKEN/);
    expect(runner.requests).toHaveLength(1);
  });

  it('classifies a logged-out ("Not logged in / /login") transcript as auth failure with NO nudge', async () => {
    // A logged-out subscription otherwise falls through as a confusing JSON
    // parse error mid-pipeline; classify it (no key needed) so the operator
    // gets an actionable message and no wasted nudge.
    const runner = createMockRunner({
      id: 'claude',
      script: () => ({ ok: false, rc: 1, transcriptText: 'Not logged in · Please run /login' }),
    });
    const promise = runAgentPhase({
      phase: 'plan',
      templateArgs: ['56', 'T', 'B', '', 'src/x.ts', 'GitHub issue'],
      state,
      runner,
      env: { PATH: '/bin' }, // no ANTHROPIC_* key: detection must not depend on one
    });
    await expect(promise).rejects.toThrow(RunnerAuthError);
    await expect(promise).rejects.toThrow(/not logged in/i);
    expect(runner.requests).toHaveLength(1); // no nudge on an auth failure
    expect(existsSync(join(tmp, 'a1b2c3d4', 'plan', 'transcript-2.log'))).toBe(false);
  });

  it('classifies a persistent transient API 5xx as RunnerTransientError (after the nudge)', async () => {
    // A momentary provider 500 is not a prompt problem — a nudge cannot fix it.
    // When both the first attempt and the nudge hit it, escalate to a typed
    // transient error the orchestrator retries with backoff, rather than killing
    // the run as an unparseable reply.
    const runner = createMockRunner({
      id: 'claude',
      caps: { nativeSchema: false },
      script: () => ({ transcriptText: "I'll review the code.\nAPI Error: Internal server error" }),
    });
    await expect(
      runAgentPhase({ phase: 'review', templateArgs: ['x'], state, runner, env: {} }),
    ).rejects.toThrow(RunnerTransientError);
    expect(runner.requests).toHaveLength(2); // first attempt + one nudge, then escalate
  });

  it('recovers transparently when a transient blip clears on the nudge retry', async () => {
    const runner = createMockRunner({
      caps: { nativeSchema: false },
      script: (_req, call) =>
        call === 0
          ? { transcriptText: 'API Error: 503 Service Unavailable' }
          : { transcriptText: '```json\n{"resolved": 1, "remaining": 0}\n```' },
    });
    const outcome = await runAgentPhase({ phase: 'resolve', templateArgs: ['x'], state, runner, env: {} });
    expect(outcome.data.resolved).toBe(1);
    expect(outcome.attempts).toBe(2); // the existing nudge doubled as the quick retry
  });

  it('fails after the second parse failure', async () => {
    const runner = createMockRunner({
      caps: { nativeSchema: false },
      script: () => ({ transcriptText: 'still no json' }),
    });
    await expect(
      runAgentPhase({ phase: 'resolve', templateArgs: ['x'], state, runner, env: {} }),
    ).rejects.toThrow(AdwError);
    expect(runner.requests).toHaveLength(2);
  });

  it('accepts parseable output even from a nonzero-rc run (Python parity)', async () => {
    const runner = createMockRunner({
      caps: { nativeSchema: false },
      script: () => ({
        ok: false,
        rc: 3,
        transcriptText: '```json\n{"resolved": 1, "remaining": 0}\n```',
      }),
    });
    const outcome = await runAgentPhase({ phase: 'resolve', templateArgs: ['x'], state, runner, env: {} });
    expect(outcome.data.resolved).toBe(1);
    expect(runner.requests).toHaveLength(1);
  });

  it('fails fast with NO nudge on timeout (the _TIMEOUT_EXIT_CODES parity line)', async () => {
    const runner = createMockRunner({
      script: () => ({ ok: false, rc: 124, signal: 'timeout', transcriptText: 'killed mid-thought' }),
    });
    await expect(
      runAgentPhase({ phase: 'resolve', templateArgs: ['x'], state, runner, env: {} }),
    ).rejects.toThrow(/timed out/);
    expect(runner.requests).toHaveLength(1); // no second attempt
    expect(existsSync(join(tmp, 'a1b2c3d4', 'resolve', 'transcript-2.log'))).toBe(false);
  });

  it("fails fast with NO nudge on claude's native budget signal", async () => {
    const runner = createMockRunner({
      script: () => ({ ok: false, rc: 1, signal: 'budget', transcriptText: '' }),
    });
    await expect(
      runAgentPhase({ phase: 'resolve', templateArgs: ['x'], state, runner, env: {}, maxBudgetUsd: 1 }),
    ).rejects.toThrow(/budget/);
    expect(runner.requests).toHaveLength(1);
    expect(runner.requests[0]!.maxBudgetUsd).toBe(1);
  });

  it('fails fast with NO nudge on a cancelled signal (operator kill)', async () => {
    const runner = createMockRunner({
      script: () => ({ ok: false, rc: 1, signal: 'cancelled', transcriptText: 'killed mid-phase' }),
    });
    await expect(
      runAgentPhase({ phase: 'resolve', templateArgs: ['x'], state, runner, env: {} }),
    ).rejects.toThrow(/cancelled/);
    expect(runner.requests).toHaveLength(1); // no nudge retry
    expect(existsSync(join(tmp, 'a1b2c3d4', 'resolve', 'transcript-2.log'))).toBe(false);
  });

  it('propagates parent cancellation into the runner request', async () => {
    const controller = new AbortController();
    const reason = new Error('managed run interrupted by SIGTERM');
    controller.abort(reason);
    const runner = createMockRunner({
      script: (request) => {
        expect(request.signal.aborted).toBe(true);
        expect(request.signal.reason).toBe(reason);
        return { ok: false, rc: 1, signal: 'cancelled', transcriptText: '' };
      },
    });
    await expect(
      runAgentPhase({
        phase: 'resolve',
        templateArgs: ['x'],
        state,
        runner,
        env: {},
        signal: controller.signal,
      }),
    ).rejects.toThrow(/cancelled/);
    expect(runner.requests).toHaveLength(1);
  });

  it('still accepts parseable output from a timed-out run (parse first, like Python)', async () => {
    const runner = createMockRunner({
      caps: { nativeSchema: false },
      script: () => ({
        ok: false,
        rc: 124,
        signal: 'timeout',
        transcriptText: '```json\n{"resolved": 0, "remaining": 2}\n```',
      }),
    });
    const outcome = await runAgentPhase({ phase: 'resolve', templateArgs: ['x'], state, runner, env: {} });
    expect(outcome.data.remaining).toBe(2);
  });

  it('propagates an unpriceable (null) cost from either attempt instead of a false sum', async () => {
    const runner = createMockRunner({
      caps: { nativeSchema: false },
      script: (_req, call) =>
        call === 0
          ? { transcriptText: 'no json here', usage: { costUsd: null } }
          : { transcriptText: '```json\n{"resolved": 1, "remaining": 0}\n```', usage: { costUsd: 0.4 } },
    });
    const outcome = await runAgentPhase({ phase: 'resolve', templateArgs: ['x'], state, runner, env: {} });
    expect(outcome.usage.costUsd).toBeNull();
  });

  it('arms an AbortSignal for the phase timeout', async () => {
    const runner = createMockRunner({ script: () => ({ structured: GOOD_RESOLVE }) });
    await runAgentPhase({
      phase: 'resolve',
      templateArgs: ['x'],
      state,
      runner,
      env: {},
      timeoutMs: 60_000,
    });
    expect(runner.requests[0]!.signal).toBeInstanceOf(AbortSignal);
    expect(runner.requests[0]!.signal.aborted).toBe(false);
  });

  it('aborts the phase signal with PHASE_TIMEOUT_ABORT_REASON when timeoutMs elapses', async () => {
    // Verifies the invoker sends the correct abort reason so adapter runners can
    // classify 'timeout' vs 'cancelled' (runner-claude/codex/opencode/pi all
    // key on signal.reason.message === PHASE_TIMEOUT_ABORT_REASON).
    vi.useFakeTimers();
    try {
      let capturedSignal: AbortSignal | undefined;
      const runner = createMockRunner({
        script: async (req) => {
          capturedSignal = req.signal;
          // Await the abort event, then return a timeout result (simulates a runner
          // observing the signal and giving up, like the real claude/codex adapters).
          await new Promise<void>((resolve) => {
            if (req.signal.aborted) {
              resolve();
            } else {
              req.signal.addEventListener('abort', () => resolve(), { once: true });
            }
          });
          return { ok: false, rc: 1, signal: 'timeout' as const, transcriptText: '' };
        },
      });

      const phasePromise = runAgentPhase({
        phase: 'resolve',
        templateArgs: ['x'],
        state,
        runner,
        env: {},
        timeoutMs: 100,
      });

      // Attach the rejection handler BEFORE advancing time so the promise has a
      // handler at the moment it rejects (avoids unhandledRejection warnings).
      const assertion = expect(phasePromise).rejects.toThrow(/timed out/);
      await vi.advanceTimersByTimeAsync(101);
      await assertion;

      expect(capturedSignal?.aborted).toBe(true);
      const abortReason = capturedSignal?.reason as Error | undefined;
      expect(abortReason?.message).toBe(PHASE_TIMEOUT_ABORT_REASON);
    } finally {
      vi.useRealTimers();
    }
  });
});
