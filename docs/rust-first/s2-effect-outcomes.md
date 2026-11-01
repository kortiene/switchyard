# S2-B: typed effect outcomes and durable replay

Scope: [#135](https://github.com/kortiene/switchyard/issues/135), under
[#96](https://github.com/kortiene/switchyard/issues/96). **S2 remains NO-GO.**
This slice connects the credential-free Pi replay probe to the existing Host
simulated execution journal. It adds no ledger schema, product runtime selection,
Linux execution wiring, live model access, or execution-quiescence authority.

## Effect response contract

`PiBridge::call_with_effects` accepts `PiEffectOutcome` from the trusted Rust
execution owner. Pi's effect responses are limited to these four forms:

| Rust outcome | Wire response | Meaning |
| --- | --- | --- |
| `Completed(receipt)` | `result: <original receipt>` | A durable result, reused unchanged on replay |
| `Rejected` | `error: {category: execution_rejected, admission: rejected}` | This invocation admitted no new effect |
| `Conflict` | `error: {category: operation_conflict, admission: rejected}` | The same owner identity is bound to a different payload hash |
| `Uncertain` | `error: {category: execution_uncertain, admission: uncertain}` | Execution may have happened; reconcile or block |

Rejection does not assert that an old operation never ran, clear a mission block,
erase an abandoned effect, or authorize a replacement operation. Conflict cannot
be bypassed by choosing a fresh effect identity. Pi idle or a completed tool task
does not grant the Host permission to continue an uncertain mission.

The existing `PiBridge::call` callback remains compatible: `Ok` supplies a receipt
and **every** generic `Err` becomes uncertain, including guard and conflict errors.
An error category alone cannot prove the point at which execution stopped.
Typed callbacks must obtain their evidence from the execution owner.

The TypeScript transport accepts only the three exact error/category pairs in
the table, and only on responses to `execution.request`. The Pi child enqueues
no other Pi-to-Rust method on that effect queue. Null, missing fields,
contradictory admission, unknown categories, and extra error fields terminate
the transport; they cannot become success or a recoverable tool refusal.
Completed results still use the bounded canonical JSON transport. Command
admission remains governed separately by [S2-A](s2-host-outbox.md); this change
does not widen its refusal allowlist.

## Host evidence and replay

The feature-gated `Host::execute_fake_operation_outcome` uses the existing
`execute_fake_operation` path and reads the durable ledger to classify its result.
It validates the active mission, attempt, session and generation before execution
and again before reading evidence. A stale request is refused without exposing
receipts. Failed evidence reads remain uncertain.

The existing journal binds the operation ID to the Host's immutable request hash.
A different hash for that same mission, attempt, and session conflicts. An
operation ID already stored for a different mission, attempt, or session is
Rejected, not Conflict: Conflict would reveal that the identity exists
elsewhere, and the execute path reports that case as a binding mismatch. A
completed row returns its original receipt even when a later error or lost
reply obscured success, unless that operation has also been superseded.
Supersession is checked first, so a completed-then-superseded row replays as
uncertain and does not reissue the receipt. The supersede API refuses completed
and `not_applied` rows, so that pairing is not a normal transition; replay
still prefers uncertainty over treating the receipt as durable. A proven
`not_applied` row is rejected. Prepared, uncertain, abandoned or superseded
effects stay uncertain; they are never automatically re-executed. A semantic
refusal with no durable intent establishes non-admission because the simulator
cannot apply before its intent commits. That no-row rule belongs to the
simulated classifier only. The Linux backend must not reuse it: Linux can apply
work that the simulated journal has not recorded. Storage/IO errors without
evidence remain uncertain.

The test-only wire mapper validates the bundled `file.write` request and binds it
to the simulated capability and one fixed deadline. PiBridge checks the wire
payload hash; the Host separately hashes its complete execution request. Restart
changes only the generation, never task/effect identity, arguments or deadline.
No general Pi-to-ExecutionEnv mapper is enabled in the product Host.

Adapter source changes alter the existing fingerprint. Sessions from an older
fingerprint are retained and rejected, not silently migrated. Neither the Rust
ledger schema nor the upstream Pi schema changes.

## Evidence

The gate uses the pinned source build and Node version from
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

The feature-gated tests include:

- A real Pi process killed after a Host receipt commits but before its reply.
  Reopening both owners under a new generation reuses the original operation ID
  and receipt. Two ordered slots and a nested task produce exactly three effects
  across the two processes. Changed arguments conflict without changing the receipt.
- A Pi process killed after the simulated effect applies but before a receipt
  commits. Replay returns uncertain, applies zero further effects, rejects new
  tasks and retains the Host's `needs_attention` phase and execution block.
- Errors at prepared, applied and completed checkpoints, followed by Host restart;
  only the completed checkpoint can return a durable receipt. Abandoned effects,
  stale generations and unknown storage failures cannot cause automatic reapply.
  A completed row that also has a supersession replays as uncertain. An operation
  ID bound to another mission, attempt, or session is rejected, not a conflict.
  `is_transient()` is pinned for every `Error` variant, including each one
  `execute_fake_operation` can return, so a storage or IO error cannot become a
  definite rejection when no simulated row exists.
- Wire round trips for all four outcomes, conservative legacy callbacks, and
  actual child termination for malformed or contradictory effect errors.

These replace the earlier bridge probe's private simulated receipt table with
the real Host journal. Effects remain simulated. Full scheduling/cancellation
races, non-cooperative cancellation, sustained backpressure and the final S2
evidence matrix are still required before `pi-bridge-contract` can grant S2 GO.
OS confinement and real process/filesystem effects remain separate S3 work.
