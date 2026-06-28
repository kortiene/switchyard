# Spec — Bring docs current (HANDOVER.md test counts; stale runtime claims)

- **Issue:** #41 — Bring docs current (HANDOVER.md test counts; stale runtime claims)
- **Labels:** `issue_class:docs`, `backlog`
- **Type:** Documentation-only (no production code change)
- **Owning area:** `adw_sdlc/` package docs (`HANDOVER.md`, `MVP-READINESS.md`, optionally `PARITY.md`)
- **Source:** Doc-currency assessment (`PLAN.md` §11 / `MVP-READINESS.md` / `PARITY.md` / `OBSERVED-LIVE-LEDGER.md` / `docs/DESIGN-*.md`)

---

## 1. Objective

Make the package docs match the current repository state on two specific points
the issue raises:

1. **HANDOVER.md test counts are current** — the "resume" verification gate and
   the baseline line must reflect HEAD, not an earlier commit.
2. **MVP-READINESS.md's "no test spawns a real process" claim is softened** —
   it is now false: the suite contains tests that deliberately spawn real
   subprocesses and do a real localhost network round-trip.

This is a docs-accuracy task. No runtime, kernel, prompt-pack, config, or
provider behavior changes.

---

## 2. Background and current state (verified)

### 2.1 Ground-truth measurement

Running the suite from `adw_sdlc/` on the current branch:

```
Test Files  43 passed (43)
      Tests  611 passed (611)
```

**Authoritative current totals: 611 tests across 43 test files.**

> The issue text cites `578/41 → 585/42`. Those numbers are themselves now stale:
> issues #24/#26/#27/#25 landed afterward (`598 → 605 → 606 → 611`). Do **not**
> hardcode the issue's numbers — use the empirically observed totals from the
> **final** working tree of the implementing PR (see §5, step F).

### 2.2 HANDOVER.md drift (three references to fix)

`adw_sdlc/HANDOVER.md` is an append-only narrative log. Most per-section
"`npm run verify` stays green (N tests, M files)" lines are **historical records
of the state when that work landed** and are correct as history — they must
**not** be rewritten. Only the references that claim to describe the *current /
baseline* state are wrong:

| Line | Current text | Problem | Correct value |
|------|--------------|---------|---------------|
| `HANDOVER.md:313` (§8 "Verification gates (run these to resume)") | `# 3) Full test suite (current: 605 tests, 43 files)` | Test count 6 behind HEAD; explicitly labeled "current" | `611 tests, 43 files` |
| `HANDOVER.md:1463` (§8ac, the most recent / HEAD section) | `(**611 tests, 44 files**).` | File count wrong (actual is 43); this is where the "44" file-count drift entered | `611 tests, 43 files` |
| `HANDOVER.md:1671` (§12 "Test count baseline after this session") | `**611 passing across 44 files**` | File count wrong (actual is 43) | `611 passing across 43 files` |

Why the file count is 43, not 44: §8ac (issue #25) added a `describe` block to
the **already-existing** `test/mvp-readiness-doc.test.ts` (it predates #25 — it
also holds the "issue #1 acceptance criteria" block), so no new test file was
created. The last actual file-count increment was §8aa/issue #26 adding
`test/providers-rest-transport.test.ts` (42 → 43). `vitest` confirms 43 files.

After these three edits HANDOVER's "current/baseline" claims are internally
consistent at **611 tests / 43 files** and match ground truth.

### 2.3 MVP-READINESS.md stale runtime claim

`adw_sdlc/MVP-READINESS.md` lines 4–5 (the issue's "line 6") currently read:

> PARITY.md is a checklist of green
> boxes; nearly all of them are **mocked-seam** evidence (every SDK/spawn/gh/git
> effect stubbed — verified: no test in the suite spawns a real process or touches a
> real network).

The parenthetical clause **"verified: no test in the suite spawns a real process
or touches a real network"** is now false. Confirmed real-process / real-network
tests:

- **`test/verify-gate.e2e.test.ts`** — `spawnSync('npm', ['run', 'build'])` and
  `spawnSync('rm', ['-rf', 'dist'])`: real subprocesses + real filesystem effects
  (issue #2 AC2).
- **`test/secret-boundary-audit.test.ts`** — crosses the spawn boundary to assert
  `GH_TOKEN`/`MATRIX_*`/`ADW_*`/`MX_AGENT_*` are absent from a **real spawned
  runner environment** (§8x).
- **`test/providers-rest-transport.test.ts`** — forks a real loopback HTTP server
  (`test/helpers/loopback-server.mjs`) and drives the real `restTransportViaNode`
  path (which `spawnSync`s a `node -e` one-shot fetch) over a **real localhost
  network round-trip** (§8aa, issue #26).

So both halves of the claim ("real process" and "real network") are disproven.

### 2.4 Sibling stale claims (same class, beyond the issue's two boxes)

The same "everything is mocked / no network" assertion appears elsewhere and
will read as inconsistent once MVP-READINESS is softened:

- **`adw_sdlc/PARITY.md:11`** — "every SDK/spawn/`gh`/git effect stubbed. No
  network, no API keys, no native binaries." Present-tense, general — `No
  network` is now technically false (loopback round-trip). **Recommended** to
  soften in the same PR for consistency (see §4.2).
- **`adw_sdlc/HANDOVER.md:1022`** (§8r) — "the **entire 450-test suite is mocked**
  (no test spawns a real process or hits a real network)". This is a **historical**
  past-tense record scoped to the 450-test era, when it was true; the
  real-process tests came later (§8x/§8aa). **Optional** light forward-note; safe
  to leave as dated history (see §4.2).
- `adw_sdlc/PLAN.md:159` and files under `specs/` — out of scope (PLAN is the
  roadmap describing methodology generally; `specs/*` are planning docs, not
  shipped package docs).

### 2.5 The `[MX-ADW]` progress.tag aside

The issue notes "the legacy `[MX-ADW]` progress.tag **may** also warrant
retiring." This is **out of scope for this docs issue** and should be a separate
work item, because it is **not docs-only** — it is behavior/config:

- Default lives in code: `adw_sdlc/src/config.ts:241` (`progress: { tag: '[MX-ADW]' }`)
  and the committed pack `.adw/config.json:29` (`"tag": "[MX-ADW]"`).
- It is the literal tag posted on work-item progress comments
  (`src/exec.ts:59` `formatProgress`), surfaced in CLI help (`src/cli.ts:205`).
- It is pinned by tests: `test/config.test.ts` asserts `config.progress.tag ===
  '[MX-ADW]'` and `formatProgress(...)` output.

Retiring/renaming it requires a name decision (e.g. `[ADW]` / `[SWITCHYARD-ADW]`),
config + default changes, test updates, and is observable in production output —
a behavior change, not a doc edit. See §7 (Open questions) and §9.

### 2.6 Guard tests that read these docs (must stay green; none pin target text)

- `test/mvp-readiness-doc.test.ts` — pins §0 (MVP=(A)), (B)/(C) post-MVP framing,
  the live-run playbook / `LIVE-RUN-BATCH.md` link / `ADW_TEST_CMD`, and §3
  cutover framing. It does **not** assert the line-4/5 "no real process" clause →
  softening is safe.
- `test/parity-evidence.test.ts` — re-derives the parity numbers (8 runs, 36
  native attempts, 0 hard-fails, 88.9% nudge, 5 fenced, 8 classify, 49 phases)
  from the **committed fixture corpus**, not from doc prose → unaffected, but the
  softened MVP-READINESS text **must preserve those numbers** verbatim.
- `test/handover-env-docs-normalize.test.ts` — pins HANDOVER env-naming wording
  (`ADW_*` canonical / `MX_AGENT_*` deprecated+denied), not counts → unaffected.
- `test/observed-live-ledger-doc.test.ts` — pins that MVP-READINESS links the
  observed-live ledger (a different line) → unaffected.

---

## 3. Scope

### In scope (required by the issue's acceptance criteria)
- Fix the three HANDOVER.md "current/baseline" count references (§2.2).
- Soften the MVP-READINESS.md real-process claim (§2.3).

### Recommended in this PR (low risk; keeps docs internally consistent)
- Soften PARITY.md:11 (§2.4 / §4.2).
- Optionally add a tiny regression assertion to the existing
  `test/mvp-readiness-doc.test.ts` locking in the softening (§4.3) — matches the
  repo's established pattern (issues #8, #25 each added a doc-invariant guard).

### Out of scope
- Retiring/renaming the `[MX-ADW]` progress tag (§2.5) — separate behavior issue.
- Rewriting historical per-section count lines in HANDOVER (§8w–§8ab etc.) — they
  are accurate as dated records.
- PLAN.md:159 and `specs/*` wording.
- Any kernel/runtime/prompt-pack/config/provider change.

---

## 4. Detailed implementation

> All edits are exact-string replacements. Make the doc edits first, then (if
> adding the optional guard) the test edit, then re-measure and write the final
> counts (§5).

### 4.1 HANDOVER.md count fixes (required)

**Edit 1 — `adw_sdlc/HANDOVER.md:313`** (§8 resume gate):

- Find: `# 3) Full test suite (current: 605 tests, 43 files)`
- Replace with: `# 3) Full test suite (current: 611 tests, 43 files)`

**Edit 2 — `adw_sdlc/HANDOVER.md:1463`** (§8ac, end of the section):

- Find: `(**611 tests, 44 files**).`
- Replace with: `(**611 tests, 43 files**).`

**Edit 3 — `adw_sdlc/HANDOVER.md:1671`** (§12 baseline):

- Find: `Test count baseline after this session: **611 passing across 44 files**`
- Replace with: `Test count baseline after this session: **611 passing across 43 files**`

> If the final measured totals differ from 611 (e.g. the optional guard in §4.3
> adds `it()` blocks → 611 becomes 612+), substitute the **actually observed**
> test count in all three edits. The file count stays **43** as long as no new
> `*.test.ts` file is added (adding assertions to an existing file does not change
> the file count).

### 4.2 Soften MVP-READINESS.md (required) + PARITY.md (recommended)

**Edit 4 — `adw_sdlc/MVP-READINESS.md` (lines 4–5), required.**

- Find (spans two source lines; match the whole parenthetical):
  `(every SDK/spawn/gh/git\neffect stubbed — verified: no test in the suite spawns a real process or touches a\nreal network)`
- Replace with a softened parenthetical that (a) keeps the surrounding sentence
  flowing into "Contact with reality now spans **nine** live `claude` runs …" and
  (b) names the boundary-crossing tests. Suggested wording:

  > `(most SDK/gh/git effects stubbed at the seam — though a few tests now
  > deliberately cross it: the secret-boundary audit
  > (\`test/secret-boundary-audit.test.ts\`) and the verify-gate e2e test
  > (\`test/verify-gate.e2e.test.ts\`) spawn real subprocesses, and the rest
  > transport loopback suite (\`test/providers-rest-transport.test.ts\`) drives a
  > real localhost round-trip)`

  Constraints on the rewrite:
  - Do **not** alter any numbers in the paragraph (the `nine` live runs, `0/36
    hard-fails (0.0%)`, `88.9% single-nudge` are still accurate and are
    cross-checked by `test/parity-evidence.test.ts`).
  - Preserve the paragraph's overall thesis ("all boxes green = internally
    consistent against stubs, not proven in production") — only the absolute "no
    test spawns a real process / touches a real network" claim is being relaxed.
  - Keep the `**mocked-seam**` bolded phrase intact (downstream prose and the
    sibling `specs/mvp-live-run-observation-ledger.md` reference it).

**Edit 5 — `adw_sdlc/PARITY.md:11` (recommended).**

- Find: `every SDK/spawn/`gh`/git effect stubbed. No network, no API keys, no native binaries. This is the bulk`
- Replace with wording that keeps the "mocked-seams" evidence **class** definition
  but acknowledges the handful of boundary-crossing tests, e.g.:
  `nearly every SDK/spawn/`gh`/git effect stubbed (a few tests deliberately cross the boundary — see MVP-READINESS.md). This is the bulk`
  (Phrasing is the implementer's discretion; the goal is to remove the absolute
  "No network … no native binaries" claim so PARITY and MVP-READINESS agree.)

**Edit 6 — `adw_sdlc/HANDOVER.md:1022` (optional).**

Leave as historical (it is dated to the "450-test suite" and past-tense), or add a
brief forward-reference clause, e.g. append after the parenthetical: ` (later
sections add real-process tests — see §8x / §8aa)`. Do not change the 450 figure.

### 4.3 Optional regression guard (recommended, matches repo convention)

Add a small block to the **existing** `adw_sdlc/test/mvp-readiness-doc.test.ts`
(keeps file count at 43) that locks in the softening so a revert is caught in CI —
the same pattern issues #8 and #25 used. Suggested assertions:

- The doc does **not** contain the stale literal
  `no test in the suite spawns a real process`.
- The doc **does** reference at least one real-process test by name (e.g.
  `secret-boundary-audit.test.ts` or `verify-gate.e2e.test.ts`).

Keep it to the established style (read `MVP-READINESS.md` via `REPO_ROOT`,
`describe`/`it`, plain string/regex assertions). Adding `N` `it()` blocks raises
the test count by `N`; account for that when writing the final HANDOVER numbers
(§5, step F).

---

## 5. Execution order (important — counts must be written last)

- **A.** Apply Edit 4 (MVP-READINESS softening). Required.
- **B.** Apply Edit 5 (PARITY) and Edit 6 (HANDOVER:1022) if doing the recommended/optional softenings.
- **C.** (Optional) Add the §4.3 guard block to `test/mvp-readiness-doc.test.ts`.
- **D.** Apply Edits 1–3 (HANDOVER count fixes) using placeholder values.
- **E.** From `adw_sdlc/`, run the full suite: `npx vitest run` (or `npm test`).
- **F.** Read the reporter's `Tests N passed` and `Test Files M passed`. Rewrite
  Edits 1–3 to those exact `N`/`M` values (expected: `M = 43`; `N = 611` if no
  guard added, `611 + (number of new it() blocks)` if §4.3 was done).
- **G.** Run `npm run verify` for the full local gate (typecheck, env lint,
  prompt-pack drift check, tests, build+clean).

This ordering prevents the self-defeating case where you write "611" and then a
newly-added test makes it 612.

---

## 6. Acceptance criteria

From the issue:
- [ ] **HANDOVER.md test counts current** — `HANDOVER.md:313`, `:1463`, and
      `:1671` all state the empirically-measured current totals (43 files; the
      observed test count), and no longer disagree with each other.
- [ ] **MVP-READINESS.md softened** — the absolute "no test in the suite spawns a
      real process or touches a real network" claim is gone, replaced by wording
      that names the real-process/real-network tests; the paragraph's numbers and
      `**mocked-seam**` framing are preserved.

Additional (this spec):
- [ ] `npm run verify` passes from `adw_sdlc/` (all doc-guard tests green:
      `mvp-readiness-doc`, `parity-evidence`, `handover-env-docs-normalize`,
      `observed-live-ledger-doc`).
- [ ] No production code, config, prompt-pack, or fixture changes (docs +
      optionally one existing test file only).
- [ ] (If recommended scope taken) PARITY.md:11 no longer asserts "No network …
      no native binaries" in absolute terms.
- [ ] (If optional guard added) a reverted softening fails CI.

---

## 7. Risks and mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Counts go stale again on the next merge | High (structural — append-only log with hardcoded numbers) | Inherent to the doc design; out of scope to fix here. See Open Question Q1 (de-hardcode the resume gate). |
| Writing the count before adding the optional guard → off-by-N | Medium | §5 mandates measure-and-write-last ordering. |
| Rewriting MVP-READINESS prose accidentally changes a parity number | Low | Edit 4 explicitly forbids touching numbers; `test/parity-evidence.test.ts` catches drift in the cited figures; restrict the edit to the parenthetical clause. |
| A doc-guard test secretly pins the changed text | Low (verified none do, §2.6) | Re-run the four guard tests; `npm run verify` gate. |
| Scope creep into `[MX-ADW]` retirement | Low | Explicitly deferred to a separate behavior issue (§2.5). |
| PARITY/HANDOVER:1022 edits over-broaden a docs PR | Low | Marked recommended/optional with clear opt-out; maintainer may defer. |

---

## 8. Rollback

Pure documentation (plus optionally one test file). Revert the doc commit; no
runtime, schema, or data implications. If the optional guard was added and proves
brittle, delete the added `describe`/`it` block and re-measure the count.

---

## 9. Assumptions

1. Issue #41 is a docs-currency task; the two checkboxes are the contract, and the
   `[MX-ADW]` line is an explicitly soft "may" → deferred (§2.5).
2. The current branch's working tree is the intended baseline; `611 tests / 43
   files` was measured on it. The implementer re-measures on the **final** tree.
3. Per-section historical count lines in HANDOVER are intentional dated records
   and should be preserved, not normalized.
4. Per the ADW phase contract, the orchestrator owns all git/gh; this phase
   produces only the spec — no branching/committing/PRs here.

---

## 10. Open questions

- **Q1 (recommended follow-up, not blocking):** The §8 resume gate hardcodes a
  count that drifts every PR. Should it instead say "run `npm test` and confirm
  the reported totals" (no hardcoded number), eliminating this recurring
  staleness class? Out of scope for #41; worth a tiny separate doc tweak.
- **Q2:** Should `[MX-ADW]` be retired, and if so to what tag
  (`[ADW]` / `[SWITCHYARD-ADW]` / project-derived)? Needs a maintainer decision
  plus config + test changes — file as a separate `issue_class` change item.
- **Q3:** Adopt the recommended PARITY.md:11 softening in this PR, or keep #41
  strictly to its two acceptance boxes and file PARITY/HANDOVER:1022 separately?
  Recommendation: include the PARITY.md softening here (cheap, prevents a fresh
  cross-doc inconsistency); treat HANDOVER:1022 as optional.
```
