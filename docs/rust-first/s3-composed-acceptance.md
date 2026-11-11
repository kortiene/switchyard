# Composed S3 acceptance evidence

Refs #156, #97, #94, after B1–B6 and cgroup retention #155 (`5874eec`). This
adds one credential-free evidence-collection scenario. **Collection is not the
reviewed S3 acceptance decision.** Bounded acceptance of the collected evidence
is recorded below, accepted 2026-11-10. FakeRuntime stays the product default;
Pi is not enabled in the CLI, and product Pi wiring, S4–S8, and live acceptance
remain separate.

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

Both JSON files deliberately say `PENDING_REVIEW`. That artifact field is not
the reviewed decision and is not rewritten when the decision is recorded. The
supported runner must exercise real Git, processes, Linux namespaces/cgroups
and Host SIGKILL. An ARM64 cross-target type/lint check or a synthetic
validator fixture is not substitute OS evidence.

## Bounded acceptance — accepted 2026-11-10

**Decision: bounded S3 acceptance of the composed Host/Pi/Linux evidence,
accepted 2026-11-10 in [#97](https://github.com/kortiene/switchyard/issues/97)
and [#94](https://github.com/kortiene/switchyard/issues/94).** Reviewed merged
[#157](https://github.com/kortiene/switchyard/pull/157) at
`65a7ce7b8b91fbdc9a89308c8a52e397a17fbecc`, tree
`45e756fae02d2a45298b33df8ea159edbfecceb8` (identical to the reviewed PR tree
at head `bebc82a`). The post-merge
[execution-confinement run](https://github.com/kortiene/switchyard/actions/runs/42207467133)
(push to `main`, attempt 1,
[job 127857594458](https://github.com/kortiene/switchyard/actions/runs/42207467133/job/127857594458)
success) and all eight post-merge checks passed. The
[artifact `13498690183`](https://github.com/kortiene/switchyard/actions/runs/42207467133/artifacts/13498690183)
was downloaded independently. Its archive SHA-256
`e848305b0863167c490ecf4654db26ea8fbe04862b2484fbc5c8434ea44918ae` matches
GitHub's recorded digest. All four manifest bindings in
`s3-composed-evidence.json` match:

| File | SHA-256 |
| --- | --- |
| `s3-composed-acceptance.json` | `0f766f0e818ecb5347094ade5b10b066e6bb726013801b88eb4e526563988b90` |
| `pi-host-dispatch.log` | `490385eb5935c4d01f47ee44a0fcda34230e661dd38c10d6c3acbd0701876440` |
| `bridge-environment.txt` | `a464ece28a8e0a5b0e0ce1509d1379d762c37a8d31d14d199acce0dc34d0f4f6` |
| `cgroup-delegation.txt` | `b400be267dfbc58198b9b161c1be1e42223d44e7323ceff9750c2fdcc2e60491` |

`scripts/s3-acceptance-check.mjs` at `65a7ce7` revalidated the report against
that commit's tree and `pins.json`. The regenerated manifest is byte-identical,
and the validator's own 24 tests pass. The artifact deliberately retains
`PENDING_REVIEW`; this is the separate reviewed decision. **FakeRuntime remains
the product default.**

Scenario `host-pi-linux-egress-recovery-stop-v1` on ubuntu-24.04
`linux-x86_64`. Accepted observations, all in one mission, attempt and session
(`m-64fc964d…`, `a-06710fb4…`, `s-e62b546b…`):

- The Host was killed with SIGKILL (`signal 9`) at the committed file-receipt
  barrier in generation 2. Generation 3 recovered the original file receipt
  without repeating the write or relaunching the process, and the stale binding
  was rejected.
- Exactly one `write_file`, one `exec_start` and one `exec_status` effect ran,
  all real (`simulated: false`) on `linux-bubblewrap-0.9.0`. The start was a
  running handle with no terminal receipt and an unproven, nonempty cgroup
  (`populated 1`). It received a `cancelled` receipt only after the Pause proof
  (`populated 0`), and that receipt was unchanged through terminal Cancel.
- Pause and Cancel both had `host_supervisor` origin, proven bridge termination
  (generations 3 and 4) and a stopped heartbeat. Inert reopen stayed `paused`
  with no new effects.
- The old generation-2 bridge leaf has a retirement receipt (`removed: true`,
  `populated 0`).
- The local startup probe passed in generations 2, 3 and 4. There were 3 stub
  requests and 0 decoy hits, and every required denial probe is present.
- The original checkout's branch, HEAD, index, README, status and diff were
  unchanged.
- The Host suite reports `16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.

**Acceptance boundary.** This accepts only the collected composed S3 evidence
for that scenario. Its limits are in scope as stated: a seeded
awaiting-plan-approval fixture, faux Pi model responses, and local HTTP
startup connectivity probes. These are not live model requests, an exactly-once
network claim, a TLS route, or S4 planning. This acceptance does not authorize
product CLI or Pi enablement, provider credentials or live-provider spend, a
TLS or model client, S4–S8 implementation, or full M1 / issue-to-PR acceptance.
Frozen `adw_sdlc/`, state v1, prompts and pins are unchanged. Reboot,
power-loss or adversarial quiescence and other platforms are not claimed. #97
stays open for product Pi wiring, including the Pi-path operator-stop admission
test, under its own reviewed gate. #94's S3 slice box and all milestone
completion-evidence boxes stay unchecked.
