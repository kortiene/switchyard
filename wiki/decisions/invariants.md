---
type: Decision Register
title: Control-plane invariants and decision status
description: The implemented, in-development, and deferred choices that define Switchyard's safety and extensibility boundaries.
tags: [decisions, invariants, security, architecture]
timestamp: "2026-07-18T13:26:10Z"
---

# Status vocabulary

* **Implemented** means current source and tests enforce the choice.
* **In development** means the current checkout implements it, but it may not yet be a committed or released contract.
* **Deferred** means design material exists but the behavior is intentionally absent.

# Register

| Decision | Status | Consequence |
| --- | --- | --- |
| Use a TypeScript/Node control plane with four runner adapters. | Implemented | Claude, Codex, OpenCode, and Pi share one kernel rather than four workflows. |
| Keep one `AgentRunner.runPhase()` seam and branch on capabilities. | Implemented | Backend details stay in adapters; the orchestrator consumes normalized requests/results. |
| Make phase flow deterministic and keep built-in control semantics in the kernel. | Implemented | Projects may configure known/custom phases, but cannot redefine load-bearing built-in meanings. |
| Let the orchestrator own Git, forge, CI, and merge operations. | Implemented | Runner children edit files but do not receive forge authority in phased mode. |
| Build runner environments from a deny-by-default allowlist. | Implemented | Unknown and control-plane secrets are withheld unless isolation is explicitly bypassed. |
| Preserve version-one state fields and make extensions additive. | Implemented | Resume and cross-language fixtures remain compatible across engine-specific metadata additions. |
| Separate universal kernel behavior from repository project packs. | Implemented | Projects configure policy, prompts, schemas, models, and providers without forking core orchestration. |
| Prefer declarative CLI/REST provider descriptors to in-process project code. | Implemented | Common provider integrations can be configured and validated without expanding trusted code. |
| Support Switchyard-owned linked-worktree lifecycle. | In development | The current checkout adds durable ownership, exact reconciliation, conservative retention, and non-force cleanup. |
| Add arbitrary out-of-process provider code plugins. | Deferred | The trust, packaging, protocol, and lifecycle boundary requires a separate design and implementation. |
| Add cross-run retrieved memory or LlamaIndex-based enrichment. | Deferred | Determinism and auditable repository context remain primary until provenance, staleness, and injection controls are settled. |

# Change constraints

The following require an explicit design and security/compatibility review, not an incidental refactor:

* weakening environment allowlists or denied prefixes;
* giving runners Git or forge authority;
* changing phase preamble contracts or load-bearing structured fields;
* making additive state fields required for legacy resume;
* allowing project configuration to bypass built-in safety checks;
* force-removing or broadly pruning managed worktrees;
* executing project-supplied provider code inside the control-plane process.

# Reading historical design material

Design documents explain rationale but may lag implementation. Determine present status from current source, configuration, and tests; record a proposal as implemented only when those sources agree.

# Citations

[1] [Resolved D1-D6 migration decisions](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/PLAN.md#2-resolved-decisions-settled)

[2] [Operational invariants](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/HANDOVER.md#3-operational-invariants--do-not-change-without-a-separate-design-pass)

[3] [Managed-worktree design](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/docs/DESIGN-managed-git-worktrees.md)

[4] [Deferred memory decision](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/MEMORY_STACK.md)
