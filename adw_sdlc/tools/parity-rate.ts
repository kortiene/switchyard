/**
 * Structured-output hard-failure-rate recorder.
 *
 * PARITY.md's Section-10 parity bar is: a native-schema backend's hard-failure
 * rate must be **no worse** than the fenced-JSON+nudge path "over the parity
 * runs". Today that rate is asserted by a *structural argument* and backed by a
 * single live run — PARITY.md itself files it under "Rate (live, owed)". This
 * tool turns that argument into a measurement: point it at one or more completed
 * run workspaces (`agents/{adw_id}/`) and it classifies every phase invocation
 * and reports the per-path hard-failure rate, refusing to declare the bar met on
 * an insufficient sample.
 *
 * It reads ONLY on-disk artifacts the invoker already writes — no kernel import,
 * no SDK, no network — so it can run against any archived run:
 *   - `state.json` → `runner`, `completed_phases`, `issue_number` (snake_case;
 *     src/state.ts toJSON()).
 *   - per-phase `prompt.txt` → contains the fenced-JSON contract footer iff the
 *     fenced path was taken (`emitJsonContract`, src/phases.ts buildFooter);
 *     a native-schema runner's first prompt omits it. See FENCED_MARKER.
 *   - per-phase `transcript.log` (first attempt) and `transcript-2.log` (the
 *     single nudge retry, written only when the first attempt failed to
 *     parse/validate — src/run-phase.ts).
 *
 * The pure classification/aggregation/verdict layer lives in `parity-rate-core.ts`
 * (no `node:fs`, no `process`); this file is the I/O + rendering + CLI shell. It
 * `export *`s the core surface so existing importers (test/parity-rate.test.ts,
 * `tsx tools/parity-rate.ts`) keep working unchanged.
 *
 * Usage: tsx tools/parity-rate.ts [--min N] [--max-native-rate PCT] [--json] <run-dir | agents-dir> ...
 * Exit:  1 only when a configured bar is measured AND fails; 0 for meets/insufficient.
 */

import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  PATHS,
  type PhaseInputs,
  type RunAnalysis,
  aggregate,
  attempts,
  classifyRun,
  nativeAbsoluteVerdict,
  pct,
  verdict,
} from './parity-rate-core.js';

// Re-export the pure classifier surface so external importers and the CLI entry
// point keep their byte-identical public API (classifyPhasePath, classifyOutcome,
// aggregate, verdict, FENCED_MARKER, the types, classifyRun, …).
export * from './parity-rate-core.js';

const PROMPT = 'prompt.txt';
const FIRST = 'transcript.log';
const RETRY = 'transcript-2.log';

function readMaybe(path: string): string | null {
  return existsSync(path) ? readFileSync(path, 'utf8') : null;
}

/**
 * Load one run workspace off disk into RunInputs and delegate classification to
 * the pure `classifyRun`. A child entry is a "phase" iff it `isDirectory()` AND
 * contains `transcript.log`; everything else (run-root files, never-ran phases)
 * is skipped silently. An unreadable `state.json` yields an error analysis with
 * no phases — the run still appears (as an error line) in the report.
 */
export function analyzeRun(runDir: string): RunAnalysis {
  const statePath = join(runDir, 'state.json');
  let state: Record<string, unknown>;
  try {
    state = JSON.parse(readFileSync(statePath, 'utf8')) as Record<string, unknown>;
  } catch (err) {
    return classifyRun({
      adwId: runDir,
      issue: null,
      runner: null,
      completedPhases: [],
      phases: [],
      error: `unreadable state.json: ${String(err)}`,
    });
  }
  const adwId = typeof state['adw_id'] === 'string' ? state['adw_id'] : runDir;
  const issue = typeof state['issue_number'] === 'string' ? state['issue_number'] : null;
  const runner = typeof state['runner'] === 'string' ? state['runner'] : null;
  const completedPhases = Array.isArray(state['completed_phases']) ? (state['completed_phases'] as string[]) : [];

  const phases: PhaseInputs[] = [];
  for (const name of readdirSync(runDir)) {
    const phaseDir = join(runDir, name);
    if (!statSync(phaseDir).isDirectory() || !existsSync(join(phaseDir, FIRST))) {
      continue; // not a phase dir (run-root files, or a phase that never ran)
    }
    phases.push({
      phase: name,
      promptText: readMaybe(join(phaseDir, PROMPT)),
      hasRetry: existsSync(join(phaseDir, RETRY)),
    });
  }
  return classifyRun({ adwId, issue, runner, completedPhases, phases });
}

export function renderReport(runs: RunAnalysis[], minAttempts: number, maxNativeRatePct?: number): string {
  const agg = aggregate(runs);
  const ok = runs.filter((r) => !r.error);
  const out: string[] = [];
  out.push('# Structured-output hard-failure rate — measured');
  out.push('');
  out.push(`Runs analyzed: **${ok.length}**${runs.length !== ok.length ? ` (${runs.length - ok.length} unreadable)` : ''}` +
    (ok.length ? ` — ${ok.map((r) => `${r.adwId}${r.runner ? `/${r.runner}` : ''}`).join(', ')}` : ''));
  out.push('');
  out.push('| Path | attempts | clean | nudged→ok | HARD-FAIL | hard-fail rate | nudge rate |');
  out.push('|---|---:|---:|---:|---:|---:|---:|');
  for (const p of PATHS) {
    const b = agg[p];
    const a = attempts(b);
    if (a === 0 && b.uncounted === 0) continue;
    const label = p === 'classify' ? 'classify *(structuredCall; excluded from bar)*' : p;
    out.push(`| ${label} | ${a} | ${b.clean} | ${b.nudgedOk} | ${b.hardFail} | ${pct(b.hardFail, a)} | ${pct(b.nudgedOk + b.hardFail, a)} |`);
  }
  out.push('');
  const uncounted = PATHS.reduce((s, p) => s + agg[p].uncounted, 0);
  out.push(`Uncounted (timeout/budget/cancel/in-progress — excluded from the bar per PARITY.md): **${uncounted}**`);
  out.push('');
  out.push('## Verdict — comparative bar (native hard-fail rate ≤ fenced)');
  out.push('');
  out.push(verdict(agg, minAttempts).line);
  out.push('');
  if (maxNativeRatePct !== undefined) {
    out.push(`## Verdict — absolute native bar (≤ ${maxNativeRatePct}%)`);
    out.push('');
    out.push(nativeAbsoluteVerdict(agg, minAttempts, maxNativeRatePct).line);
    out.push('');
  }
  out.push('## Per-run breakdown');
  for (const r of runs) {
    if (r.error) {
      out.push(`- **${r.adwId}** — ⚠️ ${r.error}`);
      continue;
    }
    const cells = r.phases.map((p) => {
      const mark = p.outcome === 'hard-fail' ? ' ⛔' : p.outcome === 'nudged-ok' ? ' ⚠' : '';
      return `${p.phase}:${p.path}/${p.outcome}${mark}`;
    });
    out.push(`- **${r.adwId}**${r.issue ? ` (issue #${r.issue})` : ''}${r.runner ? `, runner=${r.runner}` : ''} — ${cells.join('  ') || '(no phases ran)'}`);
  }
  return out.join('\n');
}

/** A path is a run dir if it holds a state.json; otherwise scan its child dirs (an `agents/` parent). */
function findRunDirs(input: string): string[] {
  if (existsSync(join(input, 'state.json'))) {
    return [input];
  }
  if (!existsSync(input) || !statSync(input).isDirectory()) {
    return [];
  }
  return readdirSync(input)
    .map((name) => join(input, name))
    .filter((p) => statSync(p).isDirectory() && existsSync(join(p, 'state.json')));
}

function main(argv: string[]): number {
  const inputs: string[] = [];
  let minAttempts = 20;
  let maxNativeRatePct: number | undefined;
  let asJson = false;
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--json') asJson = true;
    else if (arg === '--min') {
      minAttempts = Number(argv[(i += 1)]);
      if (!Number.isFinite(minAttempts) || minAttempts < 1) {
        process.stderr.write('--min requires a positive number\n');
        return 2;
      }
    } else if (arg === '--max-native-rate') {
      maxNativeRatePct = Number(argv[(i += 1)]);
      if (!Number.isFinite(maxNativeRatePct) || maxNativeRatePct < 0 || maxNativeRatePct > 100) {
        process.stderr.write('--max-native-rate requires a percentage in [0, 100]\n');
        return 2;
      }
    } else if (arg !== undefined && !arg.startsWith('--')) inputs.push(arg);
  }
  if (inputs.length === 0) {
    process.stderr.write('usage: tsx tools/parity-rate.ts [--min N] [--max-native-rate PCT] [--json] <run-dir | agents-dir> ...\n');
    return 2;
  }
  const runDirs = [...new Set(inputs.flatMap(findRunDirs))];
  if (runDirs.length === 0) {
    process.stderr.write('no run workspaces found (looked for directories containing state.json)\n');
    return 2;
  }
  const runs = runDirs.map(analyzeRun);
  const agg = aggregate(runs);
  const comparative = verdict(agg, minAttempts);
  const absolute = maxNativeRatePct !== undefined ? nativeAbsoluteVerdict(agg, minAttempts, maxNativeRatePct) : null;
  if (asJson) {
    process.stdout.write(`${JSON.stringify({ runs, aggregate: agg, comparative, absolute }, null, 2)}\n`);
  } else {
    process.stdout.write(`${renderReport(runs, minAttempts, maxNativeRatePct)}\n`);
  }
  return comparative.ok === false || absolute?.ok === false ? 1 : 0;
}

if (process.argv[1] !== undefined && process.argv[1] === fileURLToPath(import.meta.url)) {
  process.exit(main(process.argv.slice(2)));
}
