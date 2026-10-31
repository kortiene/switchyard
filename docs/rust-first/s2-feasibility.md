# S2 feasibility: published Pi Durable 1.4.0

Decision: **NO-GO for the currently published 1.4.0 package against the reviewed
S2 baseline** (2026-10-29). Tracking: [#96](https://github.com/kortiene/switchyard/issues/96),
parent [#94](https://github.com/kortiene/switchyard/issues/94).

## Current status — 2026-10-30

[#106](https://github.com/kortiene/switchyard/pull/106) merged this published-package
NO-GO. [#107](https://github.com/kortiene/switchyard/pull/107) merged the pinned
source candidate in `runtime/pi-source/`; its verdict is `NEEDS-FULL-S2`.
[#108](https://github.com/kortiene/switchyard/pull/108) is open and ready for review with
partial bridge probes. Its diagnostic finding is corrected and re-reviewed.
It is not merged, and it is not S2 acceptance or live
execution. [#96](https://github.com/kortiene/switchyard/issues/96) remains
**NO-GO** until the complete gate in that issue passes. This section is the
status to update when that PR moves. This note does not execute the gate and
does not claim GO or live execution. The 2026-10-29 evidence below stays as
recorded.

S1 is complete in [#105](https://github.com/kortiene/switchyard/pull/105).
S2 is not complete. Rust remains the sole long-term engine, including the native
agent runtime accepted in #104. This finding neither rejects Pi in general nor
changes the accepted architecture. Pi-dependent live work in S3-S8 remains blocked.

## Package and source evidence

The planning baseline is source commit
[`e65b3b88c08412bb8ffbd21fb5dc5756064a6e5a`](https://github.com/earendil-works/pi/tree/e65b3b88c08412bb8ffbd21fb5dc5756064a6e5a).
Its manifest says `1.4.0`. The published npm package exists, but npm's returned
[SLSA provenance](https://registry.npmjs.org/-/npm/v1/attestations/@earendil-works%2fpi-durable@1.4.0)
names tag `v1.4.0` and commit
[`4a080252f06caf63a525d7873c5d94d3c23887e0`](https://github.com/earendil-works/pi/tree/4a080252f06caf63a525d7873c5d94d3c23887e0).
The provenance statement was inspected, not independently signature-verified.

The installed package's source maps reproduce the published commit's two
relevant source files exactly, rather than the planning commit's files:

| Source path under `packages/durable/src/harness/` | Reviewed Git blob | Published/installed Git blob |
| --- | --- | --- |
| `types.ts` | `d7f4a05ec86b1977d2d1c318cded2a192588901f` | `289b5c4e18b0a36267ef5f5e36d9afda2a649ecf` |
| `submissions.ts` | `66f116af3c5f36547a9a8e0670c334a3d85e800e` | `0cf838a93932cac6d41ec5e209f987eda1546611` |

This is a demonstrated API difference, not merely different commit IDs. The
published declarations lack the reviewed `submit(tx, draft)` overload,
`whenIdle: "reject"`, and `type: "continue"` submission. The newer implementation
can be inspected in the pinned
[types](https://github.com/earendil-works/pi/blob/e65b3b88c08412bb8ffbd21fb5dc5756064a6e5a/packages/durable/src/harness/types.ts)
and [submission code](https://github.com/earendil-works/pi/blob/e65b3b88c08412bb8ffbd21fb5dc5756064a6e5a/packages/durable/src/harness/submissions.ts).

Installed and tested pins:

- Node `22.23.4`; Pi Durable, Pi AI and Chord `1.4.0`.
- TypeScript `5.9.3`; `@types/node` `22.19.0`.
- Pi Durable tarball integrity:
  `sha512-EtIyXH1MBSXdJSkodmy+CYXuwfve3gPrD4GNCZ0XmuRWLPupwRlNQ7fdaXjRcLoKb6SWkT/yEzK+sMmrqvYK+Q==`.
- Every transitive package is recorded in `runtime/pi/package-lock.json`; install
  scripts are disabled. No `latest`, private imports or runtime forks are used.

The npm version list inspected on 2026-10-29 ended at `1.4.0`. No newer release
was selected or silently substituted. These source comparisons do not prove
whole-build reproducibility or supply-chain attestation validity.

## Executable observations

| Probe | Observed result | Meaning |
| --- | --- | --- |
| Idle input with `whenBusy: "steer", whenIdle: "reject"` | Accepted; one faux-model call; submission `done` | Published runtime ignores the newer idle guard. It can start new work where a caller expects steering-only behavior. |
| Same request ID, different content | Original submission returned; one model call and one user entry | Pi deduplication is useful but does not enforce Switchyard's payload-conflict rule. A bridge-owned durable hash binding is still required. |
| SIGKILL after Pi admission, before an external host receives the receipt | Reopened public API finds the same submission; replay produces one entry | Limited positive evidence for Pi persistence/deduplication. It does not prove a crash during admission, the Rust outbox, model/tool recovery or exactly-once effects. |
| Source/declaration audit | Published source matches the release commit, not the reviewed source | The mandatory source/package compatibility criterion fails. |
| Adoption gate | Exit `1`, JSON decision `NO-GO` | Audit success cannot be mistaken for S2 approval. |

The runtime behavior is exercised using public imports and a faux provider;
storage queries use public `Harness.commit` / `Tx.submissionByRequest`, not
private tables. Child-process probes receive an empty environment. The audit
does not load operator provider credentials or call external model services.

An inspect-then-submit workaround for idle rejection is insufficient: the run
can become idle between the read and admission. Any compatibility adapter must
prove the intended atomic behavior, not just observe that the model was busy
when inspected. The missing transactional overload is a compatibility delta;
it is not a claim that every alternative durable adapter design is impossible.

## Reproduce and interpret the checks

From `runtime/pi`, with the exact Node version selected:

```bash
npm ci
npm run verify
npm run --silent audit:package
npm run --silent gate
```

`verify` runs typechecking and six tests (five executable probes and a decision
regression). The successful tests
confirm the recorded incompatibilities. `audit:package` prints evidence;
`gate` must exit `1`. A nonzero result is not to be masked or treated as GO.

The report derives `NO-GO` from non-empty observed `blockers`. With no package
blockers it reports `NEEDS-FULL-S2`, never `GO`, and the adoption gate still exits
`1`. CI also requires non-empty blockers. The inverted package tests deliberately
fail when a candidate becomes compatible: preserve this published-1.4.0 evidence,
introduce separate candidate compatibility tests, and review any change to these
expectations together with the pin. Do not weaken tests to keep an audit green
or interpret compatible package APIs as a passed bridge contract.

CI's `pi-package-audit` runs those diagnostic tests and uploads the JSON report.
It is deliberately **not** `pi-bridge-contract`, and does not meet that required
S2 check. Rust/TypeScript gates retain their original names and behavior. No
branch protection or ruleset is changed by this work.

## What remains unproven

All S2 integration acceptance remains open: Rust process supervision and outbox
reconciliation, protocol/version/binding checks, bounded bidirectional transport,
snapshot recovery, stable multi-effect and nested-tool replay through Rust
receipts, registry restrictions, and observable abort settlement. Exported
`taskId` and nested-call types alone do not prove replay behavior.

This audit did not add a process bridge. Later bridge work is tracked in
Current status above. In particular, the S1 fake runtime's
verification/publication notifications are not exposed to Pi. Future external
frames must not authorize verification PASS, approval, publication, or writer
quiescence; those remain Rust-owned facts. S3 confinement and S8 live acceptance
are separate gates even after S2 succeeds.

## Recommended bounded remedy

The 2026-10-29 audit recommended a separately reviewable source build of
`e65b3b88c08412bb8ffbd21fb5dc5756064a6e5a`, with the workspace dependency closure,
Node, build tools, upstream lockfile, and source provenance pinned, and with a
check that the emitted artifacts expose the reviewed APIs. That recommendation
did not approve a bridge. Do not mix the built source with similarly numbered
published workspace packages without a documented compatibility check.

[#107](https://github.com/kortiene/switchyard/pull/107) merged that candidate in
[runtime/pi-source](../../runtime/pi-source/README.md). Its complete five-package
workspace closure is built from the reviewed source with the upstream pinned
catalog. Two local builds matched every committed artifact hash, and four
public-API compatibility tests pass. Its verdict remains `NEEDS-FULL-S2`. It does
not alter the published-package NO-GO above.

S2 is NO-GO until the whole gate passes; see Current status and
[#96](https://github.com/kortiene/switchyard/issues/96). A newer compatible published package with
reviewed provenance is another bounded remedy when available. If neither the
source candidate nor a published pin can pass that gate, review a native-Rust
implementation gate and downstream scope. Retain S1/fake operation; do not
promote the legacy TypeScript engine or weaken the contract.
