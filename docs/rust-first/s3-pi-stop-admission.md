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
null. That null generation is synthetic: the CLI reconcile does not leave a
healthy tree unverified. The other three defects fail reconcile as isolated
workspace errors and stay failed through stop delivery.

Recovery drops the Host after the stop is admitted and before delivery.
Reopen and `recover` record one abort receipt; a later `dispatch_stops`
returns 0 and keeps that receipt. A lost acknowledgement is separate.
`acknowledge` writes the abort receipt and `state='acknowledged'` in one
update, so `fail_ack` leaves the stop pending with no stored receipt. Reopen
and `recover` then record one receipt and do not replace it on the next pass.
That is re-delivery of the host acknowledgement, not proof that the bridge
skipped a second abort. `busy-once` is the only new confined fixture name;
`crash-after-abort-settled` stays off this allowlist.

A forged `ConfirmStopped` from `Operator` or `RuntimeObservation` is still
refused while the pause is pending. The supervisor confirmation is the one
that completes the stop.

## Live process-hold and an undelivered Continue

`busy-once` never starts a workspace process, so its heartbeat and descendant
checks are empty. The same suite now composes the existing `process-hold`
fixture with a further Continue. No new bridge-child mode was added, and the
fixture allowlists are unchanged.

The turn is running, the heartbeat advances, and the writer cgroup is
populated before the stop. The ledger row is still `unproven` at that point.
`prepare_workspace` still returns `execution_blocked` for every unresolved
Linux row, including a live writer, so it does not create or re-verify a
worktree over that tree. `recover_workspace` skips `verify` only when every
blocking row is an `exec_start` this generation is still supervising. It
returns the recorded workspace and does not mark it ready again. An uncertain
or unsupervised row is not bypassed. FakeRuntime has no supervised writer, so
it still returns `execution_blocked`. Work dispatch still refuses the writer.
The Continue follows the non-stop CLI order and stays pending with no receipt:
confined dispatch returns `unresolved effects block confined dispatch` before
any bridge frame. It is not a protocol `busy` refusal, and it is not
acknowledged or resent.

Pause and Cancel then follow reconcile, admit, `dispatch_stops`. The stop
keeps one abort receipt. The Continue is superseded. `ConfirmStopped` /
`ConfirmCancelled` are still only `host_supervisor`, and only after bridge
termination is `populated 0`. The writer cgroup was populated before the stop
and is `populated 0` after, including the Linux ledger quiescence. The
heartbeat is frozen. `executionBlocked` is false. `process.marker` stays
`once`. The original checkout is unchanged. A later `dispatch_stops` returns
0 and does not add a Linux receipt.

Recovery does not use in-process `drop`, because that reaps the supervised
writer. The host process is killed after the helper is stopped, while the
detached heartbeat is still populated. Reopen and `recover` then stop that
writer once. No abort receipt existed before recovery, so this is first
delivery of the acknowledgement, not re-delivery and not reuse. The Continue
is superseded rather than resumed. Pause/Cancel is recorded only after the
recovered bridge termination is proven. A second `recover` keeps the same
abort receipt and the same cancelled Linux receipt.

A writer left `entered` or `running` by that dead owner is not in the new
host's active set, and `finish_linux_identity` rejects its previous
`started_generation`. `recover` delivers the admitted stop before reconcile
can quarantine that row. Stop delivery proves the recorded leaf and retains
one cancelled receipt; the reconcile inside that stop completes it without
launching the command again. `kill_and_prove` checks device, inode, and boot
id before it writes `cgroup.kill`, so a reused path is not killed. An
uncertain row is left alone. A missing or mismatched cgroup is quarantined
and the mission goes to `needs_attention` instead of failing the stop with a
guard error. The heartbeat check waits two observed write intervals.

Confined `recover` continues into workspace reconcile and work dispatch only
when that call delivered a stop and the Linux block is gone. A killed host
with a pending Continue and no admitted stop still returns `execution_blocked`
and does not acknowledge or resend the Continue. FakeRuntime still returns
`execution_blocked` from the same check.

## Issue wording

#97 says `dispatch_stops` does not yet deliver Pi stops past a blocked or
unready workspace, and that a busy pending row must not prevent Pause/Cancel
in the same live Host before Pi is enabled in the product CLI. FakeRuntime
stop delivery past workspace readiness landed earlier. On the confined path,
both the busy-row case and delivery past those workspace defects are now
covered by the tests above. A live `process-hold` writer with an undelivered
Continue is covered separately, in the section above. That coverage is tests
only. It does not enable Pi, change the CLI default, or close #97 or #94.
