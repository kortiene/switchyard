# S2-D: storage failure and close/reopen recovery

Scope: [#96](https://github.com/kortiene/switchyard/issues/96), after merged
#139 (`cee9245182e4ee17209ce4e860ecebb9df42123b`). **S2 remains NO-GO and
FakeRuntime remains the product default.** These are credential-free tests
against the pinned Pi source candidate; they do not enable Linux effects or
prove OS quiescence.

## Corrected behavior

The previous close path checked only Pi's live tasks. An idle conversation can
still have an abort between its durable intent and settlement commits. The
regression `close refuses an abort at afterAbortRecorded even on an idle
runtime` failed on the merged baseline: close returned success and closed
storage underneath the unfinished abort.

The adapter now counts admitted session calls, including readers. Close returns
`busy` with retry disposition while any such call is unfinished. It also checks
persisted stop intent, live tasks and unsettled submissions, so a freshly
reopened session cannot close over an incomplete stop merely because its
in-memory call count is zero. Calls that arrive while those checks are still
running get `busy`, not `session_unavailable`. A throw from the check, including
`snapshot` or `inspect`, resets that fence so in-process `Runtime` state stays
consistent. The direct Runtime test can still inspect and close. Over the
bridge, `transport.ts` treats any non-`ProtocolError` from dispatch as fatal
(`fail`/`failFatal`), so a real `snapshot` or `inspect` throw still ends the
peer and the Host reopens. The `closing` reset after `harness.close` throws is
the same: it keeps the in-process Runtime consistent, and a non-protocol throw
over the bridge is still fatal. Once close is admitted it fences later calls.
A busy close leaves the session usable. A successful close is terminal for that
connection; reopening requires a new supervised process and generation.
Close remains connection-local, with no durable business-command receipt. A
lost close reply requires killing/reaping the peer and reopening the retained
session, rather than assuming completion or deleting its files.

Storage exceptions during `Conversation.abort` now keep their original cause.
Only expiration of that call's own deadline becomes `cancellation_timeout`.
The classification follows the fired deadline when no storage error was
observed, so a wrapped abort reason is still a timeout and a SQLite failure
is not.
The launcher also observes the public `Harness.closed` failure result and
terminates the transport, including when storage fails in the scheduler with
no outstanding host request. Failures retain the bounded diagnostic and cannot
emit a success acknowledgement for an uncommitted operation.

## Fault model and evidence

Trusted constructor barriers expose the public SQLite database facade to the
conformance launcher. The write-failure fixtures enable `PRAGMA query_only = ON`
at a selected boundary. SQLite itself rejects the subsequent write; no private
Pi table is read, modified or used for recovery. The flag is connection-local,
so the recovery process starts with writable storage. The close fixture wraps
the public database `close` method and throws after it succeeds, modeling a lost
close acknowledgement. It is an injected failure, not an actual filesystem
close error. Neither fixture is selectable through protocol/model input.

| Boundary | Test evidence | Required result |
| --- | --- | --- |
| Before submission admission commit | `transport.test.ts`: `storage-submit`; `pi_host_tests/storage.rs`: admission failure | No success reply or durable submission/receipt. Rust retains the pending original request; authorized replay admits it once. |
| Before abort intent commit | `storage-abort-intent`, Node and Host tests | No stop document; original work receipt survives. Pending Pause/Cancel keeps its identity and can be replayed after reopen. |
| Pi cancellation's storage write | `storage-abort-cancel`; direct cancellation cause test | Original SQLite failure, not cancellation timeout. Incomplete stop survives and the peer exits. |
| Before bridge abort settlement commit | `storage-abort-settlement`, Node and Host tests | Runtime work can be aborted while the bridge stop remains incomplete. Rust must not acknowledge it until explicit recovery settles it. |
| After Pi settles, Rust acknowledgement write fails | `rust_ack_write_failure_recovers_a_settled_pi_abort_without_repeating_it` | A real Rust SQLite trigger aborts the acknowledgement transaction. Recovery acknowledges the original Pi receipt; an armed settlement-crash fixture verifies cancellation is not reapplied. |
| Background scheduler storage failure | `a background SQLite failure terminates the peer without another host request` | Peer exits without another request or request timer. Admission already committed remains inspectable, whether its reply arrived or was lost. |
| Close's database acknowledgement fails | `storage-close` | No close success reply; nonzero peer exit and retained session. Two later generations reopen and close it successfully. |
| Close races with abort before settlement or before reply | `cancellation.test.ts`: both close/abort barriers | Close returns busy; the original abort can finish. |
| Call arrives while a close check will return busy, or that check throws | Same file: pending-close and close-check throw tests | The concurrent call is `busy` and retryable. The throw resets in-process Runtime state so a direct caller can still inspect and close. A non-protocol throw over the bridge, including a real `snapshot`/`inspect` failure or a throw from `harness.close`, is still fatal: the peer exits and the Host reopens. |
| Tool ignores its abort context | Same file: uncooperative abort | The deadline reports `cancellation_timeout` without requiring the thrown reason to be the deadline's reason object. |
| Close races with submission before transaction entry | Same file: admitted submit | Close returns busy; submission commits its original receipt and can be cancelled. |
| Close wins before later calls | Same file: admitted close | Later submit, abort, inspect and close are unavailable throughout close; the same instance cannot reopen. |
| Restart with incomplete stop | Transport storage matrix | Close is busy even without a live request. New input stays rejected; changed stop payload conflicts. |

The Node matrix reopens twice, compares original command receipts and hashes,
reaches idle after explicit abort replay, and checks that the waiting tool did
not run again. The Rust tests exercise both Pause and Cancel across all three
abort write failures, reap failed peers, retain the Pi database, preserve the
pending outbox request, and recover across two owner generations. Mission,
attempt and command counts remain unchanged. Runtime settlement leaves the
mission `pausing` or `cancelling`; injected fake stop confirmations are refused.

## Reproduction and validation

Use the committed Rust 1.99.1 and Node 22.23.4 pins. Build/install the integrity-
locked source candidate using `runtime/pi-source/README.md`, then run:

```sh
./scripts/rust-verify.sh
cd runtime/pi-source
node node_modules/typescript/lib/tsc.js --noEmit
node --experimental-strip-types --test test/*.test.ts
cd ../..
export SWITCHYARD_PI_NODE="$(command -v node)"
cargo clippy --workspace --all-targets --features pi-contract --locked -- -D warnings
cargo test --workspace --features pi-contract --locked
```

Local validation passed formatting, standard Clippy and 157 core + 9 CLI tests;
feature-enabled Clippy and 193 core + 12 bridge + 9 CLI tests; TypeScript
typechecking and all 37 Node tests. No test was ignored or skipped.

The existing `pi-bridge-probe` and `pi-source-candidate` workflows include these
new tests automatically. They remain partial evidence, not `pi-bridge-contract`.
Local Node validation reuses existing locked artifacts and dependencies and
checks their integrity/source-map agreement; CI independently rebuilds them.
Local Linux confinement execution is not part of this slice; its existing CI
workflow remains required.

## Compatibility and remaining gate

The adapter fingerprint changes. Older Pi probe ledgers remain incompatible
and retained for inspection; no migration, new session substitution, wire
version change or Rust ledger schema change is introduced. FakeRuntime ledgers
are unaffected.

The next S2 step is to compose Host scheduling, effect replay and cancellation
into the full acceptance matrix, audit every mandatory property against exact
reviewed revisions, and add the complete `pi-bridge-contract` job before a
reviewed GO decision. This matrix covers selected logical write failures and
process interleavings, not disk exhaustion, torn writes, machine power loss or
every possible storage failure.

S3 Track B still owns Pi-to-Linux wiring, bridge confinement/model-only egress,
real process-tree quiescence, and Pi stop delivery through blocked workspaces.
An abort or close acknowledgement grants no verification/publication authority.
No live provider, product Pi enablement, legacy TypeScript engine change or
issue-to-PR publication acceptance run is included.
