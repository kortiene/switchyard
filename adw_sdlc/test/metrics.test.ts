/**
 * Per-phase cost/duration metrics (MetricsCollector). Covers the rollup math
 * (totals, null-cost poisoning, nudge rate), the deterministic injected clock,
 * the snake_case on-disk document, and the best-effort save — all written to
 * the additive agents/{adw_id}/metrics.json, NEVER state.json (which the
 * cross-language + engine-parity suites pin as a byte/equality contract).
 */

import { mkdtempSync, readFileSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { MetricsCollector, METRICS_FILENAME, type Clock } from '../src/metrics.js';
import { AdwState, setAgentsDir } from '../src/state.js';

let tmp: string;

/** A clock that advances by fixed steps so durations are deterministic. */
function steppedClock(steps: number[]): Clock {
  let i = 0;
  return () => steps[Math.min(i++, steps.length - 1)]!;
}

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), 'adw-metrics-'));
  setAgentsDir(tmp);
});

afterEach(() => {
  setAgentsDir(null);
  rmSync(tmp, { recursive: true, force: true });
});

function newState(): AdwState {
  const state = new AdwState({ adwId: 'a1b2c3d4', runner: 'claude' });
  return state;
}

describe('MetricsCollector.record + summary', () => {
  it('rounds duration from the injected clock and defaults attempts to 1', () => {
    // marks/records interleave: start=100, record reads 350 -> 250ms.
    const m = new MetricsCollector(newState(), steppedClock([100, 350.4]));
    const mark = m.start();
    m.record('plan', mark, { costUsd: 0.5, inputTokens: 10, outputTokens: 2 });
    const [row] = m.phases();
    expect(row).toMatchObject({ phase: 'plan', durationMs: 250, attempts: 1, costUsd: 0.5 });
    expect(row!.inputTokens).toBe(10);
  });

  it('never reports a negative duration if the clock goes backwards', () => {
    const m = new MetricsCollector(newState(), steppedClock([500, 100]));
    const mark = m.start();
    m.record('plan', mark, {});
    expect(m.phases()[0]!.durationMs).toBe(0);
  });

  it('sums totals and computes the nudge rate across phases', () => {
    const m = new MetricsCollector(newState(), steppedClock([0, 10, 10, 40, 40, 60]));
    m.record('plan', m.start(), { costUsd: 1 }, { attempts: 1 });
    m.record('implement', m.start(), { costUsd: 2 }, { attempts: 2 }); // nudged
    m.record('tests', m.start(), { costUsd: 0.5 }, { attempts: 1 });
    const s = m.summary();
    expect(s.phases).toBe(3);
    expect(s.attempts).toBe(4);
    expect(s.nudgedPhases).toBe(1);
    expect(s.nudgeRate).toBeCloseTo(1 / 3, 5);
    expect(s.totalCostUsd).toBeCloseTo(3.5, 10);
    expect(s.totalDurationMs).toBe(60);
  });

  it('poisons the total cost to null once any phase is unpriceable', () => {
    const m = new MetricsCollector(newState());
    m.record('plan', m.start(), { costUsd: 1 });
    m.record('implement', m.start(), { costUsd: null }); // unpriceable
    m.record('tests', m.start(), { costUsd: 2 });
    expect(m.summary().totalCostUsd).toBeNull();
  });

  it('treats undefined per-phase cost as null in the row (observability only)', () => {
    const m = new MetricsCollector(newState());
    m.record('plan', m.start(), {}); // no costUsd at all
    expect(m.phases()[0]!.costUsd).toBeNull();
    // Undefined cost is unknown for metrics purposes, so the metrics rollup is unknown.
    expect(m.summary().totalCostUsd).toBeNull();
  });

  it('reports an empty summary before any phase is recorded', () => {
    const s = new MetricsCollector(newState()).summary();
    expect(s).toMatchObject({ phases: 0, attempts: 0, nudgedPhases: 0, nudgeRate: 0, totalCostUsd: 0 });
  });
});

describe('MetricsCollector.toJSON + save', () => {
  it('writes a snake_case metrics.json with summary + per-phase rows', () => {
    const m = new MetricsCollector(newState(), steppedClock([0, 100, 100, 300]));
    m.record('plan', m.start(), { costUsd: 0.25, inputTokens: 5 }, { attempts: 1, model: 'claude-opus-4-8' });
    m.record('implement', m.start(), { costUsd: 1.0 }, { attempts: 2, model: 'claude-opus-4-8' });
    m.save();

    const path = join(tmp, 'a1b2c3d4', METRICS_FILENAME);
    expect(existsSync(path)).toBe(true);
    const doc = JSON.parse(readFileSync(path, 'utf8'));
    expect(doc.adw_id).toBe('a1b2c3d4');
    expect(doc.runner).toBe('claude');
    expect(doc.summary).toMatchObject({
      phases: 2,
      attempts: 3,
      nudged_phases: 1,
      total_cost_usd: 1.25,
    });
    expect(doc.summary.nudge_rate).toBeCloseTo(0.5, 4);
    expect(doc.phases).toHaveLength(2);
    expect(doc.phases[0]).toMatchObject({
      phase: 'plan',
      model: 'claude-opus-4-8',
      attempts: 1,
      cost_usd: 0.25,
      input_tokens: 5,
    });
    expect(doc.phases[1]).toMatchObject({ phase: 'implement', attempts: 2, cost_usd: 1.0 });
  });

  it('does NOT write a file when nothing was recorded (no empty artifact)', () => {
    new MetricsCollector(newState()).save();
    expect(existsSync(join(tmp, 'a1b2c3d4', METRICS_FILENAME))).toBe(false);
  });

  it('preserves a null total_cost_usd in the written document', () => {
    const m = new MetricsCollector(newState());
    m.record('plan', m.start(), { costUsd: null });
    m.save();
    const doc = JSON.parse(readFileSync(join(tmp, 'a1b2c3d4', METRICS_FILENAME), 'utf8'));
    expect(doc.summary.total_cost_usd).toBeNull();
    expect(doc.phases[0].cost_usd).toBeNull();
  });

  it('save is best-effort: a bad workspace path never throws', () => {
    const state = new AdwState({ adwId: 'a1b2c3d4' });
    setAgentsDir('/proc/nonexistent-cannot-write\0/x'); // invalid path
    const m = new MetricsCollector(state);
    m.record('plan', m.start(), { costUsd: 1 });
    expect(() => m.save()).not.toThrow();
  });
});
