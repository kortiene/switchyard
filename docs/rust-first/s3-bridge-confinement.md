# S3-B1: separately confined credential-free bridge

Scope: [#97](https://github.com/kortiene/switchyard/issues/97), after the explicit
[S2 acceptance](s2-contract-gate.md) of merged #141. This is a test-only policy
and real PiBridge integration harness. The product Host/CLI retains FakeRuntime;
there is no new product launch configuration, provider credential or live model.

## Boundary

The trusted Rust test harness creates a fresh private state directory and a
fixed launcher, then uses the existing `PiBridge` owner lock, JSONL transport,
handshake, deadlines and kill/reap behavior. The launcher preserves stdio and
passes the opaque binding/allowlisted fixture arguments without evaluating them.
It ignores no required argument and cannot be configured by a protocol frame.
No unconfined fallback exists. The test crate's optional `bridge-confinement`
feature depends on core; product core/CLI do not depend on the test crate.

| Resource | Policy |
| --- | --- |
| Runtime | Exact Node 22.23.4 executable, selected Ubuntu loader libraries, pinned Pi source/install, adapter files and trusted fixture are read-only |
| Writable storage | Fresh private session directory and an 8 MiB tmpfs; no mission parent, repository/worktree, home, forge configuration or host sockets are mounted |
| Ownership | Rust opens the SQLite sibling owner lock before launch; that exact lock inode is masked read-only inside the otherwise writable session directory |
| Environment | Cleared by Rust and Bubblewrap; only LANG and the fixed LD_PRELOAD policy path are set by the launcher |
| Namespaces | Separate mount, PID, network, user, IPC, UTS and cgroup namespaces; further user namespaces disabled, all capabilities dropped, new session and die-with-parent |
| Processes | Native seccomp denies fork/vfork/execve/execveat; clone permits only threads with VM/SIGHAND/THREAD and no namespace flags; clone3 returns ENOSYS for glibc's pthread fallback |
| Network | No network namespace access plus seccomp socket/connect/bind/listen/accept/send/receive denial, including Unix sockets and loopback listeners; io_uring denied |
| Supervision | Bubblewrap PID status is outside the mounted state; original PID/start identities and namespace members must be quiescent before reopen in the tests |
| Limits | CPU 10 seconds, per-file size 16 MiB, 128 open descriptors, Node old-space 128 MiB, bounded existing protocol/diagnostics and response deadlines |

The C preload constructor installs `no_new_privs` and seccomp before Node starts
threads or imports JavaScript. This depends on the reviewed dynamically linked
Linux x86_64 Node runtime; other architectures are refused. Node's fixed read-only
`--require` guard requires the constructor success marker plus the kernel's
NoNewPrivs/Seccomp state before loading the adapter. Missing or corrupt libraries
therefore cannot silently degrade into an unconfined bridge, even though the ELF
loader normally ignores a failed LD_PRELOAD. The marker is internal launch state,
not a credential, and is retained for worker guards. No caller environment can
supply it through the cleared environment. Once installed, the kernel filter
cannot be removed by JavaScript and is inherited by threads.

This is a focused syscall deny policy for a pinned, trusted runtime, not a general
hostile-native-code syscall allowlist. The in-process tool allowlist from S2 still
applies. The negative fixture alone uses Worker eval to test native enforcement;
it is not registered as a model tool or a project extension. Dynamic libraries,
node_modules and launch paths are trusted harness inputs, not repository text.
The private filesystem/limits are not a total disk quota, hard RSS cap, thread
quota, reboot recovery or adversarial kernel proof. Bridge cgroup resource limits
and durable production supervision remain later work.

## Executable evidence

`scripts/bridge-confinement-verify.sh` runs inside the existing required
`execution-confinement` job on Ubuntu 24.04 x86_64 with Bubblewrap
`0.9.0-1ubuntu0.3`, Node `22.23.4`, and Rust `1.99.1`. The workflow rebuilds the
integrity-locked Pi source before testing. Missing dependencies or namespace
support fail the job. There are no skips, path filters or optional success paths.

- A native self-test attempts actual fork, vfork, process clone, clone3, execve,
  execveat, socket, io_uring and unshare calls. A pthread positive control works
  and independently gets process/network denials.
- `bridge_boundary_denials_preserve_host_authority_and_private_sqlite_recovers`
  invokes actual reads/writes against host-existing protected sentinels and a
  symlink escape; runtime and owner-lock write/unlink/rename attempts; the
  existing `/node` executable; host TCP/Unix positive-control endpoints and a
  loopback listener. A Node worker runs and inherits subprocess denial. Private
  SQLite/temp writes succeed, a real Pi submission is answered, Rust kills the
  supervisor, quiescence is observed, and generation 2 recovers the same receipt.
- `confined_uncooperative_abort_reaps_namespace_before_inert_reopen` enters the
  actual deliberately hung Pi tool, observes the live namespace, waits for the
  bounded abort failure, requires Rust reaping and namespace quiescence, then
  recovers the original abort intent without rescheduling the armed tool.
- `invalid_or_missing_policy_never_starts_an_unconfined_bridge` proves a missing
  policy is refused and corrupt/empty preload libraries terminate at the guard
  without opening the Pi database.

The job retains native/Rust logs, boundary and stop reports, exact Git commit/tree,
source provenance, Node/compiler versions and policy/runtime/library hashes in
`execution-confinement-evidence`. The existing S3-A tests and full S2 contract
gate continue unchanged. A required workflow result is evidence for its exact
tested revision, not automatic full S3 acceptance.

Local validation: C compilation with warnings denied, raw syscall/pthread tests,
Node main/worker subprocess and TCP/Unix/listener denial controls, Rust formatting,
Clippy and test compilation passed. This workspace cannot create Bubblewrap
namespaces; complete boundary/recovery acceptance must come from the Ubuntu CI
run, not from those local controls.

## Handoff

Review this policy before extracting a product launch API. S3 still owns Pi-to-
Linux effects/handles and replay, operator stops past blocked workspaces, shared
revocation/quiescence and evidence-backed reuse. Model-only egress needs a separate
OS/proxy design and positive/negative endpoint tests; this slice denies all
networking and contains no credential broker. The native Rust agent runtime
remains the long-term target; confinement belongs to Rust regardless of adapter.

The harness bind-mounts its fresh session directory read-write, so the bridge
can plant symlinks in `state/`. That is acceptable for this test-only harness.
The product launch API must open bridge-writable paths with `O_NOFOLLOW` /
`openat2` `RESOLVE_BENEATH`, or only through the owner-locked database.

Policy references: [kernel seccomp semantics](https://docs.kernel.org/userspace-api/seccomp_filter.html)
and [Bubblewrap's security model](https://github.com/containers/bubblewrap/blob/v0.9.0/README.md).
Seccomp alone is not the filesystem boundary; the mount policy is essential.
