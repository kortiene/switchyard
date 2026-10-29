# Switchyard wiki update log

## 2026-10-29

* **Accepted direction**: Recorded the owner's decisions. Rust is the sole long-term engine, including a native Rust agent runtime. Pi Durable 1.4.0 behind the sandboxed TypeScript bridge is a proof-of-concept adapter, not the end state. `adw_sdlc/` stays only as a reference; there is no coexistence or user-compatibility promise. Python compatibility is dropped and state v1 is frozen. `runner-pi` and the TypeScript execution path are frozen legacy. TypeScript managed-worktree work is stopped; S3 ([#97](https://github.com/kortiene/switchyard/issues/97)) owns native worktree lifecycle. Pi run `babe0070` (PR #73) is partial evidence only; an unassisted Pi real-issue run is still owed.
* **Proposed architecture**: Added the Rust-first/Pi Durable direction to the decision register, linked its normative transitions, replay identity, confinement and integration gate, and updated decision navigation. Superseded later the same day by the accepted-direction entry. The TypeScript checkout remains the implemented reference, not the long-term engine.

## 2026-07-18

* **Initialization**: Created the OKF v0.1 bundle, its first concept set, authoring templates, and validation guidance.
* **Validation**: Added the producer-side `wiki:check` quality gate and linked it into the repository verification chain.
