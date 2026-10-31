# Initial Rust / Pi runtime protocol

Status: proposed v0 contract to prove in [S2 / #96](https://github.com/kortiene/switchyard/issues/96). S2 is NO-GO; see [#96](https://github.com/kortiene/switchyard/issues/96) and [s2-feasibility.md](s2-feasibility.md). No existing Rust SDK or upstream wire protocol is implied. Companion: [architecture](README.md).

The TypeScript bridge in this contract is the M1 proof-of-concept adapter for Pi Durable 1.4.0. It is not the end state. The long-term agent runtime is native Rust and implements this public contract for new attempts. `runner-pi` is frozen legacy and is not this bridge.

## Ownership and transport

Rust owns mission state and launches one trusted, pinned TypeScript bridge for the active proof-of-concept attempt. The bridge owns that Pi storage for its lifetime. Rust holds the exclusive host/attempt lock; fail startup if ownership cannot be established. Both processes use the same recorded attempt/session identity. The bridge is replaceable; Pi session internals are not the mission contract.

Use UTF-8 JSON Lines over stdio. Stdout carries protocol frames only; stderr is bounded diagnostic logging with secrets redacted. Frame size, outstanding-request limits, operation deadlines, protocol version, and supported capabilities are negotiated/validated in the handshake. Oversized frames or malformed JSON close the connection and trigger reconciliation, never an assumed retry of an external effect.

Requests are bidirectional: Rust invokes runtime methods; the bridge invokes Rust execution methods. IDs are unique per connection and origin. Distinguish a transient wire ID from a durable command/operation ID. Never synchronously wait for a tool response while preventing the reader from handling other frames. Streams require bounded buffering and explicit truncation/gap reporting.

Proposed envelope:

```json
{
  "protocol": 0,
  "kind": "request",
  "id": "host:17",
  "method": "runtime.submit",
  "binding": {
    "missionId": "m-1",
    "attemptId": "a-1",
    "sessionId": "s-1",
    "generation": 3
  },
  "commandId": "c-42",
  "params": {
    "requestId": "a-1:c-42",
    "content": "Implement the approved plan",
    "payloadHash": "sha256:..."
  }
}
```

A response repeats wire ID and binding and contains either result or a typed error. The trusted runtime never supplies an authoritative mission status. Events carry session identity and a cursor plus notification kind; a cursor gap requires a snapshot/resubscription. Persisted snapshot/receipts, not event delivery, decide recovery.

Command payload hashing uses RFC 8785 canonical JSON (JCS), UTF-8, then SHA-256. S1 implements that canonical form for null, booleans, strings, arrays, objects, and integers, and rejects non-integer numbers instead of inventing ES6 number text. S2 still freezes shared Rust/TypeScript vectors, including non-integers, Unicode, and delimiter cases. Keep session identity opaque; mapping to the underlying Pi storage/conversation is adapter-owned and durable.

## Host-to-runtime methods

| Method | Result/semantics |
| --- | --- |
| runtime.hello | Adapter/protocol versions, package/source identity, storage backend and capabilities; no work admitted |
| runtime.open | Create or reopen the bound session; return conversation identity and current committed state |
| runtime.submit | Admit input with stable requestId; return durable submission receipt, not completion |
| runtime.steer | Admit input with explicit busy/idle behavior; return receipt and admission state |
| runtime.inspect | Snapshot of submissions, run/failure, live owned work and recoverable receipts |
| runtime.watch | Start observation from a supported cursor or request full resync; partial stream is never authoritative |
| runtime.abort | Request abort for the bound work; distinguish accepted from settled |
| runtime.close | Stop scheduling and close storage after owned work settles or report an explicit timeout |

The bridge implements these methods through public Pi APIs. Where the upstream watch surface does not support exact replay, the adapter returns resync_required and serves a snapshot. Do not invent replay guarantees.

Mutations pass through Rust authorization and revision checks before dispatch. UI/browser clients cannot talk to this bridge directly. The bridge validates binding/generation for every command and execution callback.

## Receipts and duplicate handling

- Rust transactionally records command ID, payload hash, expected mission revision, permitted effect, and pending outbox state.
- A bridge admission uses a deterministic Pi requestId derived from attempt plus command ID.
- Lost acknowledgment: inspect/resend with the same identity. Never create a new ID to bypass uncertainty.
- Same ID + same payload returns the original receipt. Same ID + different payload is command_conflict.
- Persist bridge mapping/receipt state through public runtime documents or a documented adapter-owned journal; do not write private Pi tables.
- Read-only methods are repeatable. Administrative methods such as open/abort/close need explicit operation-specific replay behavior; do not assume Pi input deduplication covers every method.
- Runtime storage failure ends the bridge session and surfaces an error. Rust reconciles on reopen rather than treating the last notification as final.
- Sidecar reopening alone does not prove workspace effects are recoverable.

## Execution methods

Pi's ExecutionEnv forwards only the operations needed by the selected coding tools. Proposed surface: file read/range-read, directory list, write/edit, exec start/status/cancel. Source/package compatibility and the exact required signatures are S2/S3 deliverables.

Every mutating operation carries an operation ID, immutable argument hash, attempt/session/generation, capability scope, and deadline. Rust validates permissions and records intent before execution. Completed receipts are returned on duplicate requests; uncertain effects are reconciled or blocked.

### Stable execution identity under tool replay

For M1, derive each mutating effect identity as:

```text
operationId = "op-v1:" + hex(SHA256(JCS([
  "switchyard.exec.v1", attemptId, sessionId, piToolTaskId, effectKey
])))
```

JCS is RFC 8785 canonical JSON encoded as UTF-8; all identity components here are strings. S2 must freeze shared Rust/TypeScript vectors, including Unicode and delimiter cases. The Pi tool's durable task identity (public Tool API taskId, subject to S2's reopen proof) is the owner; a provider's tool-call label, transient wire ID, process ID or in-memory counter is not sufficient.

Each bundled adapter assigns stable semantic effect keys within that tool task, such as write-file:v1, edit-file:replace:v1, or exec:start:v1. A tool with several effects needs distinct keys: a deterministic slot list derived from immutable tool input, or a mapping durably committed through supported runtime state before any dispatch. Nested calls use their own durable child tool-task identity. A restart must recover the same key map. If an API cannot propagate this identity to ExecutionEnv, S2 is no-go; do not invent a fresh ID.

The operation ID excludes supervisor generation, connection/wire ID, timestamps, retry ordinal and argument hash. Those can change at recovery; using them would bypass deduplication. Rust authenticates the current generation before inspecting/returning a receipt. Arguments, method, capability scope and effective deadline policy have a separate immutable canonical payload hash bound to the operation ID; a changed payload is operation_conflict and requires inspection, not a new operation ID. Reconnect does not silently extend the original operation's deadline.

On Pi replay, the same tool task/effect key returns the completed Rust receipt or the existing running handle. A prepared/unknown receipt is reconciled or blocked; it never licenses blind re-execution. A legitimately new tool task has a new operation ID even for identical arguments; deduplication across different tasks is a separate domain rule.

For an edit that rereads a changed file after a crash, the adapter must recover the already-recorded mutation payload/receipt or surface a conflict rather than recompute different bytes under a fresh ID. exec status/cancel refer to the original start handle; each mutating cancellation request also has a stable command identity.

Mandatory conformance: interrupt after Rust completes an effect but before Pi stores its result, reopen under a new supervisor generation, and prove the same operation ID yields one effect and its original receipt. Also prove multiple effects in one tool do not collide, nested calls retain identity, altered arguments conflict, and uncertain receipts do not execute automatically.

The runtime cannot select arbitrary host paths, another attempt's worktree, credentials, verifier policy, GitHub operations, or an unconstrained host execution backend. Paths are resolved inside the confined environment with protection against traversal and symlink escape. A path-prefix check is insufficient.

A shell tool may execute within its authorized sandbox, but cannot bypass the host boundary by inheriting the host's filesystem, sockets, environment, or writable Git metadata. Direct argv execution is preferred for host-owned verifier operations. The bridge itself must satisfy the separate in-process tool allowlist and OS-enforced filesystem/network/process policy in [Bridge process confinement](README.md#bridge-process-confinement-and-in-process-allowlist); tool forwarding alone does not establish that boundary.

For long commands, return a durable operation handle and expose bounded stdout/stderr events plus a final receipt. A timeout/cancel does not report success. Process exit, truncation, runtime failure, and uncertain termination remain distinguishable.

## Completion and cancellation

| Fact | What it proves |
| --- | --- |
| Command admitted | Input is persisted and identified |
| Submission answered | Model produced an answer for the submission |
| Runtime idle | No relevant owned agent/task work remains under the documented scope |
| Execution quiescent | Rust confirms the supervised writer/process environment is stopped/drained |
| Verified | Rust has complete passing evidence for the inspected content |
| Approved | An authorized actor accepted the exact current review digest |
| Published | Delivery receipt reconciles to the approved Git output and PR |

Neither answered nor idle alone proves verified. Before collecting evidence, Rust stops new input/tool admission for that revision, drains agent/background work, and establishes execution quiescence. M1 does not enable detached background agents.

Pi's abort is cooperative. If it times out, Rust escalates through its supervisor and confinement backend, then confirms descendants are gone. If confirmation is unavailable, mark needs_attention and quarantine the workspace. A generation fence rejects stale messages but does not stop filesystem writes by itself.

Pause is an explicit command, not loss of connection. A disconnected browser only disconnects observation. Host shutdown/crash closes or breaks the execution channel; ongoing sandbox jobs must be stopped or quarantined according to the recorded policy.

## Recovery scenarios

| Boundary | Required action |
| --- | --- |
| Rust commits outbox before dispatch | Deliver pending command with original identity |
| Pi accepts before Rust records reply | Reconcile existing submission; do not submit twice |
| Tool effect occurs before receipt | Recover handle/outcome or stop as uncertain |
| Bridge dies while child command runs | Revoke generation, stop/drain environment, inspect workspace |
| Watch misses events | Snapshot and resubscribe; do not infer completion from silence |
| Cancel acknowledged before child exit | Continue waiting/escalating; do not declare quiescence |
| Verification finishes before Rust restarts | Read persisted evidence and revalidate content before approval |
| Runtime/package changes mid-attempt | Reject incompatible reopen; retain pinned version or explicit migration path |

## Errors, compatibility, and tests

Use stable categories including incompatible_protocol, unsupported_capability, binding_mismatch, stale_generation, command_conflict, operation_conflict, busy, idle, storage_failed, interrupted, execution_uncertain, cancellation_timeout, and resync_required. Include bounded diagnostic data without secrets. Errors explicitly identify whether work was admitted, rejected, or uncertain; a transport error cannot authorize automatic repetition of arbitrary effects.

Freeze protocol version, schemas, tool/configuration fingerprints, package lock, and runtime identity per attempt. Changing them needs compatibility tests; unsupported state is retained for inspection. The native Rust agent runtime is the long-term target and implements this public contract for new attempts. The Pi Durable bridge must not leak Pi's internal schema into that contract.

S2 owns fake-runtime/faux-provider conformance: duplicate admission, restart, lost reply, incompatible versions, wrong binding/generation, corrupt/oversized frames, backpressure, event gaps, timeout and cancellation. S3 adds real process/filesystem confinement tests. S8 composes them into acceptance. Live-provider tests are recorded separately from credential-free CI.

### S3-A2 simulation evidence

The Rust-only [simulated ExecutionEnv receipt path](s3-execution-receipts.md)
implements intent/result persistence and generation-fenced replay using FakeRuntime.
It is not wired to the Pi bridge and does not prove Pi task identity propagation,
real effects or confinement. S2 remains NO-GO.
