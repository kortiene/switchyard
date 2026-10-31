# S3: stop delivery through a blocked workspace

Scope: #129 under #97 Track A, related to #98 and milestone #94. This is a
FakeRuntime-only correction on the #128 baseline. S2 remains NO-GO; Pi stop
semantics, real process supervision and OS-enforced quiescence are unchanged.

## Delivery contract

`Host::dispatch_stops` delivers already-admitted Pause/Cancel commands in their
recorded order. It fences the owner and active attempt, reconciles execution
uncertainty, then invokes FakeRuntime's idempotent abort. Replay validates the
stored mission/attempt/session, method and payload and returns the same receipt.
The receipt is acknowledged in the command outbox. A failed acknowledgement
leaves the command pending; restart recovers the original receipt without a
second abort. This path does not inspect Git, acquire a workspace lease, verify
a workspace, apply an execution effect or complete a stop.

`dispatch_reconciled` drains these stops before the workspace readiness gate.
Other commands still require a workspace verified for the current owner, the
existing closed-phase suppression rules and no unresolved operations. A blocked
workspace leaves earlier work pending; once readiness is restored, reconciliation
retains any existing receipts and supersedes unsent work in a closed phase.
An earlier pending work row cannot prevent stop delivery.

`dispatch_pending` and `recover` retain their separate workspace reconciliation
step. An isolated uncertain/busy/transient failure permits stop delivery but
still prevents work dispatch. `recover` then polls observations and reads a
snapshot before returning the original workspace error. Host-level errors,
including a stale owner, abort before stop delivery or polling. Stop-delivery
or ledger failures are returned immediately; they cannot be treated as a
successful acknowledgement.

## Stopping is not settlement

Reconciliation quarantines prepared effects. A FakeRuntime mission with an
admitted stop remains `pausing` or `cancelling` rather than having that intent
erased on every dispatch; other nonterminal phases enter `needs_attention`.
The effects remain blocked and no uncertainty evidence is discarded.
`ConfirmStopped` and `ConfirmCancelled` retain their existing quiescence and
outbox guards. Pause from `publishing` remains forbidden. Abandonment retains
its execution block and cannot permit cancellation to complete.

The #125 resolution API still requires `needs_attention`. A simulated stop with
unresolved effects must go through that explicit attention/resolution/recovery
protocol before completion; delivery grants no recovery proof. A proven
resolution does not automatically resume a mission. Real-backend evidence and
termination remain #130/#125 work.

## CLI and inspection

`mission command --kind pause|cancel` admits the stop before draining earlier
work, then uses stop-only delivery. `--persist-only` still performs neither
workspace reconciliation nor dispatch. Command responses report the final
`commandState` and `dispatched` value from the ledger, including idempotent replay.

Exit 3 with `workspaceError` means the workspace is still blocked. A stop can
nevertheless have `dispatched: true` and `commandState: acknowledged`; inspect
its receipt in the mission's `commands`. Exit 2 remains an ambiguous failure
when it occurs after admission. Retain and reuse the same command ID and payload.

Read-only `mission status` adds `executionBlocked` and a `nextSteps` array to
each mission. It reports pending/delivered aborts, recorded workspace readiness,
unresolved effects and the distinction between delivery and quiescence. It does
not probe current lease contention, run Git or assert that the workspace is safe
to reuse.

## Validation scope

Regression cases cover Pause and Cancel across uncertain, busy, transient,
wrong-verification-generation and execution-blocked states, with lost replies
and restart. Additional cases cover a real SQLite acknowledgement failure,
stale owner and wrong session rejection, unchanged workspace/effect evidence,
#125 resolution and blocking abandonment, and stop-first CLI admission. Existing
transition tests retain publication/pause and stop-settlement policy coverage.
These tests exercise FakeRuntime and local Git/SQLite, not a model, Pi stop
integration or real execution quiescence.
