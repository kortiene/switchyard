/**
 * Per-phase run metrics (cost + duration observability).
 *
 * WHY A SEPARATE FILE, not state.json: agents/{adw_id}/state.json is the sole
 * cross-language contract (PLAN.md D4). Two suites pin it hard — the engine
 * parity test asserts the four runners write BYTE-EQUIVALENT state.json (modulo
 * id/runner/branch), and the cross-language test byte-matches it against a
 * committed fixture. Per-phase timings are wall-clock and non-deterministic, so
 * folding them into state.json would break both. They live in their own
 * additive artifact, agents/{adw_id}/metrics.json, which nothing treats as a
 * contract and resume never reads.
 *
 * The data answers the cost/duration questions the evaluation flagged: where a
 * run spends money and time, and how often the nudge-retry fires (each retry is
 * a second billed runner call, summed into the phase usage). See
 * docs/COST-AND-DURATION.md.
 */

import { randomBytes } from 'node:crypto';
import { mkdirSync, renameSync, unlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import type { PhaseUsage } from './invoker.js';
import type { AdwState } from './state.js';

export const METRICS_FILENAME = 'metrics.json';

/** A single runner invocation's recorded cost/duration sample. */
export interface PhaseMetric {
  phase: string;
  /** Model id the phase resolved to (tier routing / override), when known. */
  model?: string;
  /** Wall-clock milliseconds for the phase (all attempts), rounded to an int. */
  durationMs: number;
  /** Runner calls the phase consumed: 1 clean, 2 when the nudge-retry fired. */
  attempts: number;
  /** Dollars for the phase; null = unpriceable (poisons the run total). */
  costUsd: number | null;
  inputTokens?: number;
  outputTokens?: number;
  cachedInputTokens?: number;
  reasoningTokens?: number;
}

/** Injectable monotonic clock so tests get deterministic durations. */
export type Clock = () => number;

/** Default clock: high-resolution monotonic milliseconds. */
export const defaultClock: Clock = () => performance.now();

/**
 * Accumulates per-phase samples for one run and renders the metrics document.
 * Pure aside from the explicit save(); the orchestrator owns one collector per
 * run and records a sample after each phase completes.
 */
export class MetricsCollector {
  private readonly samples: PhaseMetric[] = [];

  constructor(
    private readonly state: AdwState,
    private readonly clock: Clock = defaultClock,
  ) {}

  /** Start timing a phase; returns the start mark to hand back to record(). */
  start(): number {
    return this.clock();
  }

  /**
   * Record one completed phase. `usage.costUsd` follows the same convention as
   * the run total: undefined ⇒ unknown/no-op (recorded as null here for the
   * per-phase row, which is observability only), null ⇒ explicitly unpriceable.
   */
  record(
    phase: string,
    startMark: number,
    usage: PhaseUsage,
    extra: { attempts?: number; model?: string } = {},
  ): void {
    const durationMs = Math.max(0, Math.round(this.clock() - startMark));
    const metric: PhaseMetric = {
      phase,
      durationMs,
      attempts: extra.attempts ?? 1,
      costUsd: usage.costUsd ?? null,
    };
    if (extra.model !== undefined) metric.model = extra.model;
    if (usage.inputTokens !== undefined) metric.inputTokens = usage.inputTokens;
    if (usage.outputTokens !== undefined) metric.outputTokens = usage.outputTokens;
    if (usage.cachedInputTokens !== undefined) metric.cachedInputTokens = usage.cachedInputTokens;
    if (usage.reasoningTokens !== undefined) metric.reasoningTokens = usage.reasoningTokens;
    this.samples.push(metric);
  }

  /** The recorded samples, in record order (defensive copy). */
  phases(): PhaseMetric[] {
    return this.samples.map((s) => ({ ...s }));
  }

  /** Run-level rollup: totals + the nudge-retry rate the cost work targets. */
  summary(): {
    phases: number;
    attempts: number;
    nudgedPhases: number;
    nudgeRate: number;
    totalDurationMs: number;
    /** Sum of priced phases; null once any phase is unpriceable. */
    totalCostUsd: number | null;
  } {
    let attempts = 0;
    let nudgedPhases = 0;
    let totalDurationMs = 0;
    let totalCostUsd: number | null = 0;
    for (const s of this.samples) {
      attempts += s.attempts;
      if (s.attempts > 1) nudgedPhases += 1;
      totalDurationMs += s.durationMs;
      if (s.costUsd === null) {
        totalCostUsd = null;
      } else if (totalCostUsd !== null) {
        totalCostUsd += s.costUsd;
      }
    }
    const phases = this.samples.length;
    return {
      phases,
      attempts,
      nudgedPhases,
      nudgeRate: phases === 0 ? 0 : nudgedPhases / phases,
      totalDurationMs,
      totalCostUsd,
    };
  }

  /** The on-disk metrics document (snake_case, matching state.json style). */
  toJSON(): Record<string, unknown> {
    const s = this.summary();
    return {
      adw_id: this.state.adwId,
      runner: this.state.runner ?? null,
      summary: {
        phases: s.phases,
        attempts: s.attempts,
        nudged_phases: s.nudgedPhases,
        nudge_rate: round4(s.nudgeRate),
        total_duration_ms: s.totalDurationMs,
        total_cost_usd: s.totalCostUsd,
      },
      phases: this.samples.map((m) => ({
        phase: m.phase,
        ...(m.model !== undefined ? { model: m.model } : {}),
        duration_ms: m.durationMs,
        attempts: m.attempts,
        cost_usd: m.costUsd,
        ...(m.inputTokens !== undefined ? { input_tokens: m.inputTokens } : {}),
        ...(m.outputTokens !== undefined ? { output_tokens: m.outputTokens } : {}),
        ...(m.cachedInputTokens !== undefined ? { cached_input_tokens: m.cachedInputTokens } : {}),
        ...(m.reasoningTokens !== undefined ? { reasoning_tokens: m.reasoningTokens } : {}),
      })),
    };
  }

  /**
   * Persist agents/{adw_id}/metrics.json (best effort). Like state.save(), a
   * write failure must never abort a run — metrics are observability, never
   * load-bearing — so I/O errors are swallowed.
   */
  save(): void {
    if (this.samples.length === 0) {
      return;
    }
    try {
      const dir = this.state.workspace();
      mkdirSync(dir, { recursive: true });
      // Write-then-rename: consumers (budget accounting, parity tooling) read
      // this file while the run is live, so they must never see a torn write.
      const path = join(dir, METRICS_FILENAME);
      const tmp = `${path}.${process.pid}.${randomBytes(8).toString('hex')}.tmp`;
      try {
        writeFileSync(tmp, `${JSON.stringify(this.toJSON(), null, 2)}\n`, { encoding: 'utf8', flag: 'wx' });
        renameSync(tmp, path);
      } catch (error) {
        try {
          unlinkSync(tmp);
        } catch {
          // The temporary file may not have been created.
        }
        throw error;
      }
    } catch {
      // best effort
    }
  }
}

function round4(value: number): number {
  return Math.round(value * 10_000) / 10_000;
}
