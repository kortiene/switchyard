import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import { beforeAll, describe, expect, it } from 'vitest';

import { REPO_ROOT } from '../src/common.js';

const MVP_READINESS = join(REPO_ROOT, 'adw_sdlc', 'MVP-READINESS.md');
const LIVE_RUN_BATCH = join(REPO_ROOT, 'adw_sdlc', 'docs', 'LIVE-RUN-BATCH.md');

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
