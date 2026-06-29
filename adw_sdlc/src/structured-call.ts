/**
 * The shared single-structured-call helper on @anthropic-ai/sdk (PLAN.md D1).
 *
 * classify is the only pure structured phase — its template forbids examining
 * the codebase, so it needs no tools, no shell, and no worktree. It therefore
 * runs IN-PROCESS on the plain Anthropic SDK (messages.parse + zodOutputFormat)
 * with claude-haiku-4-5 by default; the D5 process boundary does not apply
 * because nothing here can read an env or spawn a child. This path needs a
 * pay-as-you-go ANTHROPIC_API_KEY — the public messages API does not accept a
 * Claude subscription OAuth token — so the orchestrator skips this path when no
 * key is configured and falls back to the selected runner if the API rejects
 * the classify request. The runner path honors a `claude login` subscription.
 * See orchestrator.ts classify.
 */

import { zodOutputFormat } from '@anthropic-ai/sdk/helpers/zod';
import type { z } from 'zod';

import { AdwError } from './errors.js';
import type { PhaseUsage } from './invoker.js';
import { classifyModel } from './models.js';
import { costUsd } from './pricing.js';

/** The slice of the Anthropic client structuredCall touches (injectable in tests). */
export interface AnthropicLike {
  messages: {
    parse(params: Record<string, unknown>, options?: { signal?: AbortSignal }): Promise<ParsedMessageLike>;
  };
}

interface ParsedMessageLike {
  parsed_output?: unknown;
  usage?: {
    input_tokens?: number;
    output_tokens?: number;
    cache_read_input_tokens?: number | null;
  };
}

export interface StructuredCallOptions {
  model?: string;
  /** Output budget for the reply; the single retry doubles it (truncation guard). */
  maxTokens?: number;
  signal?: AbortSignal;
  /** Injectable client for tests; defaults to a lazily constructed Anthropic(). */
  client?: AnthropicLike;
}

export interface StructuredCallResult<T> {
  value: T;
  usage: PhaseUsage;
}

const STRUCTURED_NUDGE =
  '\n\nRespond with ONLY the required structured output. Do not refuse; pick the closest match.';

/**
 * One schema-constrained call returning a Zod-validated value plus usage
 * (cost parent-computed from pricing.ts — the Anthropic SDK is token-only).
 * `parsed_output` is null on refusal/truncation, so one retry nudges with a
 * doubled max_tokens before failing (PLAN.md Section 7).
 */
export async function structuredCall<S extends z.ZodType>(
  prompt: string,
  schema: S,
  options: StructuredCallOptions = {},
): Promise<StructuredCallResult<z.infer<S>>> {
  const client = options.client ?? (await defaultClient());
  const model = options.model ?? classifyModel();
  const maxTokens = options.maxTokens ?? 1024;
  const usage: PhaseUsage = {};

  const attempt = async (text: string, budget: number): Promise<unknown> => {
    let message: ParsedMessageLike;
    try {
      message = await client.messages.parse(
        {
          model,
          max_tokens: budget,
          messages: [{ role: 'user', content: text }],
          output_config: { format: zodOutputFormat(schema) },
        },
        options.signal !== undefined ? { signal: options.signal } : undefined,
      );
    } catch (err) {
      // The Anthropic SDK throws a raw APIError (e.g. 400 credit-balance,
      // 401 auth, 429 rate limit) that is not an AdwError, so without this it
      // would propagate to the CLI as an unhandled stack-trace dump. Translate
      // it into a typed, actionable AdwError at the SDK boundary; rethrow
      // anything that is not an API error unchanged.
      throw apiErrorToAdwError(err) ?? err;
    }
    accumulateUsage(usage, message);
    return message.parsed_output ?? null;
  };

  let parsed = await attempt(prompt, maxTokens);
  if (parsed === null) {
    parsed = await attempt(prompt + STRUCTURED_NUDGE, maxTokens * 2);
  }
  if (parsed === null) {
    throw new AdwError('structured call returned no parsed output (refusal or truncation) after retry');
  }
  usage.costUsd = costUsd(model, usage);
  return { value: schema.parse(parsed), usage };
}

function accumulateUsage(usage: PhaseUsage, message: ParsedMessageLike): void {
  const u = message.usage;
  if (!u) {
    return;
  }
  if (typeof u.input_tokens === 'number') {
    usage.inputTokens = (usage.inputTokens ?? 0) + u.input_tokens;
  }
  if (typeof u.output_tokens === 'number') {
    usage.outputTokens = (usage.outputTokens ?? 0) + u.output_tokens;
  }
  if (typeof u.cache_read_input_tokens === 'number') {
    usage.cachedInputTokens = (usage.cachedInputTokens ?? 0) + u.cache_read_input_tokens;
  }
}

/**
 * The fields structuredCall reads off an Anthropic SDK APIError. The SDK is
 * imported lazily (defaultClient), so this path cannot rely on
 * `instanceof APIError`; it detects the error structurally via a numeric
 * `status` and reads the human-readable message the API returned.
 */
interface AnthropicApiErrorLike {
  status?: unknown;
  error?: unknown;
  message?: unknown;
}

/**
 * Expected, user-facing failure raised when the shared Anthropic structured-call
 * endpoint rejects classify. It carries machine-readable status/reason fields so
 * the orchestrator can distinguish API-account/service failures from other
 * structured-call failures and safely fall back to the runner path.
 */
export class StructuredCallApiError extends AdwError {
  readonly status: number;
  readonly reason: string;

  constructor(status: number, apiMessage: string, options?: ErrorOptions) {
    const trimmedMessage = apiMessage.trim();
    const reason = trimmedMessage !== '' ? trimmedMessage : `the request was rejected with HTTP ${status}`;
    super(
      `classify failed: ${reason} (Anthropic API HTTP ${status}). ` +
        'The classify phase calls the public Anthropic messages API with ANTHROPIC_API_KEY, ' +
        'which is billed pay-as-you-go and does not accept a Claude subscription. ' +
        'Resolve the API account issue (e.g. add credits / fix billing or the key), ' +
        'or set ADW_CLASSIFY_ON_RUNNER=1 to route classify through the selected runner, ' +
        'which honors a `claude login` subscription.',
      options,
    );
    this.name = 'StructuredCallApiError';
    this.status = status;
    this.reason = reason;
  }
}

/**
 * Translate a raw Anthropic SDK APIError into a typed, actionable AdwError so
 * callers can handle it as an expected API-boundary failure instead of dumping
 * an unhandled stack trace. Returns null for anything that is not an API error,
 * so the caller rethrows it unchanged.
 */
function apiErrorToAdwError(err: unknown): StructuredCallApiError | null {
  if (typeof err !== 'object' || err === null) {
    return null;
  }
  const e = err as AnthropicApiErrorLike;
  if (typeof e.status !== 'number') {
    return null;
  }
  return new StructuredCallApiError(e.status, extractApiMessage(e), { cause: err });
}

/**
 * Best human-readable text from an Anthropic API error: prefer the nested
 * `error.error.message` the API returns, falling back to the SDK Error's own
 * `message` (which is `"<status> <body>"`).
 */
function extractApiMessage(e: AnthropicApiErrorLike): string {
  const body = e.error;
  if (typeof body === 'object' && body !== null) {
    // Anthropic's API body is usually { error: { message } }, and the SDK's
    // generated helper also supports { message } at the top level. Preserve
    // both shapes so callers see the provider's specific reason instead of a
    // generic HTTP-status fallback.
    const topLevelMessage = (body as { message?: unknown }).message;
    if (typeof topLevelMessage === 'string' && topLevelMessage.trim() !== '') {
      return topLevelMessage.trim();
    }
    const nested = (body as { error?: unknown }).error;
    if (typeof nested === 'object' && nested !== null) {
      const message = (nested as { message?: unknown }).message;
      if (typeof message === 'string' && message.trim() !== '') {
        return message.trim();
      }
    }
  }
  if (typeof e.message === 'string' && e.message.trim() !== '') {
    return e.message.trim();
  }
  return '';
}

async function defaultClient(): Promise<AnthropicLike> {
  // Lazy so importing this module never constructs a client (or demands an
  // API key) — only a real un-injected call does.
  const { default: Anthropic } = await import('@anthropic-ai/sdk');
  return new Anthropic() as unknown as AnthropicLike;
}
