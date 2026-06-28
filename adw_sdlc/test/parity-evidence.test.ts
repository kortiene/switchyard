/**
 * Reproducibility guard for the committed parity evidence.
 *
 * The structured-output hard-failure rate quoted across the readiness docs
 * (PARITY.md, MVP-READINESS.md, docs/OBSERVED-LIVE-LEDGER.md) was, until this
 * corpus was vendored, only reproducible from the git-ignored `agents/`
 * workspaces on one machine. `test/fixtures/parity-runs/` now commits the
 * classification-determining artifacts (state.json + per-phase prompt.txt, with
 * transcript presence-markers); this test re-derives the documented numbers from
 * them under `npm run verify`, so a clean clone proves the rate instead of
 * trusting the prose. If the corpus or the classifier drifts, the exact figures
 * the docs cite fail here.
 */
import { existsSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

import { beforeAll, describe, expect, it } from 'vitest';

import { REPO_ROOT } from '../src/common.js';
import {
  type RunAnalysis,
  aggregate,
  analyzeRun,
  attempts,
  nativeAbsoluteVerdict,
  verdict,
} from '../tools/parity-rate.js';

const CORPUS = join(REPO_ROOT, 'adw_sdlc', 'test', 'fixtures', 'parity-runs');

/** The committed run workspaces = child dirs holding a state.json (mirrors parity-rate's findRunDirs). */
function runDirs(): string[] {
  return readdirSync(CORPUS)
    .map((name) => join(CORPUS, name))
    .filter((p) => statSync(p).isDirectory() && existsSync(join(p, 'state.json')));
}

describe('committed parity evidence reproduces the documented structured-output rate', () => {
  let runs: RunAnalysis[];
  let agg: ReturnType<typeof aggregate>;

  beforeAll(() => {
    runs = runDirs().map(analyzeRun);
    agg = aggregate(runs);
  });

  it('vendors exactly the 8 MVP live-run-batch claude runs (issues #1–#8)', () => {
    expect(runs).toHaveLength(8);
    expect(runs.every((r) => !r.error)).toBe(true);
    expect(runs.every((r) => r.runner === 'claude')).toBe(true);
    expect(new Set(runs.map((r) => r.issue))).toEqual(
      new Set(['1', '2', '3', '4', '5', '6', '7', '8']),
    );
  });

  it('native path: 36 attempts, 0 hard-fails (0.0%), 88.9% nudge rate', () => {
    const n = agg.native;
    expect(attempts(n)).toBe(36);
    expect(n.clean).toBe(4);
    expect(n.nudgedOk).toBe(32);
    expect(n.hardFail).toBe(0);
    expect(n.uncounted).toBe(0);
    // nudge rate = (nudged-ok + hard-fail) / counted attempts = 32/36 = 88.9%
    expect((n.nudgedOk + n.hardFail) / attempts(n)).toBeCloseTo(0.889, 3);
  });

  it('fenced path: 5 attempts, all clean (the forced-fenced issue #1 baseline)', () => {
    const f = agg.fenced;
    expect(attempts(f)).toBe(5);
    expect(f.clean).toBe(5);
    expect(f.nudgedOk).toBe(0);
    expect(f.hardFail).toBe(0);
  });

  it('classify path (excluded from the bar): 8 attempts, 0 hard-fails', () => {
    const c = agg.classify;
    expect(attempts(c)).toBe(8);
    expect(c.hardFail).toBe(0);
  });

  it('every classified phase came from a real prompt.txt (49 phases, none unknown)', () => {
    const phases = runs.flatMap((r) => r.phases);
    expect(phases).toHaveLength(49);
    expect(phases.every((p) => p.path !== 'unknown')).toBe(true);
  });

  it('comparative bar stays INSUFFICIENT DATA (fenced 5 < 20 needed)', () => {
    const v = verdict(agg, 20);
    expect(v.ok).toBeNull();
    expect(v.line).toContain('INSUFFICIENT DATA');
  });

  it('absolute native bar (≤ 20%) is clearable today from this committed evidence', () => {
    const v = nativeAbsoluteVerdict(agg, 20, 20);
    expect(v.ok).toBe(true);
    expect(v.line).toContain('MEETS ABSOLUTE BAR');
  });
});
