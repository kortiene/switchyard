# Delivery plan: One Recoverable, Verified Change

Status: planning. Tracker: [#94](https://github.com/kortiene/switchyard/issues/94). Architecture: [Rust-first prototype](README.md). Protocol: [initial v0 contract](runtime-protocol.md).

## Implementation sequence

Use focused PRs in dependency order. First build the domain with a fake runtime, then prove the Pi bridge, then enable confined real effects. The first implementation issue is [#95](https://github.com/kortiene/switchyard/issues/95); the first integration go/no-go is [#96](https://github.com/kortiene/switchyard/issues/96).

| Slice | Issue | Depends on | Reviewable outcome |
| --- | --- | --- | --- |
| S1 | [#95](https://github.com/kortiene/switchyard/issues/95) | Planning review | Create the smallest runnable Rust host that can create, inspect, and recover a mission using a fake runtime. |
| S2 | [#96](https://github.com/kortiene/switchyard/issues/96) | #95 | Validate the highest integration risk before building a full agent workflow: a pinned Pi Durable session controlled and recovered by Rust. |
| S3 | [#97](https://github.com/kortiene/switchyard/issues/97) | #95, #96 | Let Pi use coding tools while Rust owns the filesystem/process boundary and trusted credentials. |
| S4 | [#98](https://github.com/kortiene/switchyard/issues/98) | #95, #96, #97 | Run the mission lifecycle in Rust while Pi owns only its agent session and internal tasks. |
| S5 | [#99](https://github.com/kortiene/switchyard/issues/99) | #97, #98 | Make verified and approved states depend on observed content and host-controlled checks rather than agent messages. |
| S6 | [#100](https://github.com/kortiene/switchyard/issues/100) | #99 | Produce one explicitly authorized PR and recover safely when a GitHub response is lost. |
| S7 | [#101](https://github.com/kortiene/switchyard/issues/101) | #98, #99, #100 | Expose the complete mission experience without putting authority in the browser. |
| S8 | [#102](https://github.com/kortiene/switchyard/issues/102) | #95, #96, #97, #98, #99, #100, #101 | Close the milestone only with evidence that interruption, verification, and delivery boundaries hold. |

The issue checklist is the operational source for progress. This document explains sequencing and milestone acceptance; it must not be marked implemented merely because issues or tests exist.

## Incremental demonstrations

1. **Foundation:** create a mission through the Rust CLI, restart, inspect the same attempt and pending command. No model or forge access.
2. **Runtime proof:** Rust starts pinned Pi Durable, submits through the bridge, loses an acknowledgment, recovers one submission, and steers at a documented boundary.
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
| Crash during tool effect | Reconciled receipt or visible uncertainty; no blind unsafe replay | S3, S8 |
| Old worker/process remains alive | No replacement writer or review; stale results rejected | S3 |
| Cancellation during shell execution | All supervised descendants stopped before terminal state | S3, S4 |
| Agent changes tests or claims success | Host's frozen verification contract still decides | S5 |
| Truncated/missing evidence | INCOMPLETE / ERROR blocks approval | S5 |
| File mutation after tests | Review digest becomes stale and approval is rejected | S5 |
| Repair or usage limit reached | Counter survives restart; no automatic new paid assignment | S4, S5 |
| PR created, response lost | Reconcile original branch/PR, retain one delivery | S6 |
| Existing branch has another head | Conflict reported; no blind overwrite or silent rebase | S6 |
| Runtime version changes | Compatible pinned reopen or explicit block | S2 |
| Unsupported confinement | Live implementation/verifier fails closed | S3, S5 |
| Final operator demonstration | Reviewed digest, evidence and single PR linked in ledger | S8 |

Credential-free automation should include fake-provider/fake-forge protocol tests and real Git/process/sandbox integration on the declared supported platform. A live provider run is a separate acceptance gate. Keep the existing `npm run verify` behavior unchanged; invoke it from `adw_sdlc/` when running the current package gate. Add and document the Rust/bridge checks in S1/S2. Do not claim unrun checks pass.

## Existing backlog triage

Eight issues were open at planning time, with no open PRs. The repository is not GitHub-archived. Preserve these issue histories and states; this planning track neither closes nor silently reassigns them.

| Existing issue | Disposition for Rust M1 |
| --- | --- |
| [#28](https://github.com/kortiene/switchyard/issues/28) — fenced-output sample | Existing TypeScript parity work; no M1 dependency |
| [#30](https://github.com/kortiene/switchyard/issues/30) — nudge rate | Retain as prior cost/latency evidence; no M1 dependency |
| [#34](https://github.com/kortiene/switchyard/issues/34) — Python/TS cutover | Historical integrated-repository track; not a Rust cutover task |
| [#38](https://github.com/kortiene/switchyard/issues/38) — TS linter | Current package maintenance; bridge checks get their own explicit scope |
| [#40](https://github.com/kortiene/switchyard/issues/40) — Pi TUI tests | Existing Cockpit work; new web surface is S7 |
| [#42](https://github.com/kortiene/switchyard/issues/42) — provider code plugins | Remains demand-gated; distinct from a bundled trusted Pi adapter |
| [#43](https://github.com/kortiene/switchyard/issues/43) — provider extension tail | Deferred; M1 has one GitHub delivery path |
| [#44](https://github.com/kortiene/switchyard/issues/44) — institutional memory | Deferred beyond M1 |

Reuse behavior and test cases from the existing implementation where relevant: secret allowlists, Git ownership, worktree reconciliation, dry-run expectations, bounded repair and structured errors. Do not port the entire phase engine, generalized provider framework, or all runner backends before demonstrating one mission.

## Decisions implementation must resolve

These are bounded engineering choices, not reasons to reopen the agreed architecture:

- S1: Rust toolchain/MSRV, database transaction/sync policy, canonical hashing, CLI/API shape and owner-lock behavior.
- S2: exact installable Pi/Node versions, bridge mapping storage, public API compatibility and replay/resync details.
- S3: one supported OS/confinement backend, mount layout, process-tree supervision and egress/resource policy.
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
