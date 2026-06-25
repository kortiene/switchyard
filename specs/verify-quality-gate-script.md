# Spec: Single `verify` quality-gate script

- **Issue:** #2 — `ci: add single `verify` quality-gate script`
- **Labels:** `issue_class:ci`, `adw-live-batch`
- **Intended `issue_class`:** `ci`
- **Planned ADW run mode:** native
- **Type:** CI / developer-tooling. One `package.json` script (already drafted) +
  one README documentation edge. No production source code changes.
- **Owning artifact:** `adw_sdlc/package.json` (`scripts.verify`) +
  `adw_sdlc/README.md` (`## Development`).

---

## 1. Background & context

The ADW test gate and finalize gates do **not** run through a shell. The
orchestrator splits the configured command with `shellSplit()` and invokes
`spawnSync(bin, args)` directly (no shell) — see
`adw_sdlc/docs/LIVE-RUN-BATCH.md` lines 13–22 and `src/orchestrator.ts`
(`resolveLoop` ~line 418, `finalizeAndMerge` ~line 840) plus `src/common.ts`
`shellSplit`. Consequently a chained `ADW_TEST_CMD="a && b"` would pass `&&` as a
literal argument and fail. The live-run batch therefore requires **one** command:

```bash
ADW_TEST_CMD="npm run verify"
```

`npm run verify` chains the gates internally via npm so a single command exercises
the full local/CI quality bar.

### Relevant facts verified in-repo (do not re-derive — confirm)

- **The `verify` script already exists.** `adw_sdlc/package.json:15`:

  ```json
  "verify": "npm run typecheck && npm run lint:env && npm run pack:check && npm test && npm run build && rm -rf dist",
  ```

  This **already matches** the issue's intended chain exactly:
  `typecheck → lint:env → pack:check → test → build → rm -rf dist`.
  The issue Notes confirm this: *"A draft `verify` script may already be present
  from planning; this issue ratifies and documents it."* So the script work is a
  **ratification/verification**, not authoring from scratch.

- **The README does NOT yet mention `verify`.** `adw_sdlc/README.md` `## Development`
  (lines 126–140) lists the five gates as separate commands
  (`typecheck`, `lint:env`, `test`, `pack:check`, `build`) but never names
  `npm run verify` as the canonical single gate. This is the **primary outstanding
  gap** for the acceptance criteria.

- **There is no root `README.md`.** Confirmed (`ls README.md` at repo root → none).
  The only README with a "Development" section is `adw_sdlc/README.md`, so that is
  the documentation target.

- **`docs/LIVE-RUN-BATCH.md` already documents** the single-command gate and the
  `typecheck → … → rm -rf dist` chain (lines 24–28, 75). It is the canonical
  operator doc for the live runs; the README edit should point operators at it,
  not duplicate the batch table.

- **The constituent scripts all exist** (`adw_sdlc/package.json:11–18`):
  `typecheck` (`tsc --noEmit`), `lint:env` (`bash ../scripts/check-adw-sdlc-env.sh`),
  `pack:check` (`tsx tools/pack-generate.ts --check`), `test` (`vitest run`),
  `build` (`tsc -p tsconfig.build.json`, `outDir: dist`).

- **Test precedents for asserting on metadata exist.** `scaffold.test.ts:12–13`
  already reads and asserts on `package.json` invariants, and issue #1 shipped
  `test/mvp-readiness-doc.test.ts`, which asserts on documentation prose. A small
  guard test for issue #2 would mirror both established patterns (see §7).

---

## 2. Goal

Ratify a single `npm run verify` quality gate (the chain
`typecheck → lint:env → pack:check → test → build → rm -rf dist`) that exits
non-zero if any stage fails and leaves no `dist/` build artifact behind on the
success path, and document `verify` in `adw_sdlc/README.md` as the canonical
local/CI gate so operators know to set `ADW_TEST_CMD="npm run verify"`.

---

## 3. Scope

### In scope
- **Ratify** `scripts.verify` in `adw_sdlc/package.json`: confirm it chains
  `typecheck → lint:env → pack:check → test → build → rm -rf dist`, fail-fast via
  `&&`, ending with `rm -rf dist`. (Already present and correct — verify, don't
  rewrite. Only change it if it has drifted from this chain.)
- **Document** `verify` in `adw_sdlc/README.md` `## Development` as the canonical
  single local/CI gate, naming the `ADW_TEST_CMD="npm run verify"` usage and
  linking `docs/LIVE-RUN-BATCH.md` for the live-run rationale.
- *(Recommended, proportionate)* add a tiny guard test asserting the `verify`
  script exists and chains the expected stages, and that the README names it
  (see §7).

### Out of scope
- No changes to the constituent scripts (`typecheck`, `lint:env`, `pack:check`,
  `test`, `build`) or to their underlying tooling/config (`tsconfig*.json`,
  `vitest`, `scripts/check-adw-sdlc-env.sh`, the pack generator).
- No changes to `src/`, `.adw/` prompts/config, or the prompt pack.
- No new CI workflow files (no `.github/workflows` change is requested by the
  issue; `verify` is the command CI/operators call, not a CI runner definition).
- No actual live ADW runs (operator-driven; out of scope for this spec).
- No cross-platform rewrite of `rm -rf dist` unless the maintainer wants Windows
  support (flagged as an open question in §12; the repo already depends on `bash`
  via `lint:env`, so POSIX-only is the current baseline).

---

## 4. Files to change

| File | Change |
| --- | --- |
| `adw_sdlc/package.json` | **Verify** `scripts.verify` matches the canonical chain; edit **only** if it has drifted. (Currently correct at line 15 — expected to be a no-op.) |
| `adw_sdlc/README.md` | In `## Development`, add `verify` as the canonical single local/CI gate, show `ADW_TEST_CMD="npm run verify"`, and link `docs/LIVE-RUN-BATCH.md`. |
| `adw_sdlc/test/scaffold.test.ts` *(recommended)* | Extend with a guard asserting `scripts.verify` chains the expected stages. *(Alternative: a new `test/verify-gate-doc.test.ts` mirroring `mvp-readiness-doc.test.ts`.)* |

No other files are touched. In particular, **no prompt-pack source changes**, so
`pack:check` must not require a regenerate.

---

## 5. Implementation steps

### Step 1 — Ratify `scripts.verify` (expected no-op)

Open `adw_sdlc/package.json` and confirm line 15 reads exactly:

```json
"verify": "npm run typecheck && npm run lint:env && npm run pack:check && npm test && npm run build && rm -rf dist",
```

Checklist:
- Stage order is `typecheck → lint:env → pack:check → test → build → rm -rf dist`.
- Stages are joined with `&&` (fail-fast: any non-zero stage aborts the chain and
  propagates the non-zero exit).
- The final stage is `rm -rf dist` so no build artifact is left on success.

If — and only if — it has drifted from this chain, correct it. Do **not**
gratuitously reformat the file or reorder unrelated scripts. Expected outcome:
**no change to `package.json`** (the script already satisfies the issue).

### Step 2 — Document `verify` in the README `## Development` section

In `adw_sdlc/README.md`, update `## Development` (currently lines 126–140). Two
acceptable shapes — pick the one that reads cleanest; recommended is **(a)**:

**(a) Lead with `verify` as the canonical gate, keep the individual commands as
the breakdown.** Insert, near the top of the section, a short paragraph + block:

````markdown
`npm run verify` is the **canonical local/CI quality gate**. It runs every check
below in order and exits non-zero if any fails, removing the `dist/` build
artifact at the end:

```bash
npm run verify   # typecheck → lint:env → pack:check → test → build → rm -rf dist
```

ADW live runs use it as the single test command (the gate is shell-split and run
without a shell, so a chained `a && b` will not work — one command is required):

```bash
ADW_TEST_CMD="npm run verify"
```

See [`docs/LIVE-RUN-BATCH.md`](./docs/LIVE-RUN-BATCH.md) for the live-run rationale
and command templates.
````

Keep the existing per-gate list (lines 128–134) immediately below it as the
"what each stage does" breakdown. Requirements for the edit:
- The literal string `npm run verify` appears in `## Development`.
- The word **canonical** (or equivalent unambiguous phrasing) ties `verify` to the
  local/CI gate, satisfying AC "README mentions `verify` as the canonical local/CI
  gate."
- A relative link to `./docs/LIVE-RUN-BATCH.md` is present (consistency with the
  issue #1 playbook and avoids duplicating the batch table).
- The `ADW_TEST_CMD="npm run verify"` single-command form is shown.

Do not delete the existing five-gate breakdown — it documents what each stage does
and is still accurate.

### Step 3 — (Recommended) add a guard test

To mirror the repo's existing convention (`scaffold.test.ts` already asserts
`package.json` invariants; issue #1 shipped a doc-assertion test), add a focused
guard so the gate cannot silently regress. **Preferred:** extend
`adw_sdlc/test/scaffold.test.ts` with one `it(...)` that:

1. Reads `package.json` (the file already does this at line 13).
2. Asserts `pkg.scripts.verify` is a string.
3. Asserts it references each stage in order — e.g. checks that the substrings
   `typecheck`, `lint:env`, `pack:check`, `test`, `build`, and `rm -rf dist`
   appear, and that `index('build') < index('rm -rf dist')` (build precedes the
   cleanup) and `index('typecheck')` is the first stage. Keep the assertion
   resilient (substring/order checks), not an exact full-string equality that
   would break on any benign whitespace change.

Optionally, a second assertion (or a small `test/verify-gate-doc.test.ts`
modelled on `test/mvp-readiness-doc.test.ts`) reads `adw_sdlc/README.md` and
asserts the `## Development` section contains `npm run verify`. This directly
guards AC #3.

Keep tests minimal and dependency-free (`node:fs` + `vitest`, as the existing
tests do). Do **not** add a test that actually executes `npm run verify` (it would
recursively run the suite/build and is disproportionate).

### Step 4 — Self-check the edits

Before handing off, confirm:
- `package.json` `scripts.verify` matches the canonical chain (Step 1).
- `adw_sdlc/README.md` `## Development` contains `npm run verify`, the word
  "canonical" (or equivalent), the `ADW_TEST_CMD="npm run verify"` form, and a
  relative link to `./docs/LIVE-RUN-BATCH.md`.
- The diff touches only the files in §4 (no `src/`, no `.adw/` prompt sources).

### Step 5 — Run the gate

From `adw_sdlc/`:

```bash
npm run verify
```

Expect it green and expect `dist/` to be **absent** afterward. Because no
prompt-pack source changed, `pack:check` must not flag drift and no
`npm run pack:generate` is needed. (If `pack:check` fails, an out-of-scope edit
slipped in — revert it.) If the recommended guard test was added, it runs as part
of the `test` stage of `verify`.

---

## 6. Acceptance criteria

Mapped directly from the issue:

- [ ] **`npm run verify` runs all gates and exits non-zero if any fails.** The
  `&&` chain is fail-fast; a non-zero stage aborts and propagates. (Confirm by
  inspection + a green run; optionally a transient local check that an injected
  failure aborts — do not commit any such injection.)
- [ ] **`dist/` is removed at the end** (no build artifact left behind) — the
  chain ends with `rm -rf dist`; confirm `dist/` is absent after a green run.
- [ ] **README mentions `verify` as the canonical local/CI gate** — present in
  `adw_sdlc/README.md` `## Development`.
- [ ] *(spec-added)* `npm run verify` stays green and the diff is limited to §4
  files (no prompt-pack drift).

---

## 7. Test & verification strategy

- **Primary gate:** `npm run verify` from `adw_sdlc/` (it is itself the artifact
  under test: `typecheck → lint:env → pack:check → test → build → rm -rf dist`).
- **`dist/` removal check:** after a green `verify`, confirm `dist/` does not
  exist (e.g. `test ! -e dist && echo clean`).
- **Fail-fast check (manual, do not commit):** temporarily make one early stage
  fail (e.g. introduce a type error) and confirm `verify` exits non-zero **before**
  later stages run; then revert. This validates AC #1 without shipping anything.
- **Guard test (recommended, committed):** the `scaffold.test.ts` extension (and/or
  README doc test) asserts the script chain and the README mention so a future
  edit that drops a stage or the doc reference fails CI. This mirrors the existing
  `scaffold.test.ts` `package.json` assertions and the issue #1 doc-assertion test —
  it is the proportionate, in-convention way to lock the acceptance criteria.
- **No execution-based test of `verify` itself** (would recurse into build/tests
  and is disproportionate).

---

## 8. Risks & mitigations

| Risk | Likelihood | Mitigation |
| --- | --- | --- |
| `rm -rf dist` is POSIX-only; fails on Windows `cmd`/PowerShell | Low (repo already requires `bash` via `lint:env`) | Document POSIX/`bash` as the supported dev shell; defer a cross-platform `rimraf`/`node -e` rewrite unless Windows support is requested (open question §12). |
| `build` fails mid-way, leaving a partial `dist/` because `&& rm -rf dist` never runs | Low | Acceptable: AC scopes "removed at the end" to the success path; a failed `verify` already signals a broken state the dev must fix. Optionally note that a clean re-run removes it. Do not switch to `;`-joining (that would mask build failures). |
| README edit duplicates `docs/LIVE-RUN-BATCH.md` and the two drift | Medium | Keep the README entry a thin pointer (one block + a link); the batch doc stays the single source of truth for templates/cost/failure drills. |
| Accidental prompt-pack source edit triggers `pack:check` drift | Low | Scope strictly to §4 files; verify the diff name-list in Step 4. |
| Guard test over-specifies (exact-string match) and breaks on benign formatting | Low | Use substring + ordering assertions, not full-string equality (§5 Step 3). |
| Running `build` inside the live ADW test gate adds latency/cost per resolve loop | Low/known | Intended trade-off — the gate must prove the change compiles and ships clean; documented in `LIVE-RUN-BATCH.md`. No mitigation needed. |

This is a low-risk CI item; the script is already present, so most "risk" is in the
docs edge and avoiding scope creep.

---

## 9. Rollout / rollback

- **Rollout:** a docs edit (+ optional one-line script ratification + guard test);
  no migration, no flag, no runtime behavior change. Operators can immediately use
  `ADW_TEST_CMD="npm run verify"` (and already can, since the script exists).
- **Rollback:** revert the README/test changes; the `verify` script can remain (it
  predates this issue). Nothing downstream hard-depends on the README prose.
- **Live-run linkage:** issue #2 is run #2 in `docs/LIVE-RUN-BATCH.md`, planned in
  **native** mode; the live run is operator-driven and **out of scope for this
  spec's implementation**.

---

## 10. Key decisions

1. **Treat the script as already-landed; the deliverable is documentation + a
   guard.** The issue explicitly ratifies a pre-existing draft, and the script at
   `package.json:15` already matches the intended chain. Authoring is unnecessary;
   over-editing `package.json` risks churn.
2. **Target `adw_sdlc/README.md` (not a root README, which does not exist).** The
   only "Development" section lives there.
3. **README entry is a thin pointer to `docs/LIVE-RUN-BATCH.md`,** not a copy of the
   batch table — single source of truth for run templates stays the batch doc.
4. **Add a small substring/order guard test** rather than an execution test —
   mirrors `scaffold.test.ts` and the issue #1 doc test, locks the ACs, and avoids
   recursive build/test cost.
5. **Keep `rm -rf dist` POSIX-only for now.** The repo already requires `bash`
   (`lint:env`), so this matches the existing dev-environment assumption; a
   cross-platform rewrite is deferred unless requested.
6. **Keep `&&` (fail-fast), not `;`.** Stage failures must abort and surface a
   non-zero exit — this is the core of AC #1.

---

## 11. Assumptions

- The supported developer/CI shell is POSIX (`bash`/`zsh`); Windows is not a target
  for the gate today (consistent with `lint:env` calling `bash`).
- `dist/` is produced **only** by the `build` stage and is the only artifact the
  cleanup must remove (`tsconfig.build.json` `outDir: dist`).
- No prompt-pack source is touched, so `pack:check` stays green without a
  regenerate.
- Adding a small guard test under `test/` is welcome (the repo already keeps
  metadata/doc-assertion tests); if the maintainer prefers zero test changes, Steps
  1–2 alone satisfy the issue's stated ACs.
- The README may freely reference `ADW_TEST_CMD` even though that env knob is owned
  by the orchestrator (it is already referenced in README "Quick start" and the
  flags table).

---

## 12. Open questions

1. **Cross-platform `dist/` cleanup?** Should `rm -rf dist` be replaced with a
   portable form (e.g. `node -e "fs.rmSync('dist',{recursive:true,force:true})"` or
   a `rimraf` devDependency) so `verify` runs on Windows? Default recommendation:
   **no** for this issue (keep POSIX, matching `lint:env`); revisit only if Windows
   dev support becomes a goal.
2. **Guard test placement** — extend `scaffold.test.ts` vs. a new
   `test/verify-gate-doc.test.ts` (modelled on `mvp-readiness-doc.test.ts`).
   Recommendation: extend `scaffold.test.ts` for the script assertion; add the
   small doc-assertion either there or in a sibling file. Implementer's choice.
3. **Does CI (a `.github/workflows/*`) need to call `verify` explicitly?** The
   issue asks only for the script + README; no workflow file is requested and the
   repo has no committed remote/CI yet (`LIVE-RUN-BATCH.md` Preflight: "this
   checkout has none yet"). Out of scope here; flag as a possible follow-up when CI
   is wired up.
4. **Partial-`dist` on a failed `build`** — acceptable per AC (success-path only),
   but should the README note "a failed verify may leave a partial dist; re-run
   after fixing"? Minor; include only if it reads cleanly.
