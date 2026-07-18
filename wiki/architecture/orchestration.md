---
type: Workflow Architecture
title: Phased orchestration
description: The deterministic phase chain, conditional gates, repair loops, and resume semantics of an ADW run.
tags: [architecture, phases, gates, loops, resume]
timestamp: "2026-07-18T13:26:10Z"
---

# Default flow

```text
setup → classify → plan → implement → tests → resolve(loop) → e2e(gated)
      → review → patch(loop) → document(gated) → finalize → ci-fix(loop)
      → merge(optional) → report
```

| Stage | Owner | Effect |
| --- | --- | --- |
| `setup` | Control plane | Validate inputs, work-item state, phase assets, repository state, and branch/run identity. |
| `classify` | Shared structured call or selected runner fallback | Classify the work and produce the signal used by later routing and gates. |
| `plan` | Runner | Produce the implementation plan artifact. |
| `implement` | Runner | Edit the selected worktree and author change-request text artifacts. |
| `tests` | Runner | Add or improve focused tests. |
| `resolve` | Gate plus runner | Run the configured test command and invoke bounded repair attempts on failure. |
| `e2e` | Conditional runner phase | Run only when the change signal matches configured end-to-end hints. |
| `review` | Runner | Produce structured blocker findings. |
| `patch` | Runner loop | Repair blocker findings within a bounded attempt count. |
| `document` | Conditional runner phase | Run when hints or changed paths indicate user-visible documentation work. |
| `finalize` | Control plane | Run final gates, create the commit/change request, and begin pipeline observation. |
| `ci-fix` | Control plane plus runner loop | Feed failing pipeline evidence to bounded repair attempts, then push and re-poll. |
| `merge` | Control plane | Optionally confirm and squash-merge only after freshness and pipeline checks. |
| `report` | Control plane | Record and communicate the terminal outcome. |

# Configurable surface

The built-in agent-phase catalog is `classify`, `plan`, `implement`, `tests`, `resolve`, `e2e`, `review`, `patch`, and `document`. A run may reorder or omit known phases. Projects may register custom phases with prompt templates and result schemas; custom phases may opt into conditional gates and resolve-style command loops.

The kernel retains the semantics of built-in loops and gates. Invalid phase names, missing templates, missing schemas, unsafe schema overrides, and invalid custom control-flow keys fail during preflight.

# Structured results

The control plane composes each prompt, supplies a per-phase JSON Schema when supported, and re-validates normalized output. A malformed result receives at most one output-format nudge. Timeout, cancellation, budget, and classified authentication failures do not take that nudge path.

# Resume

State records completed phases. `--resume --adw-id <id>` loads that state, verifies work-item identity, and skips completed work while preserving review and change-request context. Managed runs add checkout-ownership validation before resuming; see the [managed-worktree playbook](/operations/managed-worktrees.md).

# Citations

[1] [Phase catalog and gates](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/phases.ts)

[2] [Orchestration control flow](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/orchestrator.ts)

[3] [Runner invocation and output validation](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/run-phase.ts)

[4] [Custom control-flow tests](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/test/custom-phase-control-flow.test.ts)
