# S3-B3: Pi process handles and stop delivery

Scope: [#97](https://github.com/kortiene/switchyard/issues/97), following merged
[#144](https://github.com/kortiene/switchyard/pull/144). This slice adds process
forwarding to the same non-default integration seam as S3-B2 and removes workspace
readiness as a prerequisite for credential-free Pi abort delivery.

## Boundaries and wire contract

`Host::execute_pi_linux_process(binding, params, deadline_ms)` accepts only the
bundled `process.start`, `process.status`, and `process.cancel` payloads. They use
`workspace-process` capability and `attempt-bound` policy. Start accepts bounded
`argv`; status/cancel accept the original `startOperationId` and
`startPayloadHash`. Unknown fields, supplied environment/cwd/deadline/binding,
malformed identities, and mismatched hashes are refused. Rust selects the real
workspace, environment, backend, cgroup, output limits, and absolute deadline.
A shell in `argv` executes only inside the Linux workspace sandbox; the separate
Pi bridge remains unable to spawn it directly.

The same strict admission/replay code serves writes and processes. An existing
intent freezes the deadline. Scope/method/argv or handle changes under an existing
identity conflict, and an old supervisor generation cannot read or execute it.
Status and cancel must refer to an actual, non-superseded process start in the same
mission/attempt/session. No migration or second mapping ledger is introduced.

| Request | Successful Pi reply | Meaning |
| --- | --- | --- |
| `process.start` | Stable `process-handle` | Start admission acknowledged; this is not exit/success evidence |
| `process.status` | Original durable observation | Historical state, quiescence and bounded final-result summary if retained |
| `process.cancel` | Original durable termination receipt | Linux cancellation performed; consumers must inspect the target state separately |

The handle is a deterministic projection of the committed start identity and
normalized hash. It contains no PID, cgroup path, or mutable status. Rust returns
it for an entered/running effect owned by the current live supervisor, or a
completed effect with a matching durable receipt. Completion therefore cannot
change the handle. A crashed start without a final receipt is **uncertain**, even
if a previous caller received a handle. It is never relaunched on replay.

Every status request has its own task/effect identity. Replaying one returns the
same historical observation, even after cancellation. A new observation requires
a new identity. A fresh status detects an entered or released start with no
final receipt and no supervisor in this Host. That covers a restart where the
caller kept the handle and never replayed start or called recovery, and a live
Host whose finished child was dropped because `result.json` was missing or
unparseable. The start is recorded as uncertain and new work is blocked. Status
only observes cgroup quiescence; it does not kill the process or infer a
successful exit from an empty cgroup. The final-result summary includes the
retained receipt hash, disposition, exit code, and at most 2 KiB of UTF-8 per
output stream, with explicit truncation flags. The complete original receipt
stays in the Rust ledger. This bound leaves room for JSON escaping within the
existing 64 KiB bridge frame.

## Pause and Cancel

`Host::dispatch_stops` checks the current owner before any stop side effect and
now supports the credential-free Pi adapter. It opens recovered sessions inert,
validates previously acknowledged command history, inspects the original abort,
and only delivers an unsettled/pending abort. It never resumes submissions or
drains ordinary pending work. Missing/corrupt Pi storage and identity conflicts
remain blocking errors. Lost Rust acknowledgements reconcile to the original Pi
receipt, including while workspace verification is unavailable.

The existing Linux stop path kills active supervisors and proves their cgroups
empty before workspace reuse. An absent final result remains uncertain and blocks
new work. Pi's abort acknowledgement does not certify OS quiescence or complete a
mission stop; the probe continues to refuse fake observation/stop-completion
claims for Pi missions.

## Executable acceptance and limits

The required `execution-confinement` job runs the new `pi-linux-process` feature
and retains `pi-linux-process.log` alongside the B1/B2 evidence. Missing namespace
or cgroup prerequisites fail, with no fallback.

- Real Pi tasks forward start/status/cancel through both confinement boundaries.
  The process handle and historical status/cancel receipts replay unchanged;
  conflicting command payloads cannot spawn another process.
- A nonzero exit and stdout become durable status evidence.
- A fresh status after Host restart marks an entered start with no receipt
  uncertain and blocks new work. One case is a workload that has already
  exited (`proven` true). Another restarts while `process-wait` is still in
  its cgroup (`proven` false, needs attention) and leaves that process alive.
  Status never kills. On a live Host, the next status does the same when a
  finished child is dropped because `result.json` is missing or unparseable.
- Admitted Pause and Cancel stop a forwarded process with detached descendants
  while its unresolved execution blocks workspace readiness. The explicit test
  driver also aborts the real confined Pi tool under the same operator identity.
  Missing final execution evidence remains blocked instead of being called success.
- Host SIGKILL at prepared, running, result-before-receipt, receipt, and completed
  boundaries proves bridge/workload quiescence and real Pi task replay with the
  original identity/deadline. Content, inode and timestamp assertions detect a
  repeated command. A running or applied command without a receipt stays uncertain.
- Real Host/Pi tests cover Pause and Cancel across uncertain, busy, unverified and
  transiently failing workspaces, lost host acknowledgements, and stale owners.
  They keep pending work withheld and refuse mission-quiescence claims.

The process integration harness uses FakeRuntime for mission setup and explicitly
drives a separately confined PiBridge, as B2 does. The Host/Pi stop tests exercise
the credential-free Pi adapter separately. These are composed test seams, **not**
a production Host/Pi/Linux driver. New Linux effects require FakeRuntime or the explicit confined Host adapter;
the CLI default, source pins, protocol version and credential policy are unchanged.
[S3-B5 Host dispatch](s3-pi-host-dispatch.md) composes these APIs without a
separate bridge driver. Production policy, model-only egress and full S3/live
acceptance remain under #97.
