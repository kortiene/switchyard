# Observed-live ledger — "mocked ✅ → seen live?"

_Last updated: 2026-06-25._

This is the **live counterpart** to [`../PARITY.md`](../PARITY.md): PARITY.md is a
checklist of green boxes, but nearly every box is **mocked-seam** evidence (every
SDK / spawn / `gh` / git effect stubbed). This ledger asks the harder question for
each [`PARITY.md` Section-10 guarantee](../PARITY.md#section-10-parity-checklist-for-the-shipped-runner):
**has it been seen at least once in a real `claude` run, not only under stubs?**

A row flips to `✅ observed-live` **only** on documented evidence from a real run.
Parity bug #332 — surfaced by the one live run that exists — is the standing proof
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
| 1 | Phase order & gating | ✅ | ✅ observed-live | Run `007fd5ba` ran the full chain end-to-end → squash-merged PR #331 (`PARITY.md:53`, `:68`). The *conditional* e2e/document gates firing identically was not separately evidenced. |
| 2 | Per-phase model routing | ✅ | 🟡 partial / inferred | The live run necessarily used per-phase routing, but no committed artifact pins the exact tier per phase. Upgrade only if `007fd5ba` phase outputs confirm tiers. |
| 3 | Selected runner edits the worktree unattended (capability parity) | ✅ | ✅ observed-live | The `claude` runner edited the worktree unattended and produced PR #331 (`PARITY.md:68`). |
| 4 | Structured output | ✅ | ✅ observed-live (rate ⏳) | Seen live including a real tests-phase contract mismatch, root-caused and fixed structurally in #332 (`PARITY.md:91–95`). The *comparative* hard-failure rate is still `INSUFFICIENT DATA` (`PARITY.md:96–103`) — not yet measured. |
| 5 | Secret withholding (fail-closed) | ✅ | ⏳ not-yet-observed | `MVP-READINESS.md:87–90` explicitly owes "the secret boundary asserted once on a *real* spawned env (not only the lint + mocks)." Mocked-only today. |
| 6 | Sandboxed-to-worktree (per runner) | ✅ | 🟡 partial / inferred | `cwd` was bound to the worktree for the live run, but the per-tool git/gh veto (`caps.perToolHook`) firing live is not evidenced. |
| 7 | Gated squash-merge | ✅ | 🟡 partial / inferred | The merge path executed live (PR #331 was squash-merged with `--yes`), but the *unattended refusal without `--yes`* was not induced live (it is one of the owed §1 failure drills). |
| 8 | Bounded loops + no-retry-on-timeout | ✅ | ⏳ not-yet-observed | `MVP-READINESS.md:80–83`: a real timeout fast-fail / budget fast-fail — "none has been seen live." Mocked-only today. |
| 9 | Resume | ✅ | ✅ observed-live | `PARITY.md:95` — "no recurrence on resume" after #332 means a real `--resume` occurred. Note: the §1 *kill-then-resume* failure drill is a different, still-owed scenario. |
| 10 | Artifacts | ✅ | ✅ observed-live | PR #331 has a real PR body + commit message → `review`/`document` wrote `pr_body.md`/`commit_message.txt` live. |
| 11 | State equivalence (cross-language) | ✅ | 🟡 partial / inferred | `PARITY.md:47` — the live run "produced a real such `state.json`" that validates against the schema. The cross-language *resume of that exact artifact* by Python is proven via fixtures/tests, not the live artifact. |
| 12 | Cost/usage | ✅ | ✅ observed-live | Cost ≈ $34.76 recorded for run `007fd5ba` (`PARITY.md:68`); native per-phase cost was captured. |
| 13 | adw/ green | ✅ | N/A (not live-observable) | The Python `adw/` unittest suite staying green is a CI / mocked-suite invariant, not a property of a `claude` live run. |

**Headline (seed state): 6 `✅`, 4 `🟡`, 2 `⏳`, 1 `N/A`.** A single live run touched
most paths once, but the load-bearing security/failure guarantees (#5 secret
boundary, #8 bounded-loop fast-fail) and several others remain owed — which is
exactly the gap [`MVP-READINESS.md` §1](../MVP-READINESS.md#1-gates-for-a--claude-ships-reliably)
calls out.

## Seed source & authoring rule

The seed comes from **one** live `claude` run: Issue #304 → PR #331
(squash-merged), parity fix #332, run `007fd5ba`, cost ≈ $34.76 — as recorded in
`PARITY.md` ([Section 10](../PARITY.md#section-10-parity-checklist-for-the-shipped-runner),
the [real-issue runs](../PARITY.md#real-issue-runs-per-runner) table, and the
[structured-output rate](../PARITY.md#structured-output-hard-failure-rate)
section) and [`MVP-READINESS.md`](../MVP-READINESS.md).

**Conservative rule (load-bearing):** a row is `✅ observed-live` **only** where a
cited committed document supports it; everything else stays `⏳` or `🟡` with a note
on the gap. Do not overclaim — a readiness dashboard that marks an unproven
guarantee green is worse than no dashboard. The `agents/007fd5ba/` artifact tree is
**not** vendored in this checkout, so the seed above is grounded in the committed
docs only; artifact-level confirmation (which would upgrade some `🟡` rows) is
pending and is the first step in the update procedure below.

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
