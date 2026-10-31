# S2-A: real Host outbox and Pi admission recovery

Scope: [#119](https://github.com/kortiene/switchyard/issues/119), under
[#96](https://github.com/kortiene/switchyard/issues/96). **S2 remains NO-GO.**
This slice connects the S1 Host ledger to the pinned, credential-free Pi child.
It does not enable live model calls, workspace effects, or a product runtime.

## Selection and authority

`Host::open` and the product CLI still select FakeRuntime. With `pi-contract`,
`Host::open_pi_probe(ledger, node)` explicitly selects the bundled faux launcher.
The runtime enum is internal; there is no new plugin or provider framework.
Each ledger records one adapter configuration in its existing attempt snapshots.
Opening a ledger with a different adapter, source, Node pin, or bridge fingerprint
fails before generation advancement. Existing fake ledgers keep their original
configuration and behavior. The CLI can inspect a Pi ledger read-only but cannot
silently mutate it using the fake adapter.

Pi frames never enter the fake notification parser. Fake plan/verification/
publication observations and commands claiming stop, cancellation, verifier
quiescence, or proven domain recovery are rejected on Pi hosts. `recover` below
means outbox/session reconciliation, not an operator assertion that all external
effects are safe. Abort receipts cannot complete `pausing` or `cancelling`:
trusted execution-quiescence observation remains S3/S4 work.

## Command identity and mapping

| S1 command | Pi method and payload |
| --- | --- |
| Start/revise planning, approve plan, authorize repair, request changes | `runtime.submit`, input containing the canonical S1 command |
| Steer | `runtime.steer`, operator content, idle rejection retained |
| Continue | `runtime.submit`, `type: continue` |
| Pause/cancel | `runtime.abort` |

Every mapped command also carries `hostCommand: {payload, payloadHash}`. Pi
checks that hash and the mission/attempt binding and includes the complete
parameters in its own method/parameter conflict hash. Thus two different S1
abort or continue commands cannot collapse to identical Pi payloads under a
reused identity. The Rust ledger retains its original canonical command hash;
the Pi wire hash is separate and verified against the mapped parameters.

The persisted S1 `request_id` is the bridge `commandId`; Pi's public submission
request ID is canonical `[attemptId, request_id]`. Generation authorizes the
connection but is excluded from both payload hashes and durable submission
identity. Restart changes the generation, never mission/attempt/session/request
IDs. A changed canonical payload under the same ID conflicts.

S1's integer-only JSON remains unchanged. The Pi path recursively rejects values
outside ±(2^53−1), validates bindings/identity lengths, and checks encoded frame
size before S1 command admission. The size budget reserves maximum wire sequence
and generation widths so a later restart does not invalidate a fitting command.
The command is revalidated from the ledger before dispatch. This mapping proves
admission; useful model prompts and mission progress observations belong to S4.

## Session creation and missing storage

Pi storage is deterministically derived beside the canonical ledger under
`<ledger>.pi-sessions/<sha256(sessionId)>.sqlite`. Path ownership is a local host
contract, not filesystem confinement.

1. Open Pi inertly and validate its pinned handshake and persisted session binding.
2. Commit `sessionInitialized: true` in the attempt's existing runtime config.
3. Only then inspect or dispatch outbox commands.

A crash between steps 1 and 2 is recoverable: no command could have been sent,
and the new owner reopens the same session using its next generation. After
step 2, both a missing file and an existing database without the original session
are errors. Corrupt storage and incompatible fingerprints also block recovery;
no new session is substituted. The initialization bit is operational metadata,
not an editable runtime pin; no table/schema migration is introduced.

## Recovery and scheduling

After reopening, the Host validates previously acknowledged receipts against Pi
before admitting any new work; a valid session with a missing old command is an
error. It then processes actual pending ledger rows in their original insertion order.
For each command it inspects Pi's public application document and submission
lookup before deciding whether dispatch is necessary. An existing settled receipt
must match the method and full mapped payload; it is recorded in the original
Rust command row. An incomplete abort is replayed to settle, never acknowledged
prematurely. Uncertain transport failures leave the row pending.

Reconciliation itself does not resume Pi. Pending pause/cancel intent is processed
first under the existing S1 suppression rules; unsent continuations are superseded
in closed phases. A recovered admission is acknowledged without starting a
replacement submission. After the whole outbox reconciles, an active planning or
implementation phase may explicitly replay its latest acknowledged work after
the most recent delivered stop. Pi returns the original receipt and resumes its
scheduler. Rust/Pi receipt disagreement, missing acknowledged submissions, pending
commands, and uncertain operations block this step. No continuation is invented.
Already-acknowledged work is recovered after a later restart by the same process.

Opening/inspection remain inert. A runtime receipt is not proof that an answer,
verification, or safe OS-level stop occurred. Generic effect callback failures
remain conservatively uncertain and no ExecutionEnv operations are enabled.

## Failure injection and verification

The bundled conformance launcher has two explicit crash fixtures: before
`runtime.submit` admission, and immediately after Pi's admission/document
transaction commits but before scheduler resume or the response. The latter
SIGKILL leaves the real S1 row pending. Tests restart the Host and use public Pi
inspection to compare the original receipt, then prove it finishes without a new
mission, attempt, or command. The product constructor selects neither crash mode.

Twelve Host tests cover:

- crash before admission, crash after admission before Rust acknowledgement,
  repeated restarts, and resumption of the original submission;
- pause/cancel after lost work acknowledgement, with an unsent continuation;
- changed canonical content with a valid but conflicting hash;
- missing, empty, corrupt and incompatible storage/configuration;
- stale generation, forged mission observations, unsafe integers and frame limits;
- uncertain effects blocking both fresh dispatch and recovered scheduling;
- an acknowledged Rust command absent in Pi;
- interruption after inert session open but before the Rust initialization marker.

Use the committed Rust and Node pins, with source artifacts built as described in
[the source candidate](../../runtime/pi-source/README.md):

```sh
./scripts/rust-verify.sh
cd runtime/pi-source
npm run verify
cd ../..
export SWITCHYARD_PI_NODE="$(command -v node)"
cargo clippy --workspace --all-targets --features pi-contract --locked -- -D warnings
cargo test --workspace --features pi-contract --locked
```

`pi-bridge-probe` now runs the feature-enabled workspace tests, including these
Host cases and the existing transport/tool-replay tests. It remains a partial
probe, not `pi-bridge-contract`. Missing dependencies fail rather than skip.

Remaining S2 work includes durable typed effect rejection/conflict/uncertainty,
non-cooperative cancellation, sustained backpressure and race cases, and the
complete acceptance matrix. S3 still owns execution confinement, process trees,
and real effect receipts. No live-provider or GitHub publication was performed.
