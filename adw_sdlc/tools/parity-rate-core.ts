/**
 * Pure parity-rate classifier — no `node:fs`, no `process`, no I/O.
 *
 * This module is the classification/aggregation/verdict layer split out of
 * `parity-rate.ts` so it can be unit-tested and reused without reading the disk
 * or printing. The I/O + rendering + CLI layer lives in `parity-rate.ts`, which
 * loads run workspaces off disk into the plain-data inputs defined here
 * (`RunInputs`/`PhaseInputs`), delegates to `classifyRun`, and re-exports this
 * whole surface so existing importers stay byte-identical.
 *
 * Classification per phase invocation (see `classifyOutcome`):
 *   done & no retry   → clean         (parsed first try)
 *   done & retry      → nudged-ok     (one nudge, then parsed)
 *   not-done & retry  → HARD-FAIL     (nudge retry also failed → the bar's count)
 *   not-done & no retry → uncounted   (fast-fail: timeout/budget/cancel — the bar
 *                                       excludes these — or a run still in flight)
 * `classify` is the shared structuredCall path (runner-independent, its own
 * internal retry that writes no transcript-2.log); it is bucketed separately and
 * kept OUT of the native-vs-fenced comparison.
 */

/**
 * Must match the fenced-JSON contract footer emitted by buildFooter() in
 * src/phases.ts when emitJsonContract is true. test/parity-rate.test.ts asserts
 * a real footer still contains this, so wording drift is caught, not silent.
 */
export const FENCED_MARKER = 'End your reply with EXACTLY one fenced';

export type ContractPath = 'native' | 'fenced' | 'classify' | 'unknown';
export type Outcome = 'clean' | 'nudged-ok' | 'hard-fail' | 'uncounted' | 'skipped';

export interface PhaseClassification {
  phase: string;
  path: ContractPath;
  outcome: Outcome;
}

export interface RunAnalysis {
  adwId: string;
  issue: string | null;
  runner: string | null;
  phases: PhaseClassification[];
  error?: string;
}

/** native iff the first prompt omits the fenced footer; `classify` is its own bucket. */
export function classifyPhasePath(promptText: string | null, phase: string): ContractPath {
  if (phase === 'classify') {
    return 'classify';
  }
  if (promptText === null) {
    return 'unknown';
  }
  return promptText.includes(FENCED_MARKER) ? 'fenced' : 'native';
}

/** Map (attempted, nudged, done) onto the bar's outcome buckets. */
export function classifyOutcome(attempted: boolean, nudged: boolean, done: boolean): Outcome {
  if (!attempted) {
    return 'skipped';
  }
  if (done) {
    return nudged ? 'nudged-ok' : 'clean';
  }
  return nudged ? 'hard-fail' : 'uncounted';
}

/** Already-loaded inputs for one phase subdir — no filesystem handle. */
export interface PhaseInputs {
  phase: string;
  promptText: string | null; // prompt.txt contents, or null if absent
  hasRetry: boolean; // transcript-2.log exists (the single nudge retry)
}

/** Already-loaded inputs for one run workspace — no filesystem handle. */
export interface RunInputs {
  adwId: string;
  issue: string | null;
  runner: string | null;
  completedPhases: string[];
  /** Only phases that were attempted (had transcript.log); see invariants in parity-rate.ts. */
  phases: PhaseInputs[];
  error?: string; // set by the loader when state.json is unreadable
}

/**
 * Pure: classify one run from already-loaded inputs. Mirror of analyzeRun's old
 * loop — every supplied phase is attempted (`attempted = true`), classified, and
 * the result is sorted by `phase.localeCompare`. An `error` short-circuits to an
 * empty-phases analysis so the run still surfaces as an error line in the report.
 */
export function classifyRun(inputs: RunInputs): RunAnalysis {
  if (inputs.error) {
    return { adwId: inputs.adwId, issue: inputs.issue, runner: inputs.runner, phases: [], error: inputs.error };
  }
  const done = new Set(inputs.completedPhases);
  const phases: PhaseClassification[] = inputs.phases.map((p) => ({
    phase: p.phase,
    path: classifyPhasePath(p.promptText, p.phase),
    outcome: classifyOutcome(true, p.hasRetry, done.has(p.phase)),
  }));
  phases.sort((a, b) => a.phase.localeCompare(b.phase));
  return { adwId: inputs.adwId, issue: inputs.issue, runner: inputs.runner, phases };
}

export interface Bucket {
  clean: number;
  nudgedOk: number;
  hardFail: number;
  uncounted: number;
}

export const PATHS: ContractPath[] = ['native', 'fenced', 'classify', 'unknown'];

/** Counted structured-output attempts = clean + nudged-ok + hard-fail (excludes uncounted). */
export function attempts(b: Bucket): number {
  return b.clean + b.nudgedOk + b.hardFail;
}

export function aggregate(runs: RunAnalysis[]): Record<ContractPath, Bucket> {
  const agg = Object.fromEntries(
    PATHS.map((p) => [p, { clean: 0, nudgedOk: 0, hardFail: 0, uncounted: 0 }]),
  ) as Record<ContractPath, Bucket>;
  for (const run of runs) {
    for (const { path, outcome } of run.phases) {
      const b = agg[path];
      if (outcome === 'clean') b.clean += 1;
      else if (outcome === 'nudged-ok') b.nudgedOk += 1;
      else if (outcome === 'hard-fail') b.hardFail += 1;
      else if (outcome === 'uncounted') b.uncounted += 1;
    }
  }
  return agg;
}

export const pct = (n: number, d: number): string => (d === 0 ? 'n/a' : `${((100 * n) / d).toFixed(1)}%`);

export interface Verdict {
  ok: boolean | null; // true = meets, false = fails, null = insufficient
  line: string;
}

/** The parity bar: native hard-fail rate ≤ fenced, only once both have ≥ minAttempts. */
export function verdict(agg: Record<ContractPath, Bucket>, minAttempts: number): Verdict {
  const n = agg.native;
  const f = agg.fenced;
  const na = attempts(n);
  const fa = attempts(f);
  if (na < minAttempts || fa < minAttempts) {
    return {
      ok: null,
      line:
        `⚠️  INSUFFICIENT DATA — native has ${na} and fenced has ${fa} counted structured-output ` +
        `attempts; need ≥ ${minAttempts} per path before the bar can be asserted. ` +
        `The structural argument in PARITY.md is not a substitute for this measurement` +
        (fa === 0 ? ' (no fenced-path runs yet — e.g. the `pi` runner has not been run live).' : '.'),
    };
  }
  const nr = n.hardFail / na;
  const fr = f.hardFail / fa;
  return nr <= fr
    ? { ok: true, line: `✅ MEETS BAR — native ${pct(n.hardFail, na)} ≤ fenced ${pct(f.hardFail, fa)} hard-fail rate.` }
    : { ok: false, line: `❌ FAILS BAR — native ${pct(n.hardFail, na)} > fenced ${pct(f.hardFail, fa)} hard-fail rate.` };
}

/**
 * The absolute native bar: native hard-fail rate ≤ maxRatePct. Unlike the
 * comparative bar it needs only the native path, so a claude-only sample can
 * clear it — the pragmatic gate for an (A)-MVP ("claude ships reliably") where a
 * fenced baseline does not yet exist.
 */
export function nativeAbsoluteVerdict(
  agg: Record<ContractPath, Bucket>,
  minAttempts: number,
  maxRatePct: number,
): Verdict {
  const n = agg.native;
  const na = attempts(n);
  if (na < minAttempts) {
    return {
      ok: null,
      line: `⚠️  INSUFFICIENT DATA — native has ${na} counted attempts; need ≥ ${minAttempts} before the absolute bar can be asserted.`,
    };
  }
  const rate = (100 * n.hardFail) / na;
  return rate <= maxRatePct
    ? { ok: true, line: `✅ MEETS ABSOLUTE BAR — native ${pct(n.hardFail, na)} hard-fail ≤ ${maxRatePct}% target (over ${na} attempts).` }
    : { ok: false, line: `❌ FAILS ABSOLUTE BAR — native ${pct(n.hardFail, na)} hard-fail > ${maxRatePct}% target (over ${na} attempts).` };
}
