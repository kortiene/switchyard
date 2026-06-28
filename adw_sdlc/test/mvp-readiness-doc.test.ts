import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import { beforeAll, describe, expect, it } from 'vitest';

import { REPO_ROOT } from '../src/common.js';

const MVP_READINESS = join(REPO_ROOT, 'adw_sdlc', 'MVP-READINESS.md');
const LIVE_RUN_BATCH = join(REPO_ROOT, 'adw_sdlc', 'docs', 'LIVE-RUN-BATCH.md');
const CLI_TS = join(REPO_ROOT, 'adw_sdlc', 'src', 'cli.ts');

describe('MVP-READINESS.md — issue #1 acceptance criteria', () => {
  let content: string;
  beforeAll(() => {
    content = readFileSync(MVP_READINESS, 'utf8');
  });

  it('§0 exists and declares MVP = (A) without "unset"', () => {
    expect(content).toMatch(/## 0\./);
    // The decision must be affirmatively recorded
    expect(content).toMatch(/\(A\)/);
    // No "unset" placeholder left in §0
    const section0 = content.slice(
      content.indexOf('## 0.'),
      content.indexOf('\n## ', content.indexOf('## 0.') + 1),
    );
    expect(section0).not.toMatch(/unset/i);
  });

  it('§0 records the decision with an explicit "Decided" or "Decision" marker', () => {
    const section0 = content.slice(
      content.indexOf('## 0.'),
      content.indexOf('\n## ', content.indexOf('## 0.') + 1),
    );
    expect(section0).toMatch(/Decision|Decided/i);
    // The decision block calls out (A) as adopted/chosen
    expect(section0).toMatch(/\(A\).*MVP|MVP.*\(A\)/s);
  });

  it('(B) and (C) are present but marked post-MVP', () => {
    expect(content).toMatch(/\(B\)/);
    expect(content).toMatch(/\(C\)/);
    // Both must be labelled post-MVP somewhere
    expect(content).toMatch(/post-MVP/i);
    // (B) and (C) gates sections should explicitly state post-MVP
    expect(content).toMatch(/Gates that \(B\).*post-MVP|post-MVP.*\(B\)/si);
    expect(content).toMatch(/Gates that \(C\).*post-MVP|post-MVP.*\(C\)/si);
  });

  it('contains a live-run playbook section', () => {
    // A heading covering live claude runs
    expect(content).toMatch(/##.*live.*claude|##.*claude.*issue/i);
  });

  it('live-run playbook links to docs/LIVE-RUN-BATCH.md', () => {
    expect(content).toContain('LIVE-RUN-BATCH.md');
  });

  it('playbook shows the ADW_TEST_CMD single-command form', () => {
    expect(content).toContain('ADW_TEST_CMD');
    expect(content).toContain('npm run verify');
  });
});

// ---------------------------------------------------------------------------
// Cross-doc consistency: cli.ts comment vs MVP-READINESS.md — issue #25
//
// Before issue #25 the DEFAULT_ENGINE JSDoc said the cutover "is done", while
// MVP-READINESS.md §3 marks (C) cutover as post-MVP / ❌ not started. The fix
// softened the comment. These tests pin the corrected wording so a revert is
// caught before it ships.
// ---------------------------------------------------------------------------

describe('cli.ts / MVP-READINESS.md cross-doc consistency — issue #25', () => {
  let cliSource: string;
  let mvpContent: string;

  beforeAll(() => {
    cliSource = readFileSync(CLI_TS, 'utf8');
    mvpContent = readFileSync(MVP_READINESS, 'utf8');
  });

  it('DEFAULT_ENGINE JSDoc in cli.ts explicitly calls the cutover milestone post-MVP', () => {
    // The JSDoc block for DEFAULT_ENGINE must say "post-MVP" in the context of
    // the cutover milestone. If a future edit removes this framing, this fires.
    const defaultEngineBlock = cliSource.slice(
      0,
      cliSource.indexOf('export const DEFAULT_ENGINE') + 500,
    );
    expect(defaultEngineBlock).toMatch(/post-MVP/i);
    expect(defaultEngineBlock).toMatch(/cutover/i);
  });

  it('cli.ts DEFAULT_ENGINE comment does not claim the cutover is already done', () => {
    // The pre-issue #25 wording said the py→ts cutover "is done". The fix
    // restricts the claim to "the ts default is set" — the full cutover
    // milestone remains post-MVP. Pin that the old "is done" phrasing is gone.
    const defaultEngineBlock = cliSource.slice(
      0,
      cliSource.indexOf('export const DEFAULT_ENGINE') + 500,
    );
    expect(defaultEngineBlock).not.toMatch(/cutover\s+is\s+done/i);
    expect(defaultEngineBlock).not.toMatch(/cutover.*?(?:is|was|has been)\s+(?:done|complete|finished)/i);
  });

  it('MVP-READINESS.md §3 (C) cutover section is post-MVP with no ✅ entries', () => {
    // Verify MVP-READINESS.md §3 remains consistent with the softened cli.ts
    // comment: the (C) cutover gate is post-MVP and ❌ not started.
    const section3Start = mvpContent.indexOf('## 3.');
    const section3End = mvpContent.indexOf('\n## ', section3Start + 1);
    const section3 =
      section3Start !== -1
        ? mvpContent.slice(section3Start, section3End === -1 ? undefined : section3End)
        : '';
    expect(section3, 'section ## 3. must exist in MVP-READINESS.md').not.toBe('');
    expect(section3).toMatch(/post-MVP/i);
    // At least one ❌ item remains in the cutover gate section.
    expect((section3.match(/❌/g) ?? []).length).toBeGreaterThanOrEqual(1);
    // No ✅ in the (C) cutover section — no gate item is done yet.
    expect(section3).not.toContain('✅');
  });

  it('no cross-doc contradiction: cli.ts does not claim cutover done while MVP-READINESS.md calls it post-MVP', () => {
    // The bug in issue #25: cli.ts said "is done", MVP-READINESS.md said post-MVP.
    // A revert of the fix would re-introduce this contradiction.
    const cutoverDoneInCli = /cutover.*?(?:is|was|has been)\s+(?:done|complete|finished)/i.test(cliSource);
    const mvpCallsCutoverPostMvp = /\(C\).*post-MVP|post-MVP.*\(C\)/si.test(mvpContent);
    // Both conditions being true simultaneously is the contradiction to prevent.
    expect(
      cutoverDoneInCli && mvpCallsCutoverPostMvp,
      'cli.ts claims cutover is done while MVP-READINESS.md marks it post-MVP — contradiction from issue #25',
    ).toBe(false);
  });
});

describe('docs/LIVE-RUN-BATCH.md — linked target exists and is non-empty', () => {
  it('file is readable and non-empty', () => {
    const batch = readFileSync(LIVE_RUN_BATCH, 'utf8');
    expect(batch.length).toBeGreaterThan(200);
    // Must contain the single-command guidance that the playbook references
    expect(batch).toContain('npm run verify');
  });

  it('explains why ADW_TEST_CMD must be a single command', () => {
    const batch = readFileSync(LIVE_RUN_BATCH, 'utf8');
    // The file must explain the shell-split constraint
    expect(batch).toMatch(/shellSplit|shell.split|shell_split/i);
  });
});

// ---------------------------------------------------------------------------
// MVP-READINESS.md preamble — real-process test acknowledgment — issue #41
//
// Before issue #41 the preamble still said "no test in the suite spawns a real
// process" (line 6 of the pre-fix document), now false because the
// secret-boundary audit, verify-gate e2e, and rest-transport loopback tests
// deliberately cross the mock seam. These guards pin the corrected wording.
// ---------------------------------------------------------------------------

describe('MVP-READINESS.md preamble — real-process test acknowledgment — issue #41', () => {
  let preamble: string;

  beforeAll(() => {
    const content = readFileSync(MVP_READINESS, 'utf8');
    // Preamble = everything up to the first `---` horizontal rule
    const hrIdx = content.indexOf('\n---\n');
    preamble = hrIdx === -1 ? content : content.slice(0, hrIdx);
  });

  it('does not claim all suite tests are purely mocked (stale "no real process" language is absent)', () => {
    // The pre-fix preamble said something like "no test in the suite spawns a
    // real process". That claim is now false. Guard that neither that phrase nor
    // any close variant reappears.
    expect(preamble).not.toMatch(/no test.*spawns? a real process/i);
    expect(preamble).not.toMatch(/no test.*real.*subprocess/i);
  });

  it('acknowledges that some tests spawn real subprocesses', () => {
    // The softened preamble must say that at least some tests cross the mock
    // seam and spawn real subprocesses or drive real network round-trips.
    expect(preamble).toMatch(/spawn real subprocess|real localhost|deliberately cross/i);
  });

  it('names the secret-boundary audit as a real-process test', () => {
    expect(preamble).toContain('secret-boundary-audit.test.ts');
  });

  it('names the verify-gate e2e test as a real-process test', () => {
    expect(preamble).toContain('verify-gate.e2e.test.ts');
  });

  it('names the rest-transport loopback suite as a real-process test', () => {
    expect(preamble).toContain('providers-rest-transport.test.ts');
  });
});
