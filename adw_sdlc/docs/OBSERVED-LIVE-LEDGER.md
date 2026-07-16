# Observed-live ledger — "mocked ✅ → seen live?"

_Last updated: 2026-07-16._

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
| 2 | Per-phase model routing | ✅ | ✅ observed-live | Runs `a6b4e6dc`, `b20d9e02`, and `c20e5a01` pin the resolved models in the [sanitized live-evidence corpus](../test/fixtures/live-evidence/): Haiku for `classify`, Sonnet for the direct `tests` invocation and `ci-fix`, and Opus for `review`. The archived all-nine-phase route table reconciles every configured agent phase rather than inferring its tier from a completed run. |
| 3 | Selected runner edits the worktree unattended (capability parity) | ✅ | ✅ observed-live | The `claude` runner edited the worktree unattended and produced PR #331 (`PARITY.md:69`). |
| 4 | Structured output | ✅ | ✅ observed-live (native rate measured; comparative rate separately gated) | Seen live including a real tests-phase contract mismatch, root-caused and fixed structurally in #332 (`PARITY.md:91–95`). Now **measured** over the 8-run batch (`npm run parity:rate -- test/fixtures/parity-runs/`): native **0/36 hard-fails (0.0%)** but an **88.9% single-nudge rate** — native rarely lands clean on the first attempt. The separate *comparative* bar (native ≤ fenced) still reads `INSUFFICIENT DATA` (fenced = 5 attempts, < 20 needed). |
| 5 | Secret withholding (fail-closed) | ✅ | ✅ observed-live | The issue #21 names-only `CLAUDE_BIN` probe crossed the real runner-spawn boundary during carrier runs `a6b4e6dc`, `b20d9e02`, and `c20e5a01`. Its sanitized parent record confirms `GH_TOKEN`, `GH_BIN`, and `MATRIX_*` / `ADW_*` / `MX_AGENT_*` sentinels were present; every spawn record reports no denied names and confirms the real Claude executable was reached ([fixture corpus](../test/fixtures/live-evidence/)). No secret values were captured. |
| 6 | Sandboxed-to-worktree (per runner) | ✅ | ✅ observed-live | Run `c20e5a01` exercised the unconditional `PreToolUse` veto against `git tag`: the operation was denied before execution and the archived before/after tag evidence shows no tag was created. Together with the run's worktree-bound `cwd`, this directly observes the Claude per-tool sandbox control ([fixture corpus](../test/fixtures/live-evidence/)). |
| 7 | Gated squash-merge | ✅ | ✅ observed-live | Run `c20e5a01` reached green PR #66 without `--yes`; the unattended merge gate refused authorization, and the archived post-run observation records the PR as OPEN and unmerged. This directly complements the earlier authorized squash-merge of PR #331 ([fixture corpus](../test/fixtures/live-evidence/)). |
| 8 | Bounded loops + no-retry-on-timeout | ✅ | ✅ observed-live | Run `a6b4e6dc` induced the real timeout fast-fail and run `b20d9e02` induced Claude's native budget fast-fail. Their sanitized artifacts pin the exact terminal errors, one first-attempt transcript apiece, and no `transcript-2.log`, demonstrating that neither signal entered the nudge retry ([fixture corpus](../test/fixtures/live-evidence/)). |
| 9 | Resume | ✅ | ✅ observed-live | Run `57b6bfea` was interrupted while a real Claude `review` subprocess was active and before the phase persisted; the same run id resumed, reran `review`, and persisted it successfully. Run `c20e5a01` separately resumed after a durable phase and printed the archived `skipping ... (already completed)` observation before continuing to green PR #66. Together they directly observe both restart-incomplete and skip-complete recovery shapes ([fixture corpus](../test/fixtures/live-evidence/)). |
| 10 | Artifacts | ✅ | ✅ observed-live | PR #331 has a real PR body + commit message → `review`/`document` wrote `pr_body.md`/`commit_message.txt` live. |
| 11 | State equivalence (cross-language) | ✅ | ✅ observed-live | The Python engine pinned at commit `d8b3569` loaded and resumed the archived TS-produced state whose source hash begins `1cf058af…`, directly observing the cross-language handoff against a real artifact ([fixture corpus](../test/fixtures/live-evidence/)). The exercise was intentionally stopped before cross-language finalization; finalization itself remains mocked and is not claimed here. |
| 12 | Cost/usage | ✅ | ✅ observed-live | Cost ≈ $34.76 recorded for run `007fd5ba` (`PARITY.md:69`); native per-phase cost was captured. |
| 13 | adw/ green | ✅ | N/A (not live-observable) | The Python `adw/` unittest suite staying green is a CI / mocked-suite invariant, not a property of a `claude` live run. |

**Headline: 12 `✅`, 0 `🟡`, 0 `⏳`, 1 `N/A`.** All twelve live-observable
guarantees now have cited real-run evidence. The remaining `N/A` row is a suite
invariant rather than a runtime behavior; the comparative structured-output rate
and cross-language finalization caveat remain separately stated and are not hidden
by this tally.

## Seed source & authoring rule

The ledger draws from **three** sources, kept distinct because their evidence differs:

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
   readiness docs themselves (this ledger came from issue #3). The raw workspaces
   under `agents/{adw_id}/` stay git-ignored, but their classification-determining
   artifacts are now **committed** at
   [`test/fixtures/parity-runs/`](../test/fixtures/parity-runs/) (transcripts replaced
   by presence-markers). Run `npm run parity:rate -- test/fixtures/parity-runs/` and the
   structured-output rate is **measured reproducibly from a clean clone**: native
   **0/36 hard-fails (0.0%)** with an **88.9% nudge rate**; fenced **5/5 clean** (so the
   *comparative* bar still reads INSUFFICIENT DATA — it needs ≥ 20 fenced attempts).
3. **The issues #20–#23 operational-evidence batch** — Claude carrier runs
   `a6b4e6dc`, `b20d9e02`, `c20e5a01`, and active-phase recovery run `57b6bfea`,
   green-but-unmerged PR #66, and the
   Python-engine state handoff `d8b3569`. The sanitized, names-only/minimal artifacts
   are committed under [`test/fixtures/live-evidence/`](../test/fixtures/live-evidence/):
   timeout and native-budget no-retry outcomes, active-phase restart plus completed-phase
   skip state, the all-nine model
   route table, real-spawn secret-boundary records, the unconditional git-tag veto,
   unattended merge refusal, and the cross-language source hash. Raw transcripts and
   all secret values remain excluded.

Together these sources directly evidence all twelve live-observable guarantees from
a clean clone. The claims remain deliberately narrower than “every possible tail ran”:
in particular, Python loaded and resumed the real TS state, but that exercise stopped
before cross-language finalization, whose proof remains mocked.

**Conservative rule (load-bearing):** a row is `✅ observed-live` **only** where cited,
committed evidence directly supports it. Do not overclaim — a readiness dashboard
that marks an unproven guarantee green is worse than no dashboard. The measurement
corpus at [`test/fixtures/parity-runs/`](../test/fixtures/parity-runs/) and the
operational corpus at [`test/fixtures/live-evidence/`](../test/fixtures/live-evidence/)
are the committed sources for the upgrades above. The former is CI-guarded by
`test/parity-evidence.test.ts`; the latter preserves only sanitized,
claim-determining evidence. A row is green only for the behavior those artifacts
directly show.

## How to update after a live `claude` run

1. **Flip the rows it evidences** to `✅ observed-live`, citing the run id and PR /
   issue number in the "Run id / evidence" cell. Keep `🟡`/`⏳` for anything the run
   did not independently prove; do not upgrade beyond what the artifacts show.
2. **Record the artifact path** — `agents/{adw_id}/{phase}/…` — as the evidence
   backing each upgraded row (e.g. per-phase `usage`/cost for #12, the persisted
   `state.json` for #11, `pr_body.md`/`commit_message.txt` for #10).
3. **Refresh the structured-output rate** with `npm run parity:rate -- agents/`
   and link the result for row #4. Its comparative rate remains a separately stated
   gate until the tool stops printing `INSUFFICIENT DATA`.
4. **Keep row order aligned with `PARITY.md` Section 10** so the two documents stay
   one-to-one and the "12 vs 13" count cannot drift.
5. **Bump the `Last updated` date** at the top and update the headline tally.
