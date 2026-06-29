/**
 * Regression for the resumed-run crash reported on issue #56:
 *
 *   >> skipping classify (already completed)
 *   error: could not parse JSON from agent output:
 *     Unexpected token 'C', "Credit bal"... (saw: "Credit balance is too low")
 *
 * Root cause (re-derived from agents/f3fa55d4 artifacts): classify completed,
 * so resume advanced to `plan`, where Claude Code — still handed a depleted
 * ANTHROPIC_API_KEY that takes precedence over the on-disk `claude login` —
 * returned a success-subtype result with is_error:true whose `result` text was
 * literally "Credit balance is too low". The runner copies that into
 * transcriptText, and the invoker tried to JSON.parse it, surfacing a useless
 * "invalid JSON" error AND burning the single nudge retry (both transcript.log
 * and transcript-2.log exist in the artifact).
 *
 * These tests drive the REAL claude runner (SDK mocked at the seam) through the
 * REAL runAgentPhase, asserting the failure is now a typed RunnerAuthError with
 * NO nudge retry — not a JSON parse error.
 */

import { mkdtempSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@anthropic-ai/claude-agent-sdk', () => ({ query: vi.fn() }));

import { query } from '@anthropic-ai/claude-agent-sdk';

import { RunnerAuthError } from '../src/errors.js';
import { runAgentPhase } from '../src/run-phase.js';
import { createRunner } from '../src/runners/runner-claude.js';
import { AdwState, setAgentsDir } from '../src/state.js';

const queryMock = vi.mocked(query);

let tmp: string;
let state: AdwState;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), 'adw-credit-balance-'));
  setAgentsDir(tmp);
  state = new AdwState({ adwId: 'f3fa55d4' });
  queryMock.mockReset();
});

afterEach(() => {
  setAgentsDir(null);
  rmSync(tmp, { recursive: true, force: true });
});

/**
 * The exact SDK result shape behind the artifact: subtype 'success' but
 * is_error true, carrying the credit-balance text as `result` (the runner
 * copies result.result into transcriptText when no assistant text streamed).
 */
function creditBalanceResult(): unknown {
  return {
    type: 'result',
    subtype: 'success',
    duration_ms: 5,
    duration_api_ms: 4,
    is_error: true,
    num_turns: 1,
    result: 'Credit balance is too low',
    stop_reason: null,
    total_cost_usd: 0,
    usage: { input_tokens: 1, output_tokens: 1 },
    modelUsage: {},
    permission_denials: [],
    uuid: 'u-1',
    session_id: 'sess-1',
  };
}

function scriptedQuery(messages: unknown[]): void {
  queryMock.mockImplementation(
    () =>
      (async function* () {
        for (const m of messages) {
          yield m as never;
        }
      })() as never,
  );
}

/**
 * The faithful artifact path: the SDK streams the assistant text, then THROWS
 * (agents/f3fa55d4 shows a `[claude runner error] Error: Claude Code returned
 * an error result` line, i.e. the runner's catch block ran).
 */
function streamThenThrow(text: string): void {
  queryMock.mockImplementation(
    () =>
      (async function* () {
        yield {
          type: 'assistant',
          message: { content: [{ type: 'text', text }] },
          parent_tool_use_id: null,
          uuid: 'a-1',
          session_id: 'sess-1',
        } as never;
        throw new Error('Claude Code returned an error result: Credit balance is too low');
      })() as never,
  );
}

describe('issue #56 resumed-run credit-balance regression', () => {
  it('raises RunnerAuthError (not a JSON parse error) and does NOT waste the nudge retry', async () => {
    // The SDK returns the credit-balance result on every call; if the invoker
    // wrongly nudged, query() would be invoked a second time.
    scriptedQuery([creditBalanceResult()]);

    const promise = runAgentPhase({
      phase: 'plan',
      templateArgs: ['56', 'T', 'B', '', 'src/x.ts', 'GitHub issue'],
      state,
      runner: createRunner(),
      // Depleted key present, exactly like the failing resume.
      env: { PATH: join(tmp, 'bin'), HOME: join(tmp, 'home'), ANTHROPIC_API_KEY: 'sk-ant-empty' },
    });

    await expect(promise).rejects.toThrow(RunnerAuthError);
    // The message must be actionable, never the old "could not parse JSON".
    await expect(promise).rejects.toThrow(/credit balance is too low/i);
    await promise.catch((err: unknown) => {
      expect(err).toBeInstanceOf(RunnerAuthError);
      expect((err as RunnerAuthError).phase).toBe('plan');
      expect((err as RunnerAuthError).message).not.toMatch(/could not parse JSON/);
    });

    // The decisive fix: auth/account failures fail fast with NO second attempt
    // (the artifact had both transcript.log AND transcript-2.log — the bug).
    expect(queryMock).toHaveBeenCalledTimes(1);
    expect(existsSync(join(tmp, 'f3fa55d4', 'plan', 'transcript-2.log'))).toBe(false);
  });

  it('handles the faithful artifact path: SDK streams the text then THROWS', async () => {
    // Mirrors agents/f3fa55d4/plan/transcript.log exactly (assistant text +
    // a thrown "returned an error result"), not just a returned result.
    streamThenThrow('Credit balance is too low');

    const promise = runAgentPhase({
      phase: 'plan',
      templateArgs: ['56', 'T', 'B', '', 'src/x.ts', 'GitHub issue'],
      state,
      runner: createRunner(),
      env: { PATH: join(tmp, 'bin'), HOME: join(tmp, 'home'), ANTHROPIC_API_KEY: 'sk-ant-empty' },
    });

    await expect(promise).rejects.toThrow(RunnerAuthError);
    await expect(promise).rejects.toThrow(/credit balance is too low/i);
    expect(queryMock).toHaveBeenCalledTimes(1); // no wasted nudge retry
    expect(existsSync(join(tmp, 'f3fa55d4', 'plan', 'transcript-2.log'))).toBe(false);
  });

  it('still reports credit-balance as auth failure when no ANTHROPIC_API_KEY is set', async () => {
    // Without the key the message differs, but it must still be a typed auth
    // failure, never a JSON parse error.
    scriptedQuery([creditBalanceResult()]);

    const promise = runAgentPhase({
      phase: 'plan',
      templateArgs: ['56', 'T', 'B', '', 'src/x.ts', 'GitHub issue'],
      state,
      runner: createRunner(),
      env: { PATH: join(tmp, 'bin'), HOME: join(tmp, 'home') },
    });

    await expect(promise).rejects.toThrow(RunnerAuthError);
    expect(queryMock).toHaveBeenCalledTimes(1);
  });
});
