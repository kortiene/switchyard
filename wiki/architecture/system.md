---
type: Architecture
title: System architecture
description: The boundaries between Switchyard's deterministic kernel, project policy, runners, providers, and generated state.
tags: [architecture, kernel, project-pack, control-plane]
timestamp: "2026-07-18T13:26:10Z"
---

# Context

Switchyard separates deterministic delivery mechanics from repository-specific policy and interchangeable external integrations.

```text
operator / cockpit
       │
       ▼
CLI ──► deterministic kernel ──► providers ──► work item, Git, forge, CI
                 │
                 ├──► project pack ──► config, prompts, optional schemas
                 │
                 ├──► AgentRunner ───► coding-agent backend
                 │
                 └──► state and metrics
```

# Boundaries

| Boundary | Main locations | Responsibility |
| --- | --- | --- |
| CLI | `adw_sdlc/src/cli.ts` | Parse commands, resolve runner/engine selection, and dispatch a run or worktree inspection. |
| Kernel | `orchestrator.ts`, `phases.ts`, `run-phase.ts` | Own deterministic control flow, gates, retries, structured results, and side-effect ordering. |
| Project pack | `.adw/config.json`, `.adw/pack.profile.json`, `.adw/prompts/` | Supply validated repository policy and generated runtime prompt context. |
| Runner seam | `invoker.ts`, `registry.ts`, `runners/` | Translate one phase request into one backend invocation and normalize the result. |
| Provider seam | `providers.ts`, `provider-descriptor.ts` | Abstract work-item, VCS, change-request, pipeline, and provider-CLI operations. |
| Persistence | `state.ts`, `metrics.ts`, managed-run modules | Preserve resume state, phase artifacts, metrics, and optional worktree ownership. |
| Cockpit | `.pi/extensions/adw-cockpit/` | Observe runs and assist operators while launching the same control plane for mutations. |

# Ownership rules

The kernel owns invariants that projects must not weaken: runner environment isolation, Git and forge authority, load-bearing phase semantics, structured-result validation, and state compatibility. The project pack may choose known phases, prompt roots, safe schema extensions, model tiers, provider descriptors, branch policy, and change-sensitive gates.

The [runner and provider seams](/architecture/runners-and-providers.md) are independent. A runner edits the selected worktree; providers let the kernel act on external systems. Neither seam changes the [security boundary](/architecture/security-boundaries.md).

# Citations

[1] [Orchestrator implementation](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/orchestrator.ts)

[2] [Validated configuration schema](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/config.ts)

[3] [Universal kernel and project-pack guide](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/docs/UNIVERSAL.md)

[4] [ADW Cockpit extension](https://github.com/kortiene/switchyard/blob/main/.pi/extensions/adw-cockpit/index.ts)
