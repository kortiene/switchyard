# S3-A4: confined Linux ExecutionEnv effects

Scope: [#130](https://github.com/kortiene/switchyard/issues/130) under
[#97](https://github.com/kortiene/switchyard/issues/97) Track A, gated by
[#125](https://github.com/kortiene/switchyard/issues/125), milestone
[#94](https://github.com/kortiene/switchyard/issues/94). Baseline: merged #131.

This is a non-default, test-driven backend. FakeRuntime remains the product
adapter. There is no execution CLI and no Pi wiring. **S2 remains NO-GO.** This
slice does not claim full S3, Track B, seccomp, an egress allowlist, or reboot
recovery.

## Supported environment

Ubuntu 24.04 x86_64, Bubblewrap **0.9.0** / package **0.9.0-1ubuntu0.3**, Rust
1.99.1, and a **delegated cgroup v2 subtree**. A CI setup step, run as root the
same way #128 installs its AppArmor profile, nests the job under its current
cgroup, enables `cpu`, `memory`, and `pids`, and chowns a `switchyard` child of
that job cgroup to the runner. The subtree is not a sibling of the job: the
runner must be able to write the common ancestor's `cgroup.procs` or it cannot
join a leaf. The setup probe joins a leaf as the runner and requires
`populated 0` after `cgroup.kill`. The Host, Bubblewrap, and workloads never
use sudo.

A leaf OOM increments the ancestor's hierarchical `memory.events`. systemd's
default `OOMPolicy=stop` would then SIGTERM the runner service. `OOMPolicy` is
not a runtime `set-property`. When systemd is PID 1, the setup step installs a
drop-in under `/run/systemd/system` (runtime-only) and runs `daemon-reload` so
the unit's policy is `continue`. The leaf
still has `memory.oom.group` 1, so the workload is what the kernel kills. If
the policy does not read back as `continue`, the job fails before any workload
runs.

If cgroup v2, a controller, `memory.swap.max`, or writability is missing,
`execution-confinement` fails. There is no rlimit-only or unconfined fallback.
`scripts/cgroup-delegate.sh` is the root setup; `scripts/confinement-verify.sh`
then runs as the runner and records the delegation next to the #128 environment
log.

An administrator still has to make `execution-confinement` a required check.
Workflow YAML cannot do that.

## Operations and identity

Each call is one op-v1 effect (`runtime-protocol.md`) with its own scope and
payload hash. Generation, the deadline, and the argument hash are not part of
the operation id. Status and cancel are their own effects; they name the start
operation id and payload hash. They do not reuse the start request as a handle.

| Scope | Operation |
| --- | --- |
| `linux.file.read.v1` | read a workspace file |
| `linux.file.range_read.v1` | read a byte range |
| `linux.dir.list.v1` | list one directory |
| `linux.file.write.v1` | write a workspace file |
| `linux.file.edit.v1` | replace one exact occurrence |
| `linux.exec.start.v1` | start a confined argv |
| `linux.exec.status.v1` | observe a start |
| `linux.exec.cancel.v1` | kill a start's cgroup |

File bytes and each exec stream are capped at 64 KiB. An operation deadline is
at most 60 seconds. Paths are workspace-relative. `.`, `..`, and `.git`
components are rejected before a cgroup is allocated. Symlinks are not rejected
by a host prefix check: the trusted helper opens them with `openat2` under
`/workspace` using `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV`,
nonblocking opens, and a regular-file check.

The launch policy is #128's: new user, pid, net, ipc, uts, and cgroup
namespaces; cleared environment; no capabilities; no new privileges; masked
worktree `.git`; Bubblewrap status on fd 3, separate from workload stdout and
stderr. The probe package stays test-only. If the host-side `.git` entry is not
a regular file, the Host refuses the launch and does not ask Bubblewrap to
mount through it, so an agent-planted symlink cannot retarget the mask. The
in-sandbox mask is read-only, so a live workload cannot replace it either.

## Supervisor identity and quiescence

Before any workload starts the Host creates a fresh cgroup leaf and commits its
canonical path, device, inode, and boot id with the intent. It then spawns
`switchyard-execution-helper supervise`, takes a pidfd while that pid cannot be
reused, and waits for READY. READY is not entry. The workload is released only
when the Host writes GO. The supervisor places the launcher in the leaf and
does not let the launcher migrate itself. The backend-entered marker is
committed only after Bubblewrap's `--json-status-fd` record contains
`child-pid`. The supervisor stays outside the leaf, so it can still record the
result after `cgroup.kill`.

`populated 0` is read only after the launcher has been reaped. An empty leaf
while the launcher can still join is not quiescence. On host SIGKILL the
supervisor sees control-pipe EOF, reaps the launcher, then writes `cgroup.kill`,
without depending on Rust `Drop`. A deadline does the same. Restart reads the
recorded identity, kills that same leaf, and requires `cgroup.events` to say
`populated 0`. A missing, replaced, or nonempty leaf fails closed. The Host
does not relaunch.

No `child-pid` plus that matching empty leaf is `not_applied`, including when
`join`, `prlimit`, Bubblewrap, or `spawn` fails before the sandbox starts.
`completed` is the original Host-captured receipt plus the same proof. A clean
denial or edit conflict is that kind of receipt. Any other helper `ok: false`
stays `uncertain` and is not completed. Entry without a receipt stays
`uncertain` even after the cgroup is empty. Abandonment is an immutable
resolution: a later cancel cannot turn that row `completed`. A known-live
effect in this process blocks new work and is not quarantined merely because
it is running, and `dispatch_stops` kills it only when a Pause or Cancel is
already admitted. A default build that sees v7 rows keeps the block and says
the quiescence is unproven; it does not settle them.

Writes and edits do not truncate the target in place. The helper opens the
parent with the same `openat2` resolve flags, writes a temporary file in that
directory, `fsync`s it, and `renameat`s it over the target, preserving the
existing mode.

Workspace Git verification does not run while a Linux effect is unresolved.
Stop delivery still does: Pause/Cancel abort FakeRuntime, then the Host returns
`execution_blocked` without reusing the worktree. Superseded generations cannot
start, settle, or read a result. Owner loss is a new generation; its reconcile
kills recorded leaves instead of only rejecting the next call.

## #125 policy reviewed for this backend

This is the real-backend policy #125 asked to review before a Linux effect can
be resolved. It does not close #125. Superseding an abandoned decision stays
open there.

`Host::resolve_linux_execution` is an operator API. It takes the same request
as the simulated resolver: stable command id, current binding, exact operation
id and payload hash, expected revision, actor, and rationale. There is no field
for a caller-supplied receipt or quiescence claim. The actor is a local audit
label, not a remote identity.

| Decision | Required durable proof | Result |
| --- | --- | --- |
| Not applied | Intent, no entered marker, no receipt, and the recorded cgroup still matches and is `populated 0` | `not_applied`. Replay returns `execution_not_applied`. |
| Completed | The original Host receipt, same identity match, and `populated 0` | Store that receipt. Replay returns it and does not run the workload again. |
| Abandon | No conclusive proof (entered, or otherwise unknown), and the cgroup is nevertheless empty and matching | The effect stays `uncertain`. The mission stays blocked. |

Abandonment is rejected while not-applied or completed proof exists. A stale
generation, wrong binding, wrong hash, stale revision, missing leaf, replaced
inode, different boot id, or nonempty cgroup is rejected and the row is kept.
Resolution does not kill a live cgroup; cancel or restart does. A lost
acknowledgement retries the same command id and payload, refreshing only the
owner generation, and returns the one audit row. The effect change, audit, and
mission revision commit in one transaction.

Resolution does not change phase, dispatch work, reopen a terminal mission, or
create verification or publication approval. After a proven outcome the
operator uses the existing explicit recovery command. Abandonment cannot be
recovered around and cannot start another writer. The simulated
`abandon-execution` CLI does not resolve a Linux row; status says so.

`mission status` adds `linuxExecutionEffects` and `linuxExecutionResolutions`
beside the simulated arrays. `executionBlocked` is true for either journal.
`nextSteps` names the proof each decision needs and the remaining uncertainty.

## Limits

Each leaf gets `memory.max` 64 MiB, `memory.swap.max` 0, `memory.oom.group` 1,
`pids.max` 64, and `cpu.max` 50000 100000. Bubblewrap is wrapped in `prlimit`
with a 512 MiB address space, 10 CPU seconds, 1 MiB file size, 128 descriptors,
and no core file. Output past 64 KiB is truncated and the cgroup is killed.
The receipt records truncation, `pids.events` max, and memory OOM counters.
Flood, fork, and memory fixtures in the acceptance test have to hit those
counters; a failure to start is not treated as evidence.

## Ledger

Forward migration v7 adds `linux_execution_effects` and
`linux_execution_resolutions` without rewriting v1–v6 rows. Simulated
`ExecutionReceipt::from_json` still accepts only simulated receipts. Triggers
reject one op-v1 identity or one resolution command id in both journals.
Read-only inspection of older ledgers does not migrate. A writer upgrades to v7.

## Evidence and non-claims

`execution-confinement` runs the #128 probe and
`cargo test -p switchyard --test linux_execution --features linux-execution`.
The default `rust-core` job does not enable the feature and does not require a
delegated cgroup. It does prove v7 migration, cross-journal identity, and that
a default host keeps a Linux row blocked and unexplained only in the sense that
it refuses to settle it.

Not claimed: power loss, host reboot, a privileged-host adversary, seccomp,
egress allowlists, arbitrary agent execution by default, S2 GO, Track B, or
full S3. Pi stop delivery is unchanged.
