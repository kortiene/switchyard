# Spec: Reconcile the cutover documentation conflict (`cli.ts` "is done" vs `(C)` post-MVP)

- **Work item:** GitHub issue #25 — _Reconcile the cutover documentation conflict (cli.ts says 'done' vs (C) post-MVP)_
- **Labels / class:** `issue_class:docs`, `backlog`, `area:cli`
- **Type:** docs/comment-only reconciliation (no production behavior change)
- **Primary in-scope file:** `adw_sdlc/src/cli.ts` (the `DEFAULT_ENGINE` docstring, currently `cli.ts:36-43`)
- **Source of truth for the milestone status:** `adw_sdlc/MVP-READINESS.md` §0 (lines ~22-38) and §3 (lines ~117-127)
- **Source:** issue evidence `cli.ts:42-48` (comment); `MVP-READINESS.md:121-124` (`(C)` ❌ not started); backlog assessment (`PLAN.md §11` / `MVP-READINESS.md` / `PARITY.md`)
- **Status:** specification only. **Do not implement as part of this phase.**
- **Local gate:** `npm run verify` (from `adw_sdlc/`)

---

## 1. Context & current state (read this first)

### 1.1 The contradiction

`cli.ts` asserts the py→ts cutover is *finished*, while `MVP-READINESS.md`
classifies the cutover milestone as **post-MVP and not started**. A reviewer
reading only `cli.ts` could prematurely treat the cutover milestone as shipped.

**The offending sentence** (`adw_sdlc/src/cli.ts`, first line of the
`DEFAULT_ENGINE` docstring — at the time of writing this is `cli.ts:37`, inside
the block spanning lines 36-43; the issue cited the pre-#27 range `42-48`):

```ts
/**
 * Standalone HealthTech port: the cutover (PLAN.md roadmap step 12) is done —
 * `ts` is the default. `py` stays a recognized engine id, but it is NOT
 * available in this standalone port: selecting it via `--engine py` /
 * `ADW_ENGINE=py` raises a deterministic `AdwError` at dispatch (no spawn, no
 * `python3` dependency), because the Python `adw/issue.py` sibling is not
 * bundled here.
 */
export const DEFAULT_ENGINE: EngineId = 'ts';
```

**The authoritative status it contradicts** (`adw_sdlc/MVP-READINESS.md`):

- §0 line ~31-32: `**(C) "cutover done"** — default flipped `py → ts`, py kept ≥ 1
  release (PLAN step 12). **(post-MVP.)**`
- §0 line ~34-36: `**Decision (recorded):** … **(B)** four-runner and **(C)**
  cutover are **post-MVP** … Decided 2026-06-25.`
- §3 heading line ~117: `## 3. Gates that (C) — cutover — adds (post-MVP)`
- §3 line ~121-127: both gate rows are **`❌` not started** — the py↔ts
  coexistence gate "cannot be validated from this standalone port" (needs the
  integrated repo), and the rollback/keep-py-≥1-release gate is `❌`.

### 1.2 What is actually true (so the softened wording stays accurate)

The two statements are reconcilable once you separate **the local default flip**
from **the cutover *milestone***:

- **The `ts` default flip is real and done — _locally, in this port._**
  `DEFAULT_ENGINE = 'ts'` and the tests assert it (`test/cli.test.ts:46`,
  `:266`, `:327`). `PARITY.md:42-55,121-122` records that the cutover (step 12) is
  *unblocked for `claude`* and the default is flipped. So "the `ts` default is
  set" is correct.
- **The cutover *milestone* (PLAN step 12) is NOT done.** Per
  `PLAN.md:103,668-685,1056` the milestone is more than flipping a default: it
  requires py↔ts coexistence validated **in the integrated repo**, keeping the
  Python `adw/` ≥ 1 stable release with a documented revert path, and the
  maintainer's cutover sign-off. None of that can even be exercised here — there
  are **0 `.py` files repo-wide** and no `adw/issue.py` (the top-level `adw/`
  ships only `fixtures/` + `state.schema.json`). `MVP-READINESS.md` §3 therefore
  correctly marks `(C)` as post-MVP / `❌`.

So the precise, non-contradictory framing is: **default flipped (done here) ≠
cutover milestone (post-MVP).** The `cli.ts` comment over-claims by saying the
cutover "is done"; it should say the **default** is set and the **milestone** is
post-MVP, and point at `MVP-READINESS.md` §3 as the authority.

### 1.3 Relationship to issue #27 (already shipped — do not redo)

Issue #27 ("make `--engine py` fail closed in the standalone port", commit
`ca75486`) already rewrote this region so `py` fails closed with a deterministic
`AdwError`. That work added the `PY_ENGINE_UNAVAILABLE` constant
(`cli.ts:54-57`) whose substring `not available in this standalone distribution`
is **pinned by tests**. Issue #25 is the small follow-up #27 left behind: it only
softens the *"cutover … is done"* clause. **Do not touch the fail-closed
behavior, `PY_ENGINE_UNAVAILABLE`, or its pinned substring.**

### 1.4 Other places that say "cutover" (audit; mostly out of scope)

A repo sweep for "cutover … done" finds the `cli.ts:37` sentence is the **only
production-code claim that the cutover is finished**. Related mentions, with the
scope call for each:

| Location | Text today | In scope for #25? |
|---|---|---|
| `adw_sdlc/src/cli.ts:37` | "the cutover (PLAN.md roadmap step 12) is done" | **YES — the fix.** |
| `adw_sdlc/test/cli.test.ts:45` | test *name* `'defaults to py until cutover'` (asserts `'ts'`) — itself stale/contradictory | **Optional** (see §5); a test name, not prod code. Issue scopes only `cli.ts` + `MVP-READINESS.md`. |
| `specs/py-engine-not-available-in-standalone.md:35` | `'ts' (cutover already done in this port)` | **No** — planning doc, not shipped guidance. Note only. |
| `adw_sdlc/PARITY.md:42-55,121-122` | cutover step 12 "unblocked"/"default flip" | **No** — already accurate (unblocked ≠ done). |
| `adw_sdlc/PLAN.md` (many) | roadmap/criteria for step 12 | **No** — historical roadmap; correctly describes the *plan*. |
| `adw_sdlc/HANDOVER.md:1115` | "(C) cutover demoted" | **No** — already says post-MVP. |

**Conclusion:** the substantive, in-scope deliverable is a one-sentence edit to
`cli.ts`. Everything else is already consistent or is an optional test-name
tidy. Be honest in the run summary that the diff is tiny.

---

## 2. The reconciliation policy (the rule to apply)

1. **Distinguish default-flip from milestone.** The replacement wording must (a)
   keep the accurate claim that `ts` is the default in this port, and (b) state
   that the cutover *milestone* is post-MVP. Never say or imply the milestone is
   "done"/"complete"/"shipped".
2. **Single source of truth.** `cli.ts` must defer to `MVP-READINESS.md` for the
   milestone's status rather than re-asserting it — add a cross-reference
   ("see `MVP-READINESS.md` §3") so the two can't silently diverge again.
3. **Preserve the rest of the docstring verbatim.** The fail-closed explanation
   for `py` (sentences 2-3 of the block) is correct and must be kept; only the
   first clause changes.
4. **Do not weaken or rephrase `PY_ENGINE_UNAVAILABLE`** (`cli.ts:54-57`) — its
   `not available in this standalone distribution` substring is test-pinned.
5. **Comment-only.** No change to `DEFAULT_ENGINE`'s value, `ENGINE_IDS`,
   `resolveEngineId`, or any runtime path.

---

## 3. Implementation steps

> Comment-only edit to one source file. Do **not** change runtime behavior, the
> prompt pack (`.adw/prompts`), or any test assertion. The orchestrator owns all
> git/gh; this phase only edits the file.

### Step 1 — Re-confirm the live line numbers

The line numbers drift; the docstring moves as `cli.ts` changes. Locate the
exact block before editing:

```bash
cd adw_sdlc
rg -n 'cutover .* is done|PLAN\.md roadmap step 12' src/cli.ts
rg -n 'DEFAULT_ENGINE' src/cli.ts
```

Confirm the target is the docstring immediately above
`export const DEFAULT_ENGINE: EngineId = 'ts';` and that its first sentence still
reads "the cutover (PLAN.md roadmap step 12) is done — `ts` is the default."

### Step 2 — Soften the first sentence of the `DEFAULT_ENGINE` docstring

Replace **only** the first sentence/clause. Keep the `py`-fails-closed sentences
that follow unchanged.

**Before:**

```ts
/**
 * Standalone HealthTech port: the cutover (PLAN.md roadmap step 12) is done —
 * `ts` is the default. `py` stays a recognized engine id, but it is NOT
 * available in this standalone port: selecting it via `--engine py` /
 * `ADW_ENGINE=py` raises a deterministic `AdwError` at dispatch (no spawn, no
 * `python3` dependency), because the Python `adw/issue.py` sibling is not
 * bundled here.
 */
```

**After (recommended wording — matches the issue's suggested phrasing and adds
the cross-reference required by acceptance #2):**

```ts
/**
 * Standalone HealthTech port: the `ts` default is set (the `py → ts` default
 * flip is local to this port); the cutover *milestone* (PLAN.md roadmap step 12)
 * is **post-MVP** — see `MVP-READINESS.md` §3, which tracks `(C)` cutover as a
 * post-MVP gate. `py` stays a recognized engine id, but it is NOT available in
 * this standalone port: selecting it via `--engine py` / `ADW_ENGINE=py` raises
 * a deterministic `AdwError` at dispatch (no spawn, no `python3` dependency),
 * because the Python `adw/issue.py` sibling is not bundled here.
 */
```

The minimum required by the acceptance is the clause swap
("the `ts` default is set; the cutover milestone is post-MVP"); the parenthetical
"(default flip is local to this port)" and the `MVP-READINESS.md` §3 reference
are recommended because they make the statement self-justifying and close the
"no contradiction remains" criterion durably.

### Step 3 — Verify no other production-code line still over-claims

```bash
cd adw_sdlc
rg -n 'cutover.*(done|complete|shipped|finished)' src/ | rg -v 'unblocked|post-MVP|post-cutover'
```

Expect **zero** hits after Step 2 (the `PARITY.md`/`PLAN.md` mentions are docs,
not `src/`, and say "unblocked"/roadmap, which is accurate). If a stray hit
remains in `src/`, reconcile it the same way (default-flip vs milestone).

### Step 4 — (Optional, see §5) Tidy the stale test name

If the maintainer wants the related contradiction closed in the same change,
rename the `test/cli.test.ts:45` case from `'defaults to py until cutover'` to
something truthful, e.g. `'defaults to the ts engine (default flipped)'`. This is
a **test-name string only** — the assertions (`expect(DEFAULT_ENGINE).toBe('ts')`
…) must not change. Left out of the core change because the issue scopes only
`cli.ts` + `MVP-READINESS.md`.

### Step 5 — Run the local gate

```bash
cd adw_sdlc
npm run verify   # typecheck → lint:env → pack:check → test → build → rm -rf dist
```

`verify` must stay green. The edit is a comment (and at most a test-name string),
so typecheck, the env lint, the prompt-pack drift check, and every assertion are
unaffected. Green proves the change broke nothing; the contradiction itself is
resolved by inspection (acceptance below).

---

## 4. Acceptance criteria

Mirrors the issue, made concrete:

1. **`cli.ts` comment softened.** The `DEFAULT_ENGINE` docstring no longer says
   the cutover "is done". It states the `ts` default is set **and** that the
   cutover milestone is post-MVP. Verify:
   `rg -n 'cutover.*is done' adw_sdlc/src/cli.ts` returns **nothing**, and the
   block contains both "default" and "post-MVP".
2. **No contradiction remains between `cli.ts` and `MVP-READINESS.md`.** Reading
   the `cli.ts` docstring and `MVP-READINESS.md` §0/§3 together yields one
   consistent story: default flipped locally; `(C)` cutover milestone is
   post-MVP. The `cli.ts` comment defers to `MVP-READINESS.md` §3 for the status.
3. **No behavior change.** `DEFAULT_ENGINE` is still `'ts'`; `ENGINE_IDS`,
   `resolveEngineId`, and the `py` fail-closed path are byte-identical.
   `PY_ENGINE_UNAVAILABLE` and its pinned `not available in this standalone
   distribution` substring are untouched.
4. **`npm run verify` is green** from `adw_sdlc/`.
5. **Scope held.** Only `adw_sdlc/src/cli.ts` changes (plus, optionally, the
   single test-name string in `test/cli.test.ts:45`); no other file is modified.

---

## 5. Risks & mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| Editing the wrong clause and breaking the `py`-fail-closed explanation | Low | §2.3 / §3 Step 2: change only the first sentence; keep sentences 2-3 verbatim. |
| Accidentally touching `PY_ENGINE_UNAVAILABLE` and breaking the pinned-substring test | Low | §2.4 forbids it; `npm run verify` (Step 5) would catch a regression. |
| Over-correcting into the opposite error (implying the default is *not* flipped) | Low | §2.1: keep the accurate "the `ts` default is set" claim; only the *milestone* is post-MVP. |
| New wording drifts from `MVP-READINESS.md` later | Low | §2.2: cross-reference §3 instead of restating the status, so the doc is the single source of truth. |
| Line numbers in the issue (`42-48`) are stale post-#27 | Done | §1.1 / §3 Step 1: relocate by content (`rg`), not by line number. |
| Scope creep into `PARITY.md` / `PLAN.md` / specs | Low | §1.4 table marks those out of scope (already accurate or planning docs). |

---

## 6. Test strategy

- **No new automated tests.** This is a comment reconciliation; the behavioral
  facts it describes are already asserted by `test/cli.test.ts` (`DEFAULT_ENGINE
  === 'ts'`, `py` fails closed with the pinned substring). Those must stay green
  unchanged.
- **Regression posture:** the `rg -n 'cutover.*is done' adw_sdlc/src/cli.ts`
  check (acceptance #1) is the repeatable guard an operator can re-run.
- **Optional follow-up (not this issue):** a doc-consistency guard that asserts
  no `src/` comment claims the cutover is "done" while `MVP-READINESS.md` marks
  `(C)` post-MVP. Note as a suggestion in the run summary; do not implement here.
- **Gate:** `npm run verify` proves typecheck/lint/pack/tests/build are intact.

---

## 7. Rollout / rollback

- **Rollout:** single comment-only change; ships through the normal phased
  pipeline. No migration, no flags, no runtime impact, zero blast radius.
- **Rollback:** revert the commit; pure documentation/comment.

---

## Summary of key decisions

- **Treat the issue as a one-sentence comment edit**, not a doc sweep: `cli.ts:37`
  is the only production-code line that claims the cutover is "done"; everything
  else in the repo already distinguishes "default flipped / unblocked" from the
  post-MVP milestone (§1.4).
- **Reconcile by separating concepts:** the `py → ts` **default flip is done in
  this port**; the cutover **milestone (PLAN step 12) is post-MVP**. The softened
  comment must keep (a) and drop the over-claim in (b).
- **Make `MVP-READINESS.md` §3 the single source of truth** by having `cli.ts`
  cross-reference it rather than re-asserting the status — preventing future
  divergence.
- **Do not touch the #27 fail-closed path or the test-pinned
  `PY_ENGINE_UNAVAILABLE` substring.**

## Assumptions

- "No contradiction remains" is satisfied by the `cli.ts` ↔ `MVP-READINESS.md`
  pair; `PARITY.md`/`PLAN.md` are already consistent (they say "unblocked" /
  describe the roadmap, not "done") and are out of scope.
- A comment-only edit cannot affect `pack:check` (it targets `cli.ts` source, not
  `.adw/prompts`), so no `npm run pack:generate` is required.
- The implementer may run `git`/`rg` read-only locally; per ADW rules the
  orchestrator performs all commits/branches/PRs.
- The recommended wording is a suggestion; any phrasing that satisfies §2's
  policy and the acceptance criteria is acceptable.

## Open questions

- **Include the stale test-name tidy (`test/cli.test.ts:45`,
  "defaults to py until cutover" → truthful name) in this change, or split it to
  its own item?** It is a related contradiction but a test-name string outside
  the issue's stated scope (§3 Step 4 / §5). Recommendation: include it as a
  trivial same-PR tidy since it is zero-risk and reinforces "no contradiction
  remains"; defer if the maintainer prefers strict scoping.
- **Should `specs/py-engine-not-available-in-standalone.md:35`
  ("cutover already done in this port") be reworded too?** It is a planning doc,
  not shipped guidance; left out here. Flag if the maintainer wants specs kept
  consistent with the softened wording.
