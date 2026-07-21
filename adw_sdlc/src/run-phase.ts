/**
 * The invoker layer over AgentRunner.runPhase (PLAN.md D6 / Section 7):
 * compose the prompt, persist prompt.txt, drive exactly one runner call —
 * with the single nudge-retry and the no-retry-on-timeout mapping living
 * HERE, once, never per adapter — and return the Zod-validated phase result.
 *
 * Semantics carried from adw/_phases.py:482-517 run_agent_phase:
 * - the reply is parsed regardless of exit code; a nonzero rc with parseable
 *   output is still a success (the bounded loops act on the parsed counts);
 * - a timed-out/killed runner won't do better on a re-run with the same time
 *   box, so a timeout (or a native budget/cancel signal) fails fast with NO
 *   nudge — only a parse/validation failure on a clean run earns the single
 *   nudge retry;
 * - a native-schema backend returning a non-conforming or null payload
 *   triggers the same single nudge (PLAN.md Section 7).
 */

import { writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { parseJson, projectRoot } from './common.js';
import { AdwError, RunnerAuthError, RunnerTransientError } from './errors.js';
import { PHASE_TIMEOUT_ABORT_REASON } from './invoker.js';
import type { AgentRunner, JsonSchema, PhaseResult, PhaseUsage } from './invoker.js';
import { modelForPhase } from './models.js';
import { composePhasePrompt, type AgentPhase } from './phases.js';
import { resolvePhaseSchema } from './schema-registry.js';
import { PHASE_SCHEMAS, type SchemaPhase } from './schemas.js';
import type { AdwState } from './state.js';
import type { z } from 'zod';

/** Verbatim from adw/_phases.py:472. */
export const NUDGE =
  '\n\nRespond with ONLY the required JSON object in a ```json fenced block, nothing else.';

const CLAUDE_PAYG_AUTH_ENV_KEYS = ['ANTHROPIC_API_KEY', 'ANTHROPIC_AUTH_TOKEN'] as const;

export interface RunAgentPhaseOptions {
  phase: SchemaPhase;
  templateArgs: readonly string[];
  state: AdwState;
  runner: AgentRunner;
  /** --model override; per-phase ADW_MODEL_<PHASE> still applies under it. */
  cliModel?: string;
  /** The allowlist env the orchestrator built (safeSubprocessEnv). */
  env: Record<string, string>;
  /** The worktree the agent edits; defaults to the project root. */
  cwd?: string;
  /** Per-call timeout in milliseconds; 0/undefined = none. */
  timeoutMs?: number;
  /** Parent cancellation (managed supervisor / embedding). */
  signal?: AbortSignal;
  /** Forwarded to backends with native budget gating (claude). */
  maxBudgetUsd?: number;
  /**
   * Measurement mode (ADW_PARITY_FORCE_FENCED_JSON): route a native-schema runner
   * through the fenced-JSON contract path it would otherwise skip — the prompt
   * carries the contract footer and no native schema is handed to the SDK. Used
   * only to harvest a fenced-path baseline for the parity hard-failure-rate bar
   * (tools/parity-rate.ts) from a runner that is natively native-schema; it has
   * no effect on a runner that is already fenced (pi). Default off ⇒ unchanged.
   */
  forceFenced?: boolean;
}

export interface AgentPhaseOutcome<P extends SchemaPhase = SchemaPhase> {
  data: z.infer<(typeof PHASE_SCHEMAS)[P]>;
  usage: PhaseUsage;
  sessionId?: string;
  /**
   * Runner calls this phase consumed: 1 on a clean first parse, 2 when the
   * single nudge-retry fired. The nudge double-charges (both attempts' tokens
   * are summed into `usage`), so this is the load-bearing signal for the
   * cost/duration work — a high cross-run attempts rate is the lever to attack
   * (see docs/COST-AND-DURATION.md). Always 1 or 2 (the invoker nudges once).
   */
  attempts: number;
}

/**
 * Render+run one agent phase through the runner seam and return its
 * validated result. Retries once with a "respond with JSON only" nudge if
 * the first reply does not parse/validate; a second failure (or any
 * timeout/budget/cancel signal) raises AdwError.
 */
export async function runAgentPhase<P extends SchemaPhase>(
  options: RunAgentPhaseOptions & { phase: P },
): Promise<AgentPhaseOutcome<P>> {
  const { phase, state, runner } = options;
  const phaseSchema = resolvePhaseSchema(phase);
  // Fenced path when the runner lacks a native schema OR measurement mode forces
  // it; the native schema is handed to the SDK only on the (unforced) native path.
  const emitJsonContract = !runner.caps.nativeSchema || options.forceFenced === true;
  const prompt = composePhasePrompt(
    phase as AgentPhase,
    options.templateArgs,
    state,
    runner.id,
    emitJsonContract,
  );
  const phaseDir = state.phaseDir(phase);
  writeFileSync(join(phaseDir, 'prompt.txt'), prompt, 'utf8');
  const model = modelForPhase(phase, runner.id, { cliModel: options.cliModel ?? '' });
  const schema = emitJsonContract ? undefined : phaseSchema.jsonSchema();

  const invoke = async (
    text: string,
    transcriptName: string,
    requestSchema: JsonSchema | undefined,
  ): Promise<PhaseResult> => {
    const controller = new AbortController();
    const timeoutMs = options.timeoutMs ?? 0;
    const timer =
      timeoutMs > 0
        ? setTimeout(() => controller.abort(new Error(PHASE_TIMEOUT_ABORT_REASON)), timeoutMs)
        : null;
    const abortFromParent = (): void => controller.abort(options.signal?.reason);
    if (options.signal?.aborted) abortFromParent();
    else options.signal?.addEventListener('abort', abortFromParent, { once: true });
    try {
      const request = {
        phase,
        prompt: text,
        model,
        cwd: options.cwd ?? projectRoot(),
        env: options.env,
        transcriptPath: join(phaseDir, transcriptName),
        signal: controller.signal,
        ...(requestSchema !== undefined ? { schema: requestSchema } : {}),
        ...(options.maxBudgetUsd !== undefined ? { maxBudgetUsd: options.maxBudgetUsd } : {}),
      };
      return await runner.runPhase(request);
    } finally {
      options.signal?.removeEventListener('abort', abortFromParent);
      if (timer !== null) {
        clearTimeout(timer);
      }
    }
  };

  const extract = (result: PhaseResult): z.infer<(typeof PHASE_SCHEMAS)[P]> => {
    const payload = result.structured ?? parseJson(result.transcriptText, 'object');
    return phaseSchema.validate(payload);
  };

  const first = await invoke(prompt, 'transcript.log', schema);
  try {
    return { data: extract(first), usage: first.usage, attempts: 1, ...sessionOf(first) };
  } catch (err) {
    if (!(err instanceof AdwError)) {
      throw err;
    }
    const firstAuthFailure = runnerAuthFailureReason(first, runner.id, options.env);
    if (firstAuthFailure !== null) {
      throw new RunnerAuthError(runner.id, phase, firstAuthFailure, { cause: err });
    }
    // Mirrors the _TIMEOUT_EXIT_CODES fail-fast: a run the parent had to kill
    // (or that hit a native cost cap) gets no nudge.
    if (first.signal === 'timeout') {
      throw new AdwError(`${phase} phase runner timed out without parseable output`, { cause: err });
    }
    if (first.signal === 'budget') {
      throw new AdwError(`${phase} phase hit the native budget cap without parseable output`, {
        cause: err,
      });
    }
    if (first.signal === 'cancelled') {
      throw new AdwError(`${phase} phase was cancelled without parseable output`, { cause: err });
    }
    // Native-schema backends get a true fenced-JSON fallback: the retry carries
    // the contract the first prompt omitted AND withholds the native schema
    // channel. This matters when a provider ignores or cannot execute a native
    // json_schema/structured-output tool: retaining that channel can make the
    // backend reject another otherwise usable prose/fenced response. The
    // first prompt deliberately omits the contract (the schema rides the native
    // channel), but the SDK can return success without a conforming payload
    // (structured_output is optional on SDKResultSuccess) — and a bare NUDGE
    // would demand "the required JSON object" the agent was never shown.
    const retryPrompt = emitJsonContract
      ? prompt
      : composePhasePrompt(phase as AgentPhase, options.templateArgs, state, runner.id, true);
    const second = await invoke(retryPrompt + NUDGE, 'transcript-2.log', undefined);
    let data: z.infer<(typeof PHASE_SCHEMAS)[P]>;
    try {
      data = extract(second);
    } catch (secondErr) {
      if (secondErr instanceof AdwError) {
        const secondAuthFailure = runnerAuthFailureReason(second, runner.id, options.env);
        if (secondAuthFailure !== null) {
          throw new RunnerAuthError(runner.id, phase, secondAuthFailure, { cause: secondErr });
        }
        // A transient provider error (API 5xx / overload) is not a prompt
        // problem — the nudge could not have fixed it. Classify it so the
        // orchestrator retries the phase with backoff instead of aborting the
        // whole run on what looks like an unparseable reply. Check both attempts
        // (the blip may have spanned the first and the nudge).
        const transient =
          runnerTransientFailureReason(second) ?? runnerTransientFailureReason(first);
        if (transient !== null) {
          throw new RunnerTransientError(runner.id, phase, transient, { cause: secondErr });
        }
      }
      throw secondErr;
    }
    // Both attempts consumed tokens; report the pair's combined usage and the
    // attempts count (2) so the caller can record the nudge-retry rate.
    return {
      data,
      usage: mergeUsage(first.usage, second.usage),
      attempts: 2,
      ...sessionOf(second),
    };
  }
}

function runnerAuthFailureReason(
  result: PhaseResult,
  runnerId: AgentRunner['id'],
  env: Record<string, string>,
): string | null {
  if (runnerId !== 'claude') {
    return null;
  }
  const text = result.transcriptText;
  const paygKeys = CLAUDE_PAYG_AUTH_ENV_KEYS.filter((key) => Object.prototype.hasOwnProperty.call(env, key));
  if (/credit balance is too low/i.test(text)) {
    return paygKeys.length > 0
      ? `Claude Code used ${paygKeys.join(', ')} and the Anthropic API credit balance is too low`
      : 'Anthropic API credit balance is too low';
  }
  if (/(?:ANTHROPIC_API_KEY|ANTHROPIC_AUTH_TOKEN|auth source)[\s\S]*takes precedence[\s\S]*(?:claude\.ai login|login)/i.test(text)) {
    return paygKeys.length > 0
      ? `Claude Code is using ${paygKeys.join(', ')} instead of claude.ai login`
      : 'Claude Code is using an auth source instead of claude.ai login';
  }
  // A logged-out subscription surfaces as "Not logged in · Please run /login"
  // and otherwise falls through as a confusing JSON-parse failure. Classify it
  // so the operator gets an actionable message (re-login, then resume) instead.
  if (/not logged in|please run\s*`?\/login`?|run\s+`?\/login`?\s+to (?:log|sign) in/i.test(text)) {
    return 'Claude Code is not logged in (run `claude` then /login, or restore ANTHROPIC_API_KEY)';
  }
  return null;
}

/**
 * Detect a transient provider failure in a runner transcript — an API 5xx,
 * "internal server error", overload/529, or a gateway error the SDK surfaced
 * instead of a reply. Returns the matched phrase (for the error message) or
 * null. Deliberately narrow: it must not match an agent merely *discussing* an
 * error, so each pattern is anchored on wording an infrastructure failure emits.
 * Auth failures are classified separately and take precedence over this.
 */
function runnerTransientFailureReason(result: PhaseResult): string | null {
  const text = result.transcriptText;
  const patterns: readonly RegExp[] = [
    /internal server error/i,
    /service unavailable/i,
    /bad gateway/i,
    /gateway time-?out/i,
    /overloaded(?:_error)?/i,
    /\b(?:api error|http error|http|status(?:\s*code)?|error)[\s:#]+5(?:00|02|03|04|29)\b/i,
    /\b529\b[^\n]{0,40}?(?:overloaded|too many|rate)/i,
  ];
  for (const re of patterns) {
    const m = text.match(re);
    if (m) {
      return m[0].trim();
    }
  }
  return null;
}

function sessionOf(result: PhaseResult): { sessionId?: string } {
  return result.sessionId !== undefined ? { sessionId: result.sessionId } : {};
}

function addCounts(a: number | undefined, b: number | undefined): number | undefined {
  return a === undefined && b === undefined ? undefined : (a ?? 0) + (b ?? 0);
}

function mergeUsage(a: PhaseUsage, b: PhaseUsage): PhaseUsage {
  const usage: PhaseUsage = {};
  const inputTokens = addCounts(a.inputTokens, b.inputTokens);
  const outputTokens = addCounts(a.outputTokens, b.outputTokens);
  const cachedInputTokens = addCounts(a.cachedInputTokens, b.cachedInputTokens);
  const reasoningTokens = addCounts(a.reasoningTokens, b.reasoningTokens);
  if (inputTokens !== undefined) usage.inputTokens = inputTokens;
  if (outputTokens !== undefined) usage.outputTokens = outputTokens;
  if (cachedInputTokens !== undefined) usage.cachedInputTokens = cachedInputTokens;
  if (reasoningTokens !== undefined) usage.reasoningTokens = reasoningTokens;
  // null means "could not be priced" (PLAN.md Section 6) — if either attempt
  // is unpriceable the pair's cost is unknown, never a false partial sum.
  if (a.costUsd === null || b.costUsd === null) {
    usage.costUsd = null;
  } else if (a.costUsd !== undefined || b.costUsd !== undefined) {
    usage.costUsd = (a.costUsd ?? 0) + (b.costUsd ?? 0);
  }
  return usage;
}
