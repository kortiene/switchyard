# Observed-live ledger — "mocked ✅ → seen live?"

_Last updated: 2026-06-28._

This is the **live counterpart** to [`../PARITY.md`](../PARITY.md): PARITY.md is a
checklist of green boxes, but nearly every box is **mocked-seam** evidence (every
SDK / spawn / `gh` / git effect stubbed). This ledger asks the harder question for
each [`PARITY.md` Section-10 guarantee](../PARITY.md#section-10-parity-checklist-for-the-shipped-runner):
**has it been seen at least once in a real `claude` run, not only under stubs?**

A row flips to `✅ observed-live` **only** on documented evidence from a real run.
Parity bug #332 — surfaced by the original live seed run — is the standing proof
that the mocks under-specify reality, so an unobserved guarantee is genuinely owed,
not a formality. This ledger is the readiness dashboard referenced by
[`../MVP-READINESS.md` §1](../MVP-READINESS.md#1-gates-for-a--claude-ships-reliably);
it is scoped to the **`claude`** cutover gate (the (A) MVP). Codex / opencode / pi
live status is tracked separately in
[`PARITY.md` → real-issue runs](../PARITY.md#real-issue-runs-per-runner).

## Status legend (observed-live vocabulary)

- **`✅ observed-live`** — seen at least once in a real run, with cited evidence.
- **`🟡 partial / inferred`** — a real run exercised the surrounding path, but the
  specific guarantee was not independently evidenced (the cell notes what is
  missing).
- **`⏳ not-yet-observed`** — mocked-only so far; owed a live observation.
- **`N/A (not live-observable)`** — not a property of a `claude` live run (e.g. a
  Python-suite CI invariant); the cell explains why.

## Ledger

Guarantee names are verbatim from `PARITY.md:24–38` so each row traces to its
source. The `Mocked?` column is `✅` for every row (all 13 are mocked-proven, or —
for `adw/ green` — proven by the Python suite), matching PARITY.

| # | Guarantee (PARITY §10) | Mocked? | Observed-live? | Run id / evidence |
| --- | --- | --- | --- | --- |
| 1 | Phase order & gating | ✅ | ✅ observed-live | Run `007fd5ba` ran the full chain end-to-end → squash-merged PR #331 (`PARITY.md:53`, `:69`). The *conditional* e2e/document gates firing identically was not separately evidenced. |
| 2 | Per-phase model routing | ✅ | 🟡 partial / inferred | The live run necessarily used per-phase routing, but no committed artifact pins the exact tier per phase. Upgrade only if `007fd5ba` phase outputs confirm tiers. |
| 3 | Selected runner edits the worktree unattended (capability parity) | ✅ | ✅ observed-live | The `claude` runner edited the worktree unattended and produced PR #331 (`PARITY.md:69`). |
| 4 | Structured output | ✅ | ✅ observed-live (rate: native measured, comparative ⏳) | Seen live including a real tests-phase contract mismatch, root-caused and fixed structurally in #332 (`PARITY.md:91–95`). Now **measured** over the 8-run batch (`npm run parity:rate -- agents/`): native **0/36 hard-fails (0.0%)** but an **88.9% single-nudge rate** — native rarely lands clean on the first attempt. The *comparative* bar (native ≤ fenced) still reads `INSUFFICIENT DATA` (fenced = 5 attempts, < 20 needed). |
| 5 | Secret withholding (fail-closed) | ✅ | ⏳ not-yet-observed | `MVP-READINESS.md:101–102` explicitly owes "the secret boundary asserted once on a *real* spawned env (not only the lint + mocks)." Mocked-only today. |
| 6 | Sandboxed-to-worktree (per runner) | ✅ | 🟡 partial / inferred | `cwd` was bound to the worktree for the live run, but the per-tool git/gh veto (`caps.perToolHook`) firing live is not evidenced. |
| 7 | Gated squash-merge | ✅ | 🟡 partial / inferred | The merge path executed live (PR #331 was squash-merged with `--yes`), but the *unattended refusal without `--yes`* was not induced live (it is one of the owed §1 failure drills). |
| 8 | Bounded loops + no-retry-on-timeout | ✅ | ⏳ not-yet-observed | `MVP-READINESS.md:88–91`: a real timeout fast-fail / budget fast-fail — "none has been seen live." Mocked-only today. |
| 9 | Resume | ✅ | ✅ observed-live | `PARITY.md:96` — "no recurrence on resume" after #332 means a real `--resume` occurred. Note: the §1 *kill-then-resume* failure drill is a different, still-owed scenario. |
| 10 | Artifacts | ✅ | ✅ observed-live | PR #331 has a real PR body + commit message → `review`/`document` wrote `pr_body.md`/`commit_message.txt` live. |
| 11 | State equivalence (cross-language) | ✅ | 🟡 partial / inferred | `PARITY.md:47` — the live run "produced a real such `state.json`" that validates against the schema. The cross-language *resume of that exact artifact* by Python is proven via fixtures/tests, not the live artifact. |
| 12 | Cost/usage | ✅ | ✅ observed-live | Cost ≈ $34.76 recorded for run `007fd5ba` (`PARITY.md:69`); native per-phase cost was captured. |
| 13 | adw/ green | ✅ | N/A (not live-observable) | The Python `adw/` unittest suite staying green is a CI / mocked-suite invariant, not a property of a `claude` live run. |

**Headline: 6 `✅`, 4 `🟡`, 2 `⏳`, 1 `N/A`** (unchanged — see the conservative rule
below). Nine live `claude` runs now exist (the original seed plus the 8-issue batch),
but the tokens stay put: the batch artifacts are git-ignored and self-referential, and
the load-bearing security/failure guarantees (#5 secret boundary, #8 bounded-loop
fast-fail) were still not induced live — which is exactly the gap
[`MVP-READINESS.md` §1](../MVP-READINESS.md#1-gates-for-a--claude-ships-reliably) calls out.

## Seed source & authoring rule

The ledger seeds from **two** sources, kept distinct because their evidence differs:

1. **The original seed run** — Issue #304 → PR #331 (squash-merged), parity fix #332,
   run `007fd5ba`, cost ≈ $34.76 — recorded in `PARITY.md`
   ([Section 10](../PARITY.md#section-10-parity-checklist-for-the-shipped-runner),
   the [real-issue runs](../PARITY.md#real-issue-runs-per-runner) table, and the
   [structured-output rate](../PARITY.md#structured-output-hard-failure-rate)
   section) and [`MVP-READINESS.md`](../MVP-READINESS.md). Its `agents/007fd5ba/`
   artifact tree is **not** vendored in this checkout, so it is grounded in the
   committed docs only.
2. **The MVP live-run batch** — 8 additional completed `claude` runs (issues #1–#8 →
   squash-merged PRs #9–#16 on `kortiene/switchyard`), per
   [`LIVE-RUN-BATCH.md`](./LIVE-RUN-BATCH.md). These runs actually authored the
   readiness docs themselves (this ledger came from issue #3). Their workspaces exist
   locally under `agents/{adw_id}/` but are **git-ignored**, so they evaporate on a
   clean clone. Run `npm run parity:rate -- agents/` over them and the
   structured-output rate is **measured**: native **0/36 hard-fails (0.0%)** with an
   **88.9% nudge rate**; fenced **5/5 clean** (so the *comparative* bar still reads
   INSUFFICIENT DATA — it needs ≥ 20 fenced attempts).

So nine live `claude` runs exist in total — but every row above stays conservative
for two reasons the batch does **not** overcome: the batch artifacts are git-ignored
(not reproducible from a clean clone) and **self-referential** (claude editing
`adw_sdlc`'s own docs/tests/CI, not shipping independent features), and the
load-bearing failure-mode (#8) and secret-boundary (#5) guarantees were still never
induced against a real spawned runner.

**Conservative rule (load-bearing):** a row is `✅ observed-live` **only** where a
cited committed document supports it; everything else stays `⏳` or `🟡` with a note
on the gap. Do not overclaim — a readiness dashboard that marks an unproven
guarantee green is worse than no dashboard. Vendoring or archiving the batch
`agents/{adw_id}/` trees (so `parity:rate` is reproducible) is the first step to
upgrading any `🟡`/`⏳` rows.

## How to update after a live `claude` run

1. **Flip the rows it evidences** to `✅ observed-live`, citing the run id and PR /
   issue number in the "Run id / evidence" cell. Keep `🟡`/`⏳` for anything the run
   did not independently prove; do not upgrade beyond what the artifacts show.
2. **Record the artifact path** — `agents/{adw_id}/{phase}/…` — as the evidence
   backing each upgraded row (e.g. per-phase `usage`/cost for #12, the persisted
   `state.json` for #11, `pr_body.md`/`commit_message.txt` for #10).
3. **Refresh the structured-output rate** with `npm run parity:rate -- agents/`
   and link the result for row #4 (it stays `(rate ⏳)` until the comparative bar
   stops printing `INSUFFICIENT DATA`).
4. **Keep row order aligned with `PARITY.md` Section 10** so the two documents stay
   one-to-one and the "12 vs 13" count cannot drift.
5. **Bump the `Last updated` date** at the top and update the headline tally.
