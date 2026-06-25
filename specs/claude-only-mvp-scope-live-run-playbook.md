# Spec: Declare Claude-only MVP scope + live-run playbook

- **Issue:** #1 — `docs: declare Claude-only MVP scope + live-run playbook`
- **Labels:** `issue_class:docs`, `adw-live-batch`
- **Intended `issue_class`:** `docs`
- **Planned ADW run mode:** forced-fenced (`ADW_PARITY_FORCE_FENCED_JSON=1`)
- **Type:** Documentation-only. No production code, prompt-pack, or config changes.
- **Owning artifact:** `adw_sdlc/MVP-READINESS.md`

---

## 1. Background & context

`adw_sdlc/MVP-READINESS.md` is the open-risk counterweight to `PARITY.md`. Its
**§0 "Pick the MVP definition"** presents three candidate bars and currently
leaves the decision **unset**:

```
> **Decision (record here):** ☐ A ☐ B ☐ C — _unset_.
```

(`adw_sdlc/MVP-READINESS.md:27`)

The three bars are:

- **(A)** "`claude` autonomously ships real issues reliably." — narrowest, marked
  **Recommended MVP** in the doc.
- **(B)** "four interchangeable runners." — PLAN.md's headline thesis, larger.
- **(C)** "cutover done" — default flipped `py → ts`, py kept ≥ 1 release.

The doc's own "Recommended minimal path to a defensible (A)-MVP" (§ near
`adw_sdlc/MVP-READINESS.md:112`) lists as **step 1**: *"Declare MVP = (A); demote
(B)/(C) in the docs."* This issue performs exactly that step plus adds an
operator-facing live-run playbook subsection.

A ready-to-create live-run batch and run-command templates already exist at
`adw_sdlc/docs/LIVE-RUN-BATCH.md`; this issue (#1 in that batch) is itself the
**forced-fenced** smoke-test / parity baseline run.

### Relevant facts verified in-repo

- `npm run verify` (in `adw_sdlc/package.json`) is:
  `typecheck → lint:env → pack:check → test → build → rm -rf dist`.
- `MVP-READINESS.md` is **not** part of the generated prompt pack (it is not under
  `.adw/prompts` and not derived from `.adw/pack.profile.json`), so editing it does
  **not** trigger a `pack:check` drift failure and needs **no** `npm run pack:generate`.
- `docs/LIVE-RUN-BATCH.md` already documents the canonical single-command gate
  `ADW_TEST_CMD="npm run verify"` and three run-command templates (native,
  forced-fenced, subscription).
- §2 and §3 of `MVP-READINESS.md` are already titled "Gates that (B) … adds" /
  "Gates that (C) … adds" but are **not** explicitly tagged "post-MVP".

---

## 2. Goal

Make `MVP-READINESS.md` §0 state the MVP decision explicitly as **(A) Claude
ships real issues reliably**, demote (B)/(C) to post-MVP, and add a short
operator playbook for running a live `claude` issue that links
`docs/LIVE-RUN-BATCH.md` and references the `npm run verify` gate — all while
keeping the (B)/(C) gate sections intact and clearly labelled post-MVP.

---

## 3. Scope

### In scope
- Edit `adw_sdlc/MVP-READINESS.md` §0 to record the decision MVP = (A).
- Demote (B)/(C) to post-MVP, both in §0 and via explicit "post-MVP" markers on
  the §2/§3 headings.
- Add a brief "How to run a live `claude` issue" playbook subsection that links
  `docs/LIVE-RUN-BATCH.md` and names the `npm run verify` gate.

### Out of scope
- No changes to `PARITY.md`, `PLAN.md`, `HANDOVER.md`, or `docs/LIVE-RUN-BATCH.md`
  content (only linked, not edited).
- No code, schema, prompt-pack, `.adw/config.json`, or `package.json` changes.
- No actual live runs and no threshold-setting (the `--max-native-rate` value
  stays a separate concern, already flagged in the doc).
- No removal of the (B)/(C) gate checklists — they remain, just relabelled.

---

## 4. Files to change

| File | Change |
| --- | --- |
| `adw_sdlc/MVP-READINESS.md` | Edit §0 decision line + framing; add post-MVP markers to §2/§3 headings; add a "How to run a live `claude` issue" subsection. |

No other files are touched.

---

## 5. Implementation steps

### Step 1 — Record the decision in §0

In `adw_sdlc/MVP-READINESS.md` §0 (`## 0. Pick the MVP definition`):

1. Replace the unset decision line (currently `adw_sdlc/MVP-READINESS.md:27`):

   ```
   > **Decision (record here):** ☐ A ☐ B ☐ C — _unset_.
   ```

   with a resolved decision that names (A) explicitly and contains **no**
   "unset" text, e.g.:

   ```
   > **Decision (recorded):** ☑ **(A)** — MVP = "`claude` autonomously ships
   > real issues reliably." **(B)** four-runner and **(C)** cutover are
   > **post-MVP** (see §2 / §3). Decided 2026-06-25.
   ```

2. Adjust the surrounding framing so it reads as decided, not pending. The line
   currently at `adw_sdlc/MVP-READINESS.md:29`
   ("The rest of this doc assumes **(A)** and marks what **(B)/(C)** add.")
   already aligns with (A); keep it, optionally tightening "assumes" → "adopts".
   The "**Decide first**" sentence near the top of §0 should be softened to past
   tense (e.g. "These were the three candidate bars; **(A)** was chosen.") so the
   section is internally consistent with the recorded decision.

3. In the (B)/(C) bullets of §0, append a brief "(post-MVP)" tag so the demotion
   is visible at the decision point as well as in §2/§3.

**Acceptance-relevant invariant:** after this step the literal token `unset`
must not appear in §0 (grep check in Step 4).

### Step 2 — Mark (B)/(C) gate sections post-MVP

Add an explicit post-MVP marker to the two section headings so the gates stay
listed but are unambiguously deferred:

- `## 2. Gates that (B) — four runners — adds` → add a "(post-MVP)" suffix or a
  one-line italic note directly under the heading, e.g.
  `_Post-MVP: not required for the (A) MVP. Listed for completeness._`
- `## 3. Gates that (C) — cutover — adds` → same treatment.

Leave the checklist bullets in both sections unchanged.

### Step 3 — Add the "How to run a live `claude` issue" playbook subsection

Add a short subsection (recommended placement: a new `### How to run a live
`claude` issue` heading at the end of §1, *or* a standalone `## How to run a live
`claude` issue` section immediately after §0 — pick whichever keeps the document
flow cleanest; placing it right after §0 maximizes visibility). It must:

1. **Link `docs/LIVE-RUN-BATCH.md`** using a relative markdown link:
   `[docs/LIVE-RUN-BATCH.md](./docs/LIVE-RUN-BATCH.md)`.
2. Name the **`npm run verify`** gate as the single test command and show the
   canonical invocation (kept consistent with `docs/LIVE-RUN-BATCH.md`):

   ```bash
   cd adw_sdlc
   ADW_TEST_CMD="npm run verify" \
     npx tsx src/cli.ts <ISSUE_NUMBER> --runner claude --yes \
     --timeout 3600 --max-budget-usd 45
   ```

3. Keep it brief (a few lines + one code block). Defer the full batch table,
   forced-fenced/subscription variants, failure-mode drills, and cost notes to
   `docs/LIVE-RUN-BATCH.md` rather than duplicating them — link out instead.
4. Optionally note the post-run classification step
   (`npm run parity:rate -- agents/`) as a one-liner with a pointer, but do not
   restate thresholds.

> Note: do **not** chain commands inside `ADW_TEST_CMD` — the orchestrator
> shell-splits it and passes args to `spawnSync` (no shell), so `a && b` would
> fail. `npm run verify` already chains internally. Reflect this only by using
> the single-command form above (it is documented in detail in
> `docs/LIVE-RUN-BATCH.md`).

### Step 4 — Self-check the edits

Before handing off, confirm:

- `grep -n "unset" adw_sdlc/MVP-READINESS.md` returns **no** match inside §0
  (ideally none in the file).
- §0 contains the explicit string identifying MVP as (A).
- The playbook subsection contains a working relative link to
  `./docs/LIVE-RUN-BATCH.md` and the text `npm run verify`.
- No other files in the diff (`git diff --name-only` should list only
  `adw_sdlc/MVP-READINESS.md`).

### Step 5 — Run the gate

From `adw_sdlc/`:

```bash
npm run verify
```

Expect it green. Because the change is docs-only and `MVP-READINESS.md` is
outside the prompt pack, `pack:check` should not flag drift and no
`pack:generate` is needed. (If `pack:check` ever fails, that indicates an
out-of-scope edit slipped in — revert it.)

---

## 6. Acceptance criteria

Mapped directly from the issue:

- [ ] **§0 shows MVP = (A) explicitly** with no "unset" text remaining in §0.
- [ ] A **live-run playbook subsection** exists and **links
  `docs/LIVE-RUN-BATCH.md`** (relative link) and references the `npm run verify`
  gate.
- [ ] **(B)/(C) gates remain listed** in §2/§3 but are clearly marked **post-MVP**.
- [ ] **`npm run verify` stays green** (docs-only change; no pack drift).
- [ ] The diff touches only `adw_sdlc/MVP-READINESS.md`.

---

## 7. Test & verification strategy

- **Primary gate:** `npm run verify` from `adw_sdlc/` (full local gate:
  typecheck → lint:env → pack:check → test → build → rm -rf dist).
- **Targeted checks (manual / grep):**
  - `grep -n "unset" adw_sdlc/MVP-READINESS.md` → no §0 match.
  - Confirm the relative link `./docs/LIVE-RUN-BATCH.md` resolves to an existing
    file (`adw_sdlc/docs/LIVE-RUN-BATCH.md`).
- **No new automated tests** are warranted: there is no test harness asserting on
  the prose of `MVP-READINESS.md`, and adding one would be disproportionate for a
  static decision record. The existing `pack:check` already guards that no
  unintended prompt-pack drift was introduced.

---

## 8. Risks & mitigations

| Risk | Likelihood | Mitigation |
| --- | --- | --- |
| Accidental edit to a prompt-pack source triggers `pack:check` failure / requires regen | Low | Scope edits strictly to `MVP-READINESS.md`; verify diff name-list in Step 4. |
| Playbook drifts from `docs/LIVE-RUN-BATCH.md` (duplicated commands diverge over time) | Medium | Keep the playbook a thin pointer; link out for the full batch/templates rather than copying them. |
| Decision recorded in docs but contradicted elsewhere (e.g. README "four runners") | Low | Out of scope to reconcile every doc; (B)/(C) remain documented as post-MVP, which is consistent with PLAN.md's broader thesis. Note as an open question. |
| "unset" string lingers elsewhere causing reviewer confusion | Low | Step 4 grep; the acceptance bar is specifically §0. |

This is the lowest-risk item in the live-run batch — explicitly chosen as the
forced-fenced smoke test.

---

## 9. Rollout / rollback

- **Rollout:** single docs commit; no migration, no flag, no runtime impact.
- **Rollback:** revert the single-file change; nothing downstream depends on the
  prose.
- **Live-run linkage:** this issue is run #1 in `docs/LIVE-RUN-BATCH.md`, planned
  in **forced-fenced** mode (`ADW_PARITY_FORCE_FENCED_JSON=1`) to also harvest a
  fenced parity baseline. That run is operator-driven and **out of scope for this
  spec's implementation** (the orchestrator/maintainer performs it); the spec only
  produces the docs change.

---

## 10. Key decisions

1. **MVP = (A)**, with (B)/(C) explicitly demoted to post-MVP — directly follows
   the doc's own "Recommended minimal path" step 1 and the §0 "Recommended MVP"
   annotation.
2. **Keep (B)/(C) gate checklists intact**, only relabel headings — preserves the
   readiness tracking without pretending those bars don't exist.
3. **Playbook is a thin pointer**, not a copy of `LIVE-RUN-BATCH.md` — avoids
   duplication drift; single source of truth for run templates stays the batch doc.
4. **No new tests; rely on `npm run verify`** — proportionate for a docs-only
   decision record; `pack:check` already guards against scope creep into the pack.

---

## 11. Assumptions

- Today's date (`2026-06-25`) is acceptable to stamp on the recorded decision.
- The maintainer/orchestrator is comfortable recording (A) as the MVP definition
  (the doc already labels (A) "Recommended MVP", so this codifies the existing
  recommendation rather than introducing a new judgment).
- `MVP-READINESS.md` remains outside the generated prompt pack (verified now); if
  that ever changes, `npm run pack:generate` + `pack:check` would be required.

---

## 12. Open questions

1. **Placement of the playbook subsection** — end of §1 vs. a standalone section
   right after §0. Recommendation: standalone after §0 for visibility, but either
   satisfies the acceptance criterion. (Implementer's choice.)
2. **Should the decision also be cross-referenced from README / HANDOVER / PLAN?**
   Out of scope here; could be a follow-up to keep the "four runners" framing in
   `README.md`/`PLAN.md` consistent with the post-MVP demotion. Flagged, not done.
3. **Threshold value** for `parity:rate --max-native-rate` is intentionally **not**
   set by this issue (it remains a placeholder in the doc and in `LIVE-RUN-BATCH.md`).
