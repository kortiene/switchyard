# Operator resolution of simulated execution effects

Scope: #125 under #97 Track A, based on merged #124. This is the first,
FakeRuntime-only implementation of the recovery policy. S2 remains NO-GO.
Real backends remain gated on backend-specific evidence, enforced writer
quiescence, confinement and process-tree supervision. This slice does not
complete those prerequisites or close #125's real-backend gate.

## Decision contract

`Host::resolve_fake_execution` is an operator API, not a runtime notification.
The request names a stable resolution command ID, current generation,
mission/attempt/session, exact operation ID and payload hash, expected mission
revision, actor and rationale. IDs are scoped to the resolution journal.
Actor is an audit label from the trusted local operator, not an authenticated
remote identity. There is no API for supplying a receipt or asserting quiescence.

| Decision | Required proof | Durable result |
| --- | --- | --- |
| Completed | Host retained the exact receipt returned by this synchronous simulated invocation. | Commit that original receipt, make completed replay read-only. |
| Not applied | Host committed intent but has not entered the backend for that invocation. | Commit `not_applied`; replay returns `execution_not_applied`. |
| Abandon | No Host-held proof is available; the operator acknowledges the unresolved outcome. | Record the decision, retain `uncertain` and the mission-wide execution block. |

Abandonment is rejected while Host-held proof exists; rejection preserves that
proof for a subsequent completed/not-applied resolution.

The evidence map is encapsulated with preparation and application in one helper, populated at the intent/apply boundaries
and lost on restart. Before entering apply, absence evidence is removed. An apply
error provides no proof of non-execution. Missing evidence is **unknown**, even
when a new FakeRuntime has an empty file map. Completed proof describes a past
simulated write; it does not prove present filesystem contents.

Simulation quiescence follows from exclusive `&mut Host`, synchronous apply and
no background execution workers. It is not transferable to a real backend.
Caller-supplied generation, rationale or `EffectProof` never substitutes for
this evidence. A stale generation, wrong binding/hash, stale revision, missing
proof, already settled effect or conflicting decision is rejected.

## Atomicity, recovery and terminal behavior

Forward ledger v6 adds `execution_resolutions` without rewriting effect rows.
The effect change, immutable audit record and mission revision increment commit
in one owner-fenced SQLite transaction. The audit binds the original request,
request hash, evidence, owner generation, timestamp and resulting revision.
One decision is allowed per effect. The original intent and starting generation
remain unchanged. Audit persistence failure rolls back all three changes.

Retry the same command ID and payload after a lost reply, refreshing only the
owner generation. The stored decision is returned before checking the now-old
expected revision or requesting vanished process-local evidence. A different
payload under that ID conflicts; another decision ID cannot overwrite it.

New decisions require `needs_attention` and the active attempt. Inspect and
reconcile first if a crash left the mission in `implementing`. Resolution never
changes phase, dispatches a command, reopens a terminal mission or manufactures
verification/review/publication approval. After a proven outcome, use the
existing explicit recovery command; remaining effects and generic S1 operations
still block it. Pause/Cancel retain their existing admission/settlement policy;
this API is not proof that their runtime work has stopped.

Terminal missions are inspectable; new decisions are rejected. Replaying an
already committed decision is read-only and remains possible with a current
binding. Abandonment is deliberately final within this slice: it cannot clear
the effect or authorize replay under a fresh effect ID. Evidence-based
superseding decisions and cross-process real-backend recovery need a separately
reviewed protocol; there is no SQL reset or generic settlement bypass.

## Operator CLI

`mission status` includes `executionEffects` plus `executionResolutions` with
request and evidence. Status is read-only, including on v1–v5 databases where
resolutions appear as an empty array. Decisions are ordered by journal insertion
(rowid), not by operator-chosen command IDs. Writer open migrates to v6.

The CLI opens a fresh Host, so it cannot claim vanished simulation proof. It
exposes only the honest operation available after restart:

```sh
switchyard mission abandon-execution --db mission.sqlite \
  --mission m-... --attempt a-... --session s-... \
  --operation-id op-v1:... --payload-hash sha256:... \
  --expected-revision 7 --command-id resolution-001 \
  --actor operator --rationale 'Outcome cannot be established; retain block'
```

Copy exact binding, operation and hash values from status; the sample IDs are
placeholders. If necessary run `mission recover` and re-inspect before choosing
the revision. Successful abandonment returns `executionBlockRetained: true`;
exit 0 means the audit committed, **not** that execution may resume. The mission
stays blocked in `needs_attention`. Cancel can be requested, but unresolved
effects still prevent terminal cancellation; lifting the block requires the
future evidence protocol in #125. Help and JSON `nextSteps` explain this. Reuse the
same arguments after ambiguous failure. There is no CLI receipt injection,
force-resume flag or real execution enabled by this command.

## Evidence

Tests cover both proven outcomes, exact receipt reuse, explicit recovery,
wrong/stale bindings and revision, conflicting IDs/payloads, missing evidence
after restart, abandonment blocking fresh IDs and recovery, other unresolved
operations, terminal immutability, rollback on audit failure and v5 upgrade.
Existing SIGKILL boundary tests remain in the execution suite. The new lost-reply
tests discard the response and reopen the Host; they do not claim power-loss
or OS process-quiescence coverage.
