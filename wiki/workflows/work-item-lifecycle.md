---
type: Workflow
title: Work-item lifecycle
description: The operator-visible path from a work item through preflight, implementation, change request, CI, and terminal reporting.
tags: [workflow, work-item, phases, change-request, resume]
timestamp: "2026-07-18T13:26:10Z"
---

# Entry

Start with a no-side-effect preview:

```bash
cd adw_sdlc
npm run issue -- <work-item-id> --dry-run
```

The CLI resolves the project root, configuration, provider set, phase chain, runner, and model routing. Preflight rejects invalid or missing phase assets before branch, state, or external mutations.

# Lifecycle

1. The control plane reads the work item and refuses a closed item unless `--force` is explicit.
2. Setup verifies repository conditions, derives branch/run identity, initializes or loads state, and optionally posts progress.
3. The configured [agent-phase chain](/architecture/orchestration.md) runs. Each completed phase is checkpointed.
4. Deterministic test/finalization gates run; bounded repair phases receive only the relevant failure evidence.
5. The control plane consumes the agent-authored commit message and change-request body, then owns commit, push, and change-request creation/update.
6. Pipeline status is polled. Bounded CI-fix attempts may edit, test, commit, and push repairs.
7. The control plane verifies base freshness and pipeline state before optional confirmation and squash merge.
8. The run reports a structured terminal outcome and performs post-run verification when enabled.

# Terminal outcomes

| Outcome | Meaning |
| --- | --- |
| `merged` | The exact change-request head was authoritatively merged. |
| `pr_ready` | `--no-merge` left a green change request open for later resume. |
| `skipped_closed` | Preflight found a terminal work item and did not force work. |
| `failed` | A gate, provider operation, runner call, or safety check failed. |
| `interrupted` | Cancellation or process interruption ended the run before completion. |

# Resume

Use the same work item and persisted ID:

```bash
npm run issue -- <work-item-id> --resume --adw-id <eight-hex-id>
```

Resume is not a blind restart: it checks identity and skips persisted completed phases. A managed run must also prove that its checkout and registry ownership still match; follow [managed-worktree recovery](/operations/managed-worktrees.md) when they do not.

# Citations

[1] [CLI dispatch](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/cli.ts)

[2] [Work-item orchestration](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/orchestrator.ts)

[3] [Structured run outcomes](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/run-outcome.ts)

[4] [Persistent state and resume](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/state.ts)
