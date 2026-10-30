# S2 process bridge: partial conformance

Status: **NEEDS-FULL-S2**. This work depends on the source candidate in
[PR #107](https://github.com/kortiene/switchyard/pull/107), which remains open.
It is a reviewable bridge implementation and credential-free probe, not S2 GO.
The product CLI and `Host` continue to use the S1 fake runtime.

## Implemented boundary

`runtime/pi-source/src/` implements versioned JSON Lines with hello, open,
submit, steer, inspect, snapshot resync, abort and close. A Rust client lives
behind the explicit `pi-contract` feature. It owns the canonical storage-path
lock, clears the child environment, bounds frame and channel sizes, checks
bindings and operation identities, and kills the peer on transport uncertainty.
It does not automatically replay a command after an error.

Pi uses SQLite WAL with `synchronous=FULL`, verified through its public database
API. Application documents bind source/adapter identity, mission, attempt,
session, generation and command payloads. Submission admission and its payload
record commit together through `Conversation.submit(tx, draft)`; no private Pi
tables are read or written. Reusing an ID with changed content conflicts.

The adapter fingerprint includes its source files, trusted fixture launcher,
source pins and runtime lock. A new source/configuration fingerprint cannot
silently reopen an old session. The host verifies the handshake identity.

Opening a session is inert: interrupted tools remain paused until an explicitly
replayed authorized submission or abort enables scheduling. The host must first
reconcile its outbox and stop intent. The bridge does not decide mission status.

Only three bundled forwarding tools are registered: a single write, two ordered
write slots, and a nested write. The Rust callback in these tests records
**simulated effects**, not workspace writes. All effects carry the durable Pi
tool task ID and a stable slot key. No code-mode, dynamic extension, MCP, shell,
provider selection or model-credential loading is exposed by the fixture.

## Protocol refinements for this probe

- Frames are at most 65,536 bytes including the newline; at most 16
  requests/effects are pending. Transport timeouts terminate the peer and require
  reconciliation. The Rust effect callback must return promptly; it is a receipt
  admission seam, not a place to wait for a long-running process.
- JSON hashing uses the shared safe-integer subset: non-integers, integers beyond
  ±(2^53−1), and lone Unicode surrogates are rejected. S1's wider integer support
  remains unchanged; these transport inputs have a narrower validated domain.
- `runtime.watch` returns `resyncRequired` plus a fresh snapshot. It is a polling
  resync surface; it does not claim event replay or a continuous subscription.
- `runtime.abort` records stop intent before requesting cancellation. While that
  intent is pending, new input is rejected. A settled abort replays its receipt
  without aborting later work. `runtimeSettled` describes Pi only; Rust must
  separately reconcile external effects and prove execution quiescence.
- `runtime.close` is connection-local and accepted only while Pi is idle. A lost
  close reply means the caller kills/reaps the peer and reopens with a new
  generation; it is not a durable business command or a publication result.
- External frames never enter S1's fake verification/publication observation
  parser. Runtime answers, idle state and cancellation cannot authorize approval.

## Evidence

Run after building/installing the source candidate:

```sh
cd runtime/pi-source
npm run verify
cd ../..
export SWITCHYARD_PI_NODE="$(command -v node)"
cargo test -p switchyard-core --features pi-contract --locked --test pi_bridge
```

The explicit Rust gate fails if its Node path/dependencies are missing; it does
not skip. Ten Node tests cover source/admission compatibility, invalid JSON
values, protocol/capability/binding/generation rejection, malformed/oversized
frames, unknown replies, and cooperative abort while remote effects remain
unacknowledged. Six Rust tests cover submission receipt recovery/conflicts,
idle steering, competing ownership/stale reopen, and a SIGKILL between a
simulated Rust effect receipt and Pi's tool result. The latter reopens under a
new generation and proves three distinct effects with the original receipt
reused for the interrupted effect, including nested task identity. Attempt and
session identifiers include Unicode and delimiter characters. Corrupt storage
is retained rather than replaced. An independently hostile peer verifies Rust's
rejection of invalid envelopes, oversized frames and forged mission authority.

## Remaining S2 acceptance

The `pi-bridge-probe` CI job is deliberately separate from the future full
`pi-bridge-contract` gate. Remaining work includes:

- integrating the adapter with the actual S1 mission outbox/recovery policy,
  including the precise lost-reply boundary and pending stop intents;
- durable Rust operation receipts for conflicts and uncertain-effect
  reconciliation rather than the probe's simulated receipt table;
- non-cooperative cancellation, sustained backpressure, corrupt storage and
  additional close/abort/reopen races;
- final review of the transport refinements and a complete S2 evidence matrix.

S3 must separately prove OS confinement, process-tree termination and safe
workspace operations. No model-provider/GitHub acceptance run is performed.
Neither this probe nor its green tests permit Pi-dependent live work.
