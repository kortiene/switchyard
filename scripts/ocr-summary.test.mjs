import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const workflow = readFileSync(new URL('../.github/workflows/ai-review-advisory.yml', import.meta.url), 'utf8');

function extractGithubScript(source) {
  const lines = source.split('\n');
  const scripts = [];
  for (let i = 0; i < lines.length; i++) {
    if (!/^\s*script:\s*\|\s*$/.test(lines[i])) continue;
    const keyIndent = lines[i].match(/^\s*/)[0].length;
    const body = [];
    for (let j = i + 1; j < lines.length; j++) {
      const line = lines[j];
      if (line.trim() === '') {
        body.push('');
        continue;
      }
      if (line.match(/^\s*/)[0].length <= keyIndent) break;
      body.push(line);
    }
    const nonempty = body.filter((line) => line.trim() !== '');
    if (nonempty.length === 0) continue;
    const common = Math.min(...nonempty.map((line) => line.match(/^\s*/)[0].length));
    const script = body.map((line) => (line.trim() === '' ? '' : line.slice(common))).join('\n');
    if (script.includes('function summaryBody')) scripts.push(script);
  }
  if (scripts.length !== 1) {
    throw new Error(`github-script block not found in .github/workflows/ai-review-advisory.yml (found ${scripts.length} script blocks containing summaryBody)`);
  }
  return scripts[0];
}

const script = extractGithubScript(workflow);
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;

test('script extraction strips the block indent and fails clearly when the block is missing', () => {
  const yaml = [
    'jobs:',
    '  steps:',
    '    - uses: actions/github-script@v9',
    '      with:',
    '        script: |',
    '          function summaryBody() { return 1; }',
    '          const indented = true;',
    '    - name: next',
  ].join('\n');
  const extracted = extractGithubScript(yaml);
  assert.match(extracted, /^function summaryBody\(\)/);
  assert.match(extracted, /^const indented = true;$/m);
  assert.throws(() => extractGithubScript('name: none\n'), /github-script block not found/);
});
const execute = new AsyncFunction('require', 'process', 'github', 'context', 'core', script);

async function render(report, outcome = 'success', existingComments = [], extra = {}) {
  const stderr = extra.stderr || '';
  const reportExists = extra.reportExists !== false;
  let created;
  let updated;
  await execute(() => ({
    existsSync: (p) => (p === '/stderr.txt' ? Boolean(stderr) : reportExists),
    readFileSync: (p) => (p === '/stderr.txt' ? stderr : JSON.stringify(report)),
  }), {
    env: {
      OCR_RESULT_PATH: '/report.json',
      OCR_REVIEW_OUTCOME: outcome,
      OCR_MAX_TOKENS_BUDGET: '200000',
      ...(stderr ? { OCR_STDERR_PATH: '/stderr.txt' } : {}),
    },
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

test('failed review summary includes the neutralized stderr error line', async () => {
  const stderr = [
    '{',
    '  "status": "failed"',
    '}',
    'Error: all 1 file review(s) failed — check your LLM configuration and API key @octocat https://example.com',
  ].join('\n');
  const { body } = await render(providerFailure, 'failure', [], { stderr });
  assert.match(body, /Runner error:.*file review\(s\) failed/);
  assert.match(body, /LLM configuration and API key/);
  assert.doesNotMatch(body, /@octocat|https:\/\//);
  assert.doesNotMatch(body, /"status": "failed"/);
});

test('a missing report still surfaces the stderr error line', async () => {
  const { body } = await render(null, 'failure', [], {
    stderr: 'Error: all 1 file review(s) failed — check your LLM configuration and API key',
    reportExists: false,
  });
  assert.match(body, /did not finish/);
  assert.match(body, /Runner error:.*LLM configuration and API key/);
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
