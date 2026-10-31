# S3-A3: Linux confinement feasibility probe

Scope: [#127](https://github.com/kortiene/switchyard/issues/127) under #97 Track A.
Baseline: merged #126 (`af46d28`). This is an isolated deterministic test harness,
not an ExecutionEnv implementation. Product Host/CLI still uses FakeRuntime.
S2 remains NO-GO; full S3 and #125's real-backend recovery gate remain open.

## Decision and supported environment

Use Bubblewrap **0.9.0**, Ubuntu package **0.9.0-1ubuntu0.3**, on **Ubuntu 24.04
Linux x86_64**, with the repository's Rust 1.99.1 toolchain. The distro security
revision is pinned, not just `bwrap --version`. Installation uses the signed
Ubuntu archive. The CI artifact records kernel, runner image/version, packages,
executable hashes, userns settings, Rust version and tested Git revision.
Hosted runner images/kernels still evolve: this is a recorded tested environment,
not a fully reproducible VM image. Package disappearance/version drift fails
until the pin is deliberately reviewed.

Refresh policy: use a dedicated reviewed PR to bump the exact package revision
in both the workflow and verification script, and this document. Review Ubuntu's
package/security changelog, rerun every confinement scenario on the declared
runner, and retain the new environment evidence before merging. If the upstream
Bubblewrap version changes, also review its policy semantics and update the
runtime version assertion. Do not silently select latest or bypass the gate when
an archive revision disappears. Installation is noninteractive and explicitly
allows the reviewed pin to replace a newer runner package.

Bubblewrap was chosen for a small namespace/mount policy without introducing a
container daemon or binding the Host to Pi. This probe does not establish a
container-image distribution system, VM-grade kernel isolation or a production
resource controller. Those require separate decisions before real execution.

CI installs a root-owned, non-setuid copy at
`/usr/local/libexec/switchyard-bwrap`, byte-compares it with `/usr/bin/bwrap`, and
loads the committed AppArmor userns permission for **that path only**. The
profile permits namespace setup; it is not the confinement policy. Global
AppArmor userns restrictions are not disabled. The unprivileged test process
invokes the copy. It never uses sudo to launch the fixture. Nested user
namespaces are explicitly disabled and tested from inside the sandbox.

## Fixed test policy

| Boundary | Configuration and evidence |
| --- | --- |
| Filesystem | New mount root; trusted `/usr` read-only, merged-/usr links, fresh `/proc` and `/dev`, 8 MiB private `/tmp`, one writable temporary Git worktree. Root is read-only after setup. |
| Git authority | Mask the worktree's `.git` file with a read-only marker. Original repository and Git administrative directory are not mounted. Check original HEAD, index, clean status and worktree pointer after each scenario. |
| Control/secrets | Host ledger, bridge storage, home/SSH/forge credential sentinels and Unix socket exist outside the mount set. Positive host-side reads/connections precede denied sandbox operations. No real credentials are used. |
| Network | Mandatory separate network namespace; connection to an existing host-loopback listener fails. No host network fallback. This fixture has no provider-network exception. |
| Processes | Mandatory user/PID/IPC/UTS/cgroup namespaces; drop all capabilities; verify no-new-privileges, start a new session, disable nested userns. Keep Bubblewrap's PID-1 reaper and `--die-with-parent`. |
| Environment/FDs | Clear environment twice; allow only PATH/LANG plus Bubblewrap's PWD. Null stdin, separate bounded stdout/stderr files, Rust close-on-exec descriptors; no host terminal or deliberate inherited credential FDs. |
| Limits | Hard per-process 512 MiB address space, 5 CPU seconds, 1 MiB file size, 64 FDs; 10-second harness deadlines. An output-flood fixture must actually reach the limit, not merely fail to start. |

Path tests cover absolute protected paths, missing files under protected parents,
relative symlink traversal and a `/proc/1/root` symlink escape. Positive writes
inside `/workspace` and `/tmp` must succeed first. Read-only bind protection is
tested against write, unlink and rename of the worktree Git marker.

This is a trusted, quiescent fixture tree at mount construction time. It does
**not** prove safe import of an actively mutated hostile directory, arbitrary
hardlinks, racing mount-source substitution or an entire compiler/toolchain
allowlist. `/usr` is the trusted runner runtime surface, not a minimal image.
No syscall allowlist/seccomp policy is claimed. Per-process rlimits do not impose
aggregate CPU/memory, process-count, disk quota or a fork-bomb policy. These are
explicit production blockers, not optional protections silently omitted from a
production backend.

## Stop and supervisor-loss proof

The Rust fixture starts a child through `setsid`, which starts a grandchild.
Each writes a heartbeat. The test obtains Bubblewrap's reported child PID,
reads its namespace from host `/proc`, enumerates that namespace, and maps all
three named fixture roles using NSpid. Each observed identity includes PID and
start time to distinguish PID reuse. Namespace identity must differ from the
observer, and each writer must be observed before termination is tested.

Test explicit supervisor SIGKILL, deadline-triggered supervisor termination, and
SIGKILL of the separate Rust supervisor **without running Drop**. Bubblewrap's
parent-death behavior and PID namespace teardown must kill the descendants.
After stopping, scan the namespace and re-check every observed identity until no
live process remains; reject workspace reuse on timeout or observation failure.
Exited zombies count as unable to write, not as proof of immediate host-side
reaping. Heartbeats must remain unchanged during a further observation window.
Only then launch a replacement fixture in the same worktree and re-check the
protected state.

This observer and status channel are for deterministic fixtures. They are not a
durable, adversarial process handle or operator quiescence certificate. The
Bubblewrap status uses a dedicated fd 3 file channel. A fixed trusted shell
duplicates the preopened file descriptor, disconnects fixture stdout to
/dev/null, and execs prlimit; Bubblewrap closes the status descriptor before
fixture exec. Fixture stderr remains the separately bounded diagnostic stream.
This separation does not make the procfs observer an adversarial certificate. The Rust supervisor is a test
process, not the product Host. Host machine reboot, power failure, PID-namespace
nesting attacks and arbitrary malicious workloads are outside this proof.

## Running and interpreting the gate

The probe is a separate workspace package under `tools/confinement-probe`.
Neither product crate depends on it. Its binary and integration tests require
`linux-confinement`; normal Rust tests do not pretend to exercise confinement.

After installing the pinned package and the scoped CI profile:

```sh
SWITCHYARD_BWRAP=/usr/local/libexec/switchyard-bwrap \
SWITCHYARD_CONFINEMENT_EVIDENCE="$PWD/confinement-evidence" \
./scripts/confinement-verify.sh
```

`execution-confinement` runs unconditionally on pull requests and main pushes.
No continue-on-error, ignored test, optional namespace or unconfined fallback is
used. Missing OS/kernel permissions, tools, package revision or observable
processes make the job fail. The GitHub artifact retains the environment record,
test log and fixture diagnostics even on test failure.

**An administrator must add `execution-confinement` to required checks.** Workflow
YAML cannot configure branch protection. A green probe is partial S3 evidence,
not full bridge/agent confinement or S2 GO. The feature remains outside product
execution until #125 evidence/recovery, aggregate resource policy, adversarial
filesystem handling, stop-direction dispatch and the remaining #97 criteria
are reviewed and implemented.

## Sources and rationale

- [Bubblewrap policy model](https://github.com/containers/bubblewrap/blob/v0.9.0/README.md)
  and [options](https://github.com/containers/bubblewrap/blob/v0.9.0/bwrap.xml):
  mount/namespace isolation, PID-1 reaper, parent-death and new-session options.
- [Linux PID namespace semantics](https://man7.org/linux/man-pages/man7/pid_namespaces.7.html):
  namespace-init death causes kernel termination of namespace members.
- [Ubuntu 24.04 release notes](https://documentation.ubuntu.com/release-notes/24.04/):
  per-application AppArmor permissions for restricted unprivileged userns.
- [Ubuntu package baseline](https://packages.ubuntu.com/noble-updates/bubblewrap):
  distro security revision of Bubblewrap 0.9.0.
