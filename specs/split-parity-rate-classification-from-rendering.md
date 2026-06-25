# Spec: Split parity-rate classification from rendering

- **Issue:** #5 — `refactor: split parity-rate classification from rendering`
- **Labels:** `issue_class:refactor`, `adw-live-batch`
- **Planned ADW run mode:** native
- **Type:** behavior-preserving refactor (no functional change)
- **Owning file:** `adw_sdlc/tools/parity-rate.ts` (+ new `adw_sdlc/tools/parity-rate-core.ts`)
- **Tests:** `adw_sdlc/test/parity-rate.test.ts`

---

## 1. Context & current state (read this first)

`tools/parity-rate.ts` is the structured-output hard-failure-rate recorder. It
reads completed run workspaces (`agents/{adw_id}/`) off disk, classifies every
phase invocation, aggregates per-path buckets, applies two verdict bars, and
prints a Markdown report (or `--json`). It is typechecked (`tsconfig.json`
includes `tools`) but **never built to `dist`** (`tsconfig.build.json` includes
only `src`), so it ships nowhere — only `test/parity-rate.test.ts` imports it.

**Important:** much of what the issue asks for is *already done*. The file today
already separates pure logic from I/O at the function level:

| Concern | Symbols (current) | Purity |
|---|---|---|
| Per-phase classification | `classifyPhasePath`, `classifyOutcome` | pure, exported, **tested** |
| Aggregation | `aggregate`, `attempts` | pure, exported, **tested** |
| Verdicts | `verdict`, `nativeAbsoluteVerdict` | pure, exported, **tested** |
| Rendering | `renderReport`, `pct` | pure string-building, `renderReport` exported, **untested** |
| Drift guard | `FENCED_MARKER` | const, exported, **tested** |
| File I/O | `analyzeRun`, `readMaybe`, `findRunDirs` | reads `node:fs` |
| CLI / printing | `main`, `import.meta.url` guard | `process.argv` / `stdout` / `exit` |

So this is **not** a green-field extraction. Doing a naive "extract the
functions" pass would churn already-extracted, already-tested code and risk the
byte-stability acceptance criterion for no gain. The genuine, valuable delta is:

1. **One remaining coupling:** `analyzeRun` interleaves filesystem reads
   (`readFileSync(state.json)`, `readdirSync`, `readMaybe(prompt.txt)`,
   `existsSync(transcript*.log)`) **with** the per-run classification loop. The
   classifier cannot be exercised without touching the disk, so per-run
   classification is the one piece not yet unit-testable in isolation.
2. **No structural boundary:** "the classifier is reusable without printing" is
   today only a convention inside one file, not an enforced module boundary. A
   future edit could reintroduce a `console.log`/`process` dependency into the
   pure path and nothing would catch it.

This spec therefore (a) extracts a **pure `classifyRun`** out of `analyzeRun`,
and (b) moves the pure classification/aggregation/verdict layer into a
dependency-free **`tools/parity-rate-core.ts`** module that imports neither
`node:fs` nor `process`, with `parity-rate.ts` re-exporting the public surface so
the CLI and the existing test import path stay byte-identical.

### Behavior that MUST be preserved exactly

- `npm run parity:rate` stdout is **byte-for-byte identical** for the same
  inputs, including the trailing newline (`main` writes `` `${renderReport(...)}\n` ``)
  and the `--json` shape (`{ runs, aggregate, comparative, absolute }`).
- Exit codes: `0` for meets/insufficient, `1` only when a configured bar is
  measured AND fails, `2` for usage/arg errors. (`main` return values unchanged.)
- The drift-guard test on `FENCED_MARKER` (`buildFooter` wording) stays intact
  and keeps passing — `FENCED_MARKER` must remain importable from
  `../tools/parity-rate.js`.
- The current `analyzeRun` filtering/ordering invariants:
  - a child entry is a "phase" iff it `isDirectory()` **and** contains
    `transcript.log` (`FIRST`); everything else is skipped silently;
  - every included phase is classified with `attempted = true` (so `analyzeRun`
    never emits the `skipped` outcome — only `classifyOutcome`'s unit test does);
  - phases are sorted by `phase.localeCompare`;
  - unreadable `state.json` yields `{ ..., phases: [], error }` and the run still
    appears (as an error line) in the report.

---

## 2. Goal & acceptance criteria

### Goal
Separate the parity-rate **classifier** (pure: per-phase classification, rate
aggregation, verdicts, and per-run classification) from the **I/O + rendering**
layer, so the classifier can be unit-tested and reused without reading the disk
or printing — while keeping CLI output byte-stable.

### Acceptance criteria (from the issue)
- [ ] No behavior change to `npm run parity:rate` output for the same inputs.
- [ ] Classification/verdict functions are exported and unit-tested.
- [ ] `npm run verify` stays green.

### Additional acceptance criteria (this spec)
- [ ] A pure `classifyRun(inputs)` exists that produces a `RunAnalysis` from
      already-loaded inputs with **no** `node:fs` / `process` access, and is
      unit-tested directly (no tmp dir).
- [ ] The pure classification layer lives in `tools/parity-rate-core.ts`, which
      imports neither `node:fs` nor `node:process`/`process`.
- [ ] `tools/parity-rate.ts` re-exports the public surface so existing importers
      (`test/parity-rate.test.ts`) and `tsx tools/parity-rate.ts` are unchanged
      in behavior.
- [ ] A golden/snapshot test pins `renderReport` output (and the `--json` object)
      for a fixed `RunAnalysis[]`, locking byte-stability against future drift.

---

## 3. Recommended design

**Two-module split** (primary recommendation), plus extracting `classifyRun`.

```
tools/
  parity-rate-core.ts   ← NEW. Pure classifier. No node:fs, no process.
  parity-rate.ts        ← I/O + rendering + CLI. Imports core, re-exports it.
```

### 3.1 `tools/parity-rate-core.ts` (pure)

Move these unchanged from `parity-rate.ts`:

- Types: `ContractPath`, `Outcome`, `PhaseClassification`, `RunAnalysis`,
  `Bucket`, `Verdict`.
- Const: `FENCED_MARKER`, and the module-private `PATHS`.
- Functions: `classifyPhasePath`, `classifyOutcome`, `attempts`, `aggregate`,
  `verdict`, `nativeAbsoluteVerdict`.
- The private `pct` helper **stays with rendering** (it is presentation
  formatting used by `verdict`/`nativeAbsoluteVerdict`/`renderReport`). Decision:
  `pct` is used by the verdict functions, so it must live in core too (verdicts
  are pure and belong in core). Keep `pct` in core; `renderReport` (rendering)
  imports it. See §6 open question O-1 if a stricter classification/rendering
  separation of `pct` is desired.

Add the new pure per-run classifier and its input types:

```ts
/** Already-loaded inputs for one run workspace — no filesystem handle. */
export interface PhaseInputs {
  phase: string;
  promptText: string | null; // prompt.txt contents, or null if absent
  hasRetry: boolean;         // transcript-2.log exists (the single nudge retry)
}

export interface RunInputs {
  adwId: string;
  issue: string | null;
  runner: string | null;
  completedPhases: string[];
  /** Only phases that were attempted (had transcript.log); see invariants. */
  phases: PhaseInputs[];
  error?: string;            // set by the loader when state.json is unreadable
}

/** Pure: classify one run from loaded inputs. Mirror of analyzeRun's old loop. */
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
```

Notes:
- `attempted` is hard-coded `true` to preserve the current behavior (the loader
  only ever supplies attempted phases). Do **not** thread `hasFirst` into
  `classifyRun`; keep the "is this a phase dir?" gate in the I/O loader so the
  emitted phase set and ordering are byte-identical.
- `classifyRun` is the only genuinely new pure function; everything else is a
  move.

### 3.2 `tools/parity-rate.ts` (I/O + rendering + CLI)

- At the top, import the core surface and **re-export** it so external importers
  and the test file keep working unchanged:
  ```ts
  export * from './parity-rate-core.js';
  import {
    FENCED_MARKER, classifyPhasePath, classifyOutcome, attempts, aggregate,
    verdict, nativeAbsoluteVerdict, pct, classifyRun,
    type RunAnalysis, type RunInputs, type PhaseInputs, type ContractPath, type Bucket,
  } from './parity-rate-core.js';
  ```
  (Keep whatever subset the renderer/CLI actually references; `export *` covers
  re-export of the public types/functions/`FENCED_MARKER`.)
- Keep here (rendering): `renderReport`. It is "rendering" per the issue title;
  it builds the report string and is the natural home for presentation. It may
  stay in `parity-rate.ts` or move to a dedicated `parity-rate-render.ts` — see
  §6 O-2. Default: keep in `parity-rate.ts` to minimize surface.
- Refactor `analyzeRun` into a thin **loader** that builds `RunInputs` from disk
  and delegates to `classifyRun`:
  ```ts
  export function analyzeRun(runDir: string): RunAnalysis {
    const statePath = join(runDir, 'state.json');
    let state: Record<string, unknown>;
    try {
      state = JSON.parse(readFileSync(statePath, 'utf8')) as Record<string, unknown>;
    } catch (err) {
      return classifyRun({
        adwId: runDir, issue: null, runner: null, completedPhases: [], phases: [],
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
      if (!statSync(phaseDir).isDirectory() || !existsSync(join(phaseDir, FIRST))) continue;
      phases.push({
        phase: name,
        promptText: readMaybe(join(phaseDir, PROMPT)),
        hasRetry: existsSync(join(phaseDir, RETRY)),
      });
    }
    return classifyRun({ adwId, issue, runner, completedPhases, phases });
  }
  ```
  This keeps the **exact** filtering (`isDirectory() && existsSync(FIRST)`),
  field extraction, and error handling; the sort now lives in `classifyRun`.
- Keep `readMaybe`, `findRunDirs`, `PROMPT`/`FIRST`/`RETRY` consts, `main`, and
  the `import.meta.url` main guard here, unchanged.

### 3.3 Why a module split (not just in-file extraction)

- It makes "the classifier is reusable without printing" a **structural,
  enforceable** property: `parity-rate-core.ts` provably imports no `node:fs` /
  `process`. A test can assert this (see §4), analogous to how `FENCED_MARKER`
  pins the contract footer wording.
- It matches the issue title's literal "split" and the live-batch purpose
  ("reusable readiness tooling").
- Re-exporting keeps the public API and CLI byte-stable, so AC #1 is protected.

A lower-risk **fallback** (in-file extraction only) is documented in §7 if the
implementer prefers the smallest diff.

---

## 4. Test plan (`test/parity-rate.test.ts`)

Keep all existing tests passing **unchanged** (they assert the public API still
behaves). Add:

1. **Pure `classifyRun` test (no tmp dir).** Construct a `RunInputs` literal with
   a mix of phases and assert the resulting `RunAnalysis.phases` (path + outcome
   + sort order) matches expectations — mirroring the existing `analyzeRun`
   synthetic-workspace test but with zero filesystem. Cover: native clean,
   native nudged-ok, native hard-fail, native uncounted, a `fenced` phase (prompt
   contains `FENCED_MARKER`), a `classify` phase (special-cased path), and an
   `unknown` phase (`promptText: null`). Assert ordering is `localeCompare`.
2. **`classifyRun` error passthrough.** `classifyRun({ ..., error: 'x' })`
   returns `{ phases: [], error: 'x' }`.
3. **`analyzeRun` still works over the real disk** — keep the existing synthetic
   workspace test as the integration check that the loader feeds `classifyRun`
   correctly. (Already present; leave intact.)
4. **Golden render test (locks AC #1).** Build a fixed `RunAnalysis[]` (or run
   `analyzeRun` over a small fixed synthetic workspace) and assert
   `renderReport(runs, minAttempts, maxNativeRatePct)` equals an inline expected
   string (use `toMatchInlineSnapshot` or an explicit expected string). Include a
   case with `maxNativeRatePct` set and one error run, so the absolute-bar
   section and the `⚠️ error` per-run line are both covered. This is new
   coverage — `renderReport` is currently untested.
5. **Core purity guard (structural).** Read `tools/parity-rate-core.ts` source
   (via `readFileSync(new URL('../tools/parity-rate-core.ts', import.meta.url))`)
   and assert it does **not** match `/from ['"]node:fs['"]/`, `/\bprocess\./`, or
   `/from ['"]node:process['"]/`. Cheap drift guard that the pure layer stays
   pure. (Analogous intent to the `FENCED_MARKER` guard.)
6. Imports: the new pure tests may import `classifyRun`/`RunInputs` from either
   `../tools/parity-rate.js` (re-exported) or `../tools/parity-rate-core.js`
   (direct). Prefer importing the **core** module directly for the pure tests to
   demonstrate reuse-without-the-CLI; keep the existing `FENCED_MARKER` drift
   import from `../tools/parity-rate.js` unchanged so the re-export is exercised.

---

## 5. Step-by-step implementation

1. Create `tools/parity-rate-core.ts`; move the pure types, `FENCED_MARKER`,
   `PATHS`, `pct`, `classifyPhasePath`, `classifyOutcome`, `attempts`,
   `aggregate`, `verdict`, `nativeAbsoluteVerdict` into it verbatim (only adjust
   nothing semantically). Add `PhaseInputs`, `RunInputs`, and `classifyRun`.
2. In `tools/parity-rate.ts`: delete the moved symbols; add
   `export * from './parity-rate-core.js'` and a named import of what the
   renderer/CLI use. Rewrite `analyzeRun` as the thin loader delegating to
   `classifyRun` (§3.2). Leave `readMaybe`, `findRunDirs`, `renderReport`,
   `main`, consts, and the `import.meta.url` guard in place.
3. Keep module-local `const FIRST/RETRY/PROMPT` in `parity-rate.ts` (the loader
   needs them). `FENCED_MARKER` moves to core and is re-exported.
4. Extend `test/parity-rate.test.ts` per §4.
5. Run focused tests, then the full gate (§8).
6. Update docs that point at the tool's shape if needed (§9) — likely
   none required, since the public symbols and CLI are unchanged.

No production `src/` code changes. No prompt-pack (`.adw/`) changes — so
`pack:check` is unaffected and `pack:generate` need not run.

---

## 6. Open questions

- **O-1 (`pct` placement).** `pct` is presentation formatting but is consumed by
  the (pure) verdict functions, so it lands in core. If a reviewer wants a
  stricter classification-vs-rendering cut, `verdict`/`nativeAbsoluteVerdict`
  could return raw numbers and let the renderer format — but that changes the
  `Verdict.line` strings' provenance and risks byte-stability. **Recommendation:**
  keep `pct` and the verdict `line` strings in core as-is (lowest risk).
- **O-2 (separate render module).** Should `renderReport` move to
  `tools/parity-rate-render.ts` for a clean three-way split (core / render /
  CLI)? It's defensible but adds a file for a ~45-line function. **Recommendation:**
  keep `renderReport` in `parity-rate.ts` unless the reviewer prefers the split;
  either way it stays out of `parity-rate-core.ts`.
- **O-3 (module split vs in-file).** If the maintainer wants the absolute minimum
  diff, use the §7 fallback (no new file). The split is recommended for the
  structural purity guarantee; confirm preference.

## 7. Fallback design (in-file extraction, smallest diff)

If a new module is unwanted: keep one file, but still extract the pure
`classifyRun` from `analyzeRun` (so per-run classification is unit-testable
without disk), group the pure functions together with a banner comment, and add
the §4 tests minus the "core purity guard" (test 5, which requires a separate
module). This satisfies the issue's literal scope and all three issue acceptance
criteria, but does not give the enforceable "no I/O in the classifier" boundary.

---

## 8. Verification

- Focused first: `npx vitest run test/parity-rate.test.ts` from `adw_sdlc/`.
- Byte-stability spot check (manual, optional but recommended): capture
  `npm run parity:rate -- <fixture>` stdout before and after the change and
  `diff` them — expect empty. (A synthetic `agents/` fixture or an existing run
  workspace works; the golden render test in §4 is the automated equivalent.)
- Full gate: `npm run verify` from `adw_sdlc/`
  (`typecheck → lint:env → pack:check → test → build && rm -rf dist`). Must stay
  green. `typecheck` covers the new `tools/parity-rate-core.ts` because
  `tsconfig.json` includes `tools`; `build` (src-only) is unaffected, so the new
  tool file correctly never reaches `dist`.

## 9. Docs to check (likely no change)

These reference the tool but describe behavior/CLI, which is unchanged; review
and only touch if wording becomes inaccurate:
- `adw_sdlc/MVP-READINESS.md` (§ describing `tools/parity-rate.ts`),
- `adw_sdlc/PARITY.md` (the `npm run parity:rate -- agents/` "measure it" note),
- `adw_sdlc/docs/LIVE-RUN-BATCH.md` (issue #5 row),
- `adw_sdlc/HANDOVER.md` (the parity-rate harness entries). If a HANDOVER update
  is conventional for this repo, add a short note that the classifier was split
  into `parity-rate-core.ts` (pure) + `parity-rate.ts` (I/O/CLI), API/CLI
  byte-stable.

---

## 10. Risks & mitigations

| Risk | Mitigation |
|---|---|
| CLI output drifts (breaks AC #1) | Re-export public API unchanged; add golden `renderReport`/`--json` snapshot test; manual `diff` spot check. |
| `FENCED_MARKER` drift guard breaks | Keep `FENCED_MARKER` exported & re-exported from `parity-rate.ts`; existing `buildFooter` drift test left untouched. |
| Per-run classification changes (filter/sort/error) | `classifyRun` mirrors the old loop exactly; loader keeps the `isDirectory() && FIRST` gate and error path; existing `analyzeRun` test left intact as the integration check. |
| New `tools/` file accidentally ships to `dist` | `tsconfig.build.json` includes only `src`; `verify` ends with `rm -rf dist`; no change needed, but note it. |
| Hidden importers break on moved symbols | Only `test/parity-rate.test.ts` imports the tool (repo-wide grep); `export *` re-export preserves the surface regardless. |
| Scope creep into `src/` or prompts | This is a tooling/test-only refactor; no `src/` or `.adw/` edits; `pack:check` unaffected. |

## 11. Out of scope

- Any change to `src/` (orchestrator, run-phase, phases, state, runners).
- Any change to prompt-pack sources or generated `.adw/prompts`.
- New CLI flags, output format changes, or new verdict semantics.
- Shipping the tool to `dist` or wiring it into `verify`.
