/**
 * Unit tests for the codex runner adapter (PLAN.md roadmap step 7).
 *
 * The SDK is replaced by vi.mock per the hermetic CI rule (PLAN.md Section
 * 9): no network, no keys, no native binary. The always-passes-env cases are
 * the highest-severity ones here — omitting CodexOptions.env flips the SDK
 * from no-inherit to full process.env inherit. The complementary
 * runner-codex-spawn.test.ts drives the REAL SDK over a mocked
 * child_process.spawn and asserts on the SDK-BUILT child env (so an
 * apiKey-routed credential could never slip past the allowlist unobserved).
 */

import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { codexCtor, startThreadMock, resumeThreadMock, runStreamedMock } = vi.hoisted(() => ({
  codexCtor: vi.fn(),
  startThreadMock: vi.fn(),
  resumeThreadMock: vi.fn(),
  runStreamedMock: vi.fn(),
}));

vi.mock('@openai/codex-sdk', () => ({
  Codex: class {
    constructor(options: unknown) {
      codexCtor(options);
    }
    startThread(options: unknown): unknown {
      return startThreadMock(options);
    }
    resumeThread(id: string, options: unknown): unknown {
      return resumeThreadMock(id, options);
    }
  },
}));

import type { CodexOptions, ThreadOptions, TurnOptions } from '@openai/codex-sdk';

import { safeSubprocessEnv } from '../src/env.js';
import { PHASE_TIMEOUT_ABORT_REASON } from '../src/invoker.js';
import type { AgentRunner, PhaseRequest } from '../src/invoker.js';
import {
  CODEX_ADW_LAUNCHER,
  CODEX_CAPS,
  CODEX_UNATTENDED_CONFIG,
  createRunner,
} from '../src/runners/runner-codex.js';

let tmp: string;
let runner: AgentRunner;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), 'adw-runner-codex-'));
  runner = createRunner();
  codexCtor.mockReset();
  startThreadMock.mockReset();
  resumeThreadMock.mockReset();
  runStreamedMock.mockReset();
  startThreadMock.mockImplementation(() => ({ runStreamed: runStreamedMock }));
  resumeThreadMock.mockImplementation(() => ({ runStreamed: runStreamedMock }));
});

afterEach(() => {
  vi.unstubAllEnvs();
  rmSync(tmp, { recursive: true, force: true });
});

function makeReq(over: Partial<PhaseRequest> = {}): PhaseRequest {
  return {
    phase: 'plan',
    prompt: 'plan the work',
    model: 'gpt-5.5',
    cwd: join(tmp, 'worktree'),
    env: { PATH: join(tmp, 'bin'), HOME: join(tmp, 'home') },
    transcriptPath: join(tmp, 'transcript.log'),
    signal: new AbortController().signal,
    ...over,
  };
}

function threadStarted(threadId = 'thread-1'): unknown {
  return { type: 'thread.started', thread_id: threadId };
}

function agentMessage(text: string, id = 'msg-1'): unknown {
  return { type: 'item.completed', item: { id, type: 'agent_message', text } };
}

function turnCompleted(over: Partial<Record<string, number>> = {}): unknown {
  return {
    type: 'turn.completed',
    usage: {
      input_tokens: 100,
      cached_input_tokens: 10,
      output_tokens: 50,
      reasoning_output_tokens: 20,
      ...over,
    },
  };
}

function scriptedEvents(events: unknown[]): void {
  runStreamedMock.mockImplementation(() =>
    Promise.resolve({
      events: (async function* () {
        for (const event of events) {
          yield event;
        }
      })(),
    }),
  );
}

function capturedCtorOptions(): CodexOptions {
  expect(codexCtor).toHaveBeenCalledTimes(1);
  return codexCtor.mock.calls[0]![0] as CodexOptions;
}

function capturedThreadOptions(): ThreadOptions {
  expect(startThreadMock).toHaveBeenCalledTimes(1);
  return startThreadMock.mock.calls[0]![0] as ThreadOptions;
}

function capturedTurnOptions(): TurnOptions {
  expect(runStreamedMock).toHaveBeenCalledTimes(1);
  return runStreamedMock.mock.calls[0]![1] as TurnOptions;
}

describe('request shape', () => {
  it('passes the verbatim allowlist env and the planned coarse sandbox grants', async () => {
    scriptedEvents([threadStarted(), agentMessage('done'), turnCompleted()]);
    const req = makeReq();
    await runner.runPhase(req);

    // Identity, not just equality: the request allowlist is the complete child
    // env.  HOME/CODEX_HOME remain the durable auth store; connector config is
    // excluded by the launcher and forced config below.
    const ctorEnv = capturedCtorOptions().env as Record<string, string>;
    expect(ctorEnv).toBe(req.env);
    expect('GH_TOKEN' in ctorEnv).toBe(false);
    expect(capturedCtorOptions().codexPathOverride).toBe(CODEX_ADW_LAUNCHER);
    expect(capturedCtorOptions().config).toEqual(CODEX_UNATTENDED_CONFIG);
    const thread = capturedThreadOptions();
    expect(thread.model).toBe('gpt-5.5');
    expect(thread.sandboxMode).toBe('workspace-write');
    expect(thread.workingDirectory).toBe(req.cwd);
    expect(thread.skipGitRepoCheck).toBe(true);
    expect(thread.approvalPolicy).toBe('never');
    expect('modelReasoningEffort' in thread).toBe(false);
    expect(runStreamedMock.mock.calls[0]![0]).toBe('plan the work');
    expect(capturedTurnOptions().signal).toBe(req.signal);
  });

  it('ALWAYS passes an explicit env and never the apiKey credential route', async () => {
    // Omitting CodexOptions.env flips the SDK to full process.env inherit
    // (dist/index.js:234-239); apiKey would inject CODEX_API_KEY into the
    // child env around the allowlist (dist/index.js:244-245).
    scriptedEvents([turnCompleted()]);
    await runner.runPhase(makeReq());

    const options = capturedCtorOptions();
    expect('env' in options).toBe(true);
    expect(options.env).toBeDefined();
    expect('apiKey' in options).toBe(false);
  });

  it('forwards the schema as outputSchema, or omits it for free-form phases', async () => {
    scriptedEvents([turnCompleted()]);
    const schema = { type: 'object', properties: { ok: { type: 'boolean' } } };
    await runner.runPhase(makeReq({ schema }));
    expect(capturedTurnOptions().outputSchema).toBe(schema);

    runStreamedMock.mockReset();
    scriptedEvents([turnCompleted()]);
    await runner.runPhase(makeReq());
    expect('outputSchema' in capturedTurnOptions()).toBe(false);
  });

  it('maps the reasoning hint to modelReasoningEffort when present', async () => {
    scriptedEvents([turnCompleted()]);
    await runner.runPhase(makeReq({ reasoning: 'high' }));
    expect(capturedThreadOptions().modelReasoningEffort).toBe('high');
  });

  it('resumes the requested thread instead of starting a new one', async () => {
    scriptedEvents([agentMessage('{"ok":true}'), turnCompleted()]);
    const result = await runner.runPhase(makeReq({ resumeSessionId: 'thread-existing' }));

    expect(startThreadMock).not.toHaveBeenCalled();
    expect(resumeThreadMock).toHaveBeenCalledTimes(1);
    expect(resumeThreadMock.mock.calls[0]![0]).toBe('thread-existing');
    expect(resumeThreadMock.mock.calls[0]![1]).toMatchObject({
      model: 'gpt-5.5',
      workingDirectory: makeReq().cwd,
    });
    expect(result.sessionId).toBe('thread-existing');
  });

  it('always uses the security launcher and leaves CODEX_BIN inside the allowlisted env', async () => {
    scriptedEvents([turnCompleted()]);
    await runner.runPhase(makeReq({ env: { ...makeReq().env, CODEX_BIN: '/opt/codex' } }));
    expect(capturedCtorOptions().codexPathOverride).toBe(CODEX_ADW_LAUNCHER);
    expect(capturedCtorOptions().env!['CODEX_BIN']).toBe('/opt/codex');

    codexCtor.mockReset();
    runStreamedMock.mockReset();
    scriptedEvents([turnCompleted()]);
    await runner.runPhase(makeReq());
    expect(capturedCtorOptions().codexPathOverride).toBe(CODEX_ADW_LAUNCHER);
    expect(capturedCtorOptions().env!['CODEX_BIN']).toBeUndefined();
  });
});

describe('env isolation (PLAN.md Section 10)', () => {
  it('hands the SDK only the allowlist when the parent env is poisoned', async () => {
    const poisoned = {
      GH_TOKEN: 'leak-gh',
      MATRIX_TOKEN: 'leak-matrix',
      ADW_SECRET: 'leak-adw',
      MX_AGENT_SECRET: 'leak-agent',
      CODEX_API_KEY: 'sk-codex',
      HOME: join(tmp, 'home'),
      PATH: join(tmp, 'bin'),
    };
    const allowlist = safeSubprocessEnv({ allowGhToken: false, runner: 'codex', source: poisoned });
    scriptedEvents([turnCompleted()]);
    await runner.runPhase(makeReq({ env: allowlist }));

    const env = capturedCtorOptions().env as Record<string, string>;
    expect(env).toBe(allowlist);
    expect(env['CODEX_API_KEY']).toBe('sk-codex');
    expect(env['HOME']).toBe(allowlist['HOME']);
    expect(env['PATH']).toBe(allowlist['PATH']);
    expect(env['CODEX_HOME']).toBeUndefined();
    expect(env['GH_TOKEN']).toBeUndefined();
    for (const key of Object.keys(env)) {
      expect(key.startsWith('MATRIX_'), key).toBe(false);
      expect(key.startsWith('ADW_'), key).toBe(false);
      expect(key.startsWith('MX_AGENT_'), key).toBe(false);
    }
  });
});

describe('result mapping', () => {
  it('maps a success: structured from the final JSON message, disjoint usage, parent-priced cost', async () => {
    scriptedEvents([
      threadStarted('thread-42'),
      agentMessage('{"decision":"approve"}'),
      turnCompleted(),
    ]);
    const result = await runner.runPhase(makeReq());

    expect(result.ok).toBe(true);
    expect(result.rc).toBe(0);
    expect(result.signal).toBe('none');
    expect(result.structured).toEqual({ decision: 'approve' });
    expect(result.sessionId).toBe('thread-42');
    // input_tokens (100) INCLUDES cached (10): mapped disjoint as 90 + 10.
    // gpt-5.5 pricing $5/$30 in/out, $0.50 cache read per MTok:
    // (90*5 + 50*30 + 10*0.5) / 1e6.
    expect(result.usage).toEqual({
      inputTokens: 90,
      outputTokens: 50,
      cachedInputTokens: 10,
      reasoningTokens: 20,
      costUsd: (90 * 5 + 50 * 30 + 10 * 0.5) / 1_000_000,
    });
  });

  it('yields null cost for a model without a price entry (non-fatal by design)', async () => {
    scriptedEvents([turnCompleted()]);
    const result = await runner.runPhase(makeReq({ model: 'gpt-99-experimental' }));

    expect(result.ok).toBe(true);
    expect(result.usage.costUsd).toBeNull();
    expect(result.usage.inputTokens).toBe(90);
  });

  it('keeps the LAST agent message as the structured payload (SDK finalResponse parity)', async () => {
    scriptedEvents([
      agentMessage('working on it', 'msg-1'),
      agentMessage('{"done":true}', 'msg-2'),
      turnCompleted(),
    ]);
    const result = await runner.runPhase(makeReq());

    expect(result.structured).toEqual({ done: true });
    expect(result.transcriptText).toBe('working on it\n{"done":true}\n');
  });

  it('returns structured null for a prose/non-object final message (invoker fallback owns it)', async () => {
    for (const text of ['all done!', '[1,2]', '"quoted"', '42']) {
      runStreamedMock.mockReset();
      scriptedEvents([agentMessage(text), turnCompleted()]);
      const result = await runner.runPhase(makeReq());
      expect(result.ok, text).toBe(true);
      expect(result.structured, text).toBeNull();
    }
  });

  it('tees agent messages to the transcript file as they stream, not at the end', async () => {
    const req = makeReq();
    let midStream = '';
    runStreamedMock.mockImplementation(() =>
      Promise.resolve({
        events: (async function* () {
          yield agentMessage('first');
          midStream = readFileSync(req.transcriptPath, 'utf8');
          yield agentMessage('second');
          yield turnCompleted();
        })(),
      }),
    );
    const result = await runner.runPhase(req);

    expect(midStream).toBe('first\n');
    expect(result.transcriptText).toBe('first\nsecond\n');
    expect(readFileSync(req.transcriptPath, 'utf8')).toBe('first\nsecond\n');
  });

  it('tees non-message items to the transcript FILE only (transcriptText stays assistant-text-only)', async () => {
    scriptedEvents([
      {
        type: 'item.completed',
        item: {
          id: 'c1',
          type: 'command_execution',
          command: 'pnpm test',
          aggregated_output: 'ok',
          exit_code: 0,
          status: 'completed',
        },
      },
      { type: 'item.completed', item: { id: 'r1', type: 'reasoning', text: 'thinking' } },
      {
        type: 'item.completed',
        item: {
          id: 'f1',
          type: 'file_change',
          status: 'completed',
          changes: [{ kind: 'update', path: 'src/x.ts' }],
        },
      },
      {
        type: 'item.completed',
        item: {
          id: 'mcp1',
          type: 'mcp_tool_call',
          server: 'fs',
          tool: 'read',
          arguments: {},
          status: 'failed',
          error: { message: 'denied' },
        },
      },
      {
        type: 'item.completed',
        item: {
          id: 'mcp2',
          type: 'mcp_tool_call',
          server: 'fs',
          tool: 'list',
          arguments: {},
          status: 'failed',
          error: null,
        },
      },
      { type: 'item.completed', item: { id: 'w1', type: 'web_search', query: 'codex docs' } },
      {
        type: 'item.completed',
        item: {
          id: 't1',
          type: 'todo_list',
          items: [
            { text: 'read file', completed: true },
            { text: 'write reply', completed: false },
          ],
        },
      },
      { type: 'item.completed', item: { id: 'e1', type: 'error', message: 'tool exploded' } },
      { type: 'error', message: 'stream hiccup' },
      agentMessage('{"ok":true}'),
      turnCompleted(),
    ]);
    const req = makeReq();
    const result = await runner.runPhase(req);

    expect(result.transcriptText).toBe('{"ok":true}\n');
    const log = readFileSync(req.transcriptPath, 'utf8');
    expect(log).toContain('[command completed rc 0] pnpm test\nok\n');
    expect(log).toContain('[reasoning] thinking');
    expect(log).toContain('[file_change completed] update src/x.ts');
    // Default transcript allowlist is empty: unexpected server IDs/errors redact.
    expect(log).toContain('[mcp <redacted>.read failed]');
    expect(log).toContain('[mcp <redacted>.list failed]\n');
    expect(log).toContain('[web_search] codex docs');
    expect(log).toContain('[todo] [x] read file; [ ] write reply');
    expect(log).toContain('[codex error] tool exploded');
    expect(log).toContain('[codex stream error] stream hiccup');
  });

  it('uses only request HOME for durable auth and never reads ambient HOME (issue #75)', async () => {
    scriptedEvents([agentMessage('done'), turnCompleted()]);
    const req = makeReq();
    vi.stubEnv('HOME', join(tmp, 'ambient-home-must-not-be-used'));

    await runner.runPhase(req);

    expect(capturedCtorOptions().env).toBe(req.env);
    expect(capturedCtorOptions().env!['HOME']).toBe(join(tmp, 'home'));
    expect(capturedCtorOptions().env!['CODEX_HOME']).toBeUndefined();
  });

  it('keeps explicit CODEX_HOME as the durable auth location without treating it as connector opt-in', async () => {
    scriptedEvents([agentMessage('done'), turnCompleted()]);
    const explicitHome = join(tmp, 'explicit-codex-home');
    const baseReq = makeReq();
    const req = makeReq({ env: { ...baseReq.env, CODEX_HOME: explicitHome } });
    await runner.runPhase(req);

    expect(capturedCtorOptions().env).toBe(req.env);
    expect(capturedCtorOptions().env!['CODEX_HOME']).toBe(explicitHome);
    expect(capturedCtorOptions().config).toEqual(CODEX_UNATTENDED_CONFIG);
  });

  it('filters MCP tool-call transcript entries against MCP_TRANSCRIPT_SERVER_ALLOWLIST', async () => {
    const { MCP_TRANSCRIPT_SERVER_ALLOWLIST } = await import('../src/runners/runner-codex.js');
    MCP_TRANSCRIPT_SERVER_ALLOWLIST.clear();
    MCP_TRANSCRIPT_SERVER_ALLOWLIST.add('safe-github');
    scriptedEvents([
      {
        type: 'item.completed',
        item: {
          id: 'm1',
          type: 'mcp_tool_call',
          server: 'safe-github',
          tool: 'read',
          arguments: {},
          status: 'completed',
        },
      },
      {
        type: 'item.completed',
        item: {
          id: 'm2',
          type: 'mcp_tool_call',
          server: 'block-forge',
          tool: 'create-pr',
          arguments: {},
          status: 'completed',
        },
      },
      agentMessage('done'),
      turnCompleted(),
    ]);
    const req = makeReq();

    await runner.runPhase(req);

    const log = readFileSync(req.transcriptPath, 'utf8');
    // Allowed connector appears in transcript
    expect(log).toContain('[mcp safe-github.read completed]');
    // Blocked connector is redacted
    expect(log).not.toContain('block-forge');
    expect(log).toContain('[mcp <redacted>.create-pr completed]');
  });

  it('degrades missing/garbage usage fields to undefined instead of NaN (lockstep-drift guard)', async () => {
    scriptedEvents([
      agentMessage('{"ok":true}'),
      { type: 'turn.completed', usage: { output_tokens: 50, cached_input_tokens: 'oops' } },
    ]);
    const result = await runner.runPhase(makeReq());

    expect(result.ok).toBe(true);
    expect(result.usage.inputTokens).toBeUndefined();
    expect(result.usage.cachedInputTokens).toBeUndefined();
    expect(result.usage.outputTokens).toBe(50);
    // Output-only usage is still priceable; nothing in the pair may be NaN.
    expect(result.usage.costUsd).toBeCloseTo((50 * 30) / 1_000_000, 12);
    for (const value of Object.values(result.usage)) {
      expect(Number.isNaN(value as number)).toBe(false);
    }
  });

  it("maps turn.failed to a plain failure (signal 'none' → invoker nudges), message teed file-only", async () => {
    scriptedEvents([
      threadStarted(),
      agentMessage('partial'),
      { type: 'turn.failed', error: { message: 'model overloaded' } },
    ]);
    const req = makeReq();
    const result = await runner.runPhase(req);

    expect(result.ok).toBe(false);
    expect(result.rc).toBe(1);
    expect(result.signal).toBe('none');
    expect(result.transcriptText).toBe('partial\n');
    expect(result.sessionId).toBe('thread-1');
    expect(readFileSync(req.transcriptPath, 'utf8')).toContain('[codex turn.failed] model overloaded');
  });

  it('treats a stream that ends without turn.completed as a crashed run', async () => {
    scriptedEvents([threadStarted(), agentMessage('partial work')]);
    const result = await runner.runPhase(makeReq());

    expect(result.ok).toBe(false);
    expect(result.rc).toBe(1);
    expect(result.signal).toBe('none');
    expect(result.usage).toEqual({});
  });

  it('keeps captured output and reports rc 1 when the SDK throws (crashed-CLI parity)', async () => {
    runStreamedMock.mockImplementation(() =>
      Promise.resolve({
        events: (async function* () {
          yield agentMessage('began');
          throw new Error('Codex Exec exited with code 1: boom');
        })(),
      }),
    );
    const req = makeReq();
    const result = await runner.runPhase(req);

    expect(result.ok).toBe(false);
    expect(result.rc).toBe(1);
    expect(result.signal).toBe('none');
    expect(result.transcriptText).toBe('began\n');
    expect(readFileSync(req.transcriptPath, 'utf8')).toContain(
      '[codex runner error] Error: Codex Exec exited with code 1: boom',
    );
  });

  it('maps a constructor throw (missing native binary) to a failed result, never an exception', async () => {
    codexCtor.mockImplementation(() => {
      throw new Error('Unable to locate Codex CLI binaries.');
    });
    const req = makeReq();
    const result = await runner.runPhase(req);

    expect(result.ok).toBe(false);
    expect(result.rc).toBe(1);
    expect(result.signal).toBe('none');
    expect(readFileSync(req.transcriptPath, 'utf8')).toContain('Unable to locate Codex CLI binaries');
  });
});

describe('timeout / cancellation', () => {
  it("forwards the parent signal and maps a timeout abort to signal 'timeout'", async () => {
    const controller = new AbortController();
    runStreamedMock.mockImplementation(() =>
      Promise.resolve({
        events: (async function* () {
          yield agentMessage('working');
          controller.abort(new Error(PHASE_TIMEOUT_ABORT_REASON));
          throw new Error('The operation was aborted');
        })(),
      }),
    );
    const result = await runner.runPhase(makeReq({ signal: controller.signal }));

    expect(capturedTurnOptions().signal).toBe(controller.signal);
    expect(result.signal).toBe('timeout');
    expect(result.rc).toBe(124);
    expect(result.ok).toBe(false);
    expect(result.transcriptText).toBe('working\n');
  });

  it("maps an abort observed after a clean stream end to 'timeout', never success", async () => {
    const controller = new AbortController();
    controller.abort(new Error(PHASE_TIMEOUT_ABORT_REASON));
    scriptedEvents([agentMessage('working'), turnCompleted()]);
    const result = await runner.runPhase(makeReq({ signal: controller.signal }));

    expect(result.ok).toBe(false);
    expect(result.rc).toBe(124);
    expect(result.signal).toBe('timeout');
  });

  it('keeps the structured payload when the abort lands after the final message (parse-first parity)', async () => {
    const controller = new AbortController();
    runStreamedMock.mockImplementation(() =>
      Promise.resolve({
        events: (async function* () {
          yield threadStarted('thread-9');
          yield agentMessage('{"done":true}');
          yield turnCompleted();
          controller.abort(new Error(PHASE_TIMEOUT_ABORT_REASON));
        })(),
      }),
    );
    const result = await runner.runPhase(makeReq({ signal: controller.signal }));

    expect(result.signal).toBe('timeout');
    expect(result.ok).toBe(false);
    expect(result.structured).toEqual({ done: true });
    expect(result.usage.inputTokens).toBe(90);
    expect(result.sessionId).toBe('thread-9');
  });

  it("maps a non-timeout abort to signal 'cancelled'", async () => {
    const controller = new AbortController();
    controller.abort(new Error('user requested stop'));
    runStreamedMock.mockImplementation(() =>
      Promise.resolve({
        events: (async function* () {
          throw new Error('The operation was aborted');
        })(),
      }),
    );
    const result = await runner.runPhase(makeReq({ signal: controller.signal }));

    expect(result.signal).toBe('cancelled');
    expect(result.ok).toBe(false);
  });
});

describe('caps', () => {
  it('matches the PLAN.md Section 5 codex column', () => {
    expect(runner.id).toBe('codex');
    expect(runner.caps).toEqual(CODEX_CAPS);
    expect(CODEX_CAPS).toEqual({
      nativeSchema: true,
      perToolHook: false,
      envIsolation: 'explicit-no-inherit',
      costUsd: false,
      nativeBudget: false,
      resume: true,
    });
  });
});
