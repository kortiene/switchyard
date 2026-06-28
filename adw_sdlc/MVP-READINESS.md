# MVP readiness — open risks

This is the **counterweight to `PARITY.md`**. PARITY.md is a checklist of green
boxes; nearly all of them are **mocked-seam** evidence (every SDK/spawn/gh/git
effect stubbed — verified: no test in the suite spawns a real process or touches a
real network). Contact with reality now spans **nine** live `claude` runs — the
original seed (PR #331, which itself surfaced parity bug #332) plus an 8-issue
self-hosting batch (issues #1–#8 → merged PRs #9–#16 on `kortiene/switchyard`, see
[`docs/LIVE-RUN-BATCH.md`](./docs/LIVE-RUN-BATCH.md)). Measured over the batch,
native structured output has **0/36 hard-fails (0.0%)** but an **88.9% single-nudge
rate**. Still: those workspaces are git-ignored and self-referential (claude editing
this repo's own docs/tests), and the load-bearing failure-mode and secret-boundary
guarantees remain mock-only — so "all boxes green" means *the control plane is
internally consistent against stubs*, not *proven in production*. This doc tracks
what real MVP-readiness still requires.

Status legend: ✅ done · ⏳ owed (human/credential-gated) · ❌ not started ·
🔧 automatable in-repo.

---

## 0. Pick the MVP definition (gates everything)

The docs blur three very different bars. These were the three candidate bars;
**(A)** was chosen — see the recorded decision below.

- **(A) "`claude` autonomously ships real issues reliably."** Narrowest, most
  defensible. **Recommended MVP — adopted.**
- **(B) "four interchangeable runners."** PLAN.md's headline thesis — much larger.
  **(post-MVP.)**
- **(C) "cutover done"** — default flipped `py → ts`, py kept ≥ 1 release
  (PLAN step 12). **(post-MVP.)**

> **Decision (recorded):** ☑ **(A)** — MVP = "`claude` autonomously ships real
> issues reliably." **(B)** four-runner and **(C)** cutover are **post-MVP**
> (see §2 / §3). Decided 2026-06-25.

The rest of this doc adopts **(A)** and marks what **(B)/(C)** add.

---

## How to run a live `claude` issue

The canonical batch, run-command templates (native / forced-fenced /
subscription), failure-mode drills, and cost notes live in
[`docs/LIVE-RUN-BATCH.md`](./docs/LIVE-RUN-BATCH.md). Quick start for a single
issue, using the **`npm run verify`** gate as the one test command:

```bash
cd adw_sdlc
ADW_TEST_CMD="npm run verify" \
  npx tsx src/cli.ts <ISSUE_NUMBER> --runner claude --yes \
  --timeout 3600 --max-budget-usd 45
```

After the run, classify the artifacts (see the batch doc for the threshold):

```bash
npm run parity:rate -- agents/
```

> Use the single-command `ADW_TEST_CMD="npm run verify"` form: the orchestrator
> shell-splits the gate and runs it without a shell, so a chained `a && b` would
> fail. `npm run verify` already chains internally. See
> [`docs/LIVE-RUN-BATCH.md`](./docs/LIVE-RUN-BATCH.md) for the full templates.

---

## 1. Gates for (A) — claude ships reliably

- [x] ✅ **≥ 5–10 live `claude` runs across varied `issue_class`** — **8 done**
  (issues #1–#8 → merged PRs #9–#16, realized classes: docs ×3 + ci/feat/test/refactor/fix = 6 distinct),
  each fed to `npm run parity:rate -- agents/`. Caveat: every run targeted this
  repo's own docs/tests/CI (self-referential) and the workspaces are git-ignored, so
  it is a *thin* sample for "ships independent features." The batch + run-command
  templates live in [`docs/LIVE-RUN-BATCH.md`](./docs/LIVE-RUN-BATCH.md).
- [ ] 🔧/⏳ **The hard-failure bar is *measured*, not argued — partially closed.**
  Over the 8-run batch the **absolute** native rate is now measured: **0/36
  hard-fails (0.0%)**, with an **88.9% single-nudge rate** worth tracking (native
  rarely lands clean on the first try). The **comparative** bar (native ≤ fenced) is
  still `INSUFFICIENT DATA`: issue #1's forced-fenced run produced only **5 fenced
  attempts** (< 20 needed). Two ways to finish closing it, both wired:
  - **Absolute gate (claude-only):** `npm run parity:rate -- --max-native-rate <PCT> agents/`
    — already clearable today (0.0% native). Ratify `<PCT>` as the MVP threshold.
  - **Comparative gate (literal bar):** harvest ≥ 20 fenced attempts from claude with
    `ADW_PARITY_FORCE_FENCED_JSON=1` (routes the native runner through the fenced path),
    then the comparative verdict becomes computable without waiting on `pi`.
- [ ] ⏳ **Failure modes observed live, not just mocked.** Induce and confirm each
  once: a real nudge-retry that recovers, a `--timeout`-tripped fast-fail, a tiny
  `--max-budget` cap, and a kill-then-`--resume`. All are "mocked ✅"; none has
  been seen live.
- [ ] 🔧 **"Mocked ✅ → observed live?" ledger.** For each of PARITY's 13
  Section-10 guarantees, has it been seen live even once? The dashboard lives in
  [`docs/OBSERVED-LIVE-LEDGER.md`](./docs/OBSERVED-LIVE-LEDGER.md); it stands at
  **6 `✅` / 4 `🟡` / 2 `⏳` / 1 `N/A`** (seeded from PR #331 / run `007fd5ba` plus
  the 8-run batch — kept conservative because the batch artifacts are git-ignored
  and self-referential). The load-bearing #5 (secret boundary) and #8 (fast-fail)
  rows are still `⏳`. #332 is proof the mocks under-specify reality.
- [ ] ⏳ **Operational basics** for an agent that spends money and edits repos: a
  bounded cost envelope (~$35/run is real) with the `maxBudgetUsd` ceiling +
  kill-switch confirmed live; the secret boundary asserted once on a *real*
  spawned env (not only the lint + mocks); crash/cleanup behavior confirmed once.

## 2. Gates that (B) — four runners — adds (post-MVP)

_Post-MVP: not required for the (A) MVP. Listed for completeness._

- [ ] ⏳ A live run each for **codex / opencode / pi**, OR an explicit decision to
  ship claude-only and demote the rest to post-MVP.
- [ ] ⛔ **Unblock codex's credential.** It currently *cannot authenticate*
  (OAuth refresh revoked, possibly account-level — PARITY.md "real-issue runs").
  That is a hard blocker, not "owed". Prefer `OPENAI_API_KEY` (skips the OAuth
  refresh).
- [ ] ⏳ **pi needs Node ≥ 22.19** (the CI node-20 lane skips it) — bump CI or
  accept pi is unverified in CI.

## 3. Gates that (C) — cutover — adds (post-MVP)

_Post-MVP: not required for the (A) MVP. Listed for completeness._

- [ ] ❌ **`ADW_ENGINE` py↔ts coexistence tested in the *integrated* repo.**
  The Python sibling is not bundled here, so **(C) cannot be validated from this
  standalone port** — it needs the combined environment.
- [ ] ❌ Rollback plan: keep py ≥ 1 release, with a documented revert path.

## 4. Cross-cutting — the universalization surface (regardless of A/B/C)

Everything built since the parity baseline — declarative `cli`/`rest` providers,
transforms, pagination, custom phases, schema overrides — has unit tests and
**zero live validation** (a real GitLab `rest` provider has never touched a real
GitLab). **Decide scope:**

- [ ] In MVP scope → at least one live run against a real non-GitHub forge.
- [ ] Out of scope → mark it explicitly "post-MVP, unvalidated-in-anger" so the
  green unit tests are not mistaken for production-ready.

---

## Instruments (in-repo, ready)

- `tools/parity-rate.ts` (`npm run parity:rate`) — classifies every phase
  invocation from run artifacts and reports the per-path hard-failure rate;
  refuses to declare the bar met on a thin sample. Comparative bar by default;
  `--max-native-rate PCT` for the absolute gate.
- `ADW_PARITY_FORCE_FENCED_JSON=1` — routes a native-schema runner through the fenced
  path so a fenced baseline can be harvested from `claude` (no `pi` needed).
  Default off ⇒ behavior unchanged.

## What I can do vs. what needs a human

- **Automatable in-repo (🔧):** the measurement instruments above (done); a
  failure-induction harness; the "mocked → observed-live" ledger; a cost /
  secret-boundary live-audit scaffold; the universalization scope-line.
- **Not automatable (human + credentials + spend):** the actual live runs,
  unblocking codex's auth, real provider keys (opencode / pi / GitLab), and the
  maintainer's cutover sign-off.

## Recommended minimal path to a defensible (A)-MVP

1. Declare MVP = (A); demote (B)/(C) in the docs.
2. Set the MVP threshold and make the bar computable (`--max-native-rate`, and/or
   an `ADW_PARITY_FORCE_FENCED_JSON` baseline).
3. Run ~5–10 varied `claude` issues → `parity:rate` clears the threshold.
4. Induce + observe the four failure modes live.
5. Confirm the cost envelope + secret boundary live once.
6. Write the universalization scope-line.

Then "MVP-ready for `claude`" is an **audited** statement, not a self-attestation.
