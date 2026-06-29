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

// Match the §8 "How to resume" quick-gate annotation: "(current: NNN tests, MM files)".
// (The "current:" prefix scopes this to the live annotation, not the point-in-time
// per-session changelog lines like "stays green (**638 tests, 46 files**)".)
const SECTION8_RE = /current:\s*(\d+)\s+tests?,\s*(\d+)\s+files?/i;

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

  // #41 follow-up: the §8 quick-resume gate block carries its own "(current: N
  // tests, M files)" annotation. It silently drifted (638/46) from the §12
  // baseline (665/47). A floor cannot catch that; pin the two to AGREE so the
  // current-state count cannot diverge across the two locations again.
  it('§8 "(current: N tests, M files)" agrees with the §12 baseline', () => {
    const s8 = content.match(SECTION8_RE);
    const s12 = content.match(BASELINE_RE);
    expect(s8, '§8 "(current: N tests, M files)" annotation must be present').toBeTruthy();
    expect(s12, '§12 baseline must be present').toBeTruthy();
    expect(
      [s8![1], s8![2]],
      `§8 quick-resume count (${s8![1]}/${s8![2]}) must equal the §12 baseline ` +
        `(${s12![1]}/${s12![2]}) — keep the two current-state counts in sync`,
    ).toEqual([s12![1], s12![2]]);
  });
});
