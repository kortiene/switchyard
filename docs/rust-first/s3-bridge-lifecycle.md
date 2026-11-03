# S3-B4: Rust-owned confined Pi sessions

Scope: #97 and #94, following #143–#146. `ConfinedPiSession` is a non-default,
credential-free Rust API behind `pi-contract` plus `linux-execution`. It uses
the fixed source-built conformance entry point. FakeRuntime remains the
product default. Product Host/Pi/Linux dispatch, provider credentials,
model-only egress and full S3 acceptance remain separate.

## Ownership and storage

The caller supplies trusted installation paths and an existing private root
outside all workspaces. Rust walks root components without following symlinks,
requires owner-only directories, and derives the session directory from the
mission/attempt/session identity. Its flock, current lifecycle record and
recovery evidence live outside the bridge-writable `state/` directory.

Control files are opened without following links, must be owned regular files
with exactly one link, and are replaced by fsync + rename + directory fsync.
Before recovery touches bridge data, the previous cgroup must be proven empty.
State entries are inspected without following links; symlinks, hardlinks,
special files and missing original storage block reopening. Evidence and
uncertain storage are retained. The API never deletes or invents a Pi database
to make recovery succeed.

Generation must strictly increase on reopen. The stored mission, attempt,
session, runtime/policy identity and cgroup identity must match. A stale or
foreign binding is refused before a protocol call or effect callback. The
session lock is retained even after stop until the object is dropped.
Trusted installation and control-root ancestors must not be changed by another
same-UID host process; the confined bridge cannot reach them. This does not
claim protection against a compromised Host or root administrator.

## Launch and recovery ordering

1. Validate the pinned Node/Bubblewrap runtime and trusted installation files.
2. Reconcile any previous generation: validate its cgroup device, inode and
   boot identity; kill it and require `populated 0`; retain the proof.
3. Inspect the original state, allocate a fresh limited cgroup, and durably
   record prepared intent.
4. Spawn the trusted launcher, which waits on an unbuffered GO barrier. Record
   its PID/start identity, move the unreaped child into the cgroup, and persist
   released intent before sending GO.
5. The launcher verifies membership and execs the fixed sandbox command. It
   never migrates itself, and requests SIGKILL on owner death. A missing GO or
   missing membership cannot execute the sandbox.
6. Rust negotiates the original protocol and requires an inert `runtime.open`.
   It does not resume pending tools as a side effect of recovery.

Only the Host can migrate the launcher. After Host death, an unplaced child
cannot enter a leaf after an empty-cgroup proof; a placed child and descendants
are covered by cgroup termination. A crash after release intent but before Pi
creates storage may conservatively block recovery on missing storage. No
power-loss, reboot recovery or cross-host failover claim is made.

## Confinement and limits

The reviewed B1 seccomp constructor and startup guard are shared from
`runtime/pi-source/confinement/`. The bridge has a fixed read-only Node/runtime
closure, private writable state, and bounded temporary storage. Workspace,
mission database, host control directory, credentials and sockets are absent.
Networking and subprocess creation remain denied; Node threads are allowed.
The guard rejects a missing/invalid native policy before Pi opens storage.

The bridge gets its own cgroup: 256 MiB memory, no swap, 64 tasks and
`cpu.max = 50000 100000`. Existing workspace execution limits are unchanged.
The reviewed rlimits retain a 10-second CPU budget, 16 MiB per-file limit and
128 descriptors; Node's heap cap is 128 MiB and temp space is 8 MiB.
A Rust watchdog kills the recorded cgroup after 60 seconds of wall time.
This deliberately bounded credential-free session is not a long-running
production agent budget. Requests retain the existing protocol timeout,
frame limits, bounded diagnostics and typed effect outcomes.

`stop()` reaps the direct child, kills the recorded leaf, requires empty
membership, and persists termination evidence. Drop attempts the same cleanup;
a failure leaves recovery blocking. Pi abort settlement is distinct from this
bridge proof, and neither proves workspace effect success or mission completion.
No lifecycle method is accepted from the public call facade.

## Validation and handoff

The required `execution-confinement` job builds `switchyard-bridge-launcher`
and exercises the API against real source-built Pi. It retains
`bridge-storage.log`, `bridge-lifecycle.log` and lifecycle JSON evidence.
Missing platform/cgroup/namespace prerequisites fail; there is no fallback.

Coverage includes storage-link attacks and exclusive locking; original receipt
replay; stale generations; enforced cgroup settings; Host SIGKILL; a hung abort
followed by inert recovery; missing original storage/replaced cgroup refusal;
no launch without GO and membership; and the real wall deadline. Existing B1
negative boundary, B2 write, B3 process and S2 contract cases remain required.
This does not claim exhaustive fault injection at every startup instruction.

Next: connect this lifecycle API to Host-owned Pi dispatch and observations,
then route coding effects through the Linux ledger. Keep provider credentials
and network egress disabled until the separate model-egress policy and composed
S3 acceptance are reviewed. Preserve FakeRuntime defaults and the Rust-native
runtime target. Do not close #97 from B4 alone.
