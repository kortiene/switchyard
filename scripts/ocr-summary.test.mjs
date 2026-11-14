import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const workflow = readFileSync(new URL('../.github/workflows/ai-review-advisory.yml', import.meta.url), 'utf8');
const script = workflow.split('          script: |\n')[1]
  .split('\n').map((line) => line.replace(/^            /, '')).join('\n');
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
const execute = new AsyncFunction('require', 'process', 'github', 'context', 'core', script);

async function render(report, outcome = 'success', existingComments = []) {
  let created;
  let updated;
  await execute(() => ({ existsSync: () => true, readFileSync: () => JSON.stringify(report) }), {
    env: { OCR_RESULT_PATH: '/report.json', OCR_REVIEW_OUTCOME: outcome, OCR_MAX_TOKENS_BUDGET: '200000' },
  }, {
    paginate: async () => existingComments,
    rest: { issues: {
      listComments() {},
      createComment: async ({ body }) => { created = body; },
      updateComment: async ({ body, comment_id }) => { updated = { body, comment_id }; },
    } },
  }, { repo: { owner: 'test', repo: 'test' }, issue: { number: 1 } }, {
    notice(message) { assert.fail(message); },
  });
  return { body: created ?? updated?.body, updated };
}

// Reduced from the v1.13.9 report for Actions run 43612401997.
const providerFailure = {
  status: 'failed', summary: { total_tokens: 0 }, comments: null,
  manifest: { terminal_state: 'failed', coverage: { failed: [
    { path: '.github/workflows/ai-review-advisory.yml', classification: 'provider', reason: 'provider or subtask request failed' },
  ] } },
  retry_report: { requests: [{ outcome: 'failed', attempts: [
    { outcome: 'error', error_class: 'provider', failure_phase: 'http', status_code: 402 },
  ] }] },
};

test('actual provider failure displays HTTP status without claiming model usage or budget exhaustion', async () => {
  const { body } = await render(providerFailure, 'failure');
  assert.match(body, /provider during http; HTTP 402/);
  assert.match(body, /Payment Required/);
  assert.match(body, /Failed item: .*provider or subtask request failed/);
  assert.match(body, /No model token usage was reported/);
  assert.doesNotMatch(body, /Token budget exhausted|No findings\.|The diff was sent/);
});

test('v1.13.9 complete status with omitted budget flag permits a clean summary', async () => {
  const { body } = await render({ status: 'complete', summary: { total_tokens: 100 }, comments: [] });
  assert.match(body, /No findings\./);
  assert.match(body, /provider reported token usage/);
  assert.doesNotMatch(body, /Token budget exhausted/);
});

test('budget_exceeded true overrides complete status, including zero-usage rejection', async () => {
  for (const total_tokens of [0, 154305]) {
    const { body } = await render({ status: 'complete', summary: { budget_exceeded: true, total_tokens }, comments: [] });
    assert.match(body, /Token budget exhausted/);
    assert.doesNotMatch(body, /No findings\./);
  }
});

test('partial, null and malformed entries do not establish clean coverage or crash rendering', async () => {
  for (const report of [null, [], { status: 'partial' }, { comments: [null], retry_report: { requests: [null, { outcome: 'failed', attempts: [null] }] }, manifest: { coverage: { failed: [null] } } }]) {
    const { body } = await render(report);
    assert.match(body, /does not establish a clean review/);
  }
  const { body } = await render({ status: 'complete' }, 'failure');
  assert.doesNotMatch(body, /No findings\./);
});

test('provider failure fields are neutralized', async () => {
  const report = structuredClone(providerFailure);
  report.retry_report.requests[0].attempts[0].error_class = '@octocat https://example.com';
  const { body } = await render(report, 'failure');
  assert.doesNotMatch(body, /@octocat|https:\/\//);
});

test('only a bot-owned marker comment is updated', async () => {
  const body = '<!-- ocr-advisory-review -->';
  const comments = [
    { id: 1, body, user: { login: 'someone' } },
    { id: 2, body, user: { login: 'github-actions[bot]' } },
  ];
  assert.equal((await render({}, 'success', comments)).updated.comment_id, 2);
  assert.equal((await render({}, 'success', comments.slice(0, 1))).updated, undefined);
});
