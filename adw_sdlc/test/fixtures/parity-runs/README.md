# Committed parity evidence — the MVP live-run batch

This directory vendors the run artifacts behind the structured-output hard-failure
rate quoted in [`PARITY.md`](../../../PARITY.md),
[`MVP-READINESS.md`](../../../MVP-READINESS.md) and
[`docs/OBSERVED-LIVE-LEDGER.md`](../../../docs/OBSERVED-LIVE-LEDGER.md), so that
`npm run parity:rate` is **reproducible from a clean clone** — not dependent on the
ephemeral, git-ignored `agents/` workspaces on one machine.

## Provenance

The eight subdirectories are the eight live `claude` ADW runs of the MVP live-run
batch — issues #1–#8 → squash-merged PRs #9–#16 on `kortiene/switchyard`
(see [`docs/LIVE-RUN-BATCH.md`](../../../docs/LIVE-RUN-BATCH.md)). Each `<adw_id>/`
is one run; issue #1 (`bfd9405e`) ran forced-fenced, the rest ran native.

## What is vendored (and what is not)

`tools/parity-rate.ts` classifies each phase from exactly three signals; all three
are preserved verbatim:

| Artifact | Vendored? | Why |
| --- | --- | --- |
| `<adw_id>/state.json` | **verbatim** | `runner`, `completed_phases`, `issue_number` drive the outcome (clean / nudged-ok / hard-fail). |
| `<adw_id>/<phase>/prompt.txt` | **verbatim**\* | The fenced-JSON contract footer in the prompt is the native-vs-fenced signal (`FENCED_MARKER`). |
| `<adw_id>/<phase>/transcript.log` | **presence-marker** | The tool reads only that this file *exists* (the phase ran), never its content. |
| `<adw_id>/<phase>/transcript-2.log` | **presence-marker** | The tool reads only that this file *exists* (a single nudge retry fired), never its content. |

\* The only edit to `prompt.txt` content is hygiene: the orchestrator's
machine-absolute write-target paths (`…/agents/<id>/{commit_message,pr_body}…`) were
normalized to repo-relative (`agents/<id>/…`). Nothing else is altered, and the
fenced-marker the classifier reads is untouched.

The two `transcript*.log` files are replaced by a one-paragraph placeholder: the
classifier never reads their bytes, and the live agent stdout they hold is
deliberately kept out of the committed tree (size + secret-boundary hygiene). The
raw workspaces, including full transcripts, remain under the git-ignored `agents/`.

## Reproduce

```bash
cd adw_sdlc
npm run parity:rate -- test/fixtures/parity-runs/
```

Expected (also pinned by `test/parity-evidence.test.ts`, run under `npm run verify`):

| Path | attempts | clean | nudged→ok | HARD-FAIL | hard-fail rate | nudge rate |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| native | 36 | 4 | 32 | 0 | 0.0% | 88.9% |
| fenced | 5 | 5 | 0 | 0 | 0.0% | 0.0% |
| classify *(excluded from bar)* | 8 | 1 | 7 | 0 | 0.0% | 87.5% |

Comparative bar → `INSUFFICIENT DATA` (fenced 5 < 20 needed). Absolute native bar
(`--max-native-rate 20`) → MEETS (0.0% ≤ 20% over 36 attempts).
