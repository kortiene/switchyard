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

A successor creates a new mission ID as well as a new attempt/session. Command
queries are scoped to that mission, so a predecessor's pending stop cannot enter
the successor's outbox. A publication acknowledgement may make a source mission
terminal while its Cancel is still pending; that row remains inspectable under
the source identity and does not block the successor. A different attempt inside
the same mission's stop outbox is an integrity error: retain the row and reject
it rather than silently discard its evidence.

`dispatch_pending` and `recover` retain their separate workspace reconciliation
step. An isolated uncertain/busy/transient failure permits stop delivery but
still prevents work dispatch. `recover` then polls observations and reads a
snapshot before returning the original workspace error. Host-level errors,
including a stale owner, abort before stop delivery or polling. Stop-delivery
or ledger failures are returned immediately with the workspace error attached as
context; the stop failure's category/retry semantics are preserved. The CLI uses
the same diagnostic rule. These failures are not a successful acknowledgement.

## Stopping is not settlement

Reconciliation quarantines prepared effects. A FakeRuntime mission with an
admitted stop remains `pausing` or `cancelling` rather than having that intent
erased on every dispatch; other nonterminal phases enter `needs_attention`.
The effects remain blocked and no uncertainty evidence is discarded.
`ConfirmStopped` and `ConfirmCancelled` retain their existing quiescence and
outbox guards. Pause from `publishing` remains forbidden. Abandonment retains
its execution block and cannot permit cancellation to complete.

The #125 resolution API still requires `needs_attention`. From `pausing` or
`cancelling`, the operator admits `DetectUncertainty` with reason
`execution_uncertain`, the current revision and a stable command ID. The CLI form
is:

```sh
switchyard mission command --db mission.sqlite \
  --mission m-... --attempt a-... --command-id stop-uncertainty-1 \
  --expected-revision REVISION --kind detect_uncertainty \
  --payload '{"reason":"execution_uncertain"}' --persist-only
```

`--persist-only` avoids workspace reconciliation and runtime dispatch. Inspect
the resulting revision before submitting resolution. This command records
`needs_attention` and the prior stopping phase without clearing any effect or
claiming quiescence. The library's public `Host::admit` supports the same operator
command. Tests use that path for both Pause and Cancel; the CLI test reaches
abandonment from a cancelling mission through the command above.

After a matching #125 resolution, explicitly recover the recorded target:
`paused` for a paused stop, or `cancelling` before `ConfirmCancelled` for a cancel.
Resolution does not automatically resume a mission. Proof retained by the
synchronous simulator can support a completed/not-applied library resolution;
a fresh CLI process cannot reconstruct that proof and can only record blocking
abandonment. Abandonment never authorizes recovery or completion. Real-backend
evidence and termination remain #130/#125 work.

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
The transient storage case uses a SQLite trigger on workspace verification;
it changes no permissions and runs for root and unprivileged users. Successor
outbox isolation and simultaneous workspace/stop error diagnostics have explicit
regressions. These tests exercise FakeRuntime and local Git/SQLite, not a model,
Pi stop integration or real execution quiescence.
