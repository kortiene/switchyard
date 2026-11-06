# Composed S3 acceptance evidence

Refs #156, #97, #94, after B1–B6 and cgroup retention #155 (`5874eec`). This
adds one credential-free evidence-collection scenario. **Collection is not the
reviewed S3 acceptance decision.** FakeRuntime stays the product default; Pi is
not enabled in the CLI, and S4–S8/product/live acceptance remain separate.

## What is composed

`composed::s3_composed_acceptance_recovers_stops_and_enforces_egress` runs in the
existing Host dispatch suite using the real Rust Host, SQLite outbox, owned Git
worktree, confined Pi Durable session, and Linux ExecutionEnv. It neither uses
FakeRuntime nor directly drives PiBridge/ConfinedPiSession.

The mission starts from an explicitly seeded awaiting-plan-approval fixture;
normal operator approval and Host dispatch own the implementation. This does
not demonstrate S4 planning. The trusted `s3-acceptance` faux-provider script
uses only the existing forwarding registry: one file write, one process start,
and one process-status observation. No project/model extension, credentials,
new production tool, or generalized runtime capability is added.

The same Host-owned bridge runs the existing in-bridge negative probes and a
positive read through the host-pinned Unix-socket splice to a local HTTP stub.
These are **startup connectivity probes**, repeated on reopen, not model
requests. Pi's model responses remain faux. The stub is not a production HTTP
model client or TLS route; HTTPS still fails closed. No exactly-once claim is
made for network probes or future provider requests.

## Scenario and observations

1. Record the original checkout's branch, HEAD, raw index hash, content hash,
   status and diff before Host setup. Configure one local pinned endpoint and
   a second local listener that must receive no confined requests.
2. Use normal Host approval/outbox dispatch. SIGKILL the Host at the committed
   file-receipt barrier before Pi can retain the tool result. Require the first
   write and its exact durable receipt to exist; no process has started yet.
3. Change the output to a sentinel so an accidental repeat cannot hide behind
   identical bytes. Reopen the same mission/attempt/session in a new generation,
   recover the original command/effect/receipt, and reject the old binding.
4. Execute the process/status steps through the real Linux ledger. The process
   appends a launch marker and runs a detached heartbeat descendant. Require
   exactly one start and one status effect. Pi becoming idle does not imply
   workspace quiescence: the writer must still be observed running. Start
   acceptance is a handle, not a terminal receipt: require its terminal receipt
   to be absent and a retained status observation to reference the live start.
5. Invalidate workspace verification, then admit and deliver operator Pause.
   Require host-supervisor stop authority, bridge termination proof, fresh
   Linux writer quiescence, a cancelled process receipt only after that proof,
   and a heartbeat that stays stopped. The same cancellation receipt must
   survive inert reopen and terminal cancellation.
6. Reopen paused/inert, require no new effects or file/process replay, then
   cancel explicitly through the Host and require terminal stop proof.
7. Check the old bridge leaf's exact retirement receipt, all generation-specific
   policy probes/stub tokens, no decoy traffic, and unchanged original checkout.

The launcher wrapper only adds two read-only trusted probe/spec mounts. It
retains the production namespace, seccomp, mount and resource arguments. Probe
history is diagnostic fixture output, not mission authority or a stop proof.

## Required artifacts and validation

The required `execution-confinement` job retains all existing suites and emits:

- `s3-composed-acceptance.json`: exact commit/tree and pins; mission, attempt,
  session and command identities; crash boundary; receipt hashes; real Linux
  effects; retirement and stop proofs; policy generations; checkout comparison.
- `s3-composed-evidence.json`: SHA-256 bindings for that report,
  `pi-host-dispatch.log`, `bridge-environment.txt`, and `cgroup-delegation.txt`.

`node scripts/s3-acceptance-check.mjs` requires a clean exact revision, matching
pins/environment, the successful named test in the unfiltered log, complete
same-attempt recovery/stop/policy evidence, and unchanged original checkout.
Absent, filtered, stale, malformed or failed evidence fails the job. Old
reports are removed before the suite. Validator tests run with
`node --test scripts/s3-acceptance-check.test.mjs`.

Both JSON files deliberately say `PENDING_REVIEW`. The supported runner must
exercise real Git, processes, Linux namespaces/cgroups and Host SIGKILL. An
ARM64 cross-target type/lint check or a synthetic validator fixture is not
substitute OS evidence.

## Acceptance remains an explicit decision

Review the exact source commit/tree, all required CI results, the downloaded
artifact and manifest hashes, runtime/backend identities, and every observation
above. Record the bounded acceptance decision in #97/#94 and the documentation
with those evidence references; do not infer GO from a green collection job.
After merge, use main's evidence or verify the reviewed tree matches the merge.

Even reviewed S3 acceptance would not enable product Pi execution, provide a
TLS/model client or credentials, satisfy S4–S8, or authorize live-provider spend
and GitHub publication. Those require their own implementation and approvals.
