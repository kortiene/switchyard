---
type: Project Overview
title: Switchyard
description: An agent-readable and human-readable map of the Switchyard ADW control plane.
resource: https://github.com/kortiene/switchyard
tags: [switchyard, adw, control-plane, overview]
timestamp: "2026-10-30T22:00:00Z"
---

# Purpose

Switchyard turns one work item into a tested, reviewable change. The active engine is the Rust mission host in `crates/`. S1 is what this checkout can run: an application-owned SQLite ledger and a fake runtime. It does not call a model or write to GitHub.

Accepted direction (2026-10-29): Switchyard is unreleased and has no users. Rust is the sole long-term engine, including a native Rust agent runtime. Pi Durable is a proof-of-concept adapter only. S2 is NO-GO; see [#96](https://github.com/kortiene/switchyard/issues/96) and the [S2 feasibility audit](../docs/rust-first/s2-feasibility.md). The TypeScript control plane in `adw_sdlc/` is a frozen reference. `runner-pi` will be retired with it. Python compatibility is dropped, and state v1 is frozen as-is. S3 ([#97](https://github.com/kortiene/switchyard/issues/97)) owns worktree lifecycle in Rust. See [decisions](/decisions/invariants.md).

The diagram below is that frozen TypeScript reference, not the active mission host. Each agentic phase is a separate invocation of a selected coding-agent runner. That control plane owns phase order, state, local gates, Git operations, change-request operations, CI handling, and optional merge for the reference package.

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
