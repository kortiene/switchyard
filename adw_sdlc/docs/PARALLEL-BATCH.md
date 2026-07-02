# Parallel batches: worktree-per-run lanes

`run-batch-parallel.sh` supersedes `run-batch.sh` (sequential) for multi-issue
batches. It shipped a 17-issue backlog (kortiene/iroh-room, 2026-07-01→02) in
one 23h run for $631.69 — this doc pins the design and the operational
knowledge that run bought. The sequential `/issues` skill contract ("do not
parallelize") is unchanged: this runner is a separate operator tool.

## Design

- **One linked git worktree per issue run**, passed as `--project-root`.
  Per-run state (`agents/<adw-id>/`) lives inside the worktree; nothing is
  shared between lanes except the repo's common `.git`.
- **`--repo` is load-bearing**: it makes `gh pr merge --delete-branch` skip the
  local-branch checkout that would fail inside a linked worktree.
- **Lanes are dependency chains; phases are barriers.** Issues that share
  files/crates belong in the SAME lane. Barriers gate on the blocking issues
  and on `parity:rate ≤ 10%`.
- **Concurrency cap 3, staggered starts**: one gh token (secondary rate
  limits), one claude.ai subscription, N cold cargo builds, and shared-`.git`
  `config.lock`/ref-lock contention all argue against going wider.
- **The kernel owns base-freshness** (`git.ts syncWithBase`): rebase +
  re-prove gates before push AND again after the CI watch (bounded resyncs);
  force-push (with lease) is derived from actual remote divergence so it
  survives `--resume`; a fetch that keeps failing fails LOUD with a retryable
  message, never as "current".

## Retry policy (per issue, the part that costs money)

| Failure signature in the attempt log | Action |
| --- | --- |
| transient (API 5xx, parse, auth, lock) | `cargo fmt` salvage + `--resume` (cheap) |
| `the budget cap` | resume with a raised cap ONCE (`+$30`); second trip ⇒ fail |
| `rebase onto origin/… a fresh run from the moved base is required` | fresh reset: drop branch (local+remote) + worktree, rerun from new base — **never on the final attempt** (it would destroy the salvage state) |
| `still OPEN after merge` | auto-close lag: settle ~20s, re-read the issue state |
| PR for the branch already MERGED at our tip | close the issue, count done |

## The salvage runbook (proven twice live)

A completed-but-conflicted run whose state was destroyed is recoverable for
≈ $0 — the work survives as the branch commit in the shared object store:

1. `git cat-file -t <sha>` (find it in the reset log's "Could not apply <sha>"),
   `git merge-tree --write-tree origin/main <sha>` to preview conflict extent.
2. New worktree, recreate the exact branch name (it is deterministic:
   `{prefix}/{issue}-{adwId}-{slug}`), `git cherry-pick <sha>`, resolve.
3. Restore the archived `agents/<adw-id>/` (the runner banks it in
   `$ART/agents/` before every reset) into the worktree; drop `metrics*.json`.
4. Resume with a raised cap: finalize re-proves the gates over your resolution,
   then pushes/PRs/merges normally.

## Observed failure modes worth knowing

- **Doc-append conflicts dominate**: every issue's `document` phase writes the
  same README/getting-started regions, so any run in flight when another lane
  merges risks a finalize conflict. Chains absorb most of it; the rest is the
  reset/salvage path.
- **CI toolchain skew**: CI's newer clippy fired `large_futures` that local
  clippy does not — locally-green, CI-red, and a fix agent given only check
  names cannot reproduce it. The kernel now feeds a bounded `--log-failed`
  excerpt to the fix agent (`ADW_CI_LOG_EXCERPTS=0` opts out; the excerpt goes
  to the agent prompt only, never public comments — CI logs can echo secrets).
- **Budget crawl (fixed in kernel)**: accumulated cost persists in state, so a
  same-cap resume of an over-budget run used to pay for one more phase per
  attempt before dying. The kernel now fails fast BEFORE the first paid phase
  — unless every agent phase is done (a finalize-only resume spends nothing
  and stranding a paid-for run would be worse; observed live with #38).
- **Costs** (17-issue live batch): complete runs $13–$87, median ≈ $27; the
  outliers paid for CI-flake retries and discarded conflicted runs. Reference
  ceiling: `--max-budget-usd 45`, raise-once to 75.

## Env

- `ADW_CI_LOG_EXCERPTS` — `0` disables CI log excerpts in ci-fix prompts
  (default: enabled).
