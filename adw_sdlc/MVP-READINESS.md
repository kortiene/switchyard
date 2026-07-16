# MVP readiness — open risks

This is the **counterweight to `PARITY.md`**. PARITY.md is a checklist of green
boxes; nearly all of them are **mocked-seam** evidence (most SDK/spawn/gh/git
effects stubbed at the seam — though a few tests now deliberately cross it: the
secret-boundary audit (`test/secret-boundary-audit.test.ts`) and the verify-gate
e2e test (`test/verify-gate.e2e.test.ts`) spawn real subprocesses, and the rest
transport loopback suite (`test/providers-rest-transport.test.ts`) drives a real
localhost round-trip). Contact with reality now spans **thirteen** live `claude` ADW
run ids — the original seed, an 8-issue self-hosting batch, three issue-#20
failure-drill carriers, and the active-phase recovery run — plus targeted
real-spawn boundary/routing/veto probes. See
[`docs/LIVE-RUN-BATCH.md`](./docs/LIVE-RUN-BATCH.md) and the sanitized
[`test/fixtures/live-evidence/`](./test/fixtures/live-evidence/) corpus. Measured
over the Claude batch, native structured output has **0/36 hard-fails (0.0%)** but
an **88.9% single-nudge rate**. The load-bearing timeout/budget/resume,
secret-boundary, tool-veto, and unattended-merge controls have now also been
induced live and archived. The sample is still self-referential (Claude editing
this repo's own docs/tests), so this doc keeps the remaining breadth and
maintainer-sign-off caveats explicit.

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
  - **Absolute gate (claude-only):** `npm run parity:rate -- --max-native-rate 10 agents/`
    — **ratified MVP threshold: ≤ 10% hard-fail** (decided #29); clears today at 0.0%.
  - **Comparative gate (literal bar):** harvest ≥ 20 fenced attempts from claude with
    `ADW_PARITY_FORCE_FENCED_JSON=1` (routes the native runner through the fenced path),
    then the comparative verdict becomes computable without waiting on `pi`.
- [x] ✅ **Failure modes observed live, not just mocked.** The 8-run batch contains
  recovered single-nudge attempts; issue #20 added a real timeout fast-fail (run
  `a6b4e6dc`), native tiny-budget fast-fail (`b20d9e02`), active-Claude
  kill-then-restart (`57b6bfea`), and completed-phase skip (`c20e5a01`). The
  timeout and budget artifacts each have one transcript and no nudge transcript;
  the two resume runs directly cover rerunning an incomplete phase and skipping a
  completed one.
- [x] ✅ **"Mocked ✅ → observed live?" ledger.** For each of PARITY's 13
  Section-10 guarantees, has it been seen live even once? The dashboard lives in
  [`docs/OBSERVED-LIVE-LEDGER.md`](./docs/OBSERVED-LIVE-LEDGER.md); it now stands at
  **12 `✅` / 0 `🟡` / 0 `⏳` / 1 `N/A`**. Issues #20–#23 independently closed the
  six former partial/owed rows with sanitized, CI-guarded artifacts. Comparative
  structured-output sampling and cross-language finalization remain separately
  qualified rather than hidden by the tally.
- [x] ✅ **Operational basics** for an agent that spends money and edits repos.
  Issue #22 reuses issue #20's evidence: Claude's native `$0.01` cap fired; the
  completed `c20e5a01` review recorded **$0.893283** under a `$45` ceiling;
  process-group SIGINT (the Ctrl-C signal) interrupted a live Claude subprocess in
  `57b6bfea`, left the recorded process group empty and made no tracked changes, and
  preserved byte-identical pre-phase state across the interruption. The same run then
  resumed and persisted `review` successfully; its interrupted-attempt spend is not
  claimed. Issue #21's paired real-spawn audit records all five poisoned parent key
  names present and zero denied names in the Claude child, without recording values.

## 2. Gates that (B) — four runners — adds (post-MVP)

_Post-MVP: not required for the (A) MVP. Listed for completeness._

- [x] ✅ **Decided (2026-06-28): ship claude-only for the MVP**; codex / opencode / pi
  live runs are demoted to post-MVP (tracked as issues #31 / #32 / #33 under M4).
- [ ] ⛔ **codex credential — shelved to post-MVP (decided #33, 2026-06-28).** It
  currently *cannot authenticate* (OAuth refresh revoked, possibly account-level —
  PARITY.md "real-issue runs"). Revisit via `OPENAI_API_KEY` (skips the OAuth
  refresh) when (B) is taken up.
- [ ] ⏳ **pi needs Node ≥ 22.19** — the CI Node-version matrix (#37) now runs a
  Node-20.19.0 floor leg alongside Node 22, and the 20.19.0 leg cannot load pi
  (its npm engines floor is `>=22.19.0`, optional dep skipped), so pi is
  exercised only on the Node-22 leg. That leg runs the mocked suite; a real-issue
  pi run (live provider key) is still owed — see PARITY.md.

## 3. Gates that (C) — cutover — adds (post-MVP)

_Post-MVP: not required for the (A) MVP. Listed for completeness._

- [ ] ❌ **`ADW_ENGINE` py↔ts coexistence tested in the *integrated* repo.**
  The Python sibling is not bundled here, so **(C) cannot be validated from this
  standalone port** — it needs the combined environment. In the standalone port
  the `py` path now **fails closed explicitly** (selecting it raises a "not
  available in this standalone distribution" `AdwError`; issue #27); the
  coexistence gate itself stays ❌/post-MVP.
- [ ] ❌ Rollback plan: keep py ≥ 1 release, with a documented revert path.

## 4. Cross-cutting — the universalization surface (regardless of A/B/C)

Everything built since the parity baseline — declarative `cli`/`rest` providers,
transforms, pagination, custom phases, schema overrides — has unit tests and
**zero live validation** (a real GitLab `rest` provider has never touched a real
GitLab). **Decide scope:**

- [ ] In MVP scope → at least one live run against a real non-GitHub forge.
- [x] **Out of scope — DECIDED (#35, 2026-06-28).** The declarative cli/rest provider
  surface is **post-MVP, unvalidated-in-anger**: it ships with unit tests only and is
  not validated against a real non-GitHub forge. The (A) MVP is claude + GitHub only.

---

## Instruments (in-repo, ready)

- `tools/parity-rate.ts` (`npm run parity:rate`) — classifies every phase
  invocation from run artifacts and reports the per-path hard-failure rate;
  refuses to declare the bar met on a thin sample. Comparative bar by default;
  `--max-native-rate PCT` for the absolute gate.
- `ADW_PARITY_FORCE_FENCED_JSON=1` — routes a native-schema runner through the fenced
  path so a fenced baseline can be harvested from `claude` (no `pi` needed).
  Default off ⇒ behavior unchanged.
- `test/fixtures/parity-runs/` — the 8-run batch evidence (each run's `state.json` +
  per-phase `prompt.txt`; transcripts as presence-markers), committed so that
  `npm run parity:rate -- test/fixtures/parity-runs/` reproduces the rate from a clean
  clone. `test/parity-evidence.test.ts` re-derives the documented figures under
  `npm run verify`.
- `test/fixtures/live-evidence/` — sanitized issues #20–#23 operational evidence:
  timeout/budget no-retry, active-phase restart, completed-phase skip, cleanup and
  cost, real-spawn secret names, exact model routes, live git/gh veto, merge refusal,
  and cross-language state resume. `test/live-evidence.test.ts` guards the claims
  without vendoring prompts, full transcripts, or secret values.

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
4. ✅ Induce + observe the four failure modes live.
5. ✅ Confirm the cost envelope + secret boundary live once.
6. Write the universalization scope-line.

Then "MVP-ready for `claude`" is an **audited** statement, not a self-attestation.
