import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { afterAll, describe, expect, it } from 'vitest';

import { buildFooter } from '../src/phases.js';
import { AdwState } from '../src/state.js';
import {
  FENCED_MARKER,
  aggregate,
  analyzeRun,
  attempts,
  classifyOutcome,
  classifyPhasePath,
  nativeAbsoluteVerdict,
  verdict,
} from '../tools/parity-rate.js';

describe('FENCED_MARKER drift guard', () => {
  // If buildFooter's wording changes, the harness would silently misclassify
  // every fenced run as native — so pin the marker to the real footer.
  const state = new AdwState({ adwId: 'a1b2c3d4' });
  it('appears in the fenced-JSON footer and is absent from the native footer', () => {
    expect(buildFooter('plan', state, true)).toContain(FENCED_MARKER);
    expect(buildFooter('plan', state, false)).not.toContain(FENCED_MARKER);
  });
});

describe('classification', () => {
  it('path: fenced when the marker is present, native when absent, classify special-cased', () => {
    expect(classifyPhasePath(`prefix ${FENCED_MARKER} suffix`, 'plan')).toBe('fenced');
    expect(classifyPhasePath('no contract footer here', 'plan')).toBe('native');
    expect(classifyPhasePath(null, 'plan')).toBe('unknown');
    expect(classifyPhasePath('anything', 'classify')).toBe('classify');
  });

  it('outcome: the (attempted, nudged, done) matrix maps onto the bar buckets', () => {
    expect(classifyOutcome(true, false, true)).toBe('clean');
    expect(classifyOutcome(true, true, true)).toBe('nudged-ok');
    expect(classifyOutcome(true, true, false)).toBe('hard-fail'); // nudged, then failed = the bar's numerator
    expect(classifyOutcome(true, false, false)).toBe('uncounted'); // fast-fail (timeout/budget) or in-progress
    expect(classifyOutcome(false, false, false)).toBe('skipped'); // never attempted
  });
});

describe('analyzeRun over a synthetic workspace', () => {
  const dir = mkdtempSync(join(tmpdir(), 'adw-parity-'));
  afterAll(() => rmSync(dir, { recursive: true, force: true }));

  const phase = (name: string, files: Record<string, string>) => {
    mkdirSync(join(dir, name), { recursive: true });
    for (const [file, body] of Object.entries(files)) writeFileSync(join(dir, name, file), body);
  };

  it('classifies each phase from prompt/transcript files + completed_phases', () => {
    writeFileSync(
      join(dir, 'state.json'),
      JSON.stringify({ adw_id: 'a1b2c3d4', issue_number: '42', runner: 'claude', completed_phases: ['plan', 'implement'] }),
    );
    phase('plan', { 'prompt.txt': 'do the plan', 'transcript.log': '{}' }); // native clean
    phase('implement', { 'prompt.txt': 'impl', 'transcript.log': 'x', 'transcript-2.log': 'y' }); // native nudged-ok
    phase('tests', { 'prompt.txt': 't', 'transcript.log': 'x', 'transcript-2.log': 'y' }); // native HARD-FAIL (not done)
    phase('e2e', { 'prompt.txt': 'e', 'transcript.log': 'x' }); // native uncounted (not done, no nudge)

    const run = analyzeRun(dir);
    expect(run.runner).toBe('claude');
    expect(run.issue).toBe('42');
    const byPhase = Object.fromEntries(run.phases.map((p) => [p.phase, p.outcome]));
    expect(byPhase).toEqual({ plan: 'clean', implement: 'nudged-ok', tests: 'hard-fail', e2e: 'uncounted' });
    expect(run.phases.every((p) => p.path === 'native')).toBe(true);

    const agg = aggregate([run]);
    expect(agg.native).toEqual({ clean: 1, nudgedOk: 1, hardFail: 1, uncounted: 1 });
    expect(attempts(agg.native)).toBe(3); // uncounted excluded from the denominator

    // Tiny sample ⇒ the comparative bar (no fenced data) must refuse to judge.
    expect(verdict(agg, 20).ok).toBeNull();

    // Absolute native bar: 1 hard-fail / 3 attempts = 33.3% — evaluable from
    // native-only data once it clears --min, and still INSUFFICIENT below it.
    expect(nativeAbsoluteVerdict(agg, 20, 50).ok).toBeNull(); // 3 < 20 attempts
    expect(nativeAbsoluteVerdict(agg, 3, 50).ok).toBe(true); // 33.3% ≤ 50%
    expect(nativeAbsoluteVerdict(agg, 3, 10).ok).toBe(false); // 33.3% > 10%
  });
});
