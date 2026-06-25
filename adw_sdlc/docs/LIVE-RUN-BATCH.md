# Live `claude` run batch — MVP-readiness issue plan

Purpose: a ready-to-create batch of small GitHub issues to drive **5–10 varied
live `claude` ADW runs**. These satisfy the `MVP-READINESS.md` §1 gate
("≥ 5–10 live `claude` runs across varied `issue_class`") *and* advance the repo.

> Status: **in progress.** Issue #1 (docs: declare Claude-only MVP scope +
> live-run playbook) has been run live as a **forced-fenced** ADW run — the first
> live run from this checkout and the seed of the fenced-path baseline. Issue #3
> (feat: MVP live-run observation ledger) has been run as a **native** ADW run,
> producing `adw_sdlc/docs/OBSERVED-LIVE-LEDGER.md`. Issue #4 (test: failure-drill
> scaffold) has been run as a **native** ADW run, producing
> `adw_sdlc/docs/FAILURE-DRILLS.md`. Issues 2, 5–8 remain planned.

## Why a single `verify` gate first

The ADW test gate and finalize gates do **not** run through a shell — the
orchestrator splits the command with `shellSplit()` and `spawnSync(bin, args)`
(`src/orchestrator.ts` `resolveLoop` line ~418 and `finalizeAndMerge` line ~840;
`src/common.ts` `shellSplit`). So a chained `ADW_TEST_CMD="a && b"` would pass
`&&` as a literal argument and fail. Use **one** command instead:

```bash
ADW_TEST_CMD="npm run verify"
```

`npm run verify` (added to `package.json`) chains internally via npm:

```
typecheck → lint:env → pack:check → test → build → rm -rf dist
```

`ADW_FINALIZE_GATES` is newline-separated and each line is shell-split
individually, so multi-step finalize gates must be **one command per line**, not
`&&`-joined.

## Preflight (before any live run)

```bash
git remote -v          # this checkout has none yet; add origin first
gh auth status
gh repo view <owner/repo>
cd adw_sdlc && npm install && npm run verify
```

Valid `issue_class` values (must match `src/schemas.ts` `ISSUE_CLASSES` and
`.adw/prompts/classify.md`): `feat`, `fix`, `docs`, `chore`, `ci`, `test`,
`refactor`.

## The batch (8 issues, 7 distinct classes)

| # | Title | Class | Run mode | Advances |
| --- | --- | --- | --- | --- |
| 1 | docs: declare Claude-only MVP scope + live-run playbook | docs | **forced-fenced** | MVP-READINESS §0 decision + fenced baseline |
| 2 | ci: add single `verify` quality-gate script | ci | native | already landed here; issue documents/uses it |
| 3 | feat: MVP live-run observation ledger | feat | native | MVP-READINESS §1 "mocked→observed-live" ledger |
| 4 | test: failure-drill scaffold (timeout/budget/resume) | test | native | MVP-READINESS §1 failure-mode evidence |
| 5 | refactor: split parity-rate classification from rendering | refactor | native | reusable readiness tooling |
| 6 | fix: drift guard for `ADW_*` env naming in docs/prompts | fix | native | guards the env-rename migration |
| 7 | feat: live secret-boundary audit scaffold (no secret printing) | feat | native | MVP-READINESS §1 operational basics |
| 8 | chore: normalize handover/env docs after rename | chore | native | current-vs-historical env clarity |

Class coverage: `docs, ci, feat, test, refactor, fix, chore` (7 of 7). Run 1–6
for a minimal sample; all 8 for the stronger sample.

## Run-order rationale

1 first (low-risk docs, doubles as the **forced-fenced** baseline the comparative
parity bar needs). 2 next so every later run can use `ADW_TEST_CMD="npm run
verify"`. Then 3–8 by value/risk.

## Live-run command templates

Native run (after issue #2 lands, or here-and-now since `verify` already exists):

```bash
cd adw_sdlc
ADW_TEST_CMD="npm run verify" \
  npx tsx src/cli.ts <ISSUE_NUMBER> --runner claude --yes \
  --timeout 3600 --max-budget-usd 45
```

Forced-fenced baseline (issue #1):

```bash
cd adw_sdlc
ADW_PARITY_FORCE_FENCED_JSON=1 ADW_TEST_CMD="npm run verify" \
  npx tsx src/cli.ts <ISSUE_NUMBER> --runner claude --yes \
  --timeout 3600 --max-budget-usd 45
```

Claude subscription (no `ANTHROPIC_API_KEY`, using `claude login`):

```bash
ADW_CLASSIFY_ON_RUNNER=1 ADW_TEST_CMD="npm run verify" \
  npx tsx src/cli.ts <ISSUE_NUMBER> --runner claude --yes \
  --timeout 3600 --max-budget-usd 45
```

After every run, classify the artifacts:

```bash
npm run parity:rate -- --max-native-rate 20 agents/   # 20% is a starting threshold
```

## Failure-mode drills (MVP-READINESS §1)

Full runbook: [`FAILURE-DRILLS.md`](./FAILURE-DRILLS.md) — copy-paste commands and the exact expected
signal for each drill. The bullets below remain a short index.

Reuse a low-risk issue's run id; these need no separate GitHub issue.

- **Timeout fast-fail:** add `--timeout 1`; expect fail with no nudge. Then
  `--resume --adw-id <ID>`.
- **Budget fast-fail:** add `--max-budget-usd 0.01`; expect a budget signal. Then
  resume with a normal cap.
- **Kill/resume:** Ctrl-C mid-phase, then `--resume --adw-id <ID>`.
- **Nudge-retry:** ⚠ hard to induce safely — do **not** instruct the agent to
  violate the output contract on a real issue. Capture opportunistically if a real
  run naturally produces a recoverable parse failure (see `FAILURE-DRILLS.md` §Opportunistic).

## Cost / risk

One prior `claude` run was ≈ $35. Eight runs ≈ $150–$300+. Controls: keep issues
small, `--max-budget-usd 45`, run #1 as a smoke test, run `parity:rate` after
each, stop early if the hard-fail rate or cost looks wrong.

## Uncertainties

- No remote configured here → cannot create/run from this checkout yet.
- The `--max-native-rate` threshold (20%) is a placeholder; pick the real MVP
  threshold before declaring the gate met.
- Nudge-retry observation is opportunistic, not guaranteed.
- Cost figures are extrapolated from a single prior run.
