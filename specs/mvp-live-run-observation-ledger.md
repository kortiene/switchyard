# Spec: MVP live-run observation ledger

- **Issue:** #3 — `feat: MVP live-run observation ledger`
- **Labels:** `issue_class:feat`, `adw-live-batch`
- **Intended `issue_class`:** `feat`
- **Planned ADW run mode:** native
- **Type:** Docs/feature artifact (new tracking doc + one link + one focused
  doc-assertion test). **No kernel / prompt-pack / config / runtime change.**
- **Owning artifacts:** new `adw_sdlc/docs/OBSERVED-LIVE-LEDGER.md`;
  `adw_sdlc/MVP-READINESS.md` (link only).

---

## 1. Background & context

`adw_sdlc/PARITY.md` is a checklist of green boxes; almost all of them are
**mocked-seam** evidence (every SDK/spawn/`gh`/git effect stubbed). Its
**"Section 10 parity checklist (for the shipped runner)"** table
(`adw_sdlc/PARITY.md:22–38`) enumerates the per-guarantee rows the `claude`
cutover gate rests on.

`adw_sdlc/MVP-READINESS.md` is the explicit counterweight: it tracks what real
MVP-readiness still requires beyond the mocks. Its **§1 "Gates for (A) — claude
ships reliably"** contains this still-open bullet (`adw_sdlc/MVP-READINESS.md:84`):

```
- [ ] ❌ **"Mocked ✅ → observed live?" ledger.** For each of PARITY's 12 boxes,
  has it been seen live even once? Today almost none have. That ledger is the real
  readiness dashboard; #332 is proof the mocks under-specify reality.
```

This issue **builds that ledger** as a standalone, auditable document and links
it from the bullet above. The ledger is the readiness dashboard that distinguishes
"green under stubs" from "seen at least once in a real run."

### Facts verified in-repo (do not re-derive — confirm before relying)

- **PARITY.md Section 10 has 13 rows, not 12.** Counting the table at
  `adw_sdlc/PARITY.md:24–38`: (1) Phase order & gating, (2) Per-phase model
  routing, (3) Selected runner edits the worktree unattended (capability parity),
  (4) Structured output, (5) Secret withholding (fail-closed), (6)
  Sandboxed-to-worktree, (7) Gated squash-merge, (8) Bounded loops +
  no-retry-on-timeout, (9) Resume, (10) Artifacts, (11) State equivalence
  (cross-language), (12) Cost/usage, (13) `adw/` green. The "12 boxes" phrasing in
  `MVP-READINESS.md:84` predates the additive "State equivalence" row (marked
  "(this PR)"). See **§10 Key decisions** for how this spec reconciles the count.
- **One live `claude` run exists to seed from.** PARITY.md records it in two
  places: the real-issue table (`adw_sdlc/PARITY.md:68` — "Issue #304 → PR #331
  (squash-merged), parity bug fixed in #332. Cost ≈ $34.76, run `007fd5ba`") and
  the cutover criteria (`adw_sdlc/PARITY.md:47` — "a completed live `claude` run
  (PR #331) produced a real such `state.json`"). The structured-output section
  (`adw_sdlc/PARITY.md:91–95`) records that the same run "hit one tests-phase
  contract mismatch, root-caused and fixed structurally in #332 … no recurrence on
  resume."
- **The new doc is outside the prompt pack.** `docs/` files are not under
  `.adw/prompts` and are not derived from `.adw/pack.profile.json`, so adding
  `docs/OBSERVED-LIVE-LEDGER.md` and editing `MVP-READINESS.md` does **not**
  trigger a `pack:check` drift failure and needs **no** `npm run pack:generate`.
  (Same reasoning the issue #1 docs change relied on.)
- **The repo's precedent is to add a focused doc-assertion test.** Issue #1's
  spec said "no new tests," but the landed change added
  `adw_sdlc/test/mvp-readiness-doc.test.ts`, which reads the doc and asserts its
  acceptance-criteria invariants under `npm run verify`. This spec follows that
  precedent (see **§3 / §7**).
- **`npm run verify` chain** (`adw_sdlc/package.json`):
  `typecheck → lint:env → pack:check → test → build → rm -rf dist`. The new test
  runs in the `test` (vitest) step; the new `.test.ts` is typechecked.
- **No existing test must break.** `mvp-readiness-doc.test.ts` slices §0 from
  `## 0.` to the next `\n## ` and asserts on §0/§1 prose plus the presence of
  `LIVE-RUN-BATCH.md`, `ADW_TEST_CMD`, and `npm run verify`. The link added by this
  issue lives in §1 and removes nothing, so those assertions stay green.

---

## 2. Goal

Create `adw_sdlc/docs/OBSERVED-LIVE-LEDGER.md` — a table with **one row per
PARITY.md Section-10 guarantee** — where each row carries an **explicit
observed-live status** (seen in a real run, yes/no/N-A) alongside its
mocked status and the run id / evidence. Seed it conservatively from the existing
live `claude` run (PR #331 / fix #332 / run `007fd5ba`) where the documents
support it, and **link it from `MVP-READINESS.md` §1** so it becomes the live
readiness dashboard. Keep `npm run verify` green.

---

## 3. Scope

### In scope
- New file `adw_sdlc/docs/OBSERVED-LIVE-LEDGER.md` (the ledger).
- A status legend defining the observed-live vocabulary.
- One row per Section-10 guarantee (all 13 rows — see §10), each with an explicit
  observed-live token.
- A conservative seed from PR #331 / `007fd5ba` for rows the docs support.
- A one-line link to the ledger from the `MVP-READINESS.md:84` §1 bullet (and flip
  that bullet's `❌` marker to reflect that the ledger now exists).
- A focused doc-assertion test `adw_sdlc/test/observed-live-ledger-doc.test.ts`
  that encodes the acceptance criteria (recommended; see §7 for the
  test-vs-no-test decision).

### Out of scope
- **No live runs.** This spec produces the doc; it does not execute or require any
  new `claude` run. Any future runs are operator-driven and update the ledger
  later.
- **No changes to `PARITY.md`, `PLAN.md`, `HANDOVER.md`, or `LIVE-RUN-BATCH.md`
  content** (the ledger links to / cites them; it does not edit them).
- **No kernel/runtime/schema/prompt-pack/config/`package.json` change.**
- **No re-litigation of the MVP definition** (already decided = (A) in §0).
- **No new tooling** (e.g., a generator that scrapes PARITY.md into the ledger) —
  the ledger is hand-authored from the named source rows. A generator is noted as a
  possible follow-up in §12.

---

## 4. Files to change

| File | Change |
| --- | --- |
| `adw_sdlc/docs/OBSERVED-LIVE-LEDGER.md` | **New.** The ledger: title, legend, table (one row per Section-10 guarantee), seed notes, and a "how to update after a live run" footer. |
| `adw_sdlc/MVP-READINESS.md` | **Edit (§1 only).** Update the `MVP-READINESS.md:84` "Mocked ✅ → observed live? ledger" bullet to link `docs/OBSERVED-LIVE-LEDGER.md` and flip its status marker. Optionally reconcile "12 boxes" → "13" (see §10). |
| `adw_sdlc/test/observed-live-ledger-doc.test.ts` | **New (recommended).** Focused vitest doc-assertion test encoding the acceptance criteria. |

No other files are touched. (`git diff --name-only` should list only these.)

---

## 5. Implementation steps

### Step 1 — Author `docs/OBSERVED-LIVE-LEDGER.md`

Create the file with this structure:

1. **Title + one-paragraph purpose.** State that this is the "mocked ✅ → observed
   live?" dashboard for each `PARITY.md` Section-10 guarantee, the live counterpart
   to PARITY's mocked checklist, and that a row flips to observed-live only on
   documented evidence from a real run (cite `#332` as proof the mocks
   under-specify reality).

2. **Status legend** (the observed-live vocabulary). Recommended four tokens so
   every row can carry an *explicit* status (acceptance criterion #2):

   - `✅ observed-live` — seen at least once in a real run, with cited evidence.
   - `🟡 partial / inferred` — a real run exercised the surrounding path, but the
     specific guarantee was not independently evidenced (note what is missing).
   - `⏳ not-yet-observed` — mocked-only so far; owed a live observation.
   - `N/A (not live-observable)` — not a property of a `claude` live run (e.g., a
     Python-suite CI invariant); explain why.

3. **The table.** Use exactly the columns the issue names, with the observed-live
   cell carrying a legend token:

   | Guarantee (PARITY §10) | Mocked? | Observed-live? | Run id / evidence |
   | --- | --- | --- | --- |

   Add **one row per Section-10 guarantee** using the guarantee names verbatim from
   `PARITY.md:24–38` (so the rows are traceable to their source). See **§6** for the
   recommended seed values and per-row rationale. Every row's "Mocked?" cell is `✅`
   (all 13 are mocked-proven, or — for `adw/ green` — proven by the Python suite),
   matching PARITY.

4. **Seed notes block** (under the table). One short paragraph naming the seed
   source once: live `claude` run — Issue #304 → PR #331 (squash-merged), parity
   fix #332, run `007fd5ba`, cost ≈ $34.76 — and stating the conservative rule:
   *a row is only `✅ observed-live` where a cited document supports it; everything
   else stays `⏳` or `🟡`.* Cross-link `PARITY.md` (Section 10 + real-issue table +
   structured-output rate section) and `MVP-READINESS.md §1`.

5. **"How to update after a live run" footer.** 3–5 bullets: after each live
   `claude` run, (a) flip the relevant rows to `✅` with the run id / PR; (b) record
   the run's `agents/{adw_id}/…` artifact path as evidence; (c) run
   `npm run parity:rate -- agents/` and link the structured-output result; (d) keep
   the row order aligned with `PARITY.md` Section 10. This makes the ledger a living
   dashboard, not a one-shot snapshot.

**Authoring rule (load-bearing): do not overclaim.** Mark `✅ observed-live` only
where a cited document (PARITY.md / MVP-READINESS.md) or a present run artifact
states the guarantee was seen live. When in doubt, use `🟡` (with a note on the
gap) or `⏳`. MVP-READINESS §1 explicitly lists several of these as still owed
(secret boundary on a real spawned env; the four failure-mode drills) — those must
**not** be marked observed-live.

### Step 2 — Verify the seed against artifacts (if present), else stay conservative

If a run directory for `007fd5ba` exists in this checkout (e.g.
`adw_sdlc/agents/007fd5ba/`), spot-check it to upgrade `🟡` rows to `✅` where the
artifacts prove the guarantee (e.g. per-phase `usage`/cost in the phase outputs;
the persisted `state.json` for state-equivalence; a `pr_body.md`/
`commit_message.txt` for artifacts). If the directory is **absent**, do **not**
invent evidence — keep the §6 seed values (which are grounded only in the
committed docs) and note in the seed block that artifact-level confirmation is
pending.

> Check first; do not assume the artifacts are vendored. PARITY.md cites the run by
> id but the `agents/` tree may not be committed to this repo.

### Step 3 — Link the ledger from `MVP-READINESS.md` §1

Edit the single bullet at `adw_sdlc/MVP-READINESS.md:84`. Current text:

```
- [ ] ❌ **"Mocked ✅ → observed live?" ledger.** For each of PARITY's 12 boxes,
  has it been seen live even once? Today almost none have. That ledger is the real
  readiness dashboard; #332 is proof the mocks under-specify reality.
```

Replace with a version that (a) links the new doc and (b) reflects that the ledger
now exists (flip `❌`). Recommended:

```
- [ ] 🔧 **"Mocked ✅ → observed live?" ledger.** For each of PARITY's 13
  Section-10 guarantees, has it been seen live even once? The dashboard lives in
  [`docs/OBSERVED-LIVE-LEDGER.md`](./docs/OBSERVED-LIVE-LEDGER.md); today almost
  every row is still `⏳` (seeded from PR #331 / run `007fd5ba`). #332 is proof the
  mocks under-specify reality.
```

Notes:
- Use a **relative** markdown link `./docs/OBSERVED-LIVE-LEDGER.md` (the doc lives
  at `adw_sdlc/docs/`, the same relative root the §1 `LIVE-RUN-BATCH.md` link uses).
- Keep the checkbox `- [ ]` (the *gate* — every box observed-live — is not yet met;
  only the ledger artifact now exists). Flipping `❌ → 🔧` matches the doc's legend
  (`🔧 automatable in-repo`, which the ledger is) and the "Instruments" framing.
- The "12 → 13" count reconciliation is folded into this same edit (we are touching
  the sentence anyway). See §10.

### Step 4 — Add the focused doc-assertion test (recommended)

Create `adw_sdlc/test/observed-live-ledger-doc.test.ts` mirroring
`mvp-readiness-doc.test.ts` (import `REPO_ROOT` from `../src/common.js`, read the
files with `node:fs`). Assert the acceptance criteria mechanically so
`npm run verify` enforces them:

- **Ledger exists & is non-empty** (`readFileSync` length > a small floor).
- **Every Section-10 guarantee is present.** Declare the canonical list of 13
  guarantee substrings in the test (drawn from PARITY.md row names, e.g.
  `'Phase order'`, `'model routing'`, `'edits the worktree'`, `'Structured output'`,
  `'Secret withholding'`, `'Sandboxed'`, `'squash-merge'`, `'Bounded loops'`,
  `'Resume'`, `'Artifacts'`, `'State equivalence'`, `'Cost/usage'`, `'adw/ green'`)
  and assert each appears. Assert the list length is 13 so drift is caught.
- **Each data row has an explicit observed-live status.** Parse the markdown table
  rows (lines starting with `|` that are not the header/separator) and assert every
  data row contains one of the legend tokens (`✅`, `🟡`, `⏳`, or `N/A`).
- **Seed evidence is cited.** Assert the doc contains `007fd5ba` and `#331`.
- **MVP-READINESS links to the ledger.** Read `MVP-READINESS.md` and assert it
  `toContain('OBSERVED-LIVE-LEDGER.md')`.

Keep the assertions tolerant of prose wording (substring/regex, not exact-line) so
the ledger stays editable without churning the test — the same style as
`mvp-readiness-doc.test.ts`.

> If the maintainer prefers the literal "no test" reading of the issue note
> ("Docs/feature artifact; no kernel change required"), the test may be dropped —
> but then "each row has an explicit observed-live status" is enforced only by
> review, not by `verify`. Given `issue_class:feat` and the issue #1 precedent, the
> test is the recommended path. See §10.

### Step 5 — Run the gate

From `adw_sdlc/`:

```bash
npm run verify
```

Expect green. The change is docs + one test:
- `pack:check` is unaffected (the ledger is outside `.adw/prompts`; no
  `pack:generate` needed).
- `lint:env` is unaffected (no runner-env code touched).
- `typecheck`/`build` compile the new `.test.ts`.
- `test` runs the new assertions plus the unchanged `mvp-readiness-doc.test.ts`.

If `pack:check` fails, an out-of-scope edit slipped into a pack source — revert it.

---

## 6. Recommended seed values (conservative, doc-grounded)

These are the **starting** observed-live values, justified **only** by the cited
committed docs. The implementer should keep them unless Step 2 artifact inspection
warrants an upgrade; never downgrade evidence the docs assert, never upgrade beyond
what they assert.

| # | Guarantee (PARITY §10) | Observed-live? | Why (cited) |
| --- | --- | --- | --- |
| 1 | Phase order & gating | `✅ observed-live` | The full chain ran end-to-end → squash-merged PR #331 (`PARITY.md:53`, `:68`). Note: identical firing of the *conditional* e2e/document gates was not separately evidenced. |
| 2 | Per-phase model routing | `🟡 partial / inferred` | The live run necessarily used per-phase routing, but no artifact in the docs pins the exact tier per phase. Upgrade only if `007fd5ba` phase outputs confirm tiers. |
| 3 | Selected runner edits the worktree unattended | `✅ observed-live` | The `claude` runner edited the worktree unattended and produced PR #331 (`PARITY.md:68`). |
| 4 | Structured output | `✅ observed-live` (rate ⏳) | Seen live including a real tests-phase contract mismatch, fixed in #332 (`PARITY.md:91–95`). The *comparative hard-failure rate* is still `INSUFFICIENT DATA` — keep that caveat in the cell. |
| 5 | Secret withholding (fail-closed) | `⏳ not-yet-observed` | `MVP-READINESS.md:88–90` explicitly owes "the secret boundary asserted once on a *real* spawned env (not only the lint + mocks)." Mocked-only. |
| 6 | Sandboxed-to-worktree (per runner) | `🟡 partial / inferred` | `cwd` was bound to the worktree for the live run, but the per-tool git/gh veto firing live is not evidenced. |
| 7 | Gated squash-merge | `🟡 partial / inferred` | The merge path executed live (PR #331 was squash-merged with `--yes`), but the *unattended refusal without `--yes`* was not induced live. |
| 8 | Bounded loops + no-retry-on-timeout | `⏳ not-yet-observed` | `MVP-READINESS.md:80–83`: a real timeout fast-fail / budget fast-fail "none has been seen live." Mocked-only. |
| 9 | Resume | `✅ observed-live` | `PARITY.md:95` — "no recurrence on resume" after #332 means a real `--resume` occurred. Note: the §1 *kill-then-resume* failure drill is a different, still-owed scenario. |
| 10 | Artifacts | `✅ observed-live` | PR #331 has a real PR body + commit message → `review`/`document` wrote `pr_body.md`/`commit_message.txt` live. |
| 11 | State equivalence (cross-language) | `🟡 partial / inferred` | `PARITY.md:47` — the live run "produced a real such `state.json`" that validates against the schema. The cross-language *resume of that exact artifact* by Python is proven via fixtures/tests, not the live artifact — keep that nuance. |
| 12 | Cost/usage | `✅ observed-live` | Cost ≈ $34.76 recorded for run `007fd5ba` (`PARITY.md:68`); native per-phase cost was captured. |
| 13 | `adw/` green | `N/A (not live-observable)` | The Python `adw/` unittest suite staying green is a CI/mocked invariant, not a property of a `claude` live run. State the rationale in the cell. |

Net seed: **5 ✅, 4 🟡, 2 ⏳, 1 N/A** (with #4 carrying a rate caveat). This makes
the dashboard's headline honest: a single live run touched most paths once, but the
load-bearing security/failure guarantees (#5, #8) and several others remain owed —
which is exactly the point MVP-READINESS §1 makes.

---

## 7. Test & verification strategy

- **Primary gate:** `npm run verify` from `adw_sdlc/`
  (typecheck → lint:env → pack:check → test → build → rm -rf dist).
- **New focused test** (`observed-live-ledger-doc.test.ts`, per Step 4) encodes the
  four acceptance criteria as assertions: every guarantee present (count = 13), each
  row has an explicit status token, seed evidence cited, MVP-READINESS links the
  ledger. This is the auditable mechanism that keeps "stays green" meaningful — the
  same approach issue #1 used.
- **Regression guard:** the existing `mvp-readiness-doc.test.ts` must remain green;
  the §1 link edit removes nothing it asserts on and lives outside the §0 slice it
  inspects.
- **Manual checks:** relative link `./docs/OBSERVED-LIVE-LEDGER.md` resolves to the
  new file; `git diff --name-only` lists only the three files in §4.

---

## 8. Risks & mitigations

| Risk | Likelihood | Mitigation |
| --- | --- | --- |
| **Overclaiming** a guarantee as observed-live without real evidence (the most damaging failure — turns a readiness dashboard into false comfort) | Medium | Conservative authoring rule (§5 Step 1); §6 seed grounded only in cited docs; default to `🟡`/`⏳`; the test asserts presence of statuses, and review checks each `✅` against its citation. |
| `adw/ green` / Python-suite invariant mis-modeled as a live gate | Low | Explicit `N/A (not live-observable)` token with rationale in the cell. |
| The "12 vs 13" count drifts between PARITY, MVP-READINESS, and the ledger | Medium | Ledger is the single dashboard; reconcile the §1 reference to 13 in the same edit; test asserts the count is 13. |
| Brittle test that breaks on any prose edit | Low | Use substring/regex assertions and table-row parsing, not exact-line matches (mirrors `mvp-readiness-doc.test.ts`). |
| Accidental edit to a prompt-pack source triggers `pack:check` | Low | Scope edits to the three §4 files; verify the diff name-list. |
| Ledger goes stale after future live runs | Medium | The "how to update after a live run" footer makes upkeep a documented step; the §1 link keeps it discoverable. |

---

## 9. Rollout / rollback

- **Rollout:** one new docs file + one link edit + one test. No migration, flag, or
  runtime impact. Native ADW run mode (per the batch plan).
- **Rollback:** delete `docs/OBSERVED-LIVE-LEDGER.md` and
  `test/observed-live-ledger-doc.test.ts`, revert the one-line MVP-READINESS edit.
  Nothing downstream depends on any of it.
- **Live-run linkage:** this issue is #3 in `docs/LIVE-RUN-BATCH.md`, planned as a
  **native** run. That run is operator-driven and **out of scope for this spec's
  implementation** — the spec only produces the artifacts; the orchestrator/
  maintainer performs any live run.

---

## 10. Key decisions

1. **List all 13 Section-10 rows, and reconcile the "12" reference.** The issue
   says "every PARITY.md Section-10 guarantee"; the table has 13 rows
   (`PARITY.md:24–38`). The ledger lists all 13, and the same §1 edit that adds the
   link updates "12 boxes" → "13 Section-10 guarantees" so the docs agree. (The
   "12" was correct before the additive "State equivalence" row.)
2. **`adw/ green` is `N/A (not live-observable)`, not omitted.** It is a
   Section-10 row, so it appears (criterion: "lists every … guarantee"), but its
   observed-live status is N/A with a rationale (it is a Python-suite CI invariant,
   not a `claude` run property). This satisfies "each row has an explicit
   observed-live status" honestly.
3. **Four-token legend (`✅`/`🟡`/`⏳`/`N/A`)** rather than a binary yes/no. A bare
   yes/no would force over- or under-claiming on the many "exercised once but not
   independently evidenced" rows; the `🟡` token captures reality precisely and
   keeps the dashboard trustworthy.
4. **Add a focused doc-assertion test despite the issue's "no kernel change"
   note.** A test is not a kernel change; `issue_class` is **feat**; and issue #1's
   landed change set the precedent (`mvp-readiness-doc.test.ts`). The test is what
   makes "each row has an explicit observed-live status" and "linked from
   MVP-READINESS" enforceable under `verify` rather than review-only. Marked
   recommended; droppable if the maintainer insists on the literal reading.
5. **Hand-authored ledger, not a generator.** A PARITY-scraping generator is
   over-engineering for a 13-row table and would add tooling/maintenance surface
   outside the issue's scope. Noted as a possible follow-up (§12).
6. **Conservative seed from PR #331 only.** Seed exactly one run's evidence,
   grounded in committed docs; never invent artifact evidence not present in the
   checkout.

---

## 11. Assumptions

- Today's date (`2026-06-25`) is acceptable to stamp on the ledger if a "last
  updated" line is included.
- `docs/OBSERVED-LIVE-LEDGER.md` remains outside the generated prompt pack
  (verified now); if that ever changes, `pack:generate` + `pack:check` would be
  required.
- The PR #331 / `007fd5ba` evidence as recorded in `PARITY.md` is accurate; this
  spec treats those committed statements as the source of truth and does not
  re-run anything to confirm them.
- The `agents/007fd5ba/` artifact tree may or may not be vendored in this checkout;
  the seed does not depend on it (Step 2 is an optional upgrade, not a requirement).
- MVP-READINESS §0 already records MVP = (A) (verified at `MVP-READINESS.md:28–31`),
  so no MVP-definition work is in scope here.

---

## 12. Open questions

1. **Reconcile "12 boxes" wording — confirm in scope.** This spec folds the
   "12 → 13" fix into the §1 link edit. If the maintainer prefers to leave the
   number untouched, the ledger still lists 13 rows (it is the source of truth) and
   the §1 number stays a known, documented discrepancy. (Recommendation: fix it.)
2. **Should the ledger track *all four runners* or `claude` only?** PARITY's
   real-issue table is per-runner; the MVP is (A) = `claude`-only. Recommendation:
   scope the ledger to the `claude` cutover gate (one column), and note codex/
   opencode/pi are tracked separately in `PARITY.md`'s real-issue table. A
   per-runner expansion is a post-MVP follow-up.
3. **Where exactly to place the ledger link** — inside the existing §1 bullet
   (recommended, most semantically correct) vs. a new "Instruments" entry. Either
   satisfies "linked from MVP-READINESS.md."
4. **Cross-link from `PARITY.md` back to the ledger?** Would make the mocked↔live
   relationship bidirectional, but PARITY.md edits are out of scope here. Flagged,
   not done.
5. **Generator follow-up** — a small `tools/` script that emits the ledger skeleton
   from PARITY.md Section-10 rows so the row set can't drift. Out of scope; noted.
