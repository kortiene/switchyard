# Failure-mode drills — live `claude` runbook

These are **live** `claude` ADW drills. They produce the failure-mode evidence owed by
[`MVP-READINESS.md` §1](../MVP-READINESS.md#1-gates-for-a--claude-ships-reliably) and back rows
**#8** (bounded loops + no-retry-on-timeout) and **#9** (resume) of
[`docs/OBSERVED-LIVE-LEDGER.md`](./OBSERVED-LIVE-LEDGER.md). Each drill trips one fail-fast path in
the invoker layer (`src/run-phase.ts` `runAgentPhase`, branches at `run-phase.ts:135–145`); the
**deterministic** unit proof of the same mappings lives in
[`test/run-phase.test.ts`](../test/run-phase.test.ts) (`:191` timeout, `:202` budget, the
`cancelled` case alongside them), so this runbook only adds the *live* observation.

They cost a *small* amount of real money/time: the timeout and kill drills trip almost immediately
(the runner is aborted before it finishes), and the budget drill spends only up to its tiny cap.
**Running the drills is a separate, human-driven, money-spending step** — this runbook scaffolds the
procedure; capturing the evidence (and flipping ledger rows) is done by an operator with `claude`
credentials, not by an automated phase.

## How the mappings work (so the expected signal is precise)

`runAgentPhase` **parses output before it consults the signal** (`run-phase.ts:126–128`): a
timed-out or killed attempt that nonetheless produced parseable JSON is a *success* (Python parity —
`run-phase.test.ts:213`). The fast-fail-with-**no**-nudge path only fires when the killed/capped
attempt left **no** usable payload. The `signal` field (`src/invoker.ts:76`,
`'none' | 'timeout' | 'cancelled' | 'budget'`) then selects the verbatim error:

| `signal` | Trigger | Behavior | Error message (verbatim) |
| --- | --- | --- | --- |
| `timeout` | per-phase `AbortController` timer fires (`run-phase.ts:96–99`); `--timeout` is converted s→ms in `cli.ts` | fail fast, **no** nudge | `<phase> phase runner timed out without parseable output` |
| `budget` | claude's native cost cap (`caps.nativeBudget`, claude-only — `invoker.ts:100`); `--max-budget-usd` forwarded as `maxBudgetUsd` (`run-phase.ts:111`) | fail fast, **no** nudge | `<phase> phase hit the native budget cap without parseable output` |
| `cancelled` | the `AbortSignal` aborted for a non-timeout reason (operator kill / Ctrl-C) | fail fast, **no** nudge | `<phase> phase was cancelled without parseable output` |
| `none` | parse/validate failure on a clean run | **one** nudge retry, then fail | — |

## Preconditions

Mirror the [`LIVE-RUN-BATCH.md`](./LIVE-RUN-BATCH.md) preflight, minimally:

- Run from `adw_sdlc/`.
- `claude` is authenticated (`claude login`, or `ANTHROPIC_API_KEY` set).
- Use the single quality gate: `ADW_TEST_CMD="npm run verify"` (one command — the gate is not run
  through a shell, so `&&`-chained commands fail; see `LIVE-RUN-BATCH.md`).
- Pick a **low-risk** issue number (`<ISSUE>`) to target — reuse a batch issue's run rather than
  inventing spend.

`<ISSUE>` and `<ID>` below are operator-supplied placeholders. All commands run from `adw_sdlc/`.

## Drill 1 — Timeout fast-fail (`signal:'timeout'`)

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
*succeeds* instead — parse-first — so re-run to observe the fast-fail.)

**Where to look:** the failing phase under `agents/<ID>/<phase>/` (one transcript, no `-2`), and the
top-level error message.

## Drill 2 — Budget fast-fail (`signal:'budget'`)

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

**Where to look:** same as Drill 1 — a single transcript for the failing phase, budget error at the
top.

## Drill 3 — Kill, then resume (resume skips completed phases)

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
first unfinished phase. The run id is the one printed at start, `phased run id: <ID>`
(`orchestrator.ts:1062`). `--resume` without `--adw-id` errors with
`--resume requires --adw-id <id>` (`orchestrator.ts:964–965`), and a bare `--adw-id` *without*
`--resume` refuses to clobber existing state (`orchestrator.ts:974–975`).

**Where to look:** the resume run's stderr/progress log for the `skipping ... (already completed)`
lines, and the unchanged `agents/<ID>/` directories for the skipped phases.

## Opportunistic — nudge-retry (not a scripted drill)

There is **no** command here that forces a nudge, because forcing one would require telling the
agent to violate the JSON output contract on a real issue (`LIVE-RUN-BATCH.md:113–115`, issue #4
Notes) — output-contract integrity is load-bearing, so we don't script that. If a real run
*naturally* produces a recoverable parse failure, the single nudge retry (`run-phase.ts:151–156`,
which writes `transcript-2.log`) can be captured opportunistically. The deterministic proof that the
nudge happens exactly once already lives in `run-phase.test.ts` (the nudge-retry case).

## After a drill — recording evidence

Record results in [`docs/OBSERVED-LIVE-LEDGER.md`](./OBSERVED-LIVE-LEDGER.md) following its
"How to update after a live `claude` run" procedure (rows **#8** and **#9** are the ones these drills
back). Cite the **run id** and the **artifact path** (`agents/<ID>/<phase>/…`) backing each claim,
and do not overclaim: a row flips to `✅ observed-live` only on cited evidence from a real run — a
fast-fail you *expected* but did not actually observe live stays `⏳`.

## See also

- [`docs/LIVE-RUN-BATCH.md`](./LIVE-RUN-BATCH.md) — the issue batch and live-run command templates
  (this runbook expands its "Failure-mode drills" section).
- [`MVP-READINESS.md` §1](../MVP-READINESS.md#1-gates-for-a--claude-ships-reliably) — the gate these
  drills feed ("failure modes observed live, not just mocked").
- [`test/run-phase.test.ts`](../test/run-phase.test.ts) — the deterministic, mocked counterpart that
  pins the same signal→behavior mappings in CI.
