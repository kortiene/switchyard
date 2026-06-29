# Cost & duration: levers and instrumentation

This is the operator guide to what a phased ADW run costs (USD) and how long it
takes — and the knobs that move both. It pairs with the per-run metrics artifact
the engine now writes (`agents/{adw_id}/metrics.json`).

## How a run accrues cost and time

A run executes the agent-phase chain
(`classify → plan → implement → tests → resolve → e2e → review → patch → document`),
then finalize/ci-fix/merge. Each agent phase is one runner call through the
`AgentRunner.runPhase()` seam.

- **Cost** is summed additively into `state.totalCostUsd` by `recordUsage`
  (`src/orchestrator.ts`). A single unpriceable phase poisons the run total to
  `null` (it is never a false partial sum). claude/opencode/pi report dollars
  natively; codex + the shared classify call are token-only and priced from
  `src/pricing.ts`.
- **Per-phase model** is chosen by tier routing (`src/models.ts`,
  `models.phaseTiers` in `.adw/config.json`). On claude the `capable` tier is
  `claude-opus-4-8` — by far the most expensive lever.
- **The nudge-retry double-charge:** when a phase's first reply fails to
  parse/validate, the invoker (`src/run-phase.ts`) retries once with the
  fenced-JSON contract. **Both attempts are billed** and both add wall-clock
  time. `AgentPhaseOutcome.attempts` (1 or 2) surfaces this per phase.

## The metrics artifact: `agents/{adw_id}/metrics.json`

Written additively next to `state.json` (never *inside* it — `state.json` is the
byte-stable cross-language contract). Shape:

```json
{
  "adw_id": "a1b2c3d4",
  "runner": "claude",
  "summary": {
    "phases": 5,
    "attempts": 6,
    "nudged_phases": 1,
    "nudge_rate": 0.2,
    "total_duration_ms": 812345,
    "total_cost_usd": 9.21
  },
  "phases": [
    { "phase": "plan", "model": "claude-opus-4-8", "duration_ms": 120345,
      "attempts": 1, "cost_usd": 2.10, "input_tokens": 1234, "output_tokens": 567 }
  ]
}
```

- `nudge_rate` is the share of phases that needed the retry — the single most
  actionable cost/duration signal (each retry ≈ a second paid call).
- `total_cost_usd` is `null` when any phase was unpriceable, mirroring
  `state.totalCostUsd`.
- It is best-effort: a write failure never aborts a run, and no file is written
  for a run that recorded no phases.

Implementation: `src/metrics.ts` (`MetricsCollector`). The orchestrator owns one
collector per run and records a sample after each phase (including the
shared-SDK classify path and the resolve/patch/ci-fix loops).

## Levers (ranked by impact)

### 1. Cut the nudge-retry rate (highest impact)

Each retry is a second billed call **and** added latency. Measure it with
`nudge_rate` across several runs, then:

- A/B the native vs forced-fenced path (`ADW_PARITY_FORCE_FENCED_JSON=1`) and
  compare `total_cost_usd` / `total_duration_ms` / `nudge_rate`.
- Keep the structural fix already in place: native-schema retries recompose the
  prompt **with** the fenced contract footer the first prompt omitted.

### 2. Down-tier non-edit-heavy phases

`plan`, `review`, and `patch` default to the `capable` (opus) tier. Pilot them on
`mid` (sonnet) **without changing the committed defaults**, using per-phase env
overrides (precedence: `--model` > `ADW_MODEL_<PHASE>` > tier):

```bash
ADW_MODEL_PLAN=claude-sonnet-4-6 \
ADW_MODEL_REVIEW=claude-sonnet-4-6 \
ADW_MODEL_PATCH=claude-sonnet-4-6 \
ADW_TEST_CMD="npm run verify" \
  npx tsx src/cli.ts <ISSUE> --runner claude --yes --timeout 3600 --max-budget-usd 45
```

Do **not** use a global `--model` for this — it overrides every phase, including
`implement`. Compare review/patch quality and merge success before adopting.

### 3. Budget & timeout caps

- `--max-budget-usd` is enforced two ways now:
  - **Native** per-call cap on claude (`caps.nativeBudget`), `signal:'budget'`,
    fail-fast no-nudge.
  - **Parent-side soft cap** (runner-agnostic): after each phase the
    orchestrator compares the *accumulated* `state.totalCostUsd` to the cap and
    aborts before the next phase. This is the only spend stop for
    codex/opencode/pi, which have no native cap. (If the total is `null`/unknown
    the soft gate stays quiet; the native cap and `--timeout` remain backstops.)
- `--timeout <s>` aborts a runaway phase (`signal:'timeout'`, no nudge).

### 4. Trim repeated full-verify work

Phase prompts now steer agents to focused tests during a phase and reserve the
full `npm run verify` for broad/risky changes and final review; the orchestrator
runs the canonical gate once at finalize. This avoids paying for a full
typecheck+coverage+build inside every phase. (Source of truth:
`.adw/pack.profile.json` → regenerate with `npm run pack:generate`.)

### 5. CI polling cadence

`--ci-poll-interval` (default 30s) × `--ci-max-polls` (default 40) bound the
CI-watch wall-clock. Lower the interval (e.g. `--ci-poll-interval 10`) on repos
with fast checks.

## Measurement plan

1. Baseline 5–10 comparable runs; read `metrics.json` for cost, duration, and
   `nudge_rate` per phase.
2. A/B one lever at a time (fenced path, tier down-shift, prompt-slim).
3. Compare the metrics summaries and `npm run parity:rate` hard-fail rate before
   adopting any change.
