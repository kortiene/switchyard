---
type: Decision Register
title: Control-plane invariants and decision status
description: The implemented, accepted, stopped, and deferred choices that define Switchyard's safety and extensibility boundaries.
tags: [decisions, invariants, security, architecture]
timestamp: "2026-10-29T19:30:00Z"
---

# Status vocabulary

* **Implemented** means current source and tests enforce the choice.
* **Accepted** means the owner has decided the direction. Record the decision date. Acceptance is not implementation and does not by itself change current defaults.
* **Stopped** means in-progress work on that track has been halted. Existing source may remain; it is not an active development contract.
* **In development** means the current checkout implements it, but it may not yet be a committed or released contract. No register row uses this status after 2026-10-29.
* **Proposed** means a documented design under review; it is not implemented behavior or a change to current defaults.
* **Deferred** means design material exists but the behavior is intentionally absent.

# Register

| Decision | Status | Consequence |
| --- | --- | --- |
| Use a TypeScript/Node control plane with four runner adapters. | Implemented | Current checkout. Kept only as a reference while useful. No long-term coexistence or user-compatibility promise. |
| Build a Rust mission core as the sole long-term engine. Pi Durable 1.4.0 behind the sandboxed TypeScript bridge is the proof-of-concept runtime; a native Rust agent runtime is the target. | Accepted (2026-10-29) | Switchyard is unreleased and has no users. [Rust-first M1](../../docs/rust-first/README.md) and [#94](https://github.com/kortiene/switchyard/issues/94) define the recovery and approval contracts. S2 gates the proof of concept only. NO-GO does not preserve a TypeScript product path. |
| Rust mission ledger: explicit transitions, optimistic revisions, application-owned SQLite, a fake runtime, and a single-host owner lock. | Implemented | S1 ([#95](https://github.com/kortiene/switchyard/issues/95)) enforces the normative transition table in `crates/switchyard-core`. It does not resume frozen state v1, call a model, write to GitHub, or confine a Pi process. Pi Durable remains the proof-of-concept adapter. |
| Require a reviewed source/package match before adopting the Pi Durable proof of concept. | Accepted; S2 remains NO-GO | The [S2 audit](../../docs/rust-first/s2-feasibility.md) reproduces missing admission APIs in npm 1.4.0 relative to the reviewed source. Its diagnostic tests are not S2 contract acceptance. The proposed source-build remedy and full integration remain unproven; native Rust remains the target. |
| Freeze `runner-pi` and the TypeScript execution path as legacy. | Accepted (2026-10-29) | Fixes only. Retire them with the TypeScript engine. They are not the Pi Durable proof-of-concept bridge and not the native Rust runtime. |
| Keep one `AgentRunner.runPhase()` seam and branch on capabilities. | Implemented | Backend details stay in adapters; the orchestrator consumes normalized requests/results. |
| Make phase flow deterministic and keep built-in control semantics in the kernel. | Implemented | Projects may configure known/custom phases, but cannot redefine load-bearing built-in meanings. |
| Let the orchestrator own Git, forge, CI, and merge operations. | Implemented | Runner children edit files but do not receive forge authority in phased mode. |
| Build runner environments from a deny-by-default allowlist. | Implemented | Unknown and control-plane secrets are withheld unless isolation is explicitly bypassed. |
| Freeze state v1 and drop Python compatibility. | Accepted (2026-10-29) | `adw/state.schema.json` and `agents/<adw-id>/state.json` stay as they are: no further changes and no new consumers. Existing TypeScript resume may keep reading v1. That is not a Python-compatibility or new-engine promise. Do not delete the schema or `adw_sdlc/` under this decision. |
| Separate universal kernel behavior from repository project packs. | Implemented | Projects configure policy, prompts, schemas, models, and providers without forking core orchestration. |
| Prefer declarative CLI/REST provider descriptors to in-process project code. | Implemented | Common provider integrations can be configured and validated without expanding trusted code. |
| Stop TypeScript managed-worktree work. | Stopped (2026-10-29) | Do not describe the checkout as in development. S3 ([#97](https://github.com/kortiene/switchyard/issues/97)) owns worktree lifecycle natively in Rust as part of the sandbox. |
| Add arbitrary out-of-process provider code plugins. | Deferred | The trust, packaging, protocol, and lifecycle boundary requires a separate design and implementation. |
| Add cross-run retrieved memory or LlamaIndex-based enrichment. | Deferred | Determinism and auditable repository context remain primary until provenance, staleness, and injection controls are settled. |

# Change constraints

The following require an explicit design and security/compatibility review, not an incidental refactor:

* weakening environment allowlists or denied prefixes;
* giving runners Git or forge authority;
* changing phase preamble contracts or load-bearing structured fields;
* changing frozen state v1 or adding new consumers of it;
* making additive state fields required for legacy resume;
* allowing project configuration to bypass built-in safety checks;
* resuming TypeScript managed-worktree development, or force-removing or broadly pruning managed worktrees;
* executing project-supplied provider code inside the control-plane process.

# Reading historical design material

Design documents explain rationale but may lag implementation. Determine present status from current source, configuration, and tests; record a proposal as implemented only when those sources agree.

# Citations

[1] [Resolved D1-D6 migration decisions](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/PLAN.md#2-resolved-decisions-settled)

[2] [Operational invariants](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/HANDOVER.md#3-operational-invariants--do-not-change-without-a-separate-design-pass)

[3] [Managed-worktree design](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/docs/DESIGN-managed-git-worktrees.md)

[4] [Deferred memory decision](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/MEMORY_STACK.md)

[5] [Rust-first prototype and normative mission transitions](../../docs/rust-first/README.md)

[6] [Runtime protocol and replay identity](../../docs/rust-first/runtime-protocol.md)

[7] [Integration gate, fallback and planned CI](../../docs/rust-first/delivery-plan.md)
