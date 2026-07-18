---
type: Operational Playbook
title: Troubleshooting runs
description: Safe diagnosis paths for runner, output, test, CI, state, cost, and secret-boundary failures.
tags: [operations, troubleshooting, recovery, diagnostics]
timestamp: "2026-07-18T13:26:10Z"
---

# First checks

1. Preserve the run ID and do not delete `agents/<adw-id>/` or a managed lane.
2. Read the terminal error and the latest phase transcript without copying credentials into reports.
3. Inspect `state.json` for completed phases and `metrics.json` for duration, attempts, and known cost.
4. Reproduce deterministic failures with the narrow command shown by the run before starting another paid runner call.

# Symptom guide

| Symptom | Interpretation | Next action |
| --- | --- | --- |
| Authentication failure | The selected backend rejected its allowed authentication route. | Check that the runner-specific credential or login exists; inspect names/presence only, never values. |
| `signal: timeout` | The parent abort timer fired. | Inspect the transcript, choose a justified larger `--timeout`, then resume; no format nudge is expected. |
| `signal: budget` | A native limit fired, or accumulated known cost crossed the parent soft cap. | Review `metrics.json`, model tier, and scope before increasing the cap. |
| Malformed structured result | Native/fenced output did not validate. | Expect at most one nudge attempt; inspect the transcript and schema when the second attempt fails. |
| Test gate remains red | Bounded resolve attempts were exhausted or made no progress. | Run the exact test command locally, fix deterministically, and resume the same run. |
| Review blockers remain | Bounded patch attempts were exhausted. | Read structured findings, repair them manually or adjust only with evidence, then resume. |
| CI failure | A remote job is red and CI-fix could not repair it. | Inspect the named job/log excerpt and reproduce locally where possible; do not merge around the gate. |
| Unknown CI state | Provider/network reads could not establish a reliable status. | Wait for provider recovery and resume; do not treat unknown as green. |
| Resume identity mismatch | Work item, branch, state, or managed checkout does not match. | Stop and reconcile the exact run; do not edit IDs or registry files to bypass validation. |

# Secret-safe audit

The audit tooling records denied key names and boolean presence, not values. Do not print the parent environment, `GH_TOKEN`, provider tokens, runner credential files, or full environment diffs. The static `npm run lint:env` check and real-spawn audit solve different problems; run the latter only when its documented prerequisites and scope are appropriate.

# Cost and latency

Every output-format nudge is a second runner call. `metrics.json` records phase duration, attempts, tokens where available, and known cost. An unpriceable phase makes aggregate cost unknown rather than reporting a misleading partial total. Adjust one lever at a time: output reliability, model tier, timeout/budget, phase selection, or CI polling.

# Escalate safely

Retain failed, interrupted, dirty, conflicted, or remotely uncertain managed worktrees. For timeout, budget, and kill/resume drills, follow the dedicated failure-drill guide and record evidence without secrets.

# Citations

[1] [Failure-mode drill guide](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/docs/FAILURE-DRILLS.md)

[2] [Secret-boundary audit guide](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/docs/SECRET-BOUNDARY-AUDIT.md)

[3] [Cost and duration guide](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/docs/COST-AND-DURATION.md)

[4] [Runner failure classification](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/run-phase.ts)
