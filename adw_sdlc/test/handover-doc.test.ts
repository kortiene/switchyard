// ---------------------------------------------------------------------------
// HANDOVER.md doc-currency regression guard — issue #41
//
// HANDOVER.md records a "Test count baseline" line at the end of section 12
// ("How to resume"). Before issue #41 the trailing baseline was behind HEAD
// (578/41 instead of the actual count). These tests pin structural invariants
// so a stale count is caught before it ships:
//
//   • the baseline line exists and uses the canonical format
//   • the recorded test count is ≥ 600 (> the stale 578 that prompted #41)
//   • the recorded file count is ≥ 43 (> the stale 41 that prompted #41)
// ---------------------------------------------------------------------------

import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import { beforeAll, describe, expect, it } from 'vitest';

import { REPO_ROOT } from '../src/common.js';

const HANDOVER = join(REPO_ROOT, 'adw_sdlc', 'HANDOVER.md');

// Match: "Test count baseline after this session: **NNN passing across MM files**"
const BASELINE_RE = /Test count baseline after this session:\s*\*\*(\d+)\s+passing across\s+(\d+)\s+files\*\*/i;

describe('HANDOVER.md section 12 — test count baseline — issue #41', () => {
  let content: string;

  beforeAll(() => {
    content = readFileSync(HANDOVER, 'utf8');
  });

  it('section 12 ("How to resume") exists', () => {
    expect(content).toMatch(/## 12\./);
  });

  it('contains a "Test count baseline" line in the canonical bold format', () => {
    expect(content).toMatch(BASELINE_RE);
  });

  it('baseline test count is ≥ 600 (stale pre-#41 count was 578)', () => {
    const match = content.match(BASELINE_RE);
    expect(match, 'BASELINE_RE must match — preceding test should have caught this').toBeTruthy();
    const count = parseInt(match![1]!, 10);
    expect(count, `baseline test count ${count} is below the post-#41 floor of 600`).toBeGreaterThanOrEqual(600);
  });

  it('baseline file count is ≥ 43 (stale pre-#41 count was 41)', () => {
    const match = content.match(BASELINE_RE);
    expect(match, 'BASELINE_RE must match — preceding test should have caught this').toBeTruthy();
    const fileCount = parseInt(match![2]!, 10);
    expect(fileCount, `baseline file count ${fileCount} is below the post-#41 floor of 43`).toBeGreaterThanOrEqual(43);
  });
});
