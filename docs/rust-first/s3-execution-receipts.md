# S3-A2: simulated ExecutionEnv operations and durable receipts

Scope: [#123](https://github.com/kortiene/switchyard/issues/123), under
[#97](https://github.com/kortiene/switchyard/issues/97) Track A. **S2 remains
NO-GO.** This slice exercises the Rust-owned effect protocol with FakeRuntime.
It adds no CLI execution command, Pi forwarding, real file backend, subprocess,
provider call, sandbox or confinement claim.

## Contract and authorization

`Host::execute_fake_operation(&ExecutionRequest)` accepts one typed `WriteFile`
operation. It writes only to an in-memory map in FakeRuntime. The map is ephemeral;
reopening a Host does not reconstruct a simulated filesystem. The durable result
is evidence of a simulated effect, not evidence that a real file exists.

The request binds mission, attempt, session and current owner generation, plus
a stable tool-task ID and semantic effect key. The Host derives `op-v1` using the
existing [operation identity formula](runtime-protocol.md#stable-execution-identity-under-tool-replay).
Generation, arguments and deadline do not change that identity. Arguments,
method, capability scope and absolute deadline are bound by a separate canonical
payload hash; changing any of them under an existing identity conflicts.

Only `fake.workspace.write.v1` is supported. New effects require an implementing
mission, no unresolved operation, and an owned workspace verified under the
current generation. The Host checks the generation before receipt access and
again before application/settlement. An old request cannot retrieve a receipt
until its authorization generation is refreshed; a late result cannot settle
an intent started by an earlier owner.

Task IDs and effect keys are bounded to 1 KiB each, paths to 4 KiB and content to
64 KiB. Absolute paths, traversal/dot components, empty components, backslashes,
colons, NUL and `.git` components are rejected. This is simulation input
validation, **not** a symlink/race-resistant filesystem boundary. Real filesystem
operations require the later confined backend; no arbitrary callback/backend is
exposed publicly in this slice.

## Durable lifecycle

Ledger v4 adds `execution_effects` without changing historical command or generic
S1 operation receipts. Each row stores the binding, canonical request, payload
hash, starting generation and state. The older generic `settle_operation` API
cannot complete these effects. Unfinished execution effects participate in the
existing `ops_blocked` policy gates. Read-only inspection of v1–v3 remains
supported without migration.

| Boundary | Recovery behavior |
| --- | --- |
| Before intent commits | No effect has been authorized; a valid new request may proceed. |
| `prepared` commits before effect | Outcome is conservatively unknown after interruption; no blind replay. |
| Effect applied before receipt commits | Same uncertainty; an in-memory success is not durable evidence. |
| `completed` receipt commits before response | Return the original receipt without applying the effect again. |
| `uncertain` | Retain the row, block new work; no automatic reset or settlement API. |

`recover` and dispatch reconciliation quarantine unfinished intents and record
`execution_uncertain` / `needs_attention` for nonterminal missions. Replaying an
unfinished operation does the same. Terminal missions remain terminal. If a
crash splits row quarantine and mission transition, the next reconciliation
repeats the missing transition; the unfinished row blocks new work throughout.

A completed replay is receipt retrieval, not permission for new execution: it
works after pause or deadline expiry without touching the workspace or runtime.
New intent checks the absolute deadline before reservation and again before
application. Reconnect cannot extend it. This synchronous simulator does not
implement process timeouts, cancellation, running handles or resource limits.

`mission status` exposes read-only `executionEffects` entries with operation ID,
payload hash, starting generation and state. They contain no simulated file
contents. `prepared` is visible even before explicit recovery marks it uncertain.

## Evidence and next slices

The Rust gate covers replay across generations, changed arguments/scope/deadline,
distinct and nested task identities, phase/binding/path rejection, immutable
results, generation-fenced late settlement and v3 forward migration. Real child
processes are SIGKILLed after durable intent, after the simulated effect and after
the durable receipt. These prove host/ledger process-crash behavior; they do not
prove power-loss durability or recovery of real filesystem/process effects.

Next Track A work implements confined file/process backends, a supported Linux
confinement backend, bounded resources/output and process-tree supervision.
Stop-direction delivery must bypass blocked workspace dispatch before real
execution is enabled. Pi tool identity propagation and typed effect outcomes
remain #96 / Track B work, gated on S2 GO. No full S3 acceptance checkbox is
satisfied merely by passing these simulation tests.
