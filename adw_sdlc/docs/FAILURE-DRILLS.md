# Failure-mode drills — live `claude` runbook

These are **live** `claude` ADW drills. They produce the failure-mode evidence owed by
[`MVP-READINESS.md` §1](../MVP-READINESS.md#1-gates-for-a--claude-ships-reliably) and back rows
**#8** (bounded loops + no-retry-on-timeout) and **#9** (resume) of
[`docs/OBSERVED-LIVE-LEDGER.md`](./OBSERVED-LIVE-LEDGER.md). Each drill trips one fail-fast path in
the invoker layer (`src/run-phase.ts` `runAgentPhase`, branches at `run-phase.ts:135–145`); the
**deterministic** unit proof of the same mappings lives in
[`test/run-phase.test.ts`](../test/run-phase.test.ts) (`:191` timeout, `:202` budget, the
`cancelled` case alongside them), so this runbook only adds the *live* observation.

They cost real money/time. A timeout normally trips quickly, but Claude's native budget check is a
turn boundary rather than a prepaid reservation: one in-flight turn can make observed spend exceed
the requested cap. Run the drills only in an isolated worktree and retain their artifacts. The
2026-07-16 observations are archived under
[`test/fixtures/live-evidence`](../test/fixtures/live-evidence); this runbook remains the safe recipe
for repeating them.

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
| `cancelled` | a caller programmatically aborts the runner's `AbortSignal` for a non-timeout reason | fail fast, **no** nudge | `<phase> phase was cancelled without parseable output` |
| `none` | parse/validate failure on a clean run | **one** nudge retry, then fail | — |

The current CLI does **not** install a SIGINT/SIGTERM handler that converts Ctrl-C into the
`cancelled` runner signal. Ctrl-C terminates the orchestrator process. The kill/resume drill below
therefore proves process termination, child cleanup, durable state, and resume semantics; the
deterministic `cancelled` mapping remains covered by `test/run-phase.test.ts`.

A failed timeout or native-budget attempt also has no `metrics.json` row for the failed phase:
`runAgentPhase` throws before the orchestrator can call `recordUsage` or save metrics. Prove those
failures with the error, transcript cardinality, and runner audit—not by expecting a zero-cost
metrics record.

## Preconditions

Mirror the [`LIVE-RUN-BATCH.md`](./LIVE-RUN-BATCH.md) preflight, minimally:

- Run from `adw_sdlc/`.
- `claude` is authenticated (`claude login`, or `ANTHROPIC_API_KEY` set).
- Pick a **low-risk** issue number (`<ISSUE>`) to target—reuse a batch issue's run rather than
  inventing spend.
- Add a detached worktree at a clean revision and pass it as `--project-root`. Do not aim a
  destructive drill at a dirty operator checkout.
- Use `--no-progress --no-merge` and the inert `sleep 300` gate. The gate keeps a successfully
  resumed run away from commit/push/PR side effects while giving the operator a deterministic place
  to stop it.
- Wrap timeout/budget invocations in an outer process guard. The outer guard is emergency cleanup;
  evidence is valid only when the ADW's **inner** timeout/budget error appears first.
- Do **not** add `--yes`: it opts into unattended merge if a drill unexpectedly reaches finalize.

Set up the isolated target once (replace `<OWNER/REPO>` and `<ISSUE>` below):

```bash
ROOT="$(git rev-parse --show-toplevel)"
DRILL_ROOT="/tmp/switchyard-live-drill"
git -C "$ROOT" worktree add --detach "$DRILL_ROOT" origin/main
cd "$ROOT/adw_sdlc"
```

`<ID>` below is the run id printed by the orchestrator. Commands execute the current ADW package
from `adw_sdlc/`, but all issue-branch and workspace mutations stay under `$DRILL_ROOT`.

## Drill 1 — Timeout fast-fail (`signal:'timeout'`)

```bash
timeout --signal=INT --kill-after=5s 30s \
  npx tsx src/cli.ts <ISSUE> --repo <OWNER/REPO> --runner claude \
  --phases plan --project-root "$DRILL_ROOT" \
  --timeout 1 --max-budget-usd 45 --test-cmd 'sleep 300' \
  --no-progress --no-merge
```

**Expected outcome:** the first agentic phase's runner call is aborted ~1 s in. With no parseable
payload, the run fails fast with **no** nudge retry. The error reads
`<phase> phase runner timed out without parseable output`. Only `transcript.log` exists for that
phase — **no** `transcript-2.log`. (If the killed attempt happened to emit parseable JSON, the phase
*succeeds* instead — parse-first — so re-run to observe the fast-fail.)

**Where to look:** the failing phase under `agents/<ID>/<phase>/` (one transcript, no `-2`), the
top-level error message, and the absence of a failed-phase metrics row.

## Drill 2 — Budget fast-fail (`signal:'budget'`)

```bash
timeout --signal=INT --kill-after=5s 180s \
  npx tsx src/cli.ts <ISSUE> --repo <OWNER/REPO> --runner claude \
  --phases plan --project-root "$DRILL_ROOT" \
  --timeout 3600 --max-budget-usd 0.01 --test-cmd 'sleep 300' \
  --no-progress --no-merge
```

**Expected outcome:** claude's native budget gate (claude is the only runner with
`caps.nativeBudget`) trips at the `$0.01` cap and returns `signal:'budget'`. With no parseable
payload the run fails fast, no nudge: `<phase> phase hit the native budget cap without parseable
output`. The cap bounds continuation at Claude's next budget check; an already-running turn can
overshoot `$0.01`.

**Where to look:** same as Drill 1—a single transcript for the failing phase, budget error at the
top, no retry transcript, and no failed-phase metrics row.

## Drill 3 — Kill, then resume (resume skips completed phases)

```bash
# 1) Start a two-phase run; note the printed "phased run id: <ID>".
ADW_CLASSIFY_ON_RUNNER=1 \
  timeout --foreground --signal=INT --kill-after=5s 300s \
  npx tsx src/cli.ts <ISSUE> --repo <OWNER/REPO> --runner claude \
  --phases classify,plan --project-root "$DRILL_ROOT" \
  --timeout 3600 --max-budget-usd 45 --test-cmd 'sleep 300' \
  --no-progress --no-merge
# 2) Let classify complete and plan start, then Ctrl-C.

# 3) Resume the same run by its id:
ADW_CLASSIFY_ON_RUNNER=1 \
  timeout --foreground --signal=INT --kill-after=5s 300s \
  npx tsx src/cli.ts <ISSUE> --repo <OWNER/REPO> --runner claude \
  --phases classify,plan --project-root "$DRILL_ROOT" \
  --resume --adw-id <ID> --timeout 3600 --max-budget-usd 45 \
  --test-cmd 'sleep 300' --no-progress --no-merge
# 4) After plan completes and the inert gate starts, Ctrl-C again.
```

**Expected outcome:** on resume the orchestrator prints `skipping <phase> (already completed)` for
every phase that finished before the kill (`orchestrator.ts:1114–1116`), then continues from the
first unfinished phase. Ctrl-C ends the process directly; after it exits, verify no matching Claude
or wrapper descendant remains while `agents/<ID>/state.json` remains valid and resumable. The run id
is the one printed at start, `phased run id: <ID>`
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

## Observed 2026-07-16 evidence

The sanitized archive is
[`test/fixtures/live-evidence`](../test/fixtures/live-evidence). It records key names, run state,
metrics, transcript inventories, exit/error summaries, and process-cleanup observations without
archiving prompts, model output, credentials, or environment values.

| Drill | Run id | Observed result |
| --- | --- | --- |
| timeout | `a6b4e6dc` | `plan` hit the inner one-second timeout, emitted the timeout fast-fail, wrote exactly one transcript, did not nudge, and recorded no failed-phase metrics |
| native budget | `b20d9e02` | Claude received the `$0.01` native cap, returned the budget fast-fail after its in-flight turn, wrote exactly one transcript, did not nudge, and recorded no failed-phase metrics |
| in-flight kill/restart | `57b6bfea` | Process-group SIGINT (the Ctrl-C signal) landed while the real `claude-opus-4-8` subprocess was active in `review`, before a result persisted. The six-process group fell to zero, state stayed byte-identical with only `setup` complete, and the tracked target tree stayed clean. Resume reran `review`, persisted it successfully, and a final stop at the inert gate again left the recorded process group empty and state unchanged. |
| completed-phase skip | `c20e5a01` | `review` completed, Ctrl-C ended the process group during the inert pre-commit gate, resume printed `skipping review (already completed)` and reached that gate again, and both stops left no matching child process while preserving byte-identical valid state. |

Together the two resume runs record both recovery shapes: `57b6bfea` restarts an incomplete phase,
while `c20e5a01` skips one that was already durable. Run `57b6bfea` supplies issue #22's active-
runner process-group cleanup observation. Its successfully resumed `review` phase persisted a
completed-phase cost of `$1.1385020000000001` under a `$45` ceiling, but the interrupted attempt's
spend is unknown, so that number is **not** the total kill/resume cost. The earlier `c20e5a01`
snapshot (`$0.8932829999999999`) remains the separately archived completed-phase cost envelope.
Timeout/budget failures likewise throw before usage is recorded and are never assigned invented cost.

## See also

- [`docs/LIVE-RUN-BATCH.md`](./LIVE-RUN-BATCH.md) — the issue batch and live-run command templates
  (this runbook expands its "Failure-mode drills" section).
- [`MVP-READINESS.md` §1](../MVP-READINESS.md#1-gates-for-a--claude-ships-reliably) — the gate these
  drills feed ("failure modes observed live, not just mocked").
- [`test/run-phase.test.ts`](../test/run-phase.test.ts) — the deterministic, mocked counterpart that
  pins the same signal→behavior mappings in CI.
