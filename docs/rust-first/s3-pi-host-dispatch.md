# S3-B5: Host-owned confined Pi dispatch

Refs #148, #97, #94. This is a credential-free conformance candidate, not live
execution or full S3 acceptance. FakeRuntime remains the product/CLI default.

`Host::open_confined_pi(db, config, linux, fixture)` is available only on Linux
with both `pi-contract` and `linux-execution`. It acquires the owner lock before
constructing the adapter, records the configured paths and faux fixture in the
attempt, and enables the Linux backend. Sessions open lazily when a mission
binding is available. The fixture is trusted Host configuration, not model input.

Explicit review point: new Linux effects now allow the confined adapter as well
as FakeRuntime. `open_pi_probe` remains denied, including `enable_linux_execution`
on a fresh probe ledger, which still returns the FakeRuntime guard. Adapter
changes on an existing ledger remain denied. The confined check is the
`Confined` variant, not the adapter name string. No product execution command
or network permission is added.

## Ordering and authority

Before workspace recovery, the Host opens the original confined session inert.
The lifecycle layer first kills and proves the previous recorded cgroup empty.
The Host reconciles execution evidence and original outbox receipts before
resuming an acknowledged command. It settles retained effect receipts or proven
non-release from the frozen request before scheduling Pi; a captured result
without a Host receipt remains uncertain. Stop delivery reconciles lost work
acknowledgements before closing the bridge. Missing initialized control state is an error,
not permission to create a fresh Pi database. Effect callbacks run against the
same Host and repeat binding/generation checks. Existing op-v1 identities,
frozen deadlines and Linux receipt replay remain authoritative.

Pi snapshots are diagnostic observations. `runtimeIdle` or command completion
cannot approve a plan, request verification, confirm a stop, or certify OS
quiescence. Public runtime-observation admission retains the Pi restrictions.
Only the private Rust supervisor path can record stop completion, after bridge
termination proof and settlement of all workspace effects. Those commands are
stored as `host_supervisor` (`CommandOrigin::HostSupervisor`), not as a Pi
`runtime_observation`. Public admission of that origin is refused, and the
adapter still rejects forged stop, cancel, verification, and quiescence facts.
The termination proof stays on the `piHost` lifecycle record; the origin is what
separates that proof from a Pi observation. An operator stop kills supervised
workspace descendants before delivering abort and stopping the bridge, even when
workspace verification is unavailable. An interrupted effect without a receipt
remains uncertain; killing it does not invent success.

Effect callbacks accept only `file.write`, `process.start`, `process.status`,
and `process.cancel`. Any other method is rejected before either Linux mapper
runs. Each effect's deadline is half of the 60-second bridge wall
(`BRIDGE_WALL` / policy `wallSeconds`, and the Linux ledger's 60-second
ceiling), so one tool can settle inside the wall instead of racing the watchdog.

Read-only `mission status` exposes `piHost` (generation, lifecycle, observation
and observation timestamp) beside `linuxExecutionEffects`. These are last-saved
observations, not a claim that a process is currently alive or dead. Inspection
takes no owner lock and does not reconcile. Schema v10 adds one diagnostic table;
historical inspection remains read-only and forward upgrades retain old rows.

## Conformance and current limits

The Host-driven suite uses a seeded awaiting-plan-approval database fixture,
then normal operator admission, dispatch and polling. It does not use FakeRuntime
or call PiBridge directly. Planning observations/S4 are outside this slice.

`execution-confinement` runs `pi_host_dispatch` and retains
`pi-host-dispatch.log`; missing prerequisites fail without an unconfined fallback.
The suite covers writes and process handles, cancellation, pause with an
unverified workspace, protected-path denial, authority rejection, adapter
mismatch, receipt replay, and Host SIGKILL at bridge-live, pre-ACK, prepared-effect
and receipt barriers, including process-start/result crash cases. Each boundary
pins its outcome: bridge-live, pre-ACK, prepared non-release, and a persisted
receipt recover; a running process and a result captured before the receipt stay
in `needs_attention`. Every boundary fails if a write is repeated or if
`process.marker` is missing or appended. Pause proves the workload's descendants
stopped: the heartbeat freezes and `executionBlocked` is false. The existing
B1–B4 suites remain enabled.

The faux provider selects write/process script responses from the durable
conversation transcript. Its process-local response queue otherwise resets on
reopen and emits a fresh tool call after the original receipt is replayed.
This narrow TypeScript test-fixture correction is the approved exception to
#148's Rust-only scope; the production implementation remains Rust. The
no-repeat assertion remains intact. Full CI evidence is required for acceptance.

The suite also covers Host-driven forbidden file/socket/child-process probes,
stale and foreign effect bindings, and hung-extension stop/recovery. After a
`cancellation_timeout`, the mission stays `Cancelling` with bridge termination
already proven until an inert reopen confirms the stop. The negative
probe uses a test-only trusted launcher that adds two read-only fixture mounts;
all production confinement arguments remain intact. Require successful CI
evidence before accepting this candidate; compilation alone is insufficient.

The 60-second bridge wall deadline, 256 MiB and 64-task limits are conformance
budgets. Production budgets belong to S4. Forwarded effects use half of that
wall. A stopped session cannot be replaced within the same owner generation; a
later Host reopen uses a new generation.
A finished mission (terminal phase, superseded attempt, or a binding that no
longer matches) gets a proven `stop()` and is removed from the live session
map. That release does not open a replacement in the same generation. Pause is
not finished: its stopped session stays so this generation cannot start another
bridge. Cgroup leaves are retained because recovery needs their identity; each reopen
adds a leaf. [#151](https://github.com/kortiene/switchyard/issues/151) tracks
bounded cleanup that preserves recovery evidence.
The compiled source-tree lookup remains confined to this fixed conformance
entry point; `config.policy` selects the trusted native policy library, but
relocatable runtime/guard installation is still deferred. These APIs are not an
installed production runtime contract.

Local validation: Rust formatting, feature-enabled Clippy and default workspace
tests. Real OS conformance requires CI's delegated cgroups and exact runtime pins.
