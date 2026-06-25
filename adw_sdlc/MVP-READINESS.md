# MVP readiness — open risks

This is the **counterweight to `PARITY.md`**. PARITY.md is a checklist of green
boxes; nearly all of them are **mocked-seam** evidence (every SDK/spawn/gh/git
effect stubbed — verified: no test in the suite spawns a real process or touches a
real network). The only contact with reality is **one** live `claude` run
(PR #331), which itself surfaced a parity bug (#332). So "all boxes green" means
*the control plane is internally consistent against stubs* — not *proven in
production*. This doc tracks what real MVP-readiness still requires.

Status legend: ✅ done · ⏳ owed (human/credential-gated) · ❌ not started ·
🔧 automatable in-repo.

---

## 0. Pick the MVP definition (gates everything)

The docs blur three very different bars. **Decide first** — every gate below
depends on it.

- **(A) "`claude` autonomously ships real issues reliably."** Narrowest, most
  defensible. **Recommended MVP.**
- **(B) "four interchangeable runners."** PLAN.md's headline thesis — much larger.
- **(C) "cutover done"** — default flipped `py → ts`, py kept ≥ 1 release
  (PLAN step 12).

> **Decision (record here):** ☐ A ☐ B ☐ C — _unset_.

The rest of this doc assumes **(A)** and marks what **(B)/(C)** add.

---

## 1. Gates for (A) — claude ships reliably

- [ ] ⏳ **≥ 5–10 live `claude` runs across varied `issue_class`** (feat / fix /
  docs / refactor / ci / test). One green run is an anecdote; different classes
  exercise the e2e/document gates and the resolve/patch loops. Feed each to
  `npm run parity:rate -- agents/`.
- [ ] 🔧/⏳ **The hard-failure bar is *measured*, not argued.** PARITY's bar is
  native ≤ fenced, but there is **no fenced sample** yet. Two ways to close it,
  both now wired:
  - **Absolute gate (claude-only):** `npm run parity:rate -- --max-native-rate <PCT> agents/`
    — evaluable from native runs alone, so the bar stops reading INSUFFICIENT the
    moment a few claude runs exist. Pick `<PCT>` as the MVP threshold.
  - **Comparative gate (literal bar):** harvest a fenced baseline from claude with
    `MX_AGENT_FORCE_FENCED=1` (routes the native runner through the fenced path),
    then the comparative verdict becomes computable without waiting on `pi`.
- [ ] ⏳ **Failure modes observed live, not just mocked.** Induce and confirm each
  once: a real nudge-retry that recovers, a `--timeout`-tripped fast-fail, a tiny
  `--max-budget` cap, and a kill-then-`--resume`. All are "mocked ✅"; none has
  been seen live.
- [ ] ❌ **"Mocked ✅ → observed live?" ledger.** For each of PARITY's 12 boxes,
  has it been seen live even once? Today almost none have. That ledger is the real
  readiness dashboard; #332 is proof the mocks under-specify reality.
- [ ] ⏳ **Operational basics** for an agent that spends money and edits repos: a
  bounded cost envelope (~$35/run is real) with the `maxBudgetUsd` ceiling +
  kill-switch confirmed live; the secret boundary asserted once on a *real*
  spawned env (not only the lint + mocks); crash/cleanup behavior confirmed once.

## 2. Gates that (B) — four runners — adds

- [ ] ⏳ A live run each for **codex / opencode / pi**, OR an explicit decision to
  ship claude-only and demote the rest to post-MVP.
- [ ] ⛔ **Unblock codex's credential.** It currently *cannot authenticate*
  (OAuth refresh revoked, possibly account-level — PARITY.md "real-issue runs").
  That is a hard blocker, not "owed". Prefer `OPENAI_API_KEY` (skips the OAuth
  refresh).
- [ ] ⏳ **pi needs Node ≥ 22.19** (the CI node-20 lane skips it) — bump CI or
  accept pi is unverified in CI.

## 3. Gates that (C) — cutover — adds

- [ ] ❌ **`MX_AGENT_ENGINE` py↔ts coexistence tested in the *integrated* repo.**
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
- `MX_AGENT_FORCE_FENCED=1` — routes a native-schema runner through the fenced
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
   a `MX_AGENT_FORCE_FENCED` baseline).
3. Run ~5–10 varied `claude` issues → `parity:rate` clears the threshold.
4. Induce + observe the four failure modes live.
5. Confirm the cost envelope + secret boundary live once.
6. Write the universalization scope-line.

Then "MVP-ready for `claude`" is an **audited** statement, not a self-attestation.
