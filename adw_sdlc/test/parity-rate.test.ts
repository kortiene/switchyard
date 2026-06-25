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
  classifyRun,
  nativeAbsoluteVerdict,
  pct,
  renderReport,
  verdict,
} from '../tools/parity-rate.js';
// Import pure-core functions directly to validate they are exported from the extracted module.
import {
  classifyOutcome as coreClassifyOutcome,
  classifyPhasePath as coreClassifyPhasePath,
  classifyRun as coreClassifyRun,
  aggregate as coreAggregate,
  verdict as coreVerdict,
  nativeAbsoluteVerdict as coreNativeAbsoluteVerdict,
  FENCED_MARKER as coreFENCED_MARKER,
} from '../tools/parity-rate-core.js';

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

  it('returns an error analysis when state.json is missing', () => {
    const emptyDir = mkdtempSync(join(tmpdir(), 'adw-parity-err-'));
    try {
      const run = analyzeRun(emptyDir);
      expect(run.error).toMatch(/unreadable state\.json/);
      expect(run.phases).toEqual([]);
    } finally {
      rmSync(emptyDir, { recursive: true, force: true });
    }
  });

  it('classifies fenced path when prompt contains FENCED_MARKER', () => {
    const fencedDir = mkdtempSync(join(tmpdir(), 'adw-parity-fenced-'));
    try {
      writeFileSync(
        join(fencedDir, 'state.json'),
        JSON.stringify({ adw_id: 'fenced1', runner: 'pi', completed_phases: ['plan'] }),
      );
      mkdirSync(join(fencedDir, 'plan'), { recursive: true });
      writeFileSync(join(fencedDir, 'plan', 'prompt.txt'), `preamble ${FENCED_MARKER} footer`);
      writeFileSync(join(fencedDir, 'plan', 'transcript.log'), 'ok');

      const run = analyzeRun(fencedDir);
      expect(run.phases).toHaveLength(1);
      const [firstPhase] = run.phases;
      expect(firstPhase!.path).toBe('fenced');
      expect(firstPhase!.outcome).toBe('clean');
    } finally {
      rmSync(fencedDir, { recursive: true, force: true });
    }
  });

  it('ignores dirs without transcript.log and non-directory entries', () => {
    const mixedDir = mkdtempSync(join(tmpdir(), 'adw-parity-mixed-'));
    try {
      writeFileSync(
        join(mixedDir, 'state.json'),
        JSON.stringify({ adw_id: 'mix1', runner: 'claude', completed_phases: ['plan'] }),
      );
      // A dir that lacks transcript.log should be skipped
      mkdirSync(join(mixedDir, 'no-transcript'), { recursive: true });
      writeFileSync(join(mixedDir, 'no-transcript', 'prompt.txt'), 'x');
      // A regular file in the run root should be skipped
      writeFileSync(join(mixedDir, 'README.md'), 'notes');
      // A valid phase dir
      mkdirSync(join(mixedDir, 'plan'), { recursive: true });
      writeFileSync(join(mixedDir, 'plan', 'prompt.txt'), 'plan prompt');
      writeFileSync(join(mixedDir, 'plan', 'transcript.log'), 'ok');

      const run = analyzeRun(mixedDir);
      expect(run.phases.map((p) => p.phase)).toEqual(['plan']);
    } finally {
      rmSync(mixedDir, { recursive: true, force: true });
    }
  });
});

describe('classifyRun (pure core function)', () => {
  it('error input short-circuits to empty phases and preserves the error string', () => {
    const result = coreClassifyRun({
      adwId: 'err-run',
      issue: '99',
      runner: 'claude',
      completedPhases: [],
      phases: [],
      error: 'boom',
    });
    expect(result.error).toBe('boom');
    expect(result.phases).toEqual([]);
    expect(result.adwId).toBe('err-run');
  });

  it('sorts phases alphabetically by name', () => {
    const result = coreClassifyRun({
      adwId: 'sort-run',
      issue: null,
      runner: 'claude',
      completedPhases: ['tests', 'implement', 'plan'],
      phases: [
        { phase: 'tests', promptText: 'x', hasRetry: false },
        { phase: 'plan', promptText: 'x', hasRetry: false },
        { phase: 'implement', promptText: 'x', hasRetry: false },
      ],
    });
    expect(result.phases.map((p) => p.phase)).toEqual(['implement', 'plan', 'tests']);
  });

  it('assigns classify path for the classify phase regardless of prompt text', () => {
    const result = coreClassifyRun({
      adwId: 'classify-run',
      issue: null,
      runner: 'claude',
      completedPhases: ['classify'],
      phases: [{ phase: 'classify', promptText: 'anything', hasRetry: false }],
    });
    const [classifyPhase] = result.phases;
    expect(classifyPhase!.path).toBe('classify');
    expect(classifyPhase!.outcome).toBe('clean');
  });

  it('assigns unknown path when promptText is null for a non-classify phase', () => {
    const result = coreClassifyRun({
      adwId: 'unknown-run',
      issue: null,
      runner: 'claude',
      completedPhases: ['plan'],
      phases: [{ phase: 'plan', promptText: null, hasRetry: false }],
    });
    const [unknownPhase] = result.phases;
    expect(unknownPhase!.path).toBe('unknown');
  });

  it('all five outcomes are reachable through classifyRun inputs', () => {
    const result = coreClassifyRun({
      adwId: 'outcomes-run',
      issue: null,
      runner: 'claude',
      completedPhases: ['clean-phase', 'nudged-phase'],
      phases: [
        { phase: 'clean-phase', promptText: 'x', hasRetry: false },
        { phase: 'nudged-phase', promptText: 'x', hasRetry: true },
        { phase: 'hard-fail-phase', promptText: 'x', hasRetry: true },
        { phase: 'uncounted-phase', promptText: 'x', hasRetry: false },
      ],
    });
    const byPhase = Object.fromEntries(result.phases.map((p) => [p.phase, p.outcome]));
    expect(byPhase['clean-phase']).toBe('clean');
    expect(byPhase['nudged-phase']).toBe('nudged-ok');
    expect(byPhase['hard-fail-phase']).toBe('hard-fail');
    expect(byPhase['uncounted-phase']).toBe('uncounted');
  });
});

describe('verdict — meets and fails cases', () => {
  const makeAgg = (nativeFail: number, nativeTotal: number, fencedFail: number, fencedTotal: number) => {
    const nativeClean = nativeTotal - nativeFail;
    const fencedClean = fencedTotal - fencedFail;
    return coreAggregate([
      {
        adwId: 'a',
        issue: null,
        runner: 'claude',
        phases: [
          ...Array.from({ length: nativeClean }, (_, i) => ({ phase: `np-clean-${i}`, path: 'native' as const, outcome: 'clean' as const })),
          ...Array.from({ length: nativeFail }, (_, i) => ({ phase: `np-fail-${i}`, path: 'native' as const, outcome: 'hard-fail' as const })),
          ...Array.from({ length: fencedClean }, (_, i) => ({ phase: `fp-clean-${i}`, path: 'fenced' as const, outcome: 'clean' as const })),
          ...Array.from({ length: fencedFail }, (_, i) => ({ phase: `fp-fail-${i}`, path: 'fenced' as const, outcome: 'hard-fail' as const })),
        ],
      },
    ]);
  };

  it('returns ok=true when native hard-fail rate ≤ fenced hard-fail rate', () => {
    // native: 1/20 = 5%, fenced: 3/20 = 15% — native ≤ fenced ⇒ meets bar
    const agg = makeAgg(1, 20, 3, 20);
    const v = coreVerdict(agg, 20);
    expect(v.ok).toBe(true);
    expect(v.line).toContain('MEETS BAR');
  });

  it('returns ok=false when native hard-fail rate > fenced hard-fail rate', () => {
    // native: 5/20 = 25%, fenced: 1/20 = 5% — native > fenced ⇒ fails bar
    const agg = makeAgg(5, 20, 1, 20);
    const v = coreVerdict(agg, 20);
    expect(v.ok).toBe(false);
    expect(v.line).toContain('FAILS BAR');
  });

  it('returns ok=null with special message when fenced has zero attempts', () => {
    const agg = makeAgg(0, 20, 0, 0);
    const v = coreVerdict(agg, 20);
    expect(v.ok).toBeNull();
    // Should mention that no fenced-path runs exist
    expect(v.line).toContain('no fenced-path runs');
  });

  it('ok=null when only native meets min but fenced does not', () => {
    const agg = makeAgg(0, 20, 0, 5);
    const v = coreVerdict(agg, 20);
    expect(v.ok).toBeNull();
    expect(v.line).toContain('INSUFFICIENT DATA');
  });
});

describe('nativeAbsoluteVerdict — line text', () => {
  it('includes the target percentage in the verdict line', () => {
    const agg = coreAggregate([
      {
        adwId: 'a',
        issue: null,
        runner: 'claude',
        phases: Array.from({ length: 20 }, (_, i) => ({ phase: `p${i}`, path: 'native' as const, outcome: 'clean' as const })),
      },
    ]);
    const v = coreNativeAbsoluteVerdict(agg, 20, 5);
    expect(v.ok).toBe(true);
    expect(v.line).toContain('5%');
    expect(v.line).toContain('20 attempts');
  });
});

describe('aggregate — multiple runs and mixed paths', () => {
  it('accumulates buckets across runs and paths independently', () => {
    const agg = coreAggregate([
      {
        adwId: 'run1',
        issue: null,
        runner: 'claude',
        phases: [
          { phase: 'plan', path: 'native', outcome: 'clean' },
          { phase: 'implement', path: 'native', outcome: 'hard-fail' },
          { phase: 'tests', path: 'fenced', outcome: 'nudged-ok' },
        ],
      },
      {
        adwId: 'run2',
        issue: null,
        runner: 'pi',
        phases: [
          { phase: 'plan', path: 'fenced', outcome: 'clean' },
          { phase: 'classify', path: 'classify', outcome: 'uncounted' },
          { phase: 'review', path: 'native', outcome: 'nudged-ok' },
        ],
      },
    ]);

    expect(agg.native).toEqual({ clean: 1, nudgedOk: 1, hardFail: 1, uncounted: 0 });
    expect(agg.fenced).toEqual({ clean: 1, nudgedOk: 1, hardFail: 0, uncounted: 0 });
    expect(agg.classify).toEqual({ clean: 0, nudgedOk: 0, hardFail: 0, uncounted: 1 });
    expect(agg.unknown).toEqual({ clean: 0, nudgedOk: 0, hardFail: 0, uncounted: 0 });
  });

  it('skips "skipped" outcomes — they do not appear in any bucket', () => {
    const agg = coreAggregate([
      {
        adwId: 'run1',
        issue: null,
        runner: 'claude',
        phases: [{ phase: 'plan', path: 'native', outcome: 'skipped' }],
      },
    ]);
    const total = (['native', 'fenced', 'classify', 'unknown'] as const)
      .map((p) => agg[p].clean + agg[p].nudgedOk + agg[p].hardFail + agg[p].uncounted)
      .reduce((a, b) => a + b, 0);
    expect(total).toBe(0);
  });
});

describe('pct utility', () => {
  it('returns n/a for zero denominator', () => {
    expect(pct(0, 0)).toBe('n/a');
    expect(pct(5, 0)).toBe('n/a');
  });

  it('formats a percentage to one decimal place', () => {
    expect(pct(1, 3)).toBe('33.3%');
    expect(pct(0, 20)).toBe('0.0%');
    expect(pct(20, 20)).toBe('100.0%');
  });
});

describe('core exports are identical to shell re-exports', () => {
  it('FENCED_MARKER is the same value in both modules', () => {
    expect(coreFENCED_MARKER).toBe(FENCED_MARKER);
  });

  it('classifyPhasePath behaves identically when imported from either module', () => {
    expect(coreClassifyPhasePath('x', 'plan')).toBe(classifyPhasePath('x', 'plan'));
    expect(coreClassifyPhasePath(null, 'classify')).toBe(classifyPhasePath(null, 'classify'));
  });

  it('classifyOutcome behaves identically when imported from either module', () => {
    expect(coreClassifyOutcome(true, true, false)).toBe(classifyOutcome(true, true, false));
  });
});

describe('classifyPhasePath — additional edge cases', () => {
  it('empty string promptText is treated as native (contains no FENCED_MARKER)', () => {
    expect(classifyPhasePath('', 'plan')).toBe('native');
  });

  it('classify phase is always classify-path even when FENCED_MARKER is present in the prompt', () => {
    expect(classifyPhasePath(FENCED_MARKER, 'classify')).toBe('classify');
  });
});

describe('attempts utility — direct tests', () => {
  it('sums clean + nudgedOk + hardFail and excludes uncounted', () => {
    expect(attempts({ clean: 5, nudgedOk: 3, hardFail: 2, uncounted: 10 })).toBe(10);
  });

  it('returns 0 for an all-zero bucket', () => {
    expect(attempts({ clean: 0, nudgedOk: 0, hardFail: 0, uncounted: 0 })).toBe(0);
  });

  it('uncounted-only bucket has zero counted attempts', () => {
    expect(attempts({ clean: 0, nudgedOk: 0, hardFail: 0, uncounted: 7 })).toBe(0);
  });
});

describe('aggregate — error runs contribute nothing', () => {
  it('a run with an error and no phases adds nothing to any bucket', () => {
    const errRun = coreClassifyRun({
      adwId: 'err',
      issue: null,
      runner: null,
      completedPhases: [],
      phases: [],
      error: 'unreadable',
    });
    const agg = coreAggregate([errRun]);
    const total = (['native', 'fenced', 'classify', 'unknown'] as const)
      .map((p) => agg[p].clean + agg[p].nudgedOk + agg[p].hardFail + agg[p].uncounted)
      .reduce((a, b) => a + b, 0);
    expect(total).toBe(0);
  });
});

describe('verdict — fenced meets min but native does not', () => {
  it('ok=null when native has fewer attempts than minAttempts even if fenced has enough', () => {
    const agg = coreAggregate([
      {
        adwId: 'a',
        issue: null,
        runner: 'pi',
        phases: [
          ...Array.from({ length: 5 }, (_, i) => ({ phase: `n${i}`, path: 'native' as const, outcome: 'clean' as const })),
          ...Array.from({ length: 20 }, (_, i) => ({ phase: `f${i}`, path: 'fenced' as const, outcome: 'clean' as const })),
        ],
      },
    ]);
    const v = coreVerdict(agg, 20);
    expect(v.ok).toBeNull();
    expect(v.line).toContain('INSUFFICIENT DATA');
    expect(v.line).toContain('5'); // native count surfaced in message
  });
});

describe('nativeAbsoluteVerdict — fail case', () => {
  it('returns ok=false and FAILS ABSOLUTE BAR when hard-fail rate exceeds the cap', () => {
    // 5 hard-fails out of 20 attempts = 25%, which exceeds the 10% cap
    const agg = coreAggregate([
      {
        adwId: 'a',
        issue: null,
        runner: 'claude',
        phases: [
          ...Array.from({ length: 15 }, (_, i) => ({ phase: `c${i}`, path: 'native' as const, outcome: 'clean' as const })),
          ...Array.from({ length: 5 }, (_, i) => ({ phase: `f${i}`, path: 'native' as const, outcome: 'hard-fail' as const })),
        ],
      },
    ]);
    const v = coreNativeAbsoluteVerdict(agg, 20, 10);
    expect(v.ok).toBe(false);
    expect(v.line).toContain('FAILS ABSOLUTE BAR');
    expect(v.line).toContain('10%');
    expect(v.line).toContain('20 attempts');
  });

  it('returns ok=null when native attempts are below minAttempts', () => {
    const agg = coreAggregate([
      {
        adwId: 'a',
        issue: null,
        runner: 'claude',
        phases: [{ phase: 'plan', path: 'native', outcome: 'clean' }],
      },
    ]);
    const v = coreNativeAbsoluteVerdict(agg, 20, 50);
    expect(v.ok).toBeNull();
    expect(v.line).toContain('INSUFFICIENT DATA');
  });
});

describe('classifyRun — null runner and null issue passthrough', () => {
  it('propagates null runner and null issue into the RunAnalysis unchanged', () => {
    const result = coreClassifyRun({
      adwId: 'no-meta',
      issue: null,
      runner: null,
      completedPhases: [],
      phases: [],
    });
    expect(result.runner).toBeNull();
    expect(result.issue).toBeNull();
    expect(result.adwId).toBe('no-meta');
    expect(result.phases).toEqual([]);
    expect(result.error).toBeUndefined();
  });
});

describe('renderReport — structure', () => {
  it('contains the expected header and table markers', () => {
    const run = classifyRun({
      adwId: 'r1',
      issue: '1',
      runner: 'claude',
      completedPhases: [],
      phases: [],
    });
    const report = renderReport([run], 20);
    expect(report).toContain('# Structured-output hard-failure rate');
    expect(report).toContain('INSUFFICIENT DATA');
    expect(report).toContain('## Per-run breakdown');
  });

  it('surfaces error runs in the breakdown', () => {
    const errRun = classifyRun({
      adwId: 'bad-run',
      issue: null,
      runner: null,
      completedPhases: [],
      phases: [],
      error: 'state.json missing',
    });
    const report = renderReport([errRun], 20);
    expect(report).toContain('bad-run');
    expect(report).toContain('state.json missing');
  });

  it('includes the absolute bar section when maxNativeRatePct is provided', () => {
    const run = classifyRun({
      adwId: 'r2',
      issue: null,
      runner: 'claude',
      completedPhases: [],
      phases: [],
    });
    const report = renderReport([run], 20, 10);
    expect(report).toContain('absolute native bar');
    expect(report).toContain('10%');
  });

  it('renders a native table row when native phases are present', () => {
    const run = classifyRun({
      adwId: 'r-native-row',
      issue: '7',
      runner: 'claude',
      completedPhases: ['plan'],
      phases: [{ phase: 'plan', promptText: 'no contract footer', hasRetry: false }],
    });
    const report = renderReport([run], 20);
    expect(report).toContain('| native |');
    expect(report).toContain('r-native-row');
  });

  it('renders a fenced table row when a fenced phase is present', () => {
    const run = classifyRun({
      adwId: 'r-fenced-row',
      issue: '8',
      runner: 'pi',
      completedPhases: ['plan'],
      phases: [{ phase: 'plan', promptText: FENCED_MARKER, hasRetry: false }],
    });
    const report = renderReport([run], 20);
    expect(report).toContain('| fenced |');
    expect(report).toContain('runner=pi');
    expect(report).toContain('issue #8');
  });
});
