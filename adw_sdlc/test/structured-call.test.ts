/**
 * The shared classify structured call (PLAN.md D1): messages.parse +
 * zodOutputFormat with a null-parsed_output retry, token accounting, and
 * parent-computed cost (the Anthropic SDK is token-only). The client is
 * injected — no SDK construction, no network, no API key.
 */

import { describe, expect, it, vi } from 'vitest';

import { AdwError } from '../src/errors.js';
import { ClassifySchema } from '../src/schemas.js';
import { StructuredCallApiError, structuredCall, type AnthropicLike } from '../src/structured-call.js';

function fakeClient(outputs: Array<unknown>): { client: AnthropicLike; parse: ReturnType<typeof vi.fn> } {
  const parse = vi.fn();
  for (const parsed of outputs) {
    parse.mockResolvedValueOnce({
      parsed_output: parsed,
      usage: { input_tokens: 100, output_tokens: 10, cache_read_input_tokens: 50 },
    });
  }
  return { client: { messages: { parse } }, parse };
}

describe('structuredCall', () => {
  it('returns the schema-validated value with usage and parent-computed cost', async () => {
    const { client, parse } = fakeClient([{ issue_class: 'feat', reason: 'adds a thing' }]);
    const { value, usage } = await structuredCall('classify this', ClassifySchema, { client });
    expect(value).toEqual({ issue_class: 'feat', reason: 'adds a thing' });
    expect(usage.inputTokens).toBe(100);
    expect(usage.outputTokens).toBe(10);
    expect(usage.cachedInputTokens).toBe(50);
    // claude-haiku-4-5 is priced in pricing.ts, so cost is a number.
    expect(usage.costUsd).toBeCloseTo((100 * 1.0 + 10 * 5.0 + 50 * 0.1) / 1_000_000);

    expect(parse).toHaveBeenCalledTimes(1);
    const params = parse.mock.calls[0]![0] as Record<string, unknown>;
    expect(params['model']).toBe('claude-haiku-4-5');
    expect(params['output_config']).toBeDefined();
  });

  it('retries once with a doubled budget when parsed_output is null (refusal/truncation)', async () => {
    const { client, parse } = fakeClient([null, { issue_class: 'fix', reason: '' }]);
    const { value, usage } = await structuredCall('classify this', ClassifySchema, {
      client,
      maxTokens: 256,
    });
    expect(value.issue_class).toBe('fix');
    expect(parse).toHaveBeenCalledTimes(2);
    const second = parse.mock.calls[1]![0] as Record<string, unknown>;
    expect(second['max_tokens']).toBe(512);
    // Tokens from BOTH attempts are accounted.
    expect(usage.inputTokens).toBe(200);
  });

  it('raises AdwError when both attempts return null', async () => {
    const { client, parse } = fakeClient([null, null]);
    await expect(structuredCall('classify this', ClassifySchema, { client })).rejects.toThrow(AdwError);
    expect(parse).toHaveBeenCalledTimes(2);
  });

  it('rejects a parsed payload that fails the Zod schema', async () => {
    const { client } = fakeClient([{ issue_class: 'not-a-class', reason: '' }]);
    await expect(structuredCall('classify this', ClassifySchema, { client })).rejects.toThrow();
  });

  it('translates an Anthropic API error into an actionable AdwError (no raw SDK crash)', async () => {
    // Shape of @anthropic-ai/sdk APIError: numeric `status` + nested `error.error.message`.
    const apiError = Object.assign(new Error('400 Your credit balance is too low'), {
      status: 400,
      error: { type: 'error', error: { type: 'invalid_request_error', message: 'Your credit balance is too low to access the Anthropic API.' } },
    });
    const parse = vi.fn().mockRejectedValueOnce(apiError);
    const client: AnthropicLike = { messages: { parse } };

    const promise = structuredCall('classify this', ClassifySchema, { client });
    await expect(promise).rejects.toThrow(StructuredCallApiError);
    // Surfaces the API's own reason and the documented runner escape hatch.
    await expect(promise).rejects.toThrow(/credit balance is too low/);
    await expect(promise).rejects.toThrow(/ADW_CLASSIFY_ON_RUNNER=1/);
    // The original SDK error is preserved as the cause for debugging, while the
    // status/reason fields let the orchestrator safely fall back to the runner.
    await promise.catch((err: unknown) => {
      expect((err as { cause?: unknown }).cause).toBe(apiError);
      expect((err as StructuredCallApiError).status).toBe(400);
      expect((err as StructuredCallApiError).reason).toBe('Your credit balance is too low to access the Anthropic API.');
    });
    // A 400 is not retried — the credit/auth condition will not change mid-run.
    expect(parse).toHaveBeenCalledTimes(1);
  });

  it('extracts top-level Anthropic API error messages, not only nested messages', async () => {
    const apiError = Object.assign(new Error('401 bad key'), {
      status: 401,
      error: { message: 'Invalid API key supplied.' },
    });
    const parse = vi.fn().mockRejectedValueOnce(apiError);
    const client: AnthropicLike = { messages: { parse } };

    const promise = structuredCall('classify this', ClassifySchema, { client });
    await expect(promise).rejects.toThrow(/Invalid API key supplied/);
    await promise.catch((err: unknown) => {
      expect((err as StructuredCallApiError).reason).toBe('Invalid API key supplied.');
    });
  });

  it('falls back to an HTTP-status reason when an API error has no readable message', () => {
    const err = new StructuredCallApiError(503, '   ');
    expect(err.reason).toBe('the request was rejected with HTTP 503');
    expect(err.message).toContain('Anthropic API HTTP 503');
  });

  it('rethrows a non-API error unchanged (not wrapped as AdwError)', async () => {
    const boom = new TypeError('network exploded');
    const parse = vi.fn().mockRejectedValueOnce(boom);
    const client: AnthropicLike = { messages: { parse } };
    await expect(structuredCall('classify this', ClassifySchema, { client })).rejects.toBe(boom);
  });
});
