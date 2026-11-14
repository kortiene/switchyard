# Runtime adapter contract and native Rust runtime gate

Status: **proposed**. Not accepted. D1 and D2 describe how the code already works. The five gaps named in Context now go through declared profiles and exhaustive matches ([#185](https://github.com/kortiene/switchyard/issues/185)). The `AgentRuntime` trait, moving `ExecutionEnv` off `FakeRuntime`, the typed `RuntimeMethod` enum, and the conformance suite (D3–D6) are not implemented. This ADR does not change the FakeRuntime default, does not enable Pi, and does not authorize a live run.

S2 is **GO for the credential-free source-built candidate** and **NO-GO for the published npm package**. See [#96](https://github.com/kortiene/switchyard/issues/96), [s2-contract-gate.md](s2-contract-gate.md), and [s2-feasibility.md](s2-feasibility.md).

Companions: [architecture](README.md), [runtime protocol](runtime-protocol.md), [delivery plan](delivery-plan.md), [S2 contract gate](s2-contract-gate.md), [S3 execution receipts](s3-execution-receipts.md).

Proposed register row for [`wiki/decisions/invariants.md`](../../wiki/decisions/invariants.md), using that page's vocabulary:

| Decision | Status | Consequence |
| --- | --- | --- |
| The agent runtime is a simple, sealed adapter, and the Host decides. Adapter differences are declared profiles. Define `AgentRuntime` and the Rust `ExecutionEnv` as written contracts with a shared conformance suite, and gate the native Rust agent runtime with proof rows that mirror S2. | Proposed | No workflow tools in any runtime. No public or project-supplied adapters (#42 stays deferred). Trait extraction waits for the native runtime. FakeRuntime stays the product default. |

## Context

**Already decided.** Rust is the sole long-term engine and owns durable authority. Pi Durable is the proof-of-concept runtime behind a bundled, trusted, sandboxed adapter. The native Rust agent runtime is the committed long-term target (accepted 2026-10-29; [README: Architecture decision](README.md#architecture-decision)). Runtime notifications "are observations used by Rust to evaluate guards; they cannot set mission state" ([README: Normative mission transitions](README.md#normative-mission-transitions)). FakeRuntime is the product default. Pi is reached only through the explicit `--runtime confined-pi` opt-in.

**The code today** (`crates/switchyard-core/src/execution/adapter.rs`):

- `RuntimeAdapter` is a closed `pub(crate)` enum: `Fake`, `Pi` (`pi-contract`), and `Confined` (`pi-contract` + `linux-execution` + Linux). There is no trait and no `dyn`.
- Adapter differences that affect Host behavior are declared profiles, not `is_confined()` branches:
  - `RecoveryProfile`: which recovery steps run (open session and settle stops before reconcile, resume after a delivered stop, skip verify for a supervised writer).
  - `DispatchProfile.host_owned`: whether the Host routes stops, work and observation polling itself.
  - `FactProfile.uncertainty_is_host_fact`: whether uncertainty is recorded as a supervisor fact instead of a runtime observation.
  - `LinuxLedgerProfile`: whether Linux effects are admitted, an admitted stop is kept, and live effects are supervised.
  - `GuidanceProfile`: operator status text. A recorded adapter name resolves through a table of the names `name()` publishes, paired with that variant's `guidance()` value. Unknown names stay inert.
  - `AuthorityProfile`: whether the adapter may certify mission facts and quiescence, and which command check it declared (`Simulated` or `PiProtocol`). `validate_admission` and `validate_command` read this profile.
  - `ExecutionProfile.simulated`: whether the in-process simulated `ExecutionEnv` is available. Only Fake sets it.
- `Host::recover` runs one recovery sequence parameterized by `RecoveryProfile`.
- The five decisions that used to special-case Fake are closed on the enum ([#185](https://github.com/kortiene/switchyard/issues/185)). Non-host-owned observation intake is `pop_observation` / `requeue_observation`, both `Result`s, with no `expect`. `runtime_method` matches every `CommandKind` and has no `_` arm; kinds that are not pause, cancel, or steer still map to submit. Simulated apply is `RuntimeAdapter::apply_simulated`. Pi and Confined still refuse it with `simulated execution requires FakeRuntime` and do not execute. `ExecutionEnv` is still implemented by `FakeRuntime`. The trait, the typed `RuntimeMethod` enum, and an unknown-method `unsupported_capability` error (A8) are not extracted.

Without a written contract, three things stay unspecified: what a native runtime must satisfy, whether a Pi fix is "enough", and whether a new adapter is safe by default.

## Decision

### D1. The agent is simple; the Host decides (normative now)

1. A runtime gets **no workflow tools**. It reaches files and processes only through Rust-owned execution: the simulated `ExecutionEnv` today, and supervised Linux execution (supervisor and ledger) for `Confined`. The bundled tool allowlist ([README: Bridge process confinement](README.md#bridge-process-confinement-and-in-process-allowlist)) enforces this today. The proposed D6 "Controlled registry/execution" row will enforce it for new adapters.
2. **Verification, approval, publication and landing are Host operations.** The Host also owns dispatch (`DispatchProfile.host_owned`), stop admission and recovery. A runtime may report observations. It never sets mission state, settles an approval, or asserts execution quiescence.
3. Planning approval is not yet an end-to-end CLI flow. Until S4 planning lands ([#172](https://github.com/kortiene/switchyard/issues/172)), the `awaiting_plan_approval` row and its plan digest are seeded through SQL, and `approve_plan` runs against that seeded row.
4. "The Host owns publication" fixes only *where* publication runs. How it is materialized, which branch identity and credentials it uses, and who authorizes a live run are decided in [#170](https://github.com/kortiene/switchyard/issues/170) (S6) and [#167](https://github.com/kortiene/switchyard/issues/167). This ADR does not pre-decide them. M1 ends at PR publication ([README: Verification and delivery](README.md#verification-and-delivery)), so landing has no M1 implementation.

### D2. Sealed adapter set with declared profiles (normative now)

1. The adapter set is closed: `RuntimeAdapter { Fake, Pi, Confined }`, with enum dispatch. There is no public `Box<dyn AgentRuntime>` and no project-supplied adapter. This follows the bundled-trusted-adapter rule ([README: Scope and deferred work](README.md#scope-and-deferred-work)) and keeps [#42](https://github.com/kortiene/switchyard/issues/42) deferred.
2. Every adapter difference that the Host acts on is a declared profile (`RecoveryProfile`, `DispatchProfile`, `FactProfile`, `LinuxLedgerProfile`, `GuidanceProfile`, `AuthorityProfile`, `ExecutionProfile`), not a branch on the variant. The five Context gaps are closed: admission and command checks read `AuthorityProfile`, observation intake and simulated apply are adapter methods, `runtime_method` is exhaustive over `CommandKind`, and recorded guidance is a name lookup. Each profile function is an exhaustive match, so a new variant must declare each profile explicitly. The defaults are inert. `AuthorityProfile` has no `Default`; every variant sets both of its fields. Trait extraction remains D8.
3. There is one effect journal and one recovery sequence, parameterized by `RecoveryProfile`. The Host has no `is_confined()` branches.
4. Uncertainty and replay semantics stay host facts (`FactProfile.uncertainty_is_host_fact`). An adapter never owns them, and a host-owned dispatcher must record uncertainty as a host fact.
5. FakeRuntime is the product default. Pi is the proof-of-concept runtime. The native Rust runtime is the committed long-term target, and it is the only planned additional adapter.
6. Deferred:
   - Extracting the full `pub(crate) AgentRuntime` trait (D3–D5) waits for a second real adapter, the native Rust runtime, and does not start before the thin issue-to-PR path works.
   - A general plugin or multi-vendor backend layer waits until after [#102](https://github.com/kortiene/switchyard/issues/102).

### D3. Two contracts, not one (target)

1. **AgentRuntime** is the conversation and model side: admission, steering, abort, inspection, observations and quiescence reports. Pi and the native Rust runtime implement it.
2. **ExecutionEnv** is the Rust-owned effect boundary: intent, apply, observe and receipt. Only Rust backends implement it (simulated, and confined Linux). An agent runtime never implements it.

At extraction, `ExecutionEnv` moves off `FakeRuntime` into its own Host component, so the Host no longer reaches it through `fake_mut()`.

### D4. Contract surface (normative semantics)

| Concern | Contract |
| --- | --- |
| Identity | Every call carries a `RuntimeBinding` (mission, attempt, session, generation). The adapter rejects a stale generation or mismatched binding (`stale_generation`, `binding_mismatch`) before any read or side effect. Durable identities (`requestId` for admission, `op-v1` for effects) never include generation, wire ID, timestamp or retry count ([runtime-protocol.md](runtime-protocol.md#stable-execution-identity-under-tool-replay)). |
| Admission | `admit` returns exactly one of **Admitted**(receipt), **Rejected**(stable category; durably known *not* admitted), or **Uncertain**(may have been admitted). Transport errors map to Uncertain, never to Rejected. |
| Replay / idempotency | Same ID and same payload hash returns the original receipt. Same ID with a different hash is `command_conflict` / `operation_conflict`. The Host always calls `inspect` before re-sending a pending ID. The adapter never assigns a fresh ID to escape uncertainty. |
| Inspect | `inspect(id)` returns **Known**(receipt), **Absent** (proven not admitted), or **Unknown**. Only Known and Absent may settle an outbox row. |
| Abort | `abort` returns **Accepted** (request durably delivered) or **Settled** (work ended), with typed timeout or failure. Accepted is not quiescence ([runtime-protocol.md](runtime-protocol.md#completion-and-cancellation)). |
| Quiescence | `quiescence()` reports runtime idle separately from execution quiescent. Only the Rust supervisor and the ExecutionEnv can assert execution quiescence. An adapter cannot satisfy `ConfirmStopped`, `ConfirmCancelled` or `RequestVerification` by itself. |
| Observation-only reports | Adapters emit phase-free observations through one adapter-neutral channel (`pop_observation` / `requeue_observation`). Which observations an adapter may certify is `AuthorityProfile.certifies_mission_facts`. Fake certifies them. Pi and Confined do not, and `validate_admission` still returns the existing guard. The trait method in the D5 sketch is not extracted. |
| Tool-effect identity | Runtime tool calls reach the ExecutionEnv with a durable `tool_task_id` and semantic `effect_key`. The Host derives `op-v1`. On replay the same key returns the original receipt or the uncertainty. It never re-executes. |
| Errors and uncertainty | Use the protocol's stable categories ([runtime-protocol.md](runtime-protocol.md#errors-compatibility-and-tests)). Every error answers "admitted, rejected, or uncertain?". Uncertainty moves a nonterminal mission to `needs_attention` and is never retried as a new operation. |
| Versioning | `hello()` returns adapter kind, protocol version, source/package fingerprint and capabilities, frozen into `attempts.runtime_config_json`. An incompatible reopen is an explicit block. |

### D5. Rust sketch (host side, illustrative)

This trait is still the extraction target. [#185](https://github.com/kortiene/switchyard/issues/185) landed `AuthorityProfile`, `pop_observation` / `requeue_observation`, and `apply_simulated` on the sealed enum. It did not add `contract.rs`, this trait, or typed `RuntimeMethod`.

```rust
// crates/switchyard-core/src/execution/contract.rs (proposed; pub(crate); extracted with the native runtime)
pub(crate) enum RuntimeMethod { Submit, Steer, Abort, Continue }   // replaces &'static str; unknown method is unsupported_capability (A8)

pub(crate) enum Admission  { Admitted(RuntimeReceipt), Rejected { category: String }, Uncertain { detail: String } }
pub(crate) enum Inspection { Known(RuntimeReceipt), Absent, Unknown { detail: String } }
pub(crate) enum AbortState { Accepted(RuntimeReceipt), Settled(RuntimeReceipt), Unknown { detail: String } }
pub(crate) struct Quiescence { pub runtime_idle: bool, pub open_tool_tasks: Vec<String> } // never "execution quiescent"

pub(crate) trait AgentRuntime {
    fn hello(&mut self) -> Result<RuntimeIdentity>;
    fn open(&mut self, binding: &RuntimeBinding, require_existing: bool) -> Result<()>;
    fn admit(&mut self, method: RuntimeMethod, req: &RuntimeRequest, payload: &Value) -> Admission;
    fn inspect(&mut self, req: &RuntimeRequest, payload: &Value) -> Result<Inspection>;
    fn abort(&mut self, req: &RuntimeRequest) -> AbortState;
    fn quiescence(&mut self, binding: &RuntimeBinding) -> Result<Quiescence>;
    fn next_observation(&mut self, binding: &RuntimeBinding) -> Result<Option<RuntimeNotification>>;
    fn requeue_observation(&mut self, note: RuntimeNotification);          // no `expect`
    fn close(&mut self, binding: &RuntimeBinding) -> Result<()>;
    // Declared profiles stay data, not behavior:
    fn recovery_profile(&self) -> RecoveryProfile;
    fn dispatch_profile(&self) -> DispatchProfile;
    fn fact_profile(&self) -> FactProfile;
    fn linux_ledger(&self) -> LinuxLedgerProfile;
    fn guidance(&self) -> GuidanceProfile;
    fn authority(&self) -> AuthorityProfile;                               // default: certifies nothing
}

pub(crate) enum EffectObservation { Applied(ExecutionReceipt), NotApplied, Unknown { detail: String } }

pub(crate) trait ExecutionEnv {                       // Rust-owned backends only
    fn apply(&mut self, req: &ExecutionRequest) -> Result<ExecutionReceipt>;
    fn observe(&mut self, req: &ExecutionRequest) -> Result<EffectObservation>;
    fn quiesce(&mut self, binding: &RuntimeBinding) -> Result<bool>;   // writers/process tree stopped
}

// The sealed set stays the only constructor: enum dispatch, no dyn, no plugins.
pub(crate) enum RuntimeAdapter {
    Fake(FakeRuntime),
    #[cfg(feature = "pi-contract")] Pi(Box<PiRuntime>),
    #[cfg(all(feature = "pi-contract", feature = "linux-execution", target_os = "linux"))]
    Confined(Box<ConfinedRuntime>),
    /* Native(...) when the native runtime exists */
}
impl AgentRuntime for RuntimeAdapter { /* delegate per variant; exhaustive match */ }
```

Out of process, the trait is the Host-side client of the JSONL protocol in [runtime-protocol.md](runtime-protocol.md). A native runtime that runs as its own process implements the same wire protocol.

### D6. Shared conformance suite and native runtime gate

The suite is a generic harness, `fn conformance<A: AgentRuntime + FaultInjection>(make: impl Fn(&Path) -> A)`. It runs against `FakeRuntime` in `rust-core`, against Pi in `pi-bridge-contract` / `execution-confinement`, and against the native runtime in its own job. `FaultInjection` generalizes the existing `fail_next_persist` / `fail_next_simulated_effect` hooks and the SIGKILL `crash_worker` pattern.

| # | Case | Expected |
| --- | --- | --- |
| A1 | Duplicate admit, same ID and hash | Original receipt; one admission |
| A2 | Same ID, different hash | `command_conflict`; no admission |
| A3 | Lost ack (crash after runtime admit, before Host ack) | `inspect` → Known; Host acknowledges without resend |
| A4 | Crash before runtime admit | `inspect` → Absent; resend with the **same** ID |
| A5 | Transport failure mid-admit | `Uncertain`, never `Rejected` |
| A6 | Durable refusal | `Rejected{category}`; outbox records the rejection |
| A7 | Stale generation / wrong binding on every method | Rejected before any read or effect |
| A8 | Unknown/unsupported method | Typed `unsupported_capability`, **not** submit |
| B1 | Abort accepted vs settled | Distinguishable; `ConfirmStopped` refused until settled and execution quiescent |
| B2 | Abort timeout | Typed `cancellation_timeout` → `needs_attention` |
| B3 | Runtime idle ≠ execution quiescent | Adapter cannot satisfy `ConfirmStopped` / `ConfirmCancelled` / `RequestVerification` |
| C1 | Illegal observations | Quarantined, not applied; phase unchanged |
| C2 | Observation outside `AuthorityProfile` | Refused, default deny |
| C3 | Transient failure during observation apply | Requeued in order, no panic |
| C4 | Event gap / partial stream | `resync_required` → snapshot; completion never inferred from silence |
| D1 | Effect replay after a new generation | Same `op-v1`, one effect, original receipt |
| D2 | Multiple effects in one tool task; nested tasks | Distinct IDs, no collision |
| D3 | Altered args/scope/deadline under the same key | `operation_conflict` |
| D4 | Prepared/unknown effect replay | `execution_uncertain`, no apply |
| D5 | `observe` after crash | Applied / NotApplied / Unknown matches what happened |
| E1 | Reopen with a changed fingerprint | Explicit block; state retained |
| E2 | Restart before/after admission | One command, one submission, same business attempt |
| F1–F3 | Corrupt/oversized frame, backpressure, slow reply (**out-of-process only**) | Connection closed, then reconciliation; no assumed retry |

For FakeRuntime to pass D5 and B3 it needs a durable, inspectable simulated effect store. Today `simulated_files` is in memory.

The native runtime gate mirrors S2 ([delivery-plan.md](delivery-plan.md)). Passing it makes the native adapter eligible for downstream M1 live work under the same S3 confinement gate, subject to the live-run authorization in [#167](https://github.com/kortiene/switchyard/issues/167). An unproven mandatory row is NO-GO.

| Required proof | NO-GO condition | S2 row it mirrors |
| --- | --- | --- |
| Reproducible build | Native runtime and provider client not built from `Cargo.lock` under the pinned toolchain, or provider client unpinned or unaudited | Reproducible installation |
| Public recovery APIs | Cannot reopen its own documented, versioned storage and answer `inspect` (Known/Absent/Unknown) | Public recovery APIs |
| Stable tool-effect identity | Cannot persist `tool_task_id`/`effect_key` before dispatch and recover the same map across restart (D1–D5) | Stable tool-effect identity |
| Controlled registry/execution | Any tool reaches files, processes or network except through the Rust ExecutionEnv; registry not closed and frozen per attempt; any eval or dynamic loading; any workflow tool (D1) | Controlled registry/execution |
| Observable shutdown | Cannot distinguish answered / idle / abort-accepted / abort-settled, or cannot run under the Rust process supervisor (B1–B3) | Observable shutdown |
| Protocol correctness | Any row A–E (and F if out of process) fails under the pinned build | Protocol correctness |
| Credential and egress boundary | Model credentials reachable from the Host or agent sandbox, or egress not limited to the configured endpoint ([README: Workspace and trust boundary](README.md#workspace-and-trust-boundary)) | Bridge confinement (S3) |

Gate evidence is recorded in a dedicated issue, as #96 does. The proposed check name is `native-runtime-contract`. It is not required until an admin adds it.

### D7. Plan approval and workspace model

1. **Plan digest.** This ADR adopts [#172](https://github.com/kortiene/switchyard/issues/172) (S4-A) as written and adds nothing to it:
   - `awaiting_plan_approval → implementing` (approve plan): "authorized actor confirms the current plan digest; implementation policy is unchanged."
   - "Plan drift invalidates approval; approval targets the current plan digest only."
   - "Changing scope or policy requires a successor attempt and never inherits an old approval."
2. **Workspace.** Each attempt runs in a Host-owned worktree created from an exact base commit, with its lifecycle owned by S3 in Rust ([README: Workspace and trust boundary](README.md#workspace-and-trust-boundary)). This ADR records that as the model and opens no new workspace work.
3. Binding publication to a reviewed-content digest is out of scope here. See open question 4.

### D8. Sequencing

1. Adopt D1 and D2 as written. This is documentation only, with no code change.
2. Close the five Context gaps on the current enum before S4 grows ([#185](https://github.com/kortiene/switchyard/issues/185)). No trait extraction. S4–S8 build the thin issue-to-PR path on these profiles. Planning admission ([#172](https://github.com/kortiene/switchyard/issues/172)) should read `AuthorityProfile` through `validate_admission` / `validate_command`. Plan approval stays a Host operation; this ADR adds no second planning flag.
3. When native runtime work starts (not before the thin path works), extract D3–D5 in one behavior-preserving step: move `ExecutionEnv` off `FakeRuntime` into its own Host component, and replace the `&'static str` method with typed `RuntimeMethod` so an unknown method is `unsupported_capability` (A8) rather than submit. `AuthorityProfile` already exists as declared data. Coordinate with C3 [#112](https://github.com/kortiene/switchyard/issues/112), which owns `service.rs` extraction.
4. The conformance harness passes on FakeRuntime, then runs against Pi. Rows Pi fails are recorded as evidence, not waived.
5. Open the native gate issue.

## Consequences

- **Positive:**
  - A runtime has no route to workflow authority, so the native runtime gets a narrow, testable definition of done.
  - New adapters are deny-by-default for mission facts and inherit one recovery path.
  - Pi fixes and native work are judged by the same evidence.
  - The thin path is not blocked on a refactor.
- **Negative / cost:**
  - The five Context gaps are closed on the enum. Extraction still touches `adapter.rs`, `service.rs` and `service/execution.rs`, which overlap with #112 and #114: the trait, moving `ExecutionEnv` off `FakeRuntime`, and typed `RuntimeMethod` / A8.
  - FakeRuntime grows a durable effect store.
- **Unchanged:**
  - FakeRuntime stays the product default, and Pi stays opt-in.
  - The S2 records stay as they are: published package NO-GO, credential-free source-built candidate GO ([#96](https://github.com/kortiene/switchyard/issues/96), [s2-contract-gate.md](s2-contract-gate.md), [s2-feasibility.md](s2-feasibility.md)).
  - No live execution, no TypeScript fallback, no plugin mechanism (#42 stays deferred).

## Alternatives considered

1. **Keep adding variants without a contract.** Lowest churn, but nothing forces a new variant to satisfy replay, abort or authority semantics. Rejected as the contract. Kept as the construction mechanism.
2. **Public trait / `Box<dyn AgentRuntime>`.** Opens a plugin surface, which conflicts with the bundled-trusted-adapter rule and with deferred #42. Rejected.
3. **Extract the trait now.** Medium refactor with one real non-fake adapter. A general trait would push uncertainty and replay into each adapter. Deferred to native runtime work.
4. **A general multi-vendor backend layer.** Competes with S4–S8 capacity and is not needed for one verified change. Deferred until after #102.
5. **Out-of-process protocol as the only contract.** Would force FakeRuntime and unit tests through process I/O. Chosen as a layer instead: the trait is the Host-side client, the wire protocol is one transport, and rows F1–F3 are protocol-only.
6. **Give the runtime workflow tools (open PR, run verify).** Moves authority out of the Host and widens every adapter's contract. Rejected.

## Open questions for the owner

1. **Process boundary for the native runtime.** In process with the Host, or a separate supervised process? The README puts model credentials in a confined runtime domain. In process would put them in the Host's trust domain.
2. **First provider.** Which model provider and API does the native runtime target first? This fixes the egress row and credential handling. Related: [#166](https://github.com/kortiene/switchyard/issues/166).
3. **Tracking and checks.** `AuthorityProfile` now exists as declared data on the enum. Where should the conformance suite be tracked when extraction starts, and should `rust-core` become a required check before `native-runtime-contract`?
4. **Publish-time digest binding.** The README already binds approval to a digest of the attempt snapshot, reviewed workspace and evidence, rechecked at approval and publication. #167 requires publication approval for the current review digest. What that digest covers, and how it is computed, belongs to the S6/S8 decisions ([#170](https://github.com/kortiene/switchyard/issues/170), [#102](https://github.com/kortiene/switchyard/issues/102)). Hashing a raw diff would give false mismatches from line endings, generated files and mode bits, so the digest needs to be defined over a normalized tree, consistent with #170's delivery manifest and exclusion ledger.
