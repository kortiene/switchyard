---
type: Project Overview
title: Switchyard
description: An agent-readable and human-readable map of the Switchyard ADW control plane.
resource: https://github.com/kortiene/switchyard
tags: [switchyard, adw, control-plane, overview]
timestamp: "2026-07-18T13:26:10Z"
---

# Purpose

Switchyard contains `adw_sdlc`, a TypeScript and Node control plane that moves one work item through a deterministic Agentic Developer Workflow (ADW). Each agentic phase is a separate invocation of a selected coding-agent runner. The control plane owns phase order, state, local gates, Git operations, change-request operations, CI handling, and optional merge.

This wiki is a navigation and explanation layer for contributors, operators, and agents. It summarizes relationships and points back to canonical implementation sources; it does not replace the package README, source code, schemas, or tests.

# System at a glance

```text
work item
   │
   ▼
Switchyard orchestrator ──► project pack (config, prompts, schemas)
   │
   ├──► AgentRunner ──► claude | codex | opencode | pi
   ├──► providers ─────► work item | VCS | change request | CI
   └──► state/artifacts in agents/<adw-id>/ or managed-run roots
```

The default built-in flow is described in [orchestration](/architecture/orchestration.md). The most important invariant is that the [orchestrator retains Git and forge authority](/architecture/security-boundaries.md) while runner children receive a deny-by-default environment.

# Knowledge boundaries

| Material | Role | Canonical owner |
| --- | --- | --- |
| `adw_sdlc/src/`, `.adw/config.json` | Runtime behavior and project policy | Implementation |
| `adw/state.schema.json` | Portable state contract | Schema plus compatibility tests |
| `.pi/prompts/` | Neutral prompt sources | Hand-maintained source |
| `.claude/commands/`, `.adw/prompts/` | Mirrors or generated prompt-pack output | Generation tools and drift gates |
| `agents/`, `coverage/`, `dist/` | Runtime, test, or build output | Generated and ignored |
| `wiki/` | Navigable knowledge bundle | Maintained from canonical sources |

# Navigate

* Understand the system through [architecture](/architecture/).
* Look up flags and configuration in [reference](/reference/).
* Follow a delivery path in [workflows](/workflows/).
* Recover a run with [operations](/operations/).
* Review load-bearing choices in [decisions](/decisions/).

# Citations

[1] [Switchyard repository README](https://github.com/kortiene/switchyard/blob/main/README.md)

[2] [`adw_sdlc` package README](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/README.md)

[3] [Open Knowledge Format v0.1 specification](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)
