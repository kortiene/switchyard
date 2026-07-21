/**
 * Runner #1: `claude` via `@anthropic-ai/claude-agent-sdk` (PLAN.md roadmap
 * step 6, Sections 4.3-1 and 5).
 *
 * The secret boundary is the SDK's own child process: `options.env` REPLACES
 * `process.env` when set (verified on installed 0.3.183). The SDK clones that
 * allowlist, may add its own entrypoint/version/telemetry controls, and removes
 * Node debug knobs before spawn; it never merges the full parent environment.
 * This module must never spread `process.env` (enforced by env lint, unit tests,
 * and the names-only real-spawn audit in test/fixtures/live-evidence/).
 *
 * Step-6 [VERIFY] resolutions (installed sdk.d.ts, 0.3.183):
 * - CanUseTool: (toolName, input, {signal, toolUseID, ...}) =>
 *   Promise<PermissionResult>; PermissionResult is the allow/deny union with
 *   `updatedInput`/`message`.
 * - PermissionMode: 'default'|'acceptEdits'|'bypassPermissions'|'plan'|
 *   'dontAsk'|'auto' — 'acceptEdits' exists as planned.
 * - There is NO `maxStructuredOutputRetries` option; schema-retry exhaustion
 *   surfaces as the result subtype 'error_max_structured_output_retries',
 *   which this adapter maps to a failed PhaseResult with signal 'none' so the
 *   invoker's single nudge applies (PLAN.md Section 7).
 */

import { accessSync, appendFileSync, constants, statSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { delimiter, dirname, join } from 'node:path';

import { query } from '@anthropic-ai/claude-agent-sdk';
import type {
  CanUseTool,
  HookCallback,
  Options,
  SDKMessage,
  SDKResultMessage,
} from '@anthropic-ai/claude-agent-sdk';

import type {
  AgentRunner,
  PhaseRequest,
  PhaseResult,
  PhaseUsage,
  RunnerCaps,
} from '../invoker.js';
import { abortKind, TIMEOUT_RC } from './shared.js';

/** PLAN.md Section 5, claude column. */
export const CLAUDE_CAPS: RunnerCaps = {
  nativeSchema: true,
  perToolHook: true,
  envIsolation: 'explicit-no-inherit',
  costUsd: true,
  nativeBudget: true,
  resume: true,
};

/**
 * The capability grant (PLAN.md Section 4.2): today's agent is a CLI with
 * full-fs access in the worktree, so the runner must read and edit unattended
 * or every editing phase becomes a no-op.
 */
export const CLAUDE_EDIT_TOOLS = ['Read', 'Write', 'Edit', 'Glob', 'Grep', 'Bash'] as const;

/**
 * Tools auto-allowed via `allowedTools`. Bash is deliberately absent so the
 * SDK permission callback remains a defense in depth. A PreToolUse hook runs
 * for every Bash request and applies the best-effort command recognizer below;
 * a live Claude Code 2.1.211 probe showed that permission-mode safe-command
 * rules can bypass canUseTool.
 */
export const CLAUDE_AUTO_ALLOWED_TOOLS = ['Read', 'Write', 'Edit', 'Glob', 'Grep'] as const;

/**
 * Best-effort guard for a git/gh invocation at a command position (after ^,
 * a separator, command substitution, or an `env`/`command` prefix). The
 * load-bearing control is GH_TOKEN's absence from the child env — this veto
 * just fails the attempt earlier and louder (PLAN.md Section 4.4).
 */
const GIT_GH_COMMAND = /(^|[\n;&|]|\$\(|`)\s*(?:command\s+|builtin\s+|env\s+(?:\w+=\S*\s+)*)?(?:git|gh)\b/;

function bashInvokesGitGh(toolName: string, input: Record<string, unknown>): boolean {
  const command = typeof input['command'] === 'string' ? (input['command'] as string) : '';
  return toolName === 'Bash' && GIT_GH_COMMAND.test(command);
}

/**
 * Permission callback defense in depth. Denies Bash commands that invoke
 * git/gh (the orchestrator owns all git/gh, PLAN.md Section 3.3; mirrors the
 * PHASE_PREAMBLE_SHARED contract). The `tools` option removes tools outside
 * CLAUDE_EDIT_TOOLS; PreToolUse below is the unconditional hook point for the
 * remaining Bash tool, while command recognition remains best-effort.
 */
export const denyGitGh: CanUseTool = (toolName, input) => {
  if (!(CLAUDE_EDIT_TOOLS as readonly string[]).includes(toolName)) {
    return Promise.resolve({
      behavior: 'deny',
      message: `Tool '${toolName}' is outside this phase's grant (${CLAUDE_EDIT_TOOLS.join(', ')}).`,
    });
  }
  if (bashInvokesGitGh(toolName, input)) {
    return Promise.resolve({
      behavior: 'deny',
      message:
        'The orchestrator owns all git/gh operations; do not run git or gh. ' +
        'Edit files and run tests only — the pipeline commits, pushes, and opens the PR.',
    });
  }
  return Promise.resolve({ behavior: 'allow', updatedInput: input });
};

/**
 * Persist a names/category-only proof when the live per-tool veto fires. The
 * command and tool input are deliberately never serialized: they can contain
 * paths, inline credentials, or command substitutions. Audit persistence is
 * best-effort; a write failure must never replace the explicit deny verdict.
 */
export function gitGhPreToolUseHook(phase: string, auditPath: string): HookCallback {
  return async (hookInput) => {
    if (hookInput.hook_event_name !== 'PreToolUse') {
      return { continue: true };
    }
    const input =
      hookInput.tool_input !== null &&
      typeof hookInput.tool_input === 'object' &&
      !Array.isArray(hookInput.tool_input)
        ? (hookInput.tool_input as Record<string, unknown>)
        : {};
    if (bashInvokesGitGh(hookInput.tool_name, input)) {
      try {
        appendFileSync(
          auditPath,
          `${JSON.stringify({
            schema_version: 1,
            phase,
            tool_name: 'Bash',
            category: 'git-gh-veto',
            decision: 'deny',
            input_recorded: false,
          })}\n`,
          'utf8',
        );
      } catch {
        // Claude hook errors are not a blocking decision. Preserve the explicit
        // deny even when the optional audit artifact cannot be written.
      }
      return {
        continue: true,
        hookSpecificOutput: {
          hookEventName: 'PreToolUse',
          permissionDecision: 'deny',
          permissionDecisionReason:
            'The orchestrator owns all git/gh operations; do not run git or gh.',
        },
      };
    }
    return { continue: true };
  };
}

/**
 * Resolve the Claude Code binary like the Python pipeline does
 * (adw/_exec.py:201-213): CLAUDE_BIN override, then PATH, then the two
 * well-known install locations. Resolution reads the ALLOWLIST env (the same
 * env the child gets), never process.env. Unlike Python this returns
 * undefined instead of raising when nothing is found: the SDK then uses its
 * own built-in executable ("Uses the built-in executable if not specified"),
 * which is strictly more available than failing.
 */
export function resolveClaudeBin(env: Record<string, string | undefined>): string | undefined {
  const override = env['CLAUDE_BIN'];
  if (override) {
    return override;
  }
  for (const dir of (env['PATH'] ?? '').split(delimiter)) {
    if (dir && isExecutableFile(join(dir, 'claude'))) {
      return join(dir, 'claude');
    }
  }
  const home = env['HOME'] ?? homedir();
  for (const candidate of [join(home, '.claude/local/claude'), join(home, '.local/bin/claude')]) {
    if (isExecutableFile(candidate)) {
      return candidate;
    }
  }
  return undefined;
}

function isExecutableFile(path: string): boolean {
  try {
    accessSync(path, constants.X_OK);
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

/** Accumulate the text blocks of an assistant message (the human-readable transcript). */
function assistantText(message: SDKMessage): string {
  if (message.type !== 'assistant') {
    return '';
  }
  const content: unknown = message.message.content;
  if (typeof content === 'string') {
    return content === '' ? '' : `${content}\n`;
  }
  if (!Array.isArray(content)) {
    return '';
  }
  let out = '';
  for (const block of content) {
    if (
      block !== null &&
      typeof block === 'object' &&
      (block as { type?: unknown }).type === 'text' &&
      typeof (block as { text?: unknown }).text === 'string'
    ) {
      out += (block as { text: string }).text;
    }
  }
  return out === '' ? '' : `${out}\n`;
}

function usageOf(result: SDKResultMessage): PhaseUsage {
  const usage: PhaseUsage = { costUsd: result.total_cost_usd };
  const raw = result.usage as unknown as Partial<Record<string, number>>;
  if (typeof raw['input_tokens'] === 'number') usage.inputTokens = raw['input_tokens'];
  if (typeof raw['output_tokens'] === 'number') usage.outputTokens = raw['output_tokens'];
  if (typeof raw['cache_read_input_tokens'] === 'number') {
    usage.cachedInputTokens = raw['cache_read_input_tokens'];
  }
  return usage;
}

function asStructured(value: unknown): Record<string, unknown> | null {
  if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
    return value as Record<string, unknown>;
  }
  return null;
}

/**
 * Claude Code validates `--json-schema` as a schema body but does not register
 * Zod v4's draft-2020 meta-schema URI. Passing the generated top-level
 * `$schema` therefore fails before the model starts (observed live with Claude
 * Code 2.1.211). The dialect marker is annotation-only for these phase shapes;
 * remove it on a copy while preserving the schema the parent later validates.
 */
export function claudeOutputSchema(schema: Record<string, unknown>): Record<string, unknown> {
  const compatible = { ...schema };
  delete compatible['$schema'];
  return compatible;
}

/**
 * SDK 0.3.183 can enqueue an `error_max_budget_usd` result and then replace it
 * with a thrown process-exit error before the async consumer observes the
 * result. Preserve the public native-budget meaning in that observed shape.
 */
export function isClaudeBudgetError(value: unknown): boolean {
  const message = value instanceof Error ? value.message : String(value);
  return message.startsWith('Claude Code returned an error result: Reached maximum budget');
}

class ClaudeRunner implements AgentRunner {
  readonly id = 'claude' as const;
  readonly caps = CLAUDE_CAPS;

  async runPhase(req: PhaseRequest): Promise<PhaseResult> {
    // The SDK takes an AbortController; bridge the orchestrator-owned signal.
    const abortController = new AbortController();
    const forwardAbort = (): void => {
      abortController.abort(req.signal.reason as Error | undefined);
    };
    if (req.signal.aborted) {
      forwardAbort();
    } else {
      req.signal.addEventListener('abort', forwardAbort, { once: true });
    }

    writeFileSync(req.transcriptPath, '', 'utf8');
    const tee = (text: string): void => {
      if (text !== '') {
        appendFileSync(req.transcriptPath, text, 'utf8');
      }
    };

    const claudeBin = resolveClaudeBin(req.env);
    const options: Options = {
      model: req.model,
      cwd: req.cwd,
      // The SDK clones this entire replacement env and may add its own
      // entrypoint/version/telemetry controls before the child spawn.
      env: req.env,
      abortController,
      // `allowedTools` controls prompting, not availability. Restrict the tool
      // context explicitly, then auto-allow the non-Bash subset.
      tools: [...CLAUDE_EDIT_TOOLS],
      allowedTools: [...CLAUDE_AUTO_ALLOWED_TOOLS],
      permissionMode: 'acceptEdits',
      canUseTool: denyGitGh,
      hooks: {
        PreToolUse: [
          {
            hooks: [
              gitGhPreToolUseHook(
                req.phase,
                join(dirname(req.transcriptPath), 'tool-veto-audit.jsonl'),
              ),
            ],
          },
        ],
      },
      // Today's `claude -p` runs with Claude Code's default system prompt and
      // CLI-default setting sources; keep both for AS-IS parity.
      systemPrompt: { type: 'preset', preset: 'claude_code' },
      stderr: (data: string) => tee(data),
      ...(claudeBin !== undefined ? { pathToClaudeCodeExecutable: claudeBin } : {}),
      ...(req.schema !== undefined
        ? { outputFormat: { type: 'json_schema' as const, schema: claudeOutputSchema(req.schema) } }
        : {}),
      ...(req.resumeSessionId !== undefined ? { resume: req.resumeSessionId } : {}),
      ...(req.maxBudgetUsd !== undefined ? { maxBudgetUsd: req.maxBudgetUsd } : {}),
    };

    let transcriptText = '';
    let result: SDKResultMessage | undefined;
    try {
      for await (const message of query({ prompt: req.prompt, options })) {
        const text = assistantText(message);
        if (text !== '') {
          transcriptText += text;
          tee(text);
        }
        if (message.type === 'result') {
          result = message;
        }
      }
    } catch (err) {
      if (req.signal.aborted) {
        return this.failed(transcriptText, abortKind(req.signal), TIMEOUT_RC, result);
      }
      const budgetResult = result?.subtype === 'error_max_budget_usd' ? result : undefined;
      if (budgetResult !== undefined || isClaudeBudgetError(err)) {
        const reasons =
          budgetResult !== undefined && Array.isArray(budgetResult.errors)
            ? budgetResult.errors.join('\n')
            : err instanceof Error
              ? err.message
              : String(err);
        tee(`\n[claude result error_max_budget_usd] ${reasons}\n`);
        return this.failed(transcriptText, 'budget', 1, result);
      }
      // Mirror a crashed CLI run (adw/_phases.py:482-516): keep the captured
      // output, report a nonzero rc, and let the invoker parse/nudge/fail.
      tee(`\n[claude runner error] ${String(err)}\n`);
      return this.failed(transcriptText, 'none', 1, result);
    } finally {
      req.signal.removeEventListener('abort', forwardAbort);
    }

    if (req.signal.aborted) {
      return this.failed(transcriptText, abortKind(req.signal), TIMEOUT_RC, result);
    }
    if (result === undefined) {
      tee('\n[claude runner error] stream ended without a result message\n');
      return this.failed(transcriptText, 'none', 1, undefined);
    }

    if (result.subtype === 'success') {
      if (transcriptText === '' && result.result !== '') {
        transcriptText = result.result;
        tee(result.result);
      }
      return {
        ok: !result.is_error,
        structured: asStructured(result.structured_output),
        transcriptText,
        usage: usageOf(result),
        rc: result.is_error ? 1 : 0,
        signal: 'none',
        sessionId: result.session_id,
      };
    }
    // error_max_budget_usd is the native cost cap → fail fast, no nudge;
    // every other error subtype (error_during_execution, error_max_turns,
    // error_max_structured_output_retries) stays 'none' so the invoker's
    // single nudge-retry applies exactly as to a failed CLI run.
    // The SDK's failure reasons go to the transcript FILE only: appending them
    // to transcriptText would break the trailing-fenced-JSON fallback parse
    // when the assistant did finish its reply before the error.
    const reasons = Array.isArray(result.errors) ? result.errors.join('\n') : '';
    tee(`\n[claude result ${result.subtype}] ${reasons}\n`);
    return {
      ok: false,
      structured: null,
      transcriptText,
      usage: usageOf(result),
      rc: 1,
      signal: result.subtype === 'error_max_budget_usd' ? 'budget' : 'none',
      sessionId: result.session_id,
    };
  }

  private failed(
    transcriptText: string,
    signal: PhaseResult['signal'],
    rc: number,
    result: SDKResultMessage | undefined,
  ): PhaseResult {
    return {
      ok: false,
      // A terminal success result that arrived before the abort/teardown error
      // still carries the agent's structured payload; pass it through so the
      // invoker's parse-first semantics (run-phase.ts, mirroring
      // adw/_phases.py:507-513) can accept a completed run — the signal still
      // reports the abort and the invoker stays the policy owner.
      structured:
        result !== undefined && result.subtype === 'success'
          ? asStructured(result.structured_output)
          : null,
      transcriptText,
      usage: result !== undefined ? usageOf(result) : {},
      rc,
      signal,
      ...(result !== undefined ? { sessionId: result.session_id } : {}),
    };
  }
}

export function createRunner(): AgentRunner {
  return new ClaudeRunner();
}
