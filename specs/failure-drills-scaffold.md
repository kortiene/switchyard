# Spec: failure-drill scaffold (timeout / budget / resume)

- **Work item:** GitHub issue #4 — `test: failure-drill scaffold (timeout/budget/resume)`
- **Labels / class:** `issue_class:test`, `adw-live-batch`
- **Planned ADW run mode:** native (`docs/LIVE-RUN-BATCH.md` row 4)
- **Advances:** `MVP-READINESS.md` §1 "Failure modes observed live, not just mocked" and the
  `docs/OBSERVED-LIVE-LEDGER.md` rows #8 (bounded loops + no-retry-on-timeout) and #9 (resume).
- **Status:** specification only. **Do not implement as part of this phase.**

---

## 1. Goal

Provide a small, documented procedure — plus one optional deterministic test — that lets an
operator exercise three ADW failure modes against a real `claude` run:

1. `--timeout` fast-fail (the `signal:'timeout'` no-nudge mapping),
2. `--max-budget-usd` cap (the `signal:'budget'` no-nudge mapping),
3. kill-then-`--resume` (resume skips already-completed phases).

The deliverable is primarily a **documentation artifact** (`adw_sdlc/docs/FAILURE-DRILLS.md`) that
turns the terse bullet list now in `docs/LIVE-RUN-BATCH.md` (lines ~104–115) into a standalone,
copy-paste runbook with an explicit "expected signal" for each drill. The optional test closes a
real unit-coverage gap (the `cancelled` signal) rather than duplicating coverage that already exists.

This issue does **not** require running the live drills. It scaffolds the procedure; capturing the
live evidence is a separate, human-driven, money-spending step (`MVP-READINESS.md` §"What I can do
vs. what needs a human").

---

## 2. Background — how the mappings actually work (verify before writing)

The fail-fast behavior all lives in **one** place, the invoker layer, never per-adapter
(`adw_sdlc/src/run-phase.ts:runAgentPhase`). A phase result carries a `signal` field
(`adw_sdlc/src/invoker.ts:76`): `'none' | 'timeout' | 'cancelled' | 'budget'`. After the first
attempt fails to parse/validate, `runAgentPhase` branches on `signal` (`run-phase.ts:135–145`):

| `signal` | Source | Behavior | AdwError message (verbatim, for the doc/test) |
| --- | --- | --- | --- |
| `timeout` | per-phase `AbortController` timer fires (`run-phase.ts:96–99`); the orchestrator-owned `--timeout` is converted s→ms in the CLI (`cli.ts:330`) | **fail fast, no nudge** | `<phase> phase runner timed out without parseable output` |
| `budget`  | claude's native cost cap (`caps.nativeBudget` true for claude only — `invoker.ts:99`); `--max-budget-usd` forwarded as `maxBudgetUsd` (`run-phase.ts:111`) | **fail fast, no nudge** | `<phase> phase hit the native budget cap without parseable output` |
| `cancelled` | the same `AbortSignal` aborted for a non-timeout reason (operator kill / Ctrl-C path) | **fail fast, no nudge** | `<phase> phase was cancelled without parseable output` |
| `none` (parse/validate fail on a clean run) | — | **one** nudge retry, then fail | — |

Important nuance to preserve in the doc: **output is parsed before the signal is consulted**
(`run-phase.ts:126–128`). A timed-out or killed run that nonetheless produced parseable JSON is a
*success* (Python parity — see `run-phase.test.ts:213` "still accepts parseable output from a
timed-out run"). So the "expected signal" for each drill is the *fast-fail-with-no-nudge* path,
which only triggers when the killed/capped attempt has **no** usable payload.

Resume mechanics (`adw_sdlc/src/orchestrator.ts`):

- At run start the orchestrator prints the run id the operator needs for resume:
  `note(\`phased run id: <id> (workspace: ...)\`)` (`orchestrator.ts:1062`).
- `--resume` requires `--adw-id` (`orchestrator.ts:964`, `resolveState`). A bare `--adw-id` without
  `--resume` refuses to clobber existing state (`orchestrator.ts:975`).
- Per phase, `if (state.isDone(phase))` emits `note(\`skipping <phase> (already completed)\`)` and
  continues (`orchestrator.ts:1114–1116`). Phases are marked done + `state.save()`d as they
  complete, so a kill mid-run preserves all phases that finished before the kill.

### What is already covered (do not duplicate)

`adw_sdlc/test/run-phase.test.ts` **already** asserts the two mappings the issue's optional test
names:

- `run-phase.test.ts:191` — "fails fast with NO nudge on timeout" (asserts one request, no
  `transcript-2.log`, error matches `/timed out/`).
- `run-phase.test.ts:202` — "fails fast with NO nudge on claude's native budget signal" (asserts one
  request, `maxBudgetUsd` forwarded, error matches `/budget/`).

The issue body says "if not already covered" — **timeout and budget ARE already covered.** The genuine
gap is the `cancelled` signal (the kill analog, relevant to the resume drill), which has **no** test.
The optional test should therefore add the `cancelled` case, not re-add timeout/budget.

---

## 3. Scope

**In scope**

- New file `adw_sdlc/docs/FAILURE-DRILLS.md` — the runbook: per-drill purpose, exact copy-paste
  command, and expected signal/outcome.
- Optional, recommended: extend `adw_sdlc/test/run-phase.test.ts` with the missing `cancelled`
  fail-fast-no-nudge case (mocked runner; deterministic; no network/spend).
- Minimal cross-links so the new doc is discoverable: a one-line pointer from
  `docs/LIVE-RUN-BATCH.md` (its existing "Failure-mode drills" section) and a row in the README
  "Documentation map" table (`adw_sdlc/README.md` lines ~166–183).

**Out of scope**

- Running the live drills or capturing live evidence (separate human/spend task).
- Inducing the nudge-retry drill. Per the issue Notes and `LIVE-RUN-BATCH.md:113–115`, **do not**
  instruct the agent to violate the output contract to force a nudge; that drill is opportunistic.
  The doc may *mention* it as opportunistic-only, but must not provide a "make the agent misbehave"
  recipe.
- Any change to invoker/orchestrator behavior, signals, or CLI flags. This is docs + test only.
- Flipping `OBSERVED-LIVE-LEDGER.md` rows to `✅ observed-live` (that requires real run evidence,
  which this issue does not produce).

---

## 4. Implementation steps

### Step 1 — Author `adw_sdlc/docs/FAILURE-DRILLS.md`

Create the file with this structure. Keep prose tight and operator-facing. Anchor every claim to the
behavior in §2; do not invent flags or messages.

1. **Title + one-paragraph purpose.** State that these are *live* `claude` drills that produce the
   `MVP-READINESS.md` §1 failure-mode evidence, and that the deterministic unit proof of the same
   mappings lives in `test/run-phase.test.ts`. Note they cost a *small* amount of real money/time
   (the timeout/kill drills trip almost immediately; the budget drill spends up to the tiny cap).
2. **Preconditions** block — mirror `LIVE-RUN-BATCH.md` "Preflight" minimally: run from
   `adw_sdlc/`, `claude` authenticated, `ADW_TEST_CMD="npm run verify"`, and a low-risk issue number
   to target. Reuse a batch issue's run rather than inventing spend.
3. **Drill 1 — Timeout fast-fail.** Command, expected outcome, where to look. (Content in §5 below.)
4. **Drill 2 — Budget fast-fail.** Same shape.
5. **Drill 3 — Kill, then resume.** Same shape, two commands (run+kill, then resume).
6. **Opportunistic: nudge-retry.** A short paragraph stating it is *not* a scripted drill here, why
   (output-contract integrity), and that it is captured opportunistically if a real run naturally
   produces a recoverable parse failure. No recipe to force misbehavior.
7. **After a drill — recording evidence.** Point at `docs/OBSERVED-LIVE-LEDGER.md` rows #8/#9 and its
   "How to update after a live `claude` run" procedure; remind the author to cite run id + artifact
   path and not to overclaim.
8. **Cross-references.** Link back to `docs/LIVE-RUN-BATCH.md`, `MVP-READINESS.md` §1, and
   `test/run-phase.test.ts` (the deterministic counterpart).

### Step 2 — (Optional, recommended) extend `test/run-phase.test.ts` with the `cancelled` case

Add **one** focused test next to the existing timeout/budget cases (after `run-phase.test.ts:211`).
Do not duplicate timeout/budget. Suggested test (deterministic, mocked, no spend):

```ts
it('fails fast with NO nudge on a cancelled signal (operator kill)', async () => {
  const runner = createMockRunner({
    script: () => ({ ok: false, rc: 1, signal: 'cancelled', transcriptText: 'killed mid-phase' }),
  });
  await expect(
    runAgentPhase({ phase: 'resolve', templateArgs: ['x'], state, runner, env: {} }),
  ).rejects.toThrow(/cancelled/);
  expect(runner.requests).toHaveLength(1); // no nudge retry
  expect(existsSync(join(tmp, 'a1b2c3d4', 'resolve', 'transcript-2.log'))).toBe(false);
});
```

Notes for the implementer:
- `existsSync` and `join` are already imported at the top of the test file (`run-phase.test.ts:8–10`).
- This asserts the `signal:'cancelled'` branch of `run-phase.ts:143–145`, which is currently
  unverified, and matches the AdwError text `... was cancelled without parseable output`.
- If the maintainer prefers docs-only, this step is droppable — the acceptance criteria say the test
  is *optional* — but adding it is cheap, deterministic, and closes a real gap.

### Step 3 — Discoverability cross-links (small)

- In `docs/LIVE-RUN-BATCH.md`, under the existing "Failure-mode drills" heading, add a one-line
  pointer: "Full runbook: [`FAILURE-DRILLS.md`](./FAILURE-DRILLS.md)." Do not delete the existing
  summary bullets (they remain a useful index); just link the detail doc.
- In `adw_sdlc/README.md` "Documentation map" table, add a row for `docs/FAILURE-DRILLS.md`.

### Step 4 — Verify

From `adw_sdlc/`: `npm run verify` (typecheck → lint:env → pack:check → test → build → rm -rf dist).
If only the doc changed, the doc-only path still runs the full gate; it must stay green. No
`.adw/pack.profile.json` change is involved, so no `pack:generate` is needed.

---

## 5. The runbook content (what goes in `FAILURE-DRILLS.md`)

These are the prescribed drill bodies. The implementer should paste/adapt these; placeholders
`<ISSUE>` and `<ID>` are operator-supplied. All commands run from `adw_sdlc/`.

### Drill 1 — Timeout fast-fail (`signal:'timeout'`)

```bash
cd adw_sdlc
ADW_TEST_CMD="npm run verify" \
  npx tsx src/cli.ts <ISSUE> --runner claude --yes \
  --timeout 1 --max-budget-usd 45
```

**Expected outcome:** the first agentic phase's runner call is aborted ~1 s in. With no parseable
payload, the run fails fast with **no** nudge retry. The error reads
`<phase> phase runner timed out without parseable output`. Only `transcript.log` exists for that
phase — **no** `transcript-2.log`. (If the killed attempt happened to emit parseable JSON, the phase
*succeeds* instead — re-run to observe the fast-fail.)

**Where to look:** the failing phase under `agents/<ID>/<phase>/` (one transcript, no `-2`), and the
top-level error message.

### Drill 2 — Budget fast-fail (`signal:'budget'`)

```bash
cd adw_sdlc
ADW_TEST_CMD="npm run verify" \
  npx tsx src/cli.ts <ISSUE> --runner claude --yes \
  --timeout 3600 --max-budget-usd 0.01
```

**Expected outcome:** claude's native budget gate (claude is the only runner with
`caps.nativeBudget`) trips at the `$0.01` cap and returns `signal:'budget'`. With no parseable
payload the run fails fast, no nudge: `<phase> phase hit the native budget cap without parseable
output`. Spend is bounded by the tiny cap.

**Where to look:** same as Drill 1 — single transcript for the failing phase, budget error at top.

### Drill 3 — Kill, then resume (resume skips completed phases)

```bash
# 1) Start a normal run; note the printed "phased run id: <ID>".
cd adw_sdlc
ADW_TEST_CMD="npm run verify" \
  npx tsx src/cli.ts <ISSUE> --runner claude --yes \
  --timeout 3600 --max-budget-usd 45
# 2) Let at least one phase complete (watch the progress log), then Ctrl-C.

# 3) Resume the same run by its id:
ADW_TEST_CMD="npm run verify" \
  npx tsx src/cli.ts <ISSUE> --runner claude --yes \
  --resume --adw-id <ID> \
  --timeout 3600 --max-budget-usd 45
```

**Expected outcome:** on resume the orchestrator prints `skipping <phase> (already completed)` for
every phase that finished before the kill (`orchestrator.ts:1114–1116`), then continues from the
first unfinished phase. The run id is the one printed at start (`orchestrator.ts:1062`); `--resume`
without `--adw-id` errors with `--resume requires --adw-id <id>`.

**Where to look:** the resume run's stderr/progress log for the `skipping ... (already completed)`
lines, and the unchanged `agents/<ID>/` directories for the skipped phases.

### Opportunistic — nudge-retry (not a scripted drill)

State plainly: there is **no** command here that forces a nudge, because that would require telling
the agent to violate the JSON output contract on a real issue (`LIVE-RUN-BATCH.md:113–115`, issue #4
Notes). If a real run naturally produces a recoverable parse failure, the single nudge retry
(`run-phase.ts:151–156`, writing `transcript-2.log`) can be captured opportunistically. The
deterministic proof that the nudge happens exactly once already lives in `run-phase.test.ts:96`.

---

## 6. Acceptance criteria (from the issue, mapped)

- [ ] **Each of the three drills has a copy-paste command and expected outcome.** → §5 Drills 1–3,
      rendered into `FAILURE-DRILLS.md` (Step 1).
- [ ] **Any added test is deterministic (mocked runner; no network/spend).** → Step 2 uses
      `createMockRunner` with a scripted `signal:'cancelled'`; no I/O beyond the existing tmpdir
      harness. (Timeout/budget already covered — the new test adds only `cancelled`.)
- [ ] **`npm run verify` stays green.** → Step 4.

Additional done-checks for the reviewer:
- [ ] `FAILURE-DRILLS.md` does not contain a recipe to force contract violations / nudge-retry.
- [ ] Expected-signal text matches the verbatim AdwError strings in §2 (no paraphrase drift).
- [ ] Cross-links added (Step 3) so the doc is reachable from `LIVE-RUN-BATCH.md` and the README map.
- [ ] No production code (`src/`) changed; only `docs/`, `README.md`, and (optionally) the test.

---

## 7. Risks & mitigations

- **Doc/behavior drift.** If the AdwError wording or `signal` enum later changes, the runbook's
  "expected outcome" text goes stale. *Mitigation:* cite the exact source lines (`run-phase.ts`
  branches) in the doc so a future editor can re-verify; keep the wording quoted, not paraphrased.
  The optional `cancelled` test also pins the message in CI.
- **Over-duplication.** Re-adding timeout/budget tests would duplicate `run-phase.test.ts:191/202`.
  *Mitigation:* §2 explicitly scopes the optional test to the uncovered `cancelled` case only.
- **`--timeout 1` flakiness.** A `1 s` box could, in principle, let a phase finish with parseable
  output and *not* fast-fail. *Mitigation:* the doc tells the operator this is expected behavior
  (parse-first) and to re-run; it is the same value already prescribed in `LIVE-RUN-BATCH.md:108`.
- **Budget cap below a single-call cost.** `$0.01` is below any real phase cost, so the cap should
  trip on the first call. If a backend refuses to start under the cap, the operator still sees a
  fast-fail; the doc frames the signal as "budget/fail-fast", not a specific spend amount.
- **Scope creep into live evidence.** Tempting to "just run one drill." *Mitigation:* the spec and
  doc both state evidence capture is a separate human/spend step; this issue only scaffolds.

---

## 8. Test & verification strategy

- **Unit (deterministic):** the optional `cancelled` test in `run-phase.test.ts` — mocked runner,
  no network, no spend — run via `npm test` / `npm run verify`.
- **Doc lint:** `npm run verify` includes `pack:check`; the new doc is not a prompt template, so it
  does not affect the pack. Confirm the full gate is green.
- **Manual (out of scope here, documented for the operator):** the three live drills in §5, executed
  by a human with `claude` credentials and a small budget, recording evidence into
  `OBSERVED-LIVE-LEDGER.md`.

---

## 9. Assumptions

- `FAILURE-DRILLS.md` belongs under `adw_sdlc/docs/` (alongside `LIVE-RUN-BATCH.md` and
  `OBSERVED-LIVE-LEDGER.md`), matching the path named in the issue Scope.
- The existing `LIVE-RUN-BATCH.md` "Failure-mode drills" bullets stay as a short index; the new file
  is the authoritative detail. (Alternative: move the bullets entirely into the new doc — left to
  the implementer, but the spec recommends keep-and-link to minimize churn.)
- The mocked-runner harness and imports in `run-phase.test.ts` are sufficient for the `cancelled`
  test with no new test helpers (confirmed: `createMockRunner`, `existsSync`, `join`, `tmp`, `state`
  are all already in scope).
- This is an automated ADW phase: the orchestrator performs all git/gh work; this spec does not
  branch, commit, or open PRs.

## 10. Open questions

1. **Keep vs. move the `LIVE-RUN-BATCH.md` bullets?** Spec recommends keep-and-link; confirm the
   maintainer doesn't prefer a single source of truth (move + redirect).
2. **Include the optional `cancelled` test, or stay docs-only?** Recommended to include (cheap,
   deterministic, closes the one uncovered fail-fast branch). Confirm the maintainer wants the test
   change inside a `test`-class issue (it is in-character for the class).
3. **Should the doc enumerate a concrete low-risk `<ISSUE>` to reuse,** or stay generic with a
   placeholder? Spec uses a placeholder to avoid coupling to a specific batch issue's lifecycle.
