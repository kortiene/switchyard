---
type: Architecture Reference
title: State and artifacts
description: The durable run contract and the generated files used for resume, evidence, and agent handoff.
tags: [architecture, state, resume, artifacts, metrics]
timestamp: "2026-07-18T13:26:10Z"
---

# Default workspace

An ordinary run writes under `agents/<adw-id>/`. The eight-character hexadecimal ID is validated before use as a path segment.

| Path | Purpose | Resume-critical |
| --- | --- | --- |
| `state.json` | Work-item identity, branch, completed phases, review findings, change request, engine/runner, and additive metadata. | Yes |
| `<phase>/prompt.txt` | Exact composed input for a phase. | No |
| `<phase>/transcript.log` | Runner transcript captured during a phase. | No |
| `commit_message.txt` | Agent-authored commit message consumed by the orchestrator. | Used during finalize |
| `pr_body.md` | Agent-authored change-request body consumed by the orchestrator. | Used during finalize |
| `metrics.json` | Per-phase and aggregate duration, usage, and cost evidence. | No |

# State contract

`adw/state.schema.json` defines the portable version-one shape. Historical GitHub-shaped fields remain canonical for compatibility, while TypeScript-specific and provider-neutral fields are additive. Readers tolerate unknown additive fields; resume must not depend on a new field without an explicit schema and compatibility change.

State is saved after meaningful phase transitions so resume can skip completed work. Default state persistence preserves the historical contract. Managed runs use a separate durable control root, stricter atomic persistence, and worktree-local agent artifacts.

# Source-controlled versus generated

| Category | Examples | Policy |
| --- | --- | --- |
| Hand-maintained | `src/`, tests, `.adw/config.json`, `.pi/prompts/`, wiki concepts | Review as source. |
| Generated and committed | `.adw/prompts/`, `.claude/commands/` mirror | Change through their sources and regeneration/sync tools. |
| Generated and ignored | `agents/`, `coverage/`, `dist/`, batch archives | Never treat as canonical documentation or commit by default. |

# Citations

[1] [State implementation](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/state.ts)

[2] [Portable state schema](https://github.com/kortiene/switchyard/blob/main/adw/state.schema.json)

[3] [Metrics collector](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/metrics.ts)

[4] [Cross-language state tests](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/test/cross-language-state.test.ts)
