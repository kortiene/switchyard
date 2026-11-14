# Switchyard wiki update log

## 2026-11-14

* **Check inventory**: [Testing and CI](reference/testing-and-ci.md) records the `switchyard-ui` job in `verify.yml`. It runs `./scripts/ui-verify.sh` for the read-only dashboard workspace. The job is not a required ruleset check.

## 2026-11-13

* **Check inventory**: Removed `pi-bridge-probe` from [Testing and CI](reference/testing-and-ci.md). The always-on `pi-bridge-contract` job already runs those fmt, Clippy, `pi-contract` workspace, typecheck, and Node tests. `pi-bridge-probe` was not a required ruleset check. The confinement prerequisites now list the pinned Bubblewrap/AppArmor install before the Pi source build. The testing page heading is Retired bridge probe. Slice notes that named the live job now mark it retired 2026-11-13.

## 2026-11-10

* **Bounded composed S3 acceptance**: Accepted 2026-11-10 for the credential-free Host/Pi/Linux evidence collected by #157 at `65a7ce7` (tree `45e756f`). The [acceptance record](../docs/rust-first/s3-composed-acceptance.md) binds post-merge run 42207467133 and artifact 13498690183. The artifact stays `PENDING_REVIEW`; this is the separate reviewed decision. FakeRuntime remains the product default. This does not enable product Pi, authorize credentials, a live provider, TLS, S4–S8, or full M1. #97 and #94 stay open.

## 2026-11-06

* **Composed S3 evidence (#156)**: One credential-free Host/Pi/Linux scenario collects same-attempt SIGKILL/receipt recovery, real process/stop proofs, confined local-stub egress observations and original-checkout preservation. The [collection contract](../docs/rust-first/s3-composed-acceptance.md) and hash-bound artifacts remain `PENDING_REVIEW`; collection is not S3 GO or product enablement. The [retention operator guidance](../docs/rust-first/s3-bridge-cgroup-retention.md#operator-response-to-an-unknown-identity-allocation) now explicitly documents the unsupported in-place repair and safe disposable-fixture response.

## 2026-11-05

* **Bridge cgroup retention (#151)**: Rust journals allocation before mkdir, preserves the current leaf, and retires obsolete recorded leaves only after durable handoff, identity checks and fresh empty proof. Interrupted deletion replays from explicit authorization; missing current or uncertain identities still block. See the [retention contract](../docs/rust-first/s3-bridge-cgroup-retention.md). Historical backlog cleanup is bounded, audit evidence stays, and FakeRuntime/full-S3 gates are unchanged.

* **S3-B6**: Model-endpoint-only egress for the confined Pi bridge (#153). The default remains full network denial. A host-configured HTTP endpoint is reached only through a pinned proxy; unsupported HTTPS fails closed before DNS, and in-bridge tests deny every other destination. See the [egress record](../docs/rust-first/s3-model-egress.md). FakeRuntime remains the product default. This does not enable Pi or complete S3. At B6 delivery #151 and composed acceptance remained; #151 was subsequently delivered by #155. Bounded composed acceptance was recorded 2026-11-10.

## 2026-11-04

* **S3-B5**: Host-owned confined Pi dispatch is delivered in #150 (`fc7a9e4`); #148 is closed. All 7 CI checks passed, including `execution-confinement` with the `pi_host_dispatch` suite. See the [dispatch record](../docs/rust-first/s3-pi-host-dispatch.md). The slice stays credential-free and non-default; FakeRuntime remains the product default. This does not enable Pi in the product or complete S3. Model-endpoint-only egress, bounded cleanup of retained bridge cgroup leaves (#151), and the composed S3 acceptance remain.

## 2026-11-03

* **S2 status**: S2 is GO for the credential-free source-built Pi candidate, accepted 2026-11-02 in #96; see the [S2 acceptance record](../docs/rust-first/s2-contract-gate.md). Pages that said S2 is NO-GO now say so, or date the NO-GO to the slice that recorded it. The published 1.4.0 package NO-GO and the 2026-10-29 audit evidence are unchanged. GO does not enable Pi in the product, live effects or full S3.
* **Required checks**: Pages now match the enforced rulesets, with `execution-confinement` and `pi-bridge-contract` required on main.

## 2026-10-30

* **Active path**: The root README now leads with the Rust S1 host, `./scripts/rust-verify.sh`, and a repository role map. The TypeScript walkthrough, Cockpit, and quality-gate detail are one legacy reference with links to the package README and [Testing and CI](reference/testing-and-ci.md).
* **S2 status**: Live S2 status stays in the [feasibility audit](../docs/rust-first/s2-feasibility.md). Other pages say S2 is NO-GO and point there and at #96. The 2026-10-29 package-audit evidence stays dated. This log does not claim S2 GO or live execution.
* **Backlog**: The delivery-plan triage matches [#109](https://github.com/kortiene/switchyard/issues/109). #34 is closed as not planned, not completed. That close does not authorize deleting `adw/` or `adw_sdlc/`.

## 2026-10-29

* **S2 package NO-GO**: Recorded the published Pi Durable 1.4.0/source mismatch and executable faux-provider/storage probes. The diagnostic `pi-package-audit` job is not the S2 integration gate; #96 and Pi-dependent live work remain blocked.
* **Rust mission ledger**: Recorded the S1 transition kernel and application-owned SQLite ledger as implemented. Pi Durable remains the proof-of-concept adapter. The TypeScript `npm run verify` gate stays separate from the `rust-core` check.
* **Accepted direction**: Recorded the owner's decisions. Rust is the sole long-term engine, including a native Rust agent runtime. Pi Durable 1.4.0 behind the sandboxed TypeScript bridge is a proof-of-concept adapter, not the end state. `adw_sdlc/` stays only as a reference; there is no coexistence or user-compatibility promise. Python compatibility is dropped and state v1 is frozen. `runner-pi` and the TypeScript execution path are frozen legacy. TypeScript managed-worktree work is stopped; S3 ([#97](https://github.com/kortiene/switchyard/issues/97)) owns native worktree lifecycle. Pi run `babe0070` (PR #73) is partial evidence only; an unassisted Pi real-issue run is still owed.
* **Proposed architecture**: Added the Rust-first/Pi Durable direction to the decision register, linked its normative transitions, replay identity, confinement and integration gate, and updated decision navigation. Superseded later the same day by the accepted-direction entry. The TypeScript checkout remains the implemented reference, not the long-term engine.

## 2026-07-18

* **Initialization**: Created the OKF v0.1 bundle, its first concept set, authoring templates, and validation guidance.
* **Validation**: Added the producer-side `wiki:check` quality gate and linked it into the repository verification chain.
