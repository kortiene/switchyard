# S2 process bridge: partial conformance

Status: **NEEDS-FULL-S2**. This work depends on the source candidate in
[PR #107](https://github.com/kortiene/switchyard/pull/107), now merged.
It is a reviewable bridge implementation and credential-free probe, not S2 GO.
The product CLI and default `Host` continue to use the S1 fake runtime.
The explicit feature-gated [S2-A Host adapter](s2-host-outbox.md) adds real outbox
reconciliation against the same faux child without granting S2 GO.

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
Rust and TypeScript read source identity, Node version expectations, protocol
limits, capabilities and deadlines from the same `pins.json` configuration.

Opening a session is inert: interrupted tools remain paused until an explicitly
replayed authorized submission or abort enables scheduling. The host must first
reconcile its outbox and stop intent. The bridge does not decide mission status.

The normal fixture registers three bundled forwarding tools: a single write, two ordered
write slots, and a nested write. The Rust callback in these tests records
**simulated effects**, not workspace writes. All effects carry the durable Pi
tool task ID and a stable slot key. No code-mode, dynamic extension, MCP, shell,
provider selection or model-credential loading is exposed by the fixture.
The explicit [S2-C cancellation fixtures](s2-cancellation.md) additionally
register a waiting tool to test cooperative and non-cooperative cancellation.

## Protocol refinements for this probe

- Frames are at most 65,536 bytes including the newline; at most 16
  requests/effects are pending. Transport timeouts terminate the peer and require
  reconciliation. Rust's 8-second response wait restarts after each effect reply
  is queued; callback execution does not consume that response window. At most
  64 effects are serviced within one call, preventing an effect flood from
  indefinitely refreshing it. Pi separately retains its 5-second effect-reply
  timeout. The trusted synchronous Rust callback must return promptly within
  that bound; it is a receipt-admission seam, not a long-running process runner,
  and this probe cannot preempt a stuck callback. A late committed receipt still
  requires reconciliation if Pi has already timed out. No reset extends an
  external operation's deadline or authorizes repeating an effect.
- JSON hashing uses the shared safe-integer subset: non-integers, integers beyond
  ±(2^53−1), and lone Unicode surrogates are rejected. S1's wider integer support
  remains unchanged; these transport inputs have a narrower validated domain.
- `runtime.watch` returns `resyncRequired` plus a fresh snapshot. It is a polling
  resync surface; it does not claim event replay or a continuous subscription.
- `runtime.abort` records stop intent before requesting cancellation. While that
  intent is pending, new input is rejected. A settled abort replays its receipt
  without aborting later work. Concurrent copies of an unfinished abort share
  one in-flight cancellation, so a delayed duplicate cannot cancel newer work.
  `runtimeSettled` describes Pi only; Rust must
  separately reconcile external effects and prove execution quiescence.
- `runtime.close` is connection-local and accepted only with no admitted
  session calls, persisted stop intent, live tasks or unsettled submissions. A lost
  close reply means the caller kills/reaps the peer and reopens with a new
  generation; it is not a durable business command or a publication result.
- External frames never enter S1's fake verification/publication observation
  parser. Authority keys are rejected recursively in results, including nested
  snapshots and arrays. Runtime answers, idle state and cancellation cannot
  authorize approval.
- Peer failure messages include at most the last 4 KiB of stderr, with terminal
  controls escaped and a bounded wait for drain completion. These diagnostics
  are untrusted text from the credential-free fixture; a live credential-bearing
  runtime still needs the S3 diagnostic/redaction policy.
- [S2-B typed effect outcomes](s2-effect-outcomes.md) distinguish completed,
  rejected, conflict and uncertain using the existing Host journal. Legacy
  callback errors conservatively remain `execution_uncertain`; a generic S1
  error does not establish whether an effect was admitted.
- [S2-C cancellation evidence](s2-cancellation.md) covers duplicate aborts,
  stop-intent/settlement crashes, cooperative and uncooperative tools,
  sustained busy retries, bounded request bursts and overflow recovery.
- [S2-D storage/close evidence](s2-storage-recovery.md) covers failed admission,
  abort and acknowledgement writes, fatal background storage errors, and
  close/abort/reopen interleavings.

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
not skip. Node tests cover source/admission compatibility, invalid JSON
values, protocol/capability/binding/generation rejection, malformed/oversized
frames, unknown replies, and cooperative abort while remote effects remain
unacknowledged, plus strict effect-error validation. Rust tests cover submission receipt recovery/conflicts,
idle steering, competing ownership/stale reopen, and a SIGKILL between a
simulated Host effect receipt and Pi's tool result. The latter reopens under a
new generation and proves three distinct effects with the original receipt
reused for the interrupted effect, including nested task identity. A separate
post-effect/pre-receipt failure remains uncertain and blocks new effects across
restart. Both now use the real Host journal rather than a private receipt table.
Attempt and
session identifiers include Unicode and delimiter characters. Corrupt storage
is retained rather than replaced. An independently hostile peer verifies Rust's
rejection of invalid envelopes, oversized frames and nested forged mission
authority. Two slow receipt callbacks exceeding one combined response window
still return their receipts, an effect flood hits the per-call bound, failed
peers retain bounded diagnostics, and handshake pin drift is rejected.

The bridge workflow pins checkout, Node setup and Rust setup Actions by commit
SHA and applies identical PR/main-push path filters. Both source and bridge
workflows verify the npm tarball against the committed SHA-512 before installing
it with lifecycle scripts disabled; they do not resolve fresh npm integrity
metadata during CI.

## Remaining S2 acceptance

The [S2-E contract gate](s2-contract-gate.md) now assembles the earlier slices
with composed stop/tool recovery, snapshot resync, registry checks and shared
identity vectors. Its always-running `pi-bridge-contract` job is proposed
acceptance evidence; S2 remains NO-GO pending review. The list below records
the handoff from this partial probe, now assessed in that matrix.

The `pi-bridge-probe` CI job is deliberately separate from the future full
`pi-bridge-contract` gate. Remaining work includes:

- extending the [S2-A Host/outbox evidence](s2-host-outbox.md) into the full
  contract, including remaining scheduling and cancellation races;
- composing the [S2-B durable effect evidence](s2-effect-outcomes.md) with the
  complete Host scheduling and cancellation contract;
- auditing the [S2-D storage and close/reopen matrix](s2-storage-recovery.md)
  against the complete contract alongside the earlier probes;
- final review of the transport refinements and a complete S2 evidence matrix.

S3 must separately prove OS confinement, process-tree termination and safe
workspace operations. No model-provider/GitHub acceptance run is performed.
Neither this probe nor its green tests permit Pi-dependent live work.
