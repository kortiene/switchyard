# S2-C: cancellation recovery and bounded request pressure

Scope: [#96](https://github.com/kortiene/switchyard/issues/96), following
#136 and #138. **S2 remains NO-GO.** FakeRuntime remains the product default.
These are credential-free probes against the pinned Pi source candidate, not
live execution or OS quiescence evidence.

## Duplicate abort race

The durable command document deduplicated completed aborts, but two concurrent
copies of an unfinished command could both call `Conversation.abort`. If one
copy settled and reopened admission while the other was delayed, the delayed
copy could cancel a newer submission. The deterministic regression test first
failed with that newer submission marked `aborted`.

The bridge now shares one in-flight promise per abort command ID and payload
hash. A changed payload still conflicts. The promise is removed after success
or failure; the existing durable document remains the recovery authority.
An incomplete command still requires explicit replay after restart. A settled
command returns its original receipt without applying another cancellation.
No ledger schema or wire protocol change is required.

The adapter fingerprint changes because the implementation and trusted
launcher change. Existing Pi probe ledgers with the old fingerprint remain
incompatible and fail closed; no automatic migration or replacement session
is introduced. FakeRuntime ledgers are unaffected.

## Evidence

| Boundary | Executable evidence | Required result |
| --- | --- | --- |
| Delayed duplicate abort races with later work | `test/cancellation.test.ts`: delayed duplicate test | Original receipt is shared and `afterAbortRecorded` runs once; later work stays active. |
| Admission while a stop is incomplete | Same file: pending abort test | Input, steering and a different stop are rejected without receipts; changed content under the same ID conflicts. |
| SIGKILL after stop intent, before Pi cancellation | `pi_host_tests/cancellation.rs`: `abort_crashes_recover_original_stop_without_claiming_mission_quiescence` | Rust keeps the stop pending; a new generation inspects the incomplete document and replays the original command. |
| SIGKILL after settlement, before Rust acknowledgement | Same test, second crash fixture | Recovery acknowledges the stored receipt without repeating the abort. Both Pause and Cancel survive two restarts with the fixtures still armed. |
| Repeated real Pi `ConversationBusy` | `sustained_real_busy_continuation_cannot_starve_an_admitted_stop` | Eight retries keep the Continue pending; an admitted Pause/Cancel suppresses it, settles the runtime stop, and survives restart. |
| Tool ignores its abort context | `tests/pi_bridge.rs`: `uncooperative_abort_times_out_reaps_child_and_reopens_with_unsettled_intent` | The five-second abort deadline returns uncertainty; Rust kills/reaps the child, retains storage and reopens inertly. Explicit replay settles the original intent. Inspection reaches `runtimeIdle` with `stopping == false` before a second check that the tool was not rerun. |
| Bounded sustained transport pressure | `test/transport.test.ts`: bounded bursts test | Eight rounds each submit fresh work and abort it with its own command ID under 16 concurrent requests (128 total). Every round is a real cancellation; receipts and inspection stay responsive. |
| More than 16 unfinished requests | Same file: overflow test | The peer exits without acknowledging the pending stop. Reopen finds the original incomplete intent and explicit replay settles it. |

The conformance launcher supplies barriers after the durable stop-intent commit
and after the settlement commit. Neither frame parameters nor model input can
select these barriers. Two explicit test fixtures register a waiting tool: one
obeys its context and one deliberately ignores it. The normal bundled
forwarders and product runtime selection are unchanged.

An abort receipt only acknowledges Pi settlement. The Host tests explicitly
keep the mission in `pausing` or `cancelling`; fake `ConfirmStopped` and
`ConfirmCancelled` observations remain forbidden on Pi hosts. The synchronous
Rust receipt callback still cannot be preempted by this probe.

## Validation

Use Rust 1.99.1 and Node 22.23.4. With the integrity-locked source artifacts
and dependencies installed as described in `runtime/pi-source/README.md`:

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

The existing `pi-bridge-probe` job runs these Node and feature-enabled Rust
tests and independently rebuilds the source candidate. It remains a partial
probe; no job is renamed to `pi-bridge-contract` here.

Local validation passed the default Rust gate (157 core + 9 CLI tests), the
feature-enabled Clippy gate, all feature-enabled tests (190 core, 12 bridge
and 9 CLI), TypeScript typechecking and all 23 Node tests. The Node run reused
the existing locked artifacts/dependencies; the candidate integrity and
source-map checks passed. A fresh source rebuild remains part of CI.
The existing child-liveness test checks the child with
`rustix::process::test_kill_process` (signal 0, in-process) instead of `/proc`
or an external `kill` binary. `/proc` can belong to an outer container PID
namespace, and slim images may not ship `kill`. An unreaped zombie still
counts as present until this parent waits, matching the old `/proc` existence
check. The test still verifies that the child exists before termination and
is gone after; no test is skipped or weakened to accommodate the environment.

## Remaining gate work

- Compose Host scheduling, typed effect recovery and cancellation in the full
  S2 acceptance matrix; these tests do not enable Linux effects through Pi.
- Review the remaining storage-failure and close/abort/reopen interleavings,
  including behavior when storage cannot commit settlement.
- Record exact reviewed revisions and the complete mandatory evidence before
  adding `pi-bridge-contract` and changing #96 to GO.
- S3 Track B retains bridge confinement, model-only egress, real process-tree
  quiescence and stop delivery past blocked/unready workspaces. Runtime abort
  success alone satisfies none of those requirements.

No paid provider, live GitHub acceptance run, legacy TypeScript engine change,
or product Pi enablement is included.
