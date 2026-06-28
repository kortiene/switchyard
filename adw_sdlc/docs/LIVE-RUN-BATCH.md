# Live `claude` run batch — MVP-readiness issue plan

Purpose: a ready-to-create batch of small GitHub issues to drive **5–10 varied
live `claude` ADW runs**. These satisfy the `MVP-READINESS.md` §1 gate
("≥ 5–10 live `claude` runs across varied `issue_class`") *and* advance the repo.

> Status: **complete — all 8 issues run live.** Issues #1–#8 each ran as a live
> `claude` ADW run and squash-merged as PRs #9–#16 on `kortiene/switchyard`. Issue
> #1 (docs: Claude-only MVP scope) ran **forced-fenced** — the seed of the
> fenced-path baseline (5 fenced attempts); issues #2–#8 ran **native**. Measured
> over the batch, `npm run parity:rate -- test/fixtures/parity-runs/` reports native **0/36 hard-fails
> (0.0%)**, an **88.9% single-nudge rate**, and fenced **5/5 clean** (so the
> comparative bar is still INSUFFICIENT — it needs ≥ 20 fenced attempts). The batch
> evidence is committed at `test/fixtures/parity-runs/` (CI-guarded by
> `test/parity-evidence.test.ts`), so `parity:rate` reproduces from a clean clone; the
> raw `agents/` workspaces stay git-ignored. Remaining readiness work is the live failure-mode
> drills and the secret-boundary live audit (still scaffold-only), not more batch
> issues.

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

## The batch (8 issues, 7 planned classes)

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

Class coverage *as planned*: `docs, ci, feat, test, refactor, fix, chore` (7 of 7).
*As realized* (classify phase): **6 distinct** — issue #8 classified as `docs`, not
the planned `chore`, so `docs` recurs for #1/#3/#8 and `chore` was never exercised.
Run 1–6 for a minimal sample; all 8 for the stronger sample.

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

- The batch ran on `kortiene/switchyard`; a fresh checkout has no remote configured
  → add `origin` before creating/running new issues from it.
- The `--max-native-rate` threshold (20%) is a placeholder; pick the real MVP
  threshold before declaring the gate met.
- Nudge-retry observation is opportunistic, not guaranteed.
- Cost figures are extrapolated from a single prior run.
