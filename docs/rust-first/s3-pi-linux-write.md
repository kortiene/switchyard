# S3-B2: recoverable Pi-to-Linux file writes

Scope: [#97](https://github.com/kortiene/switchyard/issues/97), after the
credential-free S2 GO and merged #143's separate bridge confinement.

`Host::execute_pi_linux_write(binding, params, deadline_ms)` connects the bundled
Pi file-write forwarder to the existing Rust Linux ExecutionEnv. It requires
`pi-contract` and `linux-execution` on Linux. The integration harness runs real
Pi Durable behind the separate bridge sandbox and performs real writes in the
owned worktree through the separately confined Linux helper.

The product Host/CLI still defaults to FakeRuntime. Mission setup in this harness
uses FakeRuntime; Pi dispatch is explicitly driven through `PiBridge`. This is
not product mission/Pi runtime wiring, a production bridge launcher, a live
provider run, or complete S3 acceptance. No model or GitHub credentials are used.

## Mapping and authority

The caller supplies the binding of its authenticated, separately confined bridge
and a host-selected absolute deadline. The API does not accept a binding, backend,
executable, workspace root, or deadline from Pi parameters. A new effect requires
the enabled Linux backend, the current owner/generation, an implementing mission,
an unblocked execution ledger, a ready owned workspace and the existing Linux
authorization checks. Merely calling this API cannot enable the backend.

A new effect is rejected unless the Host runtime is FakeRuntime; lifting that
gate is S3 Track B product Pi dispatch wiring under
[#97](https://github.com/kortiene/switchyard/issues/97).

Only the exact bundled `file.write` payload is accepted: a workspace-relative
path, string content, `workspace-write` capability and `attempt-bound` deadline
policy. Unknown fields, other methods/policies, forged operation IDs/hashes,
traversal, `.git` components and oversized writes are refused before admission.
The backend enforces the filesystem boundary, including symlink escapes.

Pi's attempt/session/tool-task/effect-slot tuple maps to the same `op-v1` identity
in the Linux ledger. The mapper recomputes the wire payload hash and the normalized
Linux payload hash; the receipt retains the latter. These hashes intentionally
cover different schemas. Exact-field validation makes the file-write mapping
unambiguous; scope and policy values are fixed, not discarded extensions.

The first Linux intent already persists the normalized request and absolute
deadline in `request_json`. Replay loads that deadline before checking the
normalized hash. A replacement supervisor cannot extend it by passing a new
deadline. No new ledger or schema migration is needed. Completed receipts can be
returned after deadline expiry, while a new write still obeys the existing
60-second maximum. Simulated and Linux journal identities remain disjoint.

## Outcomes and recovery

| Observation | Pi outcome |
| --- | --- |
| Invalid binding, unsupported payload, or semantic refusal before backend entry | `execution_rejected` |
| Same owner/operation with different normalized content, or simulated journal collision | `operation_conflict` |
| Completed Linux row with its matching real-backend receipt, including a definitive denied result | Original receipt with its result/disposition unchanged |
| Durable Linux proof of non-application | `execution_rejected`; original identity is never relaunched |
| Released/applied work without a durable result, unreadable evidence, or unknown backend error | `execution_uncertain` |

After backend entry, a generic error or missing row never implies non-admission.
The mapper reads the Linux journal, not the simulated journal, and repeats owner
checks before reporting an outcome. Existing backend replay can settle a stored
receipt after proving cgroup quiescence. An applied effect without a durable
receipt remains uncertain and blocks new work. Operator supersession does not
turn an unknown outcome into a completed receipt.

`Completed` describes settlement, not successful mutation. In particular, the
Linux helper can return a durable `denied` receipt for an escaping symlink.
Consumers must inspect the receipt's disposition/result; replay retains that
denial even if the path later becomes writable.

## Executable acceptance

The existing required `execution-confinement` job runs
`scripts/bridge-confinement-verify.sh` with the new `pi-linux-write` test feature.
The script compiles the real helper, checks combined-feature Clippy, runs mapper
unit tests and retains `pi-linux-mapper.log` and `pi-linux-write.log` alongside
the existing exact revision/runtime/policy evidence. Missing namespace support,
backend pins or cgroup delegation fails; there are no optional success paths.

- Three real writes from multi-effect and nested Pi tools get distinct Linux
  operation identities and receipts. The original checkout, branch and index
  remain unchanged.
- An actual forwarded write through an escaping symlink cannot modify protected
  host content. Its durable denied receipt is retained on replay, including after
  the symlink is replaced with a writable regular file.
- A Rust child host is killed at four actual Linux boundaries: prepared before
  release; applied before receipt persistence; receipt persisted before completion;
  and completed before the reply reaches Pi. The bridge namespace must be empty
  before it is reopened with a new supervisor generation.
- Real Pi replay preserves the original task, slot and wire payload. It returns
  the original receipt or the documented rejection/uncertainty. A replacement
  marker, inode and modification timestamp prove no repeat write; the original
  stored request proves the deadline was not extended. Stale-generation calls
  cannot read receipts or apply effects.
- Unit tests cover strict payload/path/identity validation, cross-journal
  collisions, foreign bindings, deadline freezing, unreadable/corrupt evidence
  and malformed receipts. These are classifier tests, not OS confinement proof.

The `result` barrier is an additional test-only Linux checkpoint after the helper
has returned an applied result and before Rust commits its receipt. Like the
existing checkpoints it is inactive unless `LinuxExecutionConfig::crash_at` is
explicitly configured by the test host.

## Remaining work

Process forwarding, Pi Pause/Cancel past blocked workspaces, production bridge
storage/supervision/resource controls, model-only egress and full composed S3
acceptance remain separate slices. The test launcher's existing storage/path
limitations still apply; see [S3-B1](s3-bridge-confinement.md). Native Rust remains
the long-term runtime target behind the same Rust-owned execution boundary.
