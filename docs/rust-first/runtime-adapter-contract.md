# Runtime adapter contract and native Rust runtime gate

Status: **proposed**. Not accepted. The `AgentRuntime` trait and conformance suite below are not implemented. This ADR does not record a live run or a change to the FakeRuntime default.

S2 is **GO for the credential-free source-built candidate** and **NO-GO for the published npm package**. See [#96](https://github.com/kortiene/switchyard/issues/96), [s2-contract-gate.md](s2-contract-gate.md), and [s2-feasibility.md](s2-feasibility.md). Companions: [architecture](README.md), [runtime protocol](runtime-protocol.md), [delivery plan](delivery-plan.md), [S3-A2 execution receipts](s3-execution-receipts.md).

Baseline inspected: `main` at `e4938ae` (PR #124 merged 2026-10-31). The `./scripts/rust-verify.sh` gate passed locally at that commit: 102 core and 6 CLI tests. Later `main` (through #163 and #164) contradicts parts of that snapshot: the `pi-bridge-probe` workflow is removed; `RuntimeAdapter` is the closed enum `Fake`, `Pi` (`pi-contract`), and `Confined` (`pi-contract` + `linux-execution` + Linux); recovery steps are declared as `RecoveryProfile` instead of `is_confined()` inside `Host::recover`. There is still no trait. Line numbers below are from the inspected baseline.

Proposed register row for [`wiki/decisions/invariants.md`](../../wiki/decisions/invariants.md), using that page's vocabulary:

| Decision | Status | Consequence |
| --- | --- | --- |
| Define the agent-runtime adapter and the Rust ExecutionEnv as written contracts with a shared conformance suite. Gate the native Rust agent runtime with proof rows that mirror S2. | Proposed | FakeRuntime must pass the suite before any new adapter work. Pi and native adapters are judged by the same suite. Adopting this proposal does not change the source-built S2 GO and does not imply a live run. |

## Context

**What is already decided.** Rust owns durable authority, and the agent runtime sits behind a replaceable adapter. "The native Rust agent runtime is the target that replaces that adapter" ([README.md:53](README.md)). The runtime protocol says the native runtime "implements this public contract for new attempts" ([runtime-protocol.md:5](runtime-protocol.md), [:139](runtime-protocol.md)). Runtime notifications "are observations used by Rust to evaluate guards; they cannot set mission state" ([README.md:124](README.md)).

**What blocks the current path.** The published Pi Durable 1.4.0 package is NO-GO: the npm package comes from a different source commit than the reviewed baseline ([s2-feasibility.md](s2-feasibility.md)). The credential-free source-built candidate later recorded GO ([s2-contract-gate.md](s2-contract-gate.md)). On a published-package NO-GO, "downstream live work waits on a revised Pi Durable GO or a reviewed native-Rust runtime gate" ([delivery-plan.md](delivery-plan.md)). That native gate is proposed here and is not accepted. The `pi-bridge-probe` workflow named below was removed in #163; Pi evidence now runs in `pi-bridge-contract` and `execution-confinement`.

**What the code did at the inspected baseline.** Current `main` is the closed enum described in the status note, not the two-variant snapshot in these bullets:

- `RuntimeAdapter` is a closed `pub(crate)` enum: `Fake(FakeRuntime)`, plus `Pi(Box<PiRuntime>)` behind `pi-contract` (`crates/switchyard-core/src/execution/adapter.rs:10`). There is no trait. Behavior differences live in `match` arms and `#[cfg]` branches.
- Authority is decided by variant, not declared. `validate_admission` refuses runtime-observation facts and quiescence commands only `if let Self::Pi(_)` (`adapter.rs:41`). `Host::apply_notification` refuses whenever `fake_mut()` is `None` (`crates/switchyard-core/src/service.rs:841`). A third adapter would inherit whichever default the match arm gives it.
- Dispatch is stringly typed and defaults to submit:
  - `runtime_method()` maps `Pause | Cancel` to `"abort"`, `Steer` to `"steer"`, and everything else to `"submit"` (`service.rs:1640`).
  - `RuntimeAdapter::dispatch` matches `"abort"`, `"steer"`, and `_ => runtime.submit(...)` (`adapter.rs:101-103`).
  - An unrecognized method silently becomes an admission.
- Observation intake is fake-only. `Host::poll_runtime` pulls from `fake_mut().and_then(FakeRuntime::pop_notification)` and uses `.expect("notification was popped from FakeRuntime")` on requeue (`service.rs:787-815`). No adapter-neutral observation channel exists.
- Execution effects are attached to the fake agent runtime. S3-A2 (#124) adds `pub(crate) trait ExecutionEnv { fn apply(...) }` (`crates/switchyard-core/src/execution_env.rs:77`). Only `FakeRuntime` implements it (`execution/runtime.rs:498`), and `Host` reaches it through `self.runtime.fake_mut()` (`crates/switchyard-core/src/service/execution.rs:42`, `:113`). The architecture assigns the execution boundary to Rust, independent of the runtime ([README.md:57-58](README.md); #97: "the boundary itself is owned by Rust and must not depend on Pi").
- The building blocks already exist, but they are scattered:
  - stable `operation_id` (`execution/runtime.rs:14`);
  - `RuntimeBinding` with generation (`runtime.rs:31`);
  - `RuntimeReceipt` (`runtime.rs:47`);
  - `reconcile` as an inspect-before-resend step (`adapter.rs:77`; used at `service.rs:623` and `:657`);
  - narrow durable refusals via `Error::AdmissionRejected` / `rejected_admission()` (`crates/switchyard-core/src/error.rs:65`, `:140`; from #121);
  - fault injection in FakeRuntime (`fail_next_persist`, `fail_next_simulated_effect`).
- Conformance requirements are already written. The runtime protocol names the S2 conformance set ([runtime-protocol.md:141](runtime-protocol.md)) and the mandatory execution-identity conformance ([:96](runtime-protocol.md)). They are not a reusable suite: the Pi tests live in `crates/switchyard-core/tests/pi_bridge.rs` and `src/pi_host_tests.rs`, and the S1 and S3-A2 tests are FakeRuntime-specific.

Without a written contract, three things stay unspecified: what a native runtime must satisfy, whether a Pi fix is "enough", and whether a new adapter is safe by default.

## Decision

### D1. Two contracts, not one

1. **AgentRuntime** is the conversation and model side: admission, steering, abort, inspection, observations and quiescence reports. Pi (proof of concept) and the native Rust runtime implement it.
2. **ExecutionEnv** is the Rust-owned effect boundary: intent, apply, observe and receipt. It is implemented by Rust backends only (simulated now; confined file/process later, per #97 Track A). It is never implemented by an agent runtime.

`ExecutionEnv` moves off `FakeRuntime` into its own `Host` component, so `Host` no longer reaches it through `self.runtime.fake_mut()`. The simulated backend stays test-only or default-only, as today.

### D2. Contract surface (normative semantics)

| Concern | Contract |
| --- | --- |
| Identity | Every call carries a `RuntimeBinding` (mission, attempt, session, generation). The adapter rejects a stale generation or mismatched binding (`stale_generation`, `binding_mismatch`) before any read or side effect. Durable identities (`requestId` for admission, `op-v1` for effects) never include generation, wire ID, timestamp or retry count ([runtime-protocol.md:76-94](runtime-protocol.md)). |
| Admission | `admit` returns exactly one of three outcomes: **Admitted**(receipt), **Rejected**(stable category; durably known *not* admitted), or **Uncertain**(may have been admitted). Transport errors map to Uncertain, never to Rejected. This formalizes #121's narrow refusals. |
| Replay / idempotency | Same ID and same payload hash returns the original receipt. Same ID with a different hash is `command_conflict` / `operation_conflict`. The Host always calls `inspect` before re-sending a pending ID ([README.md:181](README.md)). The adapter never assigns a fresh ID to escape uncertainty. |
| Inspect | `inspect(id)` returns **Known**(receipt), **Absent** (proven not admitted), or **Unknown**. Only Known and Absent may settle an outbox row. Unknown keeps it pending or uncertain. |
| Abort | `abort` returns **Accepted** (request durably delivered) or **Settled** (work ended), with typed timeout or failure. Accepted is not quiescence ([runtime-protocol.md:104-120](runtime-protocol.md)). Abort has its own stable command identity. |
| Quiescence | `quiescence()` reports runtime idle separately from execution quiescent. Only the Rust supervisor and the ExecutionEnv can assert *execution* quiescence ([README.md:214](README.md)). An adapter may report runtime idle. It cannot satisfy `ConfirmStopped`, `ConfirmCancelled` or `RequestVerification` by itself. |
| Observation-only reports | Adapters emit `RuntimeNotification`-like observations with no phase field (`execution/runtime.rs:55`), through one adapter-neutral channel that replaces `fake_mut().pop_notification()`. Each adapter declares an `AuthorityProfile` listing which observations it may certify. Today Fake may certify simulated facts and Pi may certify none (`adapter.rs:41-63`). A default deny replaces the variant `match`. |
| Tool-effect identity | Runtime tool calls reach the ExecutionEnv with a durable `tool_task_id` and a semantic `effect_key`. The Host derives `op-v1`. On runtime replay, the same key returns the original receipt or the uncertainty. It never re-executes ([runtime-protocol.md:96](runtime-protocol.md); `service/execution.rs:211-234`). |
| Errors and uncertainty | Use the protocol's stable categories ([runtime-protocol.md:137](runtime-protocol.md)) and `Error::category()`. Every error answers "admitted, rejected, or uncertain?". Uncertain moves a nonterminal mission to `needs_attention` (`AttentionReason::*Uncertain`) and is never retried as a new operation. |
| Versioning | `hello()` returns adapter kind, protocol version, source/package fingerprint and capabilities. These are frozen into `attempts.runtime_config_json` (`service.rs:320`). An incompatible reopen is an explicit block ([runtime-protocol.md:139](runtime-protocol.md)). |

### D3. Rust sketch (host side, illustrative)

```rust
// crates/switchyard-core/src/execution/contract.rs (proposed; pub(crate))
pub(crate) enum RuntimeMethod { Submit, Steer, Abort, Continue }   // replaces &'static str + `_ => submit`

pub(crate) struct RuntimeIdentity {
    pub adapter: &'static str, pub protocol: u32,
    pub fingerprint: String, pub capabilities: Capabilities,
}

pub(crate) enum Admission  { Admitted(RuntimeReceipt), Rejected { category: String }, Uncertain { detail: String } }
pub(crate) enum Inspection { Known(RuntimeReceipt), Absent, Unknown { detail: String } }
pub(crate) enum AbortState { Accepted(RuntimeReceipt), Settled(RuntimeReceipt), Unknown { detail: String } }
pub(crate) struct Quiescence { pub runtime_idle: bool, pub open_tool_tasks: Vec<String> } // never "execution quiescent"

/// Which observations this adapter may turn into mission facts. Default: none.
pub(crate) struct AuthorityProfile { pub may_certify: &'static [ObservationKind] }

pub(crate) trait AgentRuntime {
    fn hello(&mut self) -> Result<RuntimeIdentity>;
    fn open(&mut self, binding: &RuntimeBinding, require_existing: bool) -> Result<()>;
    fn admit(&mut self, method: RuntimeMethod, req: &RuntimeRequest, payload: &Value) -> Admission;
    fn inspect(&mut self, req: &RuntimeRequest, payload: &Value) -> Result<Inspection>;
    fn abort(&mut self, req: &RuntimeRequest) -> AbortState;
    fn quiescence(&mut self, binding: &RuntimeBinding) -> Result<Quiescence>;
    fn next_observation(&mut self, binding: &RuntimeBinding) -> Result<Option<RuntimeNotification>>;
    fn requeue_observation(&mut self, note: RuntimeNotification);          // transient-failure path, no `expect`
    fn authority(&self) -> AuthorityProfile;
    fn close(&mut self, binding: &RuntimeBinding) -> Result<()>;
}

pub(crate) enum EffectObservation { Applied(ExecutionReceipt), NotApplied, Unknown { detail: String } }

pub(crate) trait ExecutionEnv {                       // Rust-owned backends only
    fn apply(&mut self, req: &ExecutionRequest) -> Result<ExecutionReceipt>;
    /// Needed by #125: inspect a prepared/uncertain effect without applying it.
    fn observe(&mut self, req: &ExecutionRequest) -> Result<EffectObservation>;
    fn quiesce(&mut self, binding: &RuntimeBinding) -> Result<bool>;   // writers/process tree stopped
}

// Host keeps a closed set: enum dispatch over trait-implementing types (no dyn, no plugins, #42 stays deferred).
pub(crate) enum RuntimeAdapter { Fake(FakeRuntime), #[cfg(feature = "pi-contract")] Pi(Box<PiRuntime>) /*, Native(...) later */ }
impl AgentRuntime for RuntimeAdapter { /* delegate per variant; exhaustive match */ }
```

The trait is the contract. The enum stays the only way to construct an adapter: no `Box<dyn AgentRuntime>` from outside the crate and no project-supplied adapters. Out of process, the trait is the Host-side client of the JSONL protocol in [runtime-protocol.md](runtime-protocol.md). A native runtime that runs as its own process implements the same wire protocol.

### D4. Shared conformance suite

The suite is a generic harness, `fn conformance<A: AgentRuntime + FaultInjection>(make: impl Fn(&Path) -> A)`, run against `FakeRuntime` in `rust-core` now. Pi evidence belongs in `pi-bridge-contract` and `execution-confinement` (the separate `pi-bridge-probe` workflow was removed in #163), and Native would run in its own job. `FaultInjection` generalizes the existing `fail_next_persist` / `fail_next_simulated_effect` hooks and the SIGKILL `crash_worker` pattern (`service/execution/tests.rs:311`).

| # | Case | Expected | Source of requirement |
| --- | --- | --- | --- |
| A1 | Duplicate admit, same ID and hash | Original receipt; one admission | runtime-protocol.md:64 |
| A2 | Same ID, different hash | `command_conflict`; no admission | runtime-protocol.md:64 |
| A3 | Lost ack (crash after runtime admit, before Host ack) | `inspect` → Known; Host acknowledges without resend | README.md:181; #121 |
| A4 | Crash before runtime admit | `inspect` → Absent; resend with the **same** ID | README.md:175-181 |
| A5 | Transport failure mid-admit | `Uncertain`, never `Rejected` | runtime-protocol.md:137 |
| A6 | Durable refusal | `Rejected{category}`; outbox records rejection (`rejection_json`) | #121 |
| A7 | Stale generation / wrong binding on every method | Rejected before any read or effect | README.md:183 |
| A8 | Unknown/unsupported method | Typed `unsupported_capability`, **not** submit | adapter.rs:103 (current gap) |
| B1 | Abort accepted vs settled | Distinguishable; `ConfirmStopped` refused until settled and execution quiescent | README.md:214; policy.rs `require_pause_settled` |
| B2 | Abort timeout | Typed `cancellation_timeout` → needs_attention | runtime-protocol.md:118 |
| B3 | Runtime idle ≠ execution quiescent | Adapter cannot satisfy `ConfirmStopped`/`ConfirmCancelled`/`RequestVerification` | adapter.rs:41-63 |
| C1 | Observations carry no phase; illegal ones are quarantined, not applied | `notification_quarantine` row; phase unchanged | README.md:163 |
| C2 | Observation outside `AuthorityProfile` | Refused, default deny | service.rs:841 (current variant check) |
| C3 | Transient failure during observation apply | Requeued in order, no panic | service.rs:796-815 |
| C4 | Event gap / partial stream | `resync_required` → snapshot; completion never inferred from silence | runtime-protocol.md:55, :130 |
| D1 | Effect replay after a new generation | Same `op-v1`, one effect, original receipt | runtime-protocol.md:96 |
| D2 | Multiple effects in one tool task; nested tasks | Distinct IDs, no collision | runtime-protocol.md:96 |
| D3 | Altered args/scope/deadline under the same key | `operation_conflict` | s3-execution-receipts.md:19-21 |
| D4 | Prepared/unknown effect replay | `execution_uncertain`, no apply | service/execution.rs:229-233 |
| D5 | `observe` after crash | Applied / NotApplied / Unknown is consistent with what happened | #125 |
| E1 | Reopen with a changed fingerprint | Explicit block; state retained | runtime-protocol.md:139 |
| E2 | Restart before/after admission | One command, one submission, same business attempt | delivery-plan.md acceptance matrix |
| F1–F3 | Corrupt/oversized frame, backpressure, slow reply (**out-of-process adapters only**) | Connection closed, then reconciliation; no assumed retry | runtime-protocol.md:11, :141 |

For FakeRuntime to pass D5 and B3, it needs a durable, inspectable simulated effect store. Today `simulated_files` is ephemeral (`execution/runtime.rs:275`; [s3-execution-receipts.md:12-14](s3-execution-receipts.md)).

### D5. Native Rust runtime gate (mirrors S2)

This gate is separate from S2. Passing it authorizes the native adapter for downstream M1 live work under the same S3 confinement gate. An unproven mandatory row is NO-GO, as in S2 ([delivery-plan.md:33](delivery-plan.md)).

| Required proof | NO-GO condition | S2 row it mirrors ([delivery-plan.md:37-42](delivery-plan.md)) |
| --- | --- | --- |
| Reproducible build | Native runtime and provider client are not built from `Cargo.lock` under the pinned toolchain (`rust-toolchain.toml`), or the provider SDK/endpoint client is unpinned or unaudited | Reproducible installation |
| Public recovery APIs | Cannot reopen session storage and answer `inspect` (Known/Absent/Unknown) for submissions and live work using its own documented, versioned storage | Public recovery APIs |
| Stable tool-effect identity | Cannot persist `tool_task_id`/`effect_key` before dispatch and recover the same map across restart (suite D1–D5) | Stable tool-effect identity |
| Controlled registry/execution | Any tool reaches files, processes or network except through the Rust ExecutionEnv; tool registry not closed and frozen per attempt; any eval or dynamic loading | Controlled registry/execution |
| Observable shutdown | Cannot distinguish answered / idle / abort-accepted / abort-settled, or cannot run under the Rust process supervisor (suite B1–B3) | Observable shutdown |
| Protocol correctness | Any conformance row A–E (and F if out of process) fails under the pinned build | Protocol correctness |
| Credential and egress boundary (native-specific) | Model credentials reachable from the Host or agent sandbox, or egress not limited to the configured endpoint ([README.md:191](README.md), [:206](README.md)) | Bridge confinement (S3) |

Gate evidence is recorded in a dedicated issue with exact commit, lockfile hash, toolchain, commands run and results, as #96 does. The proposed CI check name is `native-runtime-contract`, which is not required until an admin adds it.

### D6. Sequencing

1. Merge D1 (move `ExecutionEnv` off `FakeRuntime`), the typed `RuntimeMethod`, and `AuthorityProfile`. These are behavior-preserving, except that A8 becomes an explicit error. Coordinate with C3 #112, which owns `service.rs` extraction. Land after it, or as its own reviewed semantic change, per #112's "no semantic change in extraction" rule.
2. The conformance harness passes on FakeRuntime in `rust-core`.
3. Run the same harness against Pi in `pi-bridge-contract` / `execution-confinement`. Rows it fails become recorded evidence in #96, not waivers. The removed `pi-bridge-probe` workflow is not a target.
4. Open the native gate issue. Native work starts only after steps 1–2.

## Consequences

- **Positive:**
  - The native target gets a testable definition of done.
  - Pi fixes and native work are judged by the same evidence.
  - A new adapter is deny-by-default for mission facts.
  - The `_ => submit` fallthrough and the `expect` in `poll_runtime` go away.
  - #125 gets the `observe` primitive it needs to separate proven-completed from proven-not-applied.
- **Negative / cost:**
  - Touches `adapter.rs`, `service.rs` and `service/execution.rs`, which overlap with #112 and #114.
  - FakeRuntime grows a durable effect store.
  - The suite will expose gaps in the Pi probe, which is intended but adds S2 work.
  - About 2–3 focused PRs, plus one docs PR.
- **Unchanged:**
  - FakeRuntime remains the CLI default.
  - The published-package audit stays NO-GO. The credential-free source-built candidate stays GO. This proposal does not change either record.
  - No live execution, no TypeScript fallback ([delivery-plan.md:50](delivery-plan.md)), no plugin mechanism (#42 stays deferred).

## Alternatives considered

1. **Keep pushing a Pi pin or source build only** (`runtime/pi-source/`, `NEEDS-FULL-S2`). This remains allowed, and the ADR does not stop it. Without a contract, though, GO stays tied to Pi-specific tests, and the native target stays undefined. Not chosen as the *only* path.
2. **Keep the closed enum and add variants.** Lowest churn, but authority and method mapping stay implicit in `match` arms (`adapter.rs:41`, `:101`; `service.rs:841`), and nothing forces a new variant to satisfy replay or abort semantics. Rejected as the contract. Kept as the construction mechanism: a sealed set with enum dispatch over a trait.
3. **Public trait / `Box<dyn AgentRuntime>`.** Opens a plugin surface, which conflicts with the bundled-trusted-adapter rule ([README.md:197-199](README.md)) and with deferred #42. Rejected.
4. **Out-of-process protocol as the only contract.** The JSONL protocol ([runtime-protocol.md](runtime-protocol.md)) is already the Pi contract and gives a natural trust boundary for model credentials ([README.md:195](README.md)). Making it the sole contract would force FakeRuntime and unit tests through process I/O. Chosen as a *layer*: the trait is the Host-side client, the wire protocol is one transport, and the suite has protocol-only rows F1–F3.
5. **Defer the contract until S2 GO.** Leaves the native gate undefined while the only accepted runtime target is blocked. Rejected.

## Open questions for the owner

1. **Process boundary for the native runtime.** Should it run in process with the Host or as a separate supervised process? README.md:191 and :195 put model credentials in a confined runtime domain. In-process would put them in the Host's trust domain.
2. **First provider.** Which model provider and API does the native runtime target first? This fixes the egress row and the credential handling.
3. **Parallel or sequential.** Is the native gate meant to proceed in parallel with Pi S2 remediation, or only after a time or budget limit on Pi?
4. **Deadline semantics.** Should `deadline_ms` stay inside the effect payload hash? A runtime replay must then persist the original deadline exactly (s3-execution-receipts.md:19-21, :54).
5. **Tracking.** Should the conformance suite and the AuthorityProfile change be tracked under #97 Track A, #96, or a new issue under #94?
6. **Required check.** Is `native-runtime-contract` acceptable as a check name, and should `rust-core` become a required check first?
