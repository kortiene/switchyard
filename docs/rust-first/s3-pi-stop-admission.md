# Confined operator-stop admission

Refs #97, #94. This is a conformance test on the confined Host/Pi/Linux path.
FakeRuntime remains the product and CLI default. The product CLI does not
select Pi. `Host::open` is unchanged. This is not product Pi enablement,
live-provider approval, a TLS or model client, S4–S8, or full M1. #97 and #94
stay open.

## What the tests cover

`tools/confinement-probe/tests/pi_host_dispatch/stop_admission.rs` runs in the
`pi_host_dispatch` suite (`pi-linux-process` only). A default
`cargo test --workspace` does not build it. The fixture is `busy-once`, already
implemented by the credential-free bridge child. `model_endpoint` stays unset,
so egress remains denied.

The tests follow the CLI order without a shared stop-first Host method:
reconcile the workspace, admit Pause or Cancel as the operator, then
`dispatch_stops`. They do not drain the busy Continue first.

On one live Host, dispatch of that Continue fails with `bridge: busy` and the
row stays pending. Pause and Cancel are still admitted and delivered. The stop
keeps its abort receipt. The busy Continue is superseded and is not
acknowledged or resent. `ConfirmStopped` / `ConfirmCancelled` are recorded only
as `host_supervisor`, and only after bridge termination is proven
(`populated 0`). `executionBlocked` is false. The original checkout is
unchanged. `busy-once` starts no workspace process, so there is no heartbeat to
freeze; an empty Linux effect ledger is the workspace-descendant observation.
Process-hold pause and cancel tests still cover a live heartbeat.

The same delivery is repeated while workspace reconciliation is blocked:
unverified (`verified_generation` null), busy (the workspace lease is held),
uncertain, and transient (a SQLite trigger on `verified_generation`). A healthy
tree is re-verified by `recover_workspace`, so the unverified case applies that
null generation again after reconcile and asserts `dispatch_stops` leaves it
null. The other three defects fail reconcile as isolated workspace errors and
stay failed through stop delivery.

Recovery drops the Host after the stop is admitted and before delivery, and
again after a lost stop acknowledgement (`fail_ack`). Reopen and `recover`
reuse the abort receipt on the second pass (`dispatch_stops` returns 0). The
busy row is superseded, not resumed. Paused or Cancelled is reached only with
the termination proof. Linux effect receipts do not grow. `busy-once` is the
only new confined fixture name; `crash-after-abort-settled` stays off this
allowlist, so the test does not kill the bridge to detect a second abort.

A forged `ConfirmStopped` from `Operator` or `RuntimeObservation` is still
refused while the pause is pending. The supervisor confirmation is the one
that completes the stop.

## Issue wording

#97 says `dispatch_stops` does not yet deliver Pi stops past a blocked or
unready workspace, and that a busy pending row must not prevent Pause/Cancel
in the same live Host before Pi is enabled in the product CLI. FakeRuntime
stop delivery past workspace readiness landed earlier. On the confined path,
both the busy-row case and delivery past those workspace defects are now
covered by the tests above. That coverage is tests only. It does not enable Pi,
change the CLI default, or close #97 or #94.
