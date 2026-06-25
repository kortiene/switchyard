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
 * Classification per phase invocation:
 *   done & no retry   → clean         (parsed first try)
 *   done & retry      → nudged-ok     (one nudge, then parsed)
 *   not-done & retry  → HARD-FAIL     (nudge retry also failed → the bar's count)
 *   not-done & no retry → uncounted   (fast-fail: timeout/budget/cancel — the bar
 *                                       excludes these — or a run still in flight)
 * `classify` is the shared structuredCall path (runner-independent, its own
 * internal retry that writes no transcript-2.log); it is bucketed separately and
 * kept OUT of the native-vs-fenced comparison.
 *
 * Usage: tsx tools/parity-rate.ts [--min N] [--json] <run-dir | agents-dir> ...
 * Exit:  1 only when the bar is measured AND fails; 0 for meets/insufficient.
 */

import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

/**
 * Must match the fenced-JSON contract footer emitted by buildFooter() in
 * src/phases.ts when emitJsonContract is true. test/parity-rate.test.ts asserts
 * a real footer still contains this, so wording drift is caught, not silent.
 */
export const FENCED_MARKER = 'End your reply with EXACTLY one fenced';

const PROMPT = 'prompt.txt';
const FIRST = 'transcript.log';
const RETRY = 'transcript-2.log';

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

function readMaybe(path: string): string | null {
  return existsSync(path) ? readFileSync(path, 'utf8') : null;
}

/** Classify every phase subdir of one run workspace against its state.json. */
export function analyzeRun(runDir: string): RunAnalysis {
  const statePath = join(runDir, 'state.json');
  let state: Record<string, unknown>;
  try {
    state = JSON.parse(readFileSync(statePath, 'utf8')) as Record<string, unknown>;
  } catch (err) {
    return { adwId: runDir, issue: null, runner: null, phases: [], error: `unreadable state.json: ${String(err)}` };
  }
  const adwId = typeof state['adw_id'] === 'string' ? state['adw_id'] : runDir;
  const issue = typeof state['issue_number'] === 'string' ? state['issue_number'] : null;
  const runner = typeof state['runner'] === 'string' ? state['runner'] : null;
  const done = new Set(Array.isArray(state['completed_phases']) ? (state['completed_phases'] as string[]) : []);

  const phases: PhaseClassification[] = [];
  for (const name of readdirSync(runDir)) {
    const phaseDir = join(runDir, name);
    if (!statSync(phaseDir).isDirectory() || !existsSync(join(phaseDir, FIRST))) {
      continue; // not a phase dir (run-root files, or a phase that never ran)
    }
    const outcome = classifyOutcome(true, existsSync(join(phaseDir, RETRY)), done.has(name));
    phases.push({ phase: name, path: classifyPhasePath(readMaybe(join(phaseDir, PROMPT)), name), outcome });
  }
  phases.sort((a, b) => a.phase.localeCompare(b.phase));
  return { adwId, issue, runner, phases };
}

export interface Bucket {
  clean: number;
  nudgedOk: number;
  hardFail: number;
  uncounted: number;
}

const PATHS: ContractPath[] = ['native', 'fenced', 'classify', 'unknown'];

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

const pct = (n: number, d: number): string => (d === 0 ? 'n/a' : `${((100 * n) / d).toFixed(1)}%`);

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

export function renderReport(runs: RunAnalysis[], minAttempts: number): string {
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
  out.push('## Verdict (parity bar: native hard-fail rate ≤ fenced)');
  out.push('');
  out.push(verdict(agg, minAttempts).line);
  out.push('');
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
    } else if (arg !== undefined && !arg.startsWith('--')) inputs.push(arg);
  }
  if (inputs.length === 0) {
    process.stderr.write('usage: tsx tools/parity-rate.ts [--min N] [--json] <run-dir | agents-dir> ...\n');
    return 2;
  }
  const runDirs = [...new Set(inputs.flatMap(findRunDirs))];
  if (runDirs.length === 0) {
    process.stderr.write('no run workspaces found (looked for directories containing state.json)\n');
    return 2;
  }
  const runs = runDirs.map(analyzeRun);
  if (asJson) {
    process.stdout.write(`${JSON.stringify({ runs, aggregate: aggregate(runs), verdict: verdict(aggregate(runs), minAttempts) }, null, 2)}\n`);
  } else {
    process.stdout.write(`${renderReport(runs, minAttempts)}\n`);
  }
  return verdict(aggregate(runs), minAttempts).ok === false ? 1 : 0;
}

if (process.argv[1] !== undefined && process.argv[1] === fileURLToPath(import.meta.url)) {
  process.exit(main(process.argv.slice(2)));
}
