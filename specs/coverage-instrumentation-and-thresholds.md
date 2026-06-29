# Spec: Test-coverage instrumentation + modest thresholds (vitest v8)

- **Issue:** #36 — `Add test-coverage instrumentation + thresholds (none today)`
- **Labels:** `issue_class:ci`, `backlog`, `area:ci`
- **Intended `issue_class`:** `ci`
- **Planned ADW run mode:** native
- **Type:** CI / developer-tooling. Coverage instrumentation config + one
  devDependency + npm-script/gate wiring + a guard test + doc edges. **No
  production source (`src/`) changes.**
- **Owning files:** `adw_sdlc/vitest.config.ts` (coverage block),
  `adw_sdlc/package.json` (devDependency + `coverage` script + `verify` chain),
  `adw_sdlc/package-lock.json` (new dep), `.gitignore` (coverage output),
  `adw_sdlc/test/scaffold.test.ts` (+ a coverage guard test),
  `.github/workflows/verify.yml` (chain comment), `adw_sdlc/README.md`
  (`## Development`), `adw_sdlc/HANDOVER.md` (session entry).

---

## 1. Background & current state (read this first)

The issue's two evidence points are **partially stale** — confirm before acting:

- *"`adw_sdlc/package.json (no --coverage)`"* — **still true.**
  `package.json:13` is `"test": "vitest run"` (no `--coverage`), and there is no
  `coverage` script. `verify` (`package.json:15`) runs `npm test` with no
  coverage stage.
- *"`no vitest.config`"* — **now false.** `adw_sdlc/vitest.config.ts` **exists**
  (added later) and currently only inlines `@openai/codex-sdk` for a spawn test:

  ```ts
  // adw_sdlc/vitest.config.ts (current)
  import { defineConfig } from 'vitest/config';
  export default defineConfig({
    test: { server: { deps: { inline: ['@openai/codex-sdk'] } } },
  });
  ```

  So the work is **extending** an existing config, not creating one.

### Facts verified in-repo (do not re-derive — confirm)

- **No coverage tooling anywhere.** `@vitest/coverage-v8` is **not installed**
  (`node_modules/@vitest/` contains only vitest's own subpackages — `expect`,
  `runner`, `spy`, etc. — not `coverage-v8`). No `coverage` script, no
  `test.coverage` block, no `coverage/` output dir. The "coverage" hits across
  the repo are unrelated prose (test-breadth, `rest-transport-loopback-coverage`).
- **Vitest is pinned at `^4.1.8`** (`package.json:38`, `devDependencies`),
  resolved to **`4.1.9`** in `node_modules`. The coverage provider package
  **must match the vitest major/minor** — install `@vitest/coverage-v8@^4.1.8`
  (Vitest requires the coverage package version to track the core version). The
  committed `package-lock.json` already declares `@vitest/coverage-v8` as one of
  vitest's **optional peer dependencies at `4.1.9`**, so `npm install` will
  resolve exactly that compatible version — confirming `^4.1.8` (→ `4.1.9`) is
  the right range.
- **The canonical gate chain is pinned in three places** that move together:
  - `package.json:15` `verify` =
    `typecheck → lint:env → pack:check → mirror:check → npm test → build → rm -rf dist`.
  - `adw_sdlc/test/scaffold.test.ts:35-94` asserts (a) every stage appears as a
    substring (`:52`, incl. `npm test` at `:56`), (b) `&&` fail-fast (`:59-63`),
    (c) canonical **ordering** (`:65-74`, incl. `mirror:check < npm test < npm run build`),
    (d) ends with `rm -rf dist` (`:76`), (e) every `npm run <stage>` it references
    is a defined script (`:80-93`). **Any change to the `verify` chain must keep
    this test green.**
  - `.github/workflows/verify.yml:5-6,51` carries the chain as a comment; CI just
    runs `npm run verify` (a **required status check** on `main`).
- **`scaffold.test.ts:25-31` pins the exact key sets of `dependencies` and
  `optionalDependencies`** but says nothing about `devDependencies`. ⇒ the
  coverage package **must be a `devDependency`**; adding it to `dependencies` or
  `optionalDependencies` would break that test.
- **`tsconfig.json` includes `src`, `test`, `tools`;** `tsconfig.build.json`
  emits only `src` → `dist`. Coverage config/output is not built; no build
  change needed.
- **`coverage/` is not git-ignored.** Root `.gitignore` ignores `node_modules/`,
  `dist/`, `agents/`, `.DS_Store` — not `coverage/`. The v8 reporter writes a
  `coverage/` dir; it must be ignored.
- **There are 28 `src/*.ts` files**; `src/index.ts` is a barrel of re-exports.
  The suite is large and fully mocked (`verify.yml:46-47`: no API keys/network).
  HANDOVER baseline: **638 tests across 46 files** (`HANDOVER.md:1557,1766`).
- **The repo already keeps metadata/doc guard tests** (`scaffold.test.ts` asserts
  `package.json`/README invariants; `handover-doc.test.ts`,
  `mvp-readiness-doc.test.ts`, `observed-live-ledger-doc.test.ts` assert doc
  prose). A coverage guard test fits this established convention.

---

## 2. Goal & acceptance criteria

### Goal
Make undertested code **trip the canonical gate**: enable Vitest v8 coverage with
a modest, measured threshold, configured so a brand-new untested file/branch
counts against coverage (not silently ignored), and wire it into `npm run verify`
(the command CI and the ADW orchestrator already run).

### Acceptance criteria (from the issue)
- [ ] **Vitest v8 coverage enabled with a modest threshold** — `@vitest/coverage-v8`
      installed, `test.coverage` configured (`provider: 'v8'`) with
      `coverage.thresholds` set to a modest, currently-passing bar.
- [ ] **Wired into `npm run verify` or a parallel job** — running the canonical
      gate enforces the thresholds and exits non-zero when coverage drops below
      them.

### Additional acceptance criteria (this spec)
- [ ] **Untested files count.** Coverage `include` is `src/**/*.ts` so a new
      source file with no test reports as 0% and drags the metric down (the exact
      "an untested new branch would not trip the gate" gap the issue names).
- [ ] **Focused dev runs don't false-fail.** `coverage.enabled` stays default
      (false), so a partial run (`npx vitest run test/foo.test.ts`) does **not**
      collect coverage or enforce thresholds — only the explicit coverage run does.
- [ ] **The chain-guard test (`scaffold.test.ts`) stays green** and is updated to
      assert the coverage stage where the chain changed.
- [ ] **`coverage/` is git-ignored**; no coverage output is committed.
- [ ] **`npm run verify` stays green** on the current suite with the chosen
      threshold, and the diff touches only the §4 files (no `src/` runtime change,
      no prompt-pack drift → `pack:check`/`mirror:check` unaffected).

---

## 3. Recommended design (primary)

Add a dedicated **`coverage`** npm script (`vitest run --coverage`) and make it
the **test stage of `verify`**, with thresholds declared in `vitest.config.ts`.
This is one extra dependency, one config block, a script swap, and the guard/doc
edges — matching how `pack:check` / `mirror:check` are first-class, dedicated gate
stages rather than hidden flags.

Rationale for "in `verify`" over a separate parallel CI job (the issue allows
either): `verify` is the single canonical gate that **CI runs as a required check
and the ADW orchestrator runs in its resolve loop and pre-merge gate**. Folding
coverage in means an ADW phase that adds an undertested branch turns the gate red
in the same loop that can add the missing tests — which is exactly the issue's
intent. The full suite always runs via `vitest run`, so coverage over the whole
suite is stable (the focused-run footgun, §6, is avoided by leaving
`coverage.enabled` false). See §5 for the parallel-job alternative if the team
prefers to keep coverage off the inner resolve loop.

### 3.1 `vitest.config.ts` — add the `coverage` block

Extend the existing config (keep the `server.deps.inline` entry intact):

```ts
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    server: {
      deps: {
        // (unchanged) runner-codex-spawn.test.ts needs the SDK inlined so its
        // child_process mock applies inside the SDK.
        inline: ['@openai/codex-sdk'],
      },
    },
    coverage: {
      provider: 'v8',
      // enabled stays default (false): only `vitest run --coverage` (the
      // `coverage` script / verify stage) collects + enforces. Focused
      // `npx vitest run <file>` stays coverage-free and never false-fails.
      include: ['src/**/*.ts'],
      // Measure real logic; don't pad the number by excluding source. Keep
      // excludes to the barrel + type decls. (Revisit per the measured baseline.)
      exclude: ['src/index.ts', '**/*.d.ts'],
      reporter: ['text-summary', 'text', 'html'],
      reportsDirectory: './coverage',
      thresholds: {
        // MODEST, MEASURED floor — set from the baseline in Step 2, a few points
        // below current so normal churn doesn't flake the gate. Placeholder
        // fallback if not measuring: 60 / 60 / 55 / 60.
        lines: 60,
        functions: 60,
        branches: 55,
        statements: 60,
        autoUpdate: false, // never silently lower the bar
      },
    },
  },
});
```

Key choices:
- **`provider: 'v8'`** — required by the AC ("vitest v8 coverage"); native V8
  coverage, low overhead.
- **`include: ['src/**/*.ts']`** — measures the control plane only (not `test/`
  or `tools/`) **and** makes Vitest report files no test imports as 0%, so a new
  untested module counts against the threshold (the AC's core point). Confirm the
  installed Vitest's "all files in `include` are reported even when untested"
  behavior during Step 2 — if the pinned version requires `coverage.all: true`
  for that, add it.
- **Global (not `perFile`) thresholds** — "modest" per the issue; a single
  whole-project floor, not a per-file bar that would be brittle on a 28-file tree.
- **`autoUpdate: false`** — the floor only ratchets up by deliberate edit.

### 3.2 `package.json` — devDependency, `coverage` script, `verify` wiring

1. **Add the coverage provider to `devDependencies`** (NOT deps/optionalDeps —
   §1 / `scaffold.test.ts:25-31`), version matched to vitest:

   ```jsonc
   "devDependencies": {
     "@types/node": "^22.10.0",
     "@vitest/coverage-v8": "^4.1.8",   // ← new; track the vitest version
     "tsx": "^4.22.4",
     "typescript": "^5.9.3",
     "vitest": "^4.1.8"
   }
   ```

   This **requires `npm install` to update `package-lock.json`** (CI runs
   `npm ci`, which fails on a lockfile that omits the dep). See §11/§12 — this
   needs network and cannot be done in a no-network ADW phase.

2. **Add a `coverage` script** and **use it as the `verify` test stage**
   (keep a coverage-free `npm test` for fast focused/full runs):

   ```jsonc
   "scripts": {
     "test": "vitest run",
     "coverage": "vitest run --coverage",
     "verify": "npm run typecheck && npm run lint:env && npm run pack:check && npm run mirror:check && npm run coverage && npm run build && rm -rf dist"
   }
   ```

   New canonical chain:
   `typecheck → lint:env → pack:check → mirror:check → coverage → build → rm -rf dist`.
   (The `npm test` stage is replaced by `npm run coverage`; coverage runs the
   full suite once — no double run.)

### 3.3 `.gitignore` — ignore coverage output

Add under the build-output section of the **root** `.gitignore`:

```gitignore
# Test-coverage output (vitest v8 reporter; reportsDirectory: ./coverage)
coverage/
```

(`verify` already ends with `rm -rf dist`; `coverage/` is git-ignored rather than
rm'd so a developer can open `coverage/index.html` after a run.)

### 3.4 Tests — update the chain guard + add a coverage guard

The `verify` chain changed (`npm test` → `npm run coverage`), so
`scaffold.test.ts:35-94` **must** be updated, and a focused guard should lock the
new coverage config so it can't silently regress.

**Edit `test/scaffold.test.ts`** (the `verify` block, `:50-74`):
- Required-stages list (`:52`): replace the bare `'build'`/`'rm -rf dist'`
  expectations as needed and ensure the set includes `'coverage'`. Drop the
  separate `npm test` assertion (`:56`) **or** change it to assert
  `verifyScript` contains `npm run coverage` (the chain no longer references
  `npm test`).
- Ordering (`:65-74`): replace `idx('mirror:check') < idx('npm test')` and
  `idx('npm test') < idx('npm run build')` with
  `idx('mirror:check') < idx('npm run coverage')` and
  `idx('npm run coverage') < idx('npm run build')`.
- The "all `npm run <stage>` references point to defined scripts" test (`:80-93`)
  needs no change beyond `coverage` existing in `scripts` (it will).

**Add a coverage guard test** — preferred placement: a new
`test/coverage-config.test.ts` (mirrors `mvp-readiness-doc.test.ts` style; keep it
`node:fs` + `vitest`, dependency-free). Assert:
1. `package.json` `devDependencies['@vitest/coverage-v8']` is present (string).
2. `package.json` `scripts.coverage` exists and contains `--coverage`.
3. `scripts.verify` references `npm run coverage`.
4. `vitest.config.ts` (read as text) contains `coverage`, `provider`, `'v8'`,
   `thresholds`, and `include` — i.e. the block is wired with a provider and a
   threshold. *(Text assertions match the repo's doc-test convention and avoid
   importing the `defineConfig` module; alternatively import the default export
   and assert `config.test.coverage.provider === 'v8'` and that
   `config.test.coverage.thresholds` is a non-empty object — implementer's choice.)*

Do **not** add a test that executes `npm run coverage`/`npm run verify` (it would
recurse into the suite). The `coverage` run is already exercised end-to-end when
`verify` runs.

### 3.5 Docs & comments to update

- `adw_sdlc/README.md` `## Development`:
  - The chain comment (`README.md:143`,
    `# typecheck → lint:env → pack:check → mirror:check → test → build → rm -rf dist`)
    → reflect `coverage` in place of `test`.
  - The per-gate breakdown (`README.md:159-167`): replace/augment the
    `npm test` line and add a short `npm run coverage` bullet naming the v8
    provider and the modest threshold, plus `npm test` remaining as the
    coverage-free full run. (Keep `scaffold.test.ts:96-124`'s README assertions
    green — they only check for `npm run verify`, `canonical`,
    `ADW_TEST_CMD="npm run verify"`, and a `LIVE-RUN-BATCH.md` link, none of which
    this edit removes.)
- `.github/workflows/verify.yml:5-6,51` — the chain comment
  (`… pack:check -> mirror:check -> test -> build -> clean`) → reflect `coverage`.
  **No job-logic change** (CI still runs `npm run verify`); confirm the runner has
  the dep via `npm ci` (it will, once the lockfile is updated).
- `adw_sdlc/HANDOVER.md` — add a session entry in the established format (an
  `## 8x. Issue #36 — test-coverage instrumentation` block) noting: v8 coverage
  added, modest threshold value chosen and how (measured baseline), `coverage`
  script wired into `verify` replacing `npm test`, `coverage/` git-ignored, and
  `npm run verify` stays green with the **new test/file counts** the suite reports
  after the guard test is added (baseline was 638/46). Keep `handover-doc.test.ts`
  green (it checks the trailing verify-line format / counts — update those to the
  observed numbers).

---

## 4. Files to change

| File | Change |
| --- | --- |
| `adw_sdlc/vitest.config.ts` | Add `test.coverage` block (provider v8, include, exclude, reporter, thresholds). Keep existing `server.deps.inline`. |
| `adw_sdlc/package.json` | Add `@vitest/coverage-v8` to **devDependencies**; add `scripts.coverage`; swap `npm test` → `npm run coverage` in `scripts.verify`. |
| `adw_sdlc/package-lock.json` | Regenerated by `npm install` (new dep tree). |
| `.gitignore` | Add `coverage/`. |
| `adw_sdlc/test/scaffold.test.ts` | Update `verify`-chain stage/order assertions for `coverage`. |
| `adw_sdlc/test/coverage-config.test.ts` *(new)* | Guard: dep present, `coverage` script, `verify` references it, config has provider+thresholds. |
| `.github/workflows/verify.yml` | Update the chain **comment** only. |
| `adw_sdlc/README.md` | `## Development`: chain comment + `coverage` bullet. |
| `adw_sdlc/HANDOVER.md` | Session entry + verify-line counts. |

**Not touched:** any `src/*.ts` (no runtime change), `.adw/`/`.pi/`/`.claude/`
prompt sources (so `pack:check`/`mirror:check` stay green without regen),
`tsconfig*.json`, `scripts/check-adw-sdlc-env.sh`.

---

## 5. Step-by-step implementation

> **Order matters:** install the dep first, **measure** the real baseline, then
> set the threshold to a modest floor under it — do not guess the number.

### Step 1 — Install the coverage provider (network required)
From `adw_sdlc/`:
```bash
npm install --save-dev @vitest/coverage-v8@^4.1.8   # match the pinned vitest version
```
Confirm `package.json` `devDependencies` and `package-lock.json` both updated, and
that the installed version matches `vitest` (mismatched majors error at runtime).

### Step 2 — Add the coverage config and MEASURE the baseline
Apply the `vitest.config.ts` block from §3.1 but **with thresholds omitted or set
to 0**, then run:
```bash
npx vitest run --coverage
```
Read the `text-summary` (and `text`) output. Record the global
lines/functions/branches/statements %. Sanity-check that `include: ['src/**/*.ts']`
makes **all** src files appear (untested ones at 0%) — if they don't, add
`coverage.all: true` and re-run.

### Step 3 — Set the modest threshold floor
Set `coverage.thresholds` a few points **below** the measured baseline (so routine
churn won't flake the gate) but high enough to catch a wholly untested new module.
"Modest" per the issue → a single global floor, not per-file. If you prefer a flat
bar without tuning, the fallback `60/60/55/60` is reasonable; never set a value
the current suite can't pass.

### Step 4 — Add the `coverage` script and wire `verify`
Apply the `package.json` changes from §3.2 (add `scripts.coverage`; swap
`npm test` → `npm run coverage` in `verify`).

### Step 5 — Ignore coverage output
Add `coverage/` to the root `.gitignore` (§3.3). Verify a coverage run leaves the
working tree clean (`git status` shows no `coverage/`).

### Step 6 — Update tests
Update `scaffold.test.ts` chain assertions and add `test/coverage-config.test.ts`
(§3.4). Run them focused first:
```bash
npx vitest run test/scaffold.test.ts test/coverage-config.test.ts
```

### Step 7 — Update docs/comments
README `## Development`, `verify.yml` chain comment, HANDOVER entry + counts (§3.5).

### Step 8 — Run the full gate
```bash
npm run verify
```
Expect green, `dist/` absent afterward, `coverage/` present but git-ignored, and no
`pack:check`/`mirror:check` drift (no prompt source changed). Update the HANDOVER
test/file counts to whatever the suite now reports.

---

## 6. Alternatives considered

- **A — Coverage as a separate parallel CI job, `verify` unchanged.** Keep
  `verify` as-is; add a `coverage` script and a second job/step in `verify.yml`
  (or a sibling workflow) that runs `npm run coverage`, made a required check.
  *Pros:* keeps coverage-threshold flakiness **off the ADW inner resolve loop**
  (the orchestrator's `ADW_TEST_CMD="npm run verify"` wouldn't enforce coverage);
  zero change to `scaffold.test.ts`/`verify` chain. *Cons:* runs the suite twice
  (plain in `verify`, again with coverage), and coverage no longer gates the
  resolve loop where missing tests are most cheaply added. **Viable** and
  explicitly permitted by the AC ("or a parallel job") — choose this if the team
  wants coverage decoupled from the inner loop. **Decision hinge:** should
  coverage gate the ADW resolve loop? Primary (§3) says yes.
- **B — `"test": "vitest run --coverage"` (no `coverage` script, no `verify`
  edit).** Smallest diff: `verify`, `verify.yml`, and `scaffold.test.ts` chain all
  stay untouched (still reference `npm test`). *Cons:* every `npm test` (incl. a
  human's full run mid-feature) now enforces thresholds — "tests pass but
  `npm test` is red" is surprising — and there's no coverage-free full-suite
  script. **Rejected as primary** for that UX wrinkle, but it is the lowest-churn
  option if minimizing the diff outweighs the surprise.
- **C — `perFile` thresholds.** Per-file floors catch a single weak file the
  global average hides. *Rejected* as not "modest" — brittle across 28 files and
  noisy for routine edits; revisit later as a ratchet.
- **D — Codecov / external coverage service.** Upload lcov to a SaaS gate.
  *Rejected* — adds a third-party dependency/token and network coupling the repo
  doesn't have; the issue asks only for in-repo instrumentation + a threshold.

---

## 7. Test & verification strategy

- **Primary gate:** `npm run verify` from `adw_sdlc/` — now includes the
  `coverage` stage; it is itself the artifact under test.
- **Focused tests first:** `npx vitest run test/scaffold.test.ts
  test/coverage-config.test.ts`.
- **Threshold enforcement (manual, do not commit):** temporarily set a threshold
  just above the baseline and confirm `npm run coverage` exits non-zero; revert.
  This proves the gate actually trips on undertested code (the issue's core
  worry).
- **Untested-file detection (manual, do not commit):** temporarily add a trivial
  unexercised `src/__scratch.ts` and confirm coverage drops / thresholds fail
  (proving `include` counts untested files); delete it.
- **Clean tree:** after a run, `git status` shows no `coverage/` (git-ignored) and
  `verify` still removes `dist/`.
- **No execution-based test of `verify`/`coverage` themselves** (would recurse).

---

## 8. Risks & mitigations

| Risk | Likelihood | Mitigation |
| --- | --- | --- |
| `@vitest/coverage-v8` version drift vs `vitest` → runtime error | Medium | Pin to the same `^4.1.8` range; install via `npm install` so the lockfile resolves a compatible tree; CI `npm ci` then reproduces it. |
| Lockfile not updated → CI `npm ci` fails | Medium | Step 1 runs `npm install` and commits `package-lock.json`; the change is unmergeable until that's present (this ADW phase can't install — §12). |
| Threshold set too high → flaky/red gate on benign churn | Medium | Measure baseline first (Step 2) and set the floor a few points **below** it (Step 3); global (not per-file); `autoUpdate: false`. |
| Threshold set too low → "passes" while adding nothing of value | Low | `include: ['src/**']` makes untested files count; pick a floor near the real baseline, not a token 1%. |
| Focused dev run (`npx vitest run <file>`) false-fails on coverage | Low | `coverage.enabled` left default (false); only the explicit `--coverage` script collects/enforces. Documented in README. |
| `scaffold.test.ts` chain guard breaks when `npm test`→`npm run coverage` | High (expected) | Updated in the same change (§3.4); run it focused first. |
| `coverage/` committed by accident | Low | Add to `.gitignore` (§3.3); verify `git status` clean. |
| Coverage slows the ADW resolve loop (runs each iteration) | Low | v8 is low-overhead and the suite is fully mocked; if it proves costly, switch to Alternative A (parallel job). |
| `handover-doc.test.ts` red on the new HANDOVER entry/counts | Medium | Update the trailing verify-line counts to the observed numbers; run that doc test. |
| Coverage instrumentation perturbs a spawn/timing-sensitive test | Low | Full suite is mocked and deterministic; if one regresses, exclude that file's *source* target via `coverage.exclude`, not the test. |

This is a low-risk CI item; the main care is (a) the lockfile/network step and
(b) measuring the threshold instead of guessing.

---

## 9. Rollout / rollback

- **Rollout:** add dep + config + script/gate wiring + guard test + docs. No
  runtime behavior change, no migration, no flag. CI picks it up via the existing
  required `verify` check once the lockfile lands.
- **Rollback:** revert `verify` to use `npm test`, remove the `coverage` script,
  the `vitest.config.ts` coverage block, the devDependency, and the guard test.
  Nothing downstream hard-depends on coverage output.
- **Ratcheting:** raise the threshold (or add `perFile`) in a later issue once the
  baseline is comfortably above the floor — out of scope here ("modest" only).

---

## 10. Key decisions

1. **Extend the existing `vitest.config.ts`** (it already exists; the issue's
   "no vitest.config" evidence is stale) — preserve the `server.deps.inline`
   entry; add only the `coverage` block.
2. **`provider: 'v8'`** per the AC; **global modest thresholds**, not per-file.
3. **Wire into `npm run verify`** (replace the `npm test` stage with
   `npm run coverage`) so the single canonical gate — the one CI and the ADW
   orchestrator run — enforces coverage and trips on undertested code. The
   parallel-job route (Alternative A) is the documented fallback if coverage
   should stay off the inner resolve loop.
4. **`coverage.enabled` stays false** so focused dev runs never false-fail; only
   the explicit `--coverage` script collects and enforces.
5. **`include: ['src/**/*.ts']`** so untested files count as 0% — directly closes
   the "untested new branch wouldn't trip the gate" gap.
6. **devDependency, not dep/optionalDep** — keeps `scaffold.test.ts:25-31`'s
   pinned dependency key-sets green.
7. **Measure the baseline, then set the floor under it** — don't guess "modest."
8. **Keep `npm test` (coverage-free)** for fast focused/full runs alongside
   `npm run coverage`.

---

## 11. Assumptions

- The implementing phase **has network** and can run `npm install` to add the dep
  and update `package-lock.json` (the no-network ADW phase that wrote this spec
  cannot — §12).
- `@vitest/coverage-v8@^4.1.8` is published and compatible with `vitest@^4.1.8`
  and Node `>=20.19` (`package.json:8`).
- The current full suite passes a modest threshold (≈55–70% range) without
  rewriting tests; the spec scopes **no** new product tests, only instrumentation.
- Supported dev/CI shell is POSIX (`bash`/`zsh`), consistent with `lint:env` and
  `rm -rf dist`.
- No prompt-pack source changes ⇒ `pack:check`/`mirror:check` stay green without a
  regenerate.
- Adding a small guard test + a HANDOVER entry is welcome (repo convention); if
  the maintainer wants zero test/doc churn, the AC is still met by Steps 1–5 plus
  the `scaffold.test.ts` chain fix (which is mandatory, not optional).

---

## 12. Open questions

1. **Threshold value.** What modest floor do we commit to? Recommendation:
   measure (Step 2) and set a few points below baseline; fallback `60/60/55/60`.
   Needs a maintainer sign-off on the exact numbers once measured.
2. **In `verify` vs parallel CI job.** Primary folds coverage into `verify`
   (gates the ADW resolve loop). Confirm the team wants coverage to gate that
   inner loop; if not, take Alternative A (parallel job, `verify` unchanged).
3. **`coverage.all` / include semantics.** Confirm that on the pinned Vitest 4,
   `include: ['src/**']` alone reports untested files (vs needing `all: true`).
   Set whichever the installed version requires to make a 0%-covered new file
   count.
4. **Exclusions.** Spec excludes only `src/index.ts` (barrel) + `**/*.d.ts`.
   Should hard-to-unit-test entrypoints (`src/cli.ts`) also be excluded, or kept
   in and covered by `cli.test.ts`? Default: keep them in; lower the threshold
   rather than exclude real code.
5. **Network/lockfile in ADW.** Because this needs `npm install`, can the live
   ADW run perform it, or must a maintainer pre-install and commit the lockfile?
   If the runner is offline, this issue's *implementation* (not this spec) needs
   an online step. Flag before scheduling a live run.
6. **Reporters.** Spec emits `text-summary`/`text`/`html`. Add `lcov`/`json-summary`
   now (for a future Codecov/PR-annotation step), or defer until such a consumer
   exists? Default: defer (no consumer today).
