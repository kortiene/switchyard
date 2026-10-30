# Delivery plan: One Recoverable, Verified Change

Status: S1 implemented (#105). S2 is NO-GO; see [#96](https://github.com/kortiene/switchyard/issues/96) and [s2-feasibility.md](s2-feasibility.md). S3–S8 are planned. Tracker: [#94](https://github.com/kortiene/switchyard/issues/94). Architecture: [Rust-first prototype](README.md). Protocol: [initial v0 contract](runtime-protocol.md). Disposition recorded 2026-10-30; see [#109](https://github.com/kortiene/switchyard/issues/109) for current disposition.

## Implementation sequence

Use focused PRs in dependency order. S1's domain with a fake runtime is complete in [#105](https://github.com/kortiene/switchyard/pull/105), closing [#95](https://github.com/kortiene/switchyard/issues/95). Next prove the Pi bridge in [#96](https://github.com/kortiene/switchyard/issues/96), then enable confined real effects. S2 is NO-GO; see [#96](https://github.com/kortiene/switchyard/issues/96) and [s2-feasibility.md](s2-feasibility.md).

| Slice | Issue | Depends on | Reviewable outcome |
| --- | --- | --- | --- |
| S1 | [#95](https://github.com/kortiene/switchyard/issues/95) | Planning review | Create the smallest runnable Rust host that can create, inspect, and recover a mission using a fake runtime. |
| S2 | [#96](https://github.com/kortiene/switchyard/issues/96) | #95 | Proof-of-concept gate: a pinned Pi Durable 1.4.0 session, behind the sandboxed TypeScript bridge, controlled and recovered by Rust. Not an end-state runtime decision. |
| S3 | [#97](https://github.com/kortiene/switchyard/issues/97) | #95, #96 | Let the proof-of-concept runtime use coding tools while Rust owns the filesystem/process boundary, trusted credentials, and worktree lifecycle. TypeScript managed-worktree work is stopped. |
| S4 | [#98](https://github.com/kortiene/switchyard/issues/98) | #95, #96, #97 | Run the mission lifecycle in Rust while Pi owns only its agent session and internal tasks. |
| S5 | [#99](https://github.com/kortiene/switchyard/issues/99) | #97, #98 | Make verified and approved states depend on observed content and host-controlled checks rather than agent messages. |
| S6 | [#100](https://github.com/kortiene/switchyard/issues/100) | #99 | Produce one explicitly authorized PR and recover safely when a GitHub response is lost. |
| S7 | [#101](https://github.com/kortiene/switchyard/issues/101) | #98, #99, #100 | Expose the complete mission experience without putting authority in the browser. |
| S8 | [#102](https://github.com/kortiene/switchyard/issues/102) | #95, #96, #97, #98, #99, #100, #101 | Close the milestone only with evidence that interruption, verification, and delivery boundaries hold. |

The issue checklist is the operational source for progress. This document explains sequencing and milestone acceptance; it must not be marked implemented merely because issues or tests exist.

## S2 integration gate and fallback

S2 is the proof-of-concept gate for pinned Pi Durable 1.4.0 behind the sandboxed TypeScript bridge. Switchyard is unreleased and has no users. Rust is the sole long-term engine, including a native Rust agent runtime. GO means this proof of concept may be used for downstream M1 live work behind the replaceable adapter. GO does not make Pi Durable the end state, and it does not create a TypeScript coexistence or user-compatibility promise. `adw_sdlc/` stays only as a reference while useful. `runner-pi` and the TypeScript execution path stay frozen legacy (fixes only).

| Term | Meaning for this gate |
| --- | --- |
| Pi Durable 1.4.0 behind the sandboxed TypeScript bridge | Proof-of-concept runtime. The GO/NO-GO rows below test this adapter only. |
| Native Rust agent runtime | Long-term target behind the same replaceable adapter. A NO-GO does not demote this target and does not select the TypeScript engine instead. |
| `adw_sdlc/` | Reference while useful. No long-term coexistence or user-compatibility promise. |
| State v1 | Frozen (`adw/state.schema.json`, `agents/<adw-id>/state.json`): no further changes, no new consumers. Python compatibility is dropped. |

S2 records GO or NO-GO in its issue and evidence ledger with exact source/package/Node identities, commands, results and unresolved constraints. The [2026-10-29 package audit](s2-feasibility.md) records NO-GO for published 1.4.0: its source/API does not match the inspected baseline. S2 is NO-GO; see [#96](https://github.com/kortiene/switchyard/issues/96) and that audit. The diagnostic `pi-package-audit` check and the `pi-source-candidate` rebuild are not the planned `pi-bridge-contract` check. #96 stays NO-GO until every proof row below passes. An unproven mandatory property is NO-GO, not a warning. This cleanup does not execute that gate.

| Required proof | NO-GO condition |
| --- | --- |
| Reproducible installation | No installable, integrity-locked package/source combination matching the reviewed baseline, or undocumented source/package divergence |
| Public recovery APIs | Cannot reopen and inspect persisted submissions/live work or deduplicate admission without private table access; cursor replay is optional if snapshot resync is proven |
| Stable tool-effect identity | Cannot recover the same durable Pi tool-task identity/effect-key mapping across replay and route it to Rust receipts |
| Controlled registry/execution | Required coding tools bypass the Rust ExecutionEnv, or the bridge cannot disable arbitrary extension/code loading under the stated allowlist |
| Observable shutdown | Cannot distinguish answered/idle/abort settlement, or cannot expose enough ownership/process state for host-supervised stop and recovery |
| Protocol correctness | Lost acknowledgment, restart, identity conflict, stale-generation or bounded transport tests fail under the pinned runtime |

Package 1.4.0 is an inspected source baseline, not a waiver for a missing release. A different published version or reproducible source build may be proposed with exact integrity/provenance and a compatibility delta; review and update the pinned baseline, then rerun the full S2 gate. Do not silently use latest, import private Pi internals, or weaken a required invariant to obtain GO. S2 validates the logical confinement seam; actual bridge/agent OS enforcement is a separate blocking S3 gate.

On NO-GO:

1. Keep S1 and the fake runtime usable. Record the failing test/API/version and mark Pi-dependent live work in S3-S8 blocked by #96; do not mark #96 completed as a successful integration.
2. S3-S8 may prepare runtime-independent fixtures, domain/verification tests, UI mocks, and delivery reconciliation against fake runtime/forge adapters. Those results do not satisfy the Pi Durable proof of concept, the sandbox, or live acceptance criteria.
3. Do not fall back to `adw_sdlc/` as the product. It remains a reference only. No automatic substitution that promotes `runner-pi`, no Pi source fork, and no user-compatibility promise.
4. The target runtime remains native Rust behind the replaceable adapter. A compatible pin or bounded adapter fix may still be proposed to unblock the proof of concept; that remedy does not change the target. This NO-GO note does not itself authorize a broad rewrite. Downstream live work waits on a revised Pi Durable GO or a reviewed native-Rust runtime gate, not on the TypeScript engine.
5. Resume the real S3-S8 path only after that revised gate is GO. S3 must pass the OS confinement gate, including native worktree lifecycle, before live mission execution. TypeScript managed-worktree work stays stopped.

## Incremental demonstrations

1. **Foundation:** create a mission through the Rust CLI, restart, inspect the same attempt and pending command. No model or forge access.
2. **Runtime proof:** Rust starts the pinned Pi Durable proof of concept, submits through the bridge, loses an acknowledgment, recovers one submission, and steers at a documented boundary. This does not adopt Pi Durable as the end state.
3. **Controlled implementation:** Pi edits only its owned, confined workspace through Rust; cancellation stops descendants; stale generations cannot publish results.
4. **Verified review:** frozen checks fail, a bounded repair changes the content, new evidence passes, stale approval is refused, and the operator approves the current digest.
5. **Delivery:** materialize that approved Git tree and publish one PR; recover a simulated lost response without duplication.
6. **Product demonstration:** perform the same lifecycle through the local mission page, including browser reconnect, process restart, review and explicit publication.

The final live run uses a deliberately selected fixture repository/issue and explicit operator authorization for provider spend and publication. Planning approval does not itself run those effects.

## Acceptance matrix

| Scenario | Required observation | Owning slice |
| --- | --- | --- |
| Browser closes | Host continues; reconnect reconstructs persisted state | S7 |
| Restart before/after Pi admission | One command, one submission, same business attempt | S1, S2, S4 |
| Crash during tool effect | Stable tool-task/effect key returns the original receipt after a new generation; ambiguous outcomes block | S2, S3, S8 |
| Old worker/process remains alive | No replacement writer or review; stale results rejected | S3 |
| Cancellation during shell execution | All supervised descendants stopped before terminal state | S3, S4 |
| Agent changes tests or claims success | Host's frozen verification contract still decides | S5 |
| Truncated/missing evidence | INCOMPLETE / ERROR blocks approval | S5 |
| File mutation after tests | Review digest becomes stale and approval is rejected | S5 |
| Repair or usage limit reached | Counter survives restart; no automatic new paid assignment | S4, S5 |
| PR created, response lost | Reconcile original branch/PR, retain one delivery | S6 |
| Existing branch has another head | Conflict reported; no blind overwrite or silent rebase | S6 |
| Runtime version changes | Compatible pinned reopen or explicit block | S2 |
| Unsupported bridge or agent confinement | Live implementation/verifier fails closed; confinement CI fails rather than skips | S3, S5 |
| Pi Durable proof-of-concept gate fails | NO-GO recorded; Pi-dependent live path blocked. The TypeScript engine is not the fallback product. Native Rust remains the target runtime. Fake-only preparation is explicitly identified | S2 |
| Final operator demonstration | Reviewed digest, evidence and single PR linked in ledger | S8 |

Credential-free automation should include fake-provider/fake-forge protocol tests and real Git/process/sandbox integration on the declared supported platform. A live provider run is a separate acceptance gate. Keep the existing `npm run verify` behavior unchanged; invoke it from `adw_sdlc/` when running the current package gate. Add and document the Rust/bridge checks in S1/S2. Do not claim unrun checks pass.

## Planned CI and required checks

The current [verify workflow](../../.github/workflows/verify.yml) runs the TypeScript gate on Node 20.19.0 and 22 with check names verify (node 20.19.0) and verify (node 22). The floor leg skips the optional existing Pi SDK; both current suites use mocks. Neither proves the new Pi Durable bridge. S1 adds the separate `rust-core` job and does not change those TypeScript checks or branch protection.

As of 2026-10-30, `pi-package-audit` and `pi-source-candidate` also exist. They record the published-package NO-GO and the pinned source rebuild. They are not `pi-bridge-contract`. S2 is NO-GO; see [#96](https://github.com/kortiene/switchyard/issues/96) and [s2-feasibility.md](s2-feasibility.md). A green audit or source rebuild is not live execution.

Add these separate jobs as their implementation slices land:

| Stable proposed check name | Runner / runtime | Required coverage | Owner |
| --- | --- | --- | --- |
| rust-core | ubuntu-24.04; Rust 1.99.1, which is also the S1 MSRV (`rust-toolchain.toml`) | Formatting/lint, Rust unit/domain/persistence tests, fake runtime and real Git fixtures. The job exists; it is not a required check until a maintainer adds that exact name to branch protection or rulesets. | S1 |
| pi-bridge-contract | ubuntu-24.04; Rust toolchain plus an exact maintained Node 22 patch >=22.19.0, pinned and verified in S2 | Locked Pi install/typecheck and faux-provider protocol/replay/receipt tests; never inherit the Node 20 leg or skip Pi | S2 |
| execution-confinement | ubuntu-24.04 Linux x86_64; same pinned Rust/Node; backend/image and capabilities recorded by S3 | Real bridge and agent confinement, process-tree stop, denied filesystem/egress/credential access, replayed effect identity | S3 |
| prototype-acceptance | ubuntu-24.04; same pins and backend | Composed fake-provider/fake-forge crash drills and local UI/CLI integration; real Git/workspaces | S8, with S7 tests |

The Node patch and Rust toolchain must be committed with reproducible install instructions; Node >=22.19.0 is a compatibility floor, not an unbounded installation target. The bridge job must assert its resolved version and dependency presence. The confinement job must fail, not skip or fall back to unconfined execution, when the runner lacks a required kernel/container capability. If hosted ubuntu-24.04 cannot enforce the selected backend, S3 stays blocked until a reviewed runner/backend change supplies the same proof; do not award a synthetic green result.

Retain the existing TS checks. After each new job exists and reports reliably, an administrator must add its exact reported check name to main's branch protection/ruleset, then verify the enforced set. Renaming a job/matrix requires a corresponding protection update. Adding workflow YAML alone does not make a job required, and this plan does not claim current settings were inspected or changed. Keep stable aggregate names if a matrix is later introduced.

Live provider spend and real GitHub publication remain a separately authorized operator acceptance run, not required credential-bearing jobs on arbitrary PR code. A green prototype-acceptance job proves simulated provider/forge scenarios only.

## Existing backlog triage

Planning snapshot (2026-10-28): eight of these issues were open, and the repository had no open pull requests. The repository is not GitHub-archived. Current disposition is [consolidation tracker #109](https://github.com/kortiene/switchyard/issues/109). This section records that disposition. It does not copy #109's delivery sequence, and it does not open, close, or reassign the issues.

| Existing issue | Disposition aligned with #109 |
| --- | --- |
| [#28](https://github.com/kortiene/switchyard/issues/28) — fenced-output sample | Legacy/deferred. Retained open, with no M1 dependency and no new TypeScript feature investment. |
| [#30](https://github.com/kortiene/switchyard/issues/30) — nudge rate | Legacy/deferred. Retained open as prior cost/latency evidence, with no M1 dependency and no new TypeScript feature investment. |
| [#34](https://github.com/kortiene/switchyard/issues/34) — Python/TS cutover | Closed 2026-10-30 as not planned. The 2026-10-29 Rust direction supersedes the old cutover. Not marked completed. Closing it does not authorize deleting `adw/` or `adw_sdlc/`. State v1 stays frozen: no further changes, no new consumers. |
| [#38](https://github.com/kortiene/switchyard/issues/38) — TS linter | Legacy/deferred. Retained open, with no M1 dependency and no new TypeScript feature investment. Bridge checks keep their own explicit scope. |
| [#40](https://github.com/kortiene/switchyard/issues/40) — Pi TUI tests | Legacy/deferred. Retained open. Existing Cockpit work is not S7, and this track does not add TypeScript feature investment. |
| [#42](https://github.com/kortiene/switchyard/issues/42) — provider code plugins | Deferred and demand-gated. No implementation in the consolidation track. Distinct from a bundled trusted Pi adapter. |
| [#43](https://github.com/kortiene/switchyard/issues/43) — provider extension tail | Deferred and demand-gated. No implementation in the consolidation track. M1 has one GitHub delivery path. |
| [#44](https://github.com/kortiene/switchyard/issues/44) — institutional memory | Deferred and demand-gated. No implementation in the consolidation track. Beyond M1. |

Preserve historical issue bodies. If this table and #109 diverge, update #109 rather than growing a second queue here.

Reuse behavior and test cases from the existing implementation where relevant: secret allowlists, Git ownership, worktree reconciliation, dry-run expectations, bounded repair and structured errors. Do not port the entire phase engine, generalized provider framework, or all runner backends before demonstrating one mission.

## Decisions implementation must resolve

These are bounded engineering choices, not reasons to reopen the agreed architecture:

- S1, resolved in the mission-ledger implementation: Rust 1.99.1 is the committed toolchain and MSRV. The mission database uses WAL and `synchronous=FULL` for committed process-crash durability; power-loss recovery is not claimed. Payload hashes use the integer-only JCS subset documented in the [S1 resolutions](README.md#s1-resolutions). The CLI is `switchyard mission` with create, status, command, recover, notify, lock, successor, and content. The owner lock is an exclusive flock plus a monotonic generation row; status does not take it. Tests cover the generated transition matrix, restart, fake runtime, and real Git. Live provider and live GitHub acceptance are not claimed.
- S2: exact installable Pi/Node versions, bridge mapping storage, public API compatibility and replay/resync details.
- S3: one Linux confinement backend proven on ubuntu-24.04, bridge/agent mount layout, process-tree supervision and egress/resource policy.
- S5: verifier-visible workspace manifest, snapshot/freeze mechanism, completeness limits and artifact retention.
- S6: commit materialization/exclusion semantics, branch identity, target/base drift handling and GitHub credential setup.
- S7: minimal local web stack and local-session/origin protections.

Record each decision with its owning implementation PR and tests. Do not let an unresolved boundary silently fall back to unconfined execution or automatic approval.

## Definition of done

All eight issue acceptance checklists are satisfied and an acceptance ledger records:

- exact source/package versions, lockfiles, supported platform/backend, and actual commands run;
- input issue, base SHA, mission/attempt/session identities and approved plan;
- the injected interruption and recovery result;
- failed and passing verification evidence, repair/usage accounting, and current review digest;
- explicit plan, review and publication decisions;
- resulting commit, branch and one PR URL;
- original-checkout preservation, limitations, and any blocked/deferred cases.

M1 does not promise cross-host failover, lossless power-failure recovery, exactly-once arbitrary effects, hard provider billing caps, green external CI, automatic merge, or deployment. Claims should be no stronger than the recorded evidence.
