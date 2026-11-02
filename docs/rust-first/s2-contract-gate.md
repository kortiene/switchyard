# S2-E: contract evidence and acceptance review

Scope: [#96](https://github.com/kortiene/switchyard/issues/96), under
[#94](https://github.com/kortiene/switchyard/issues/94), after merged S2-D
[#140](https://github.com/kortiene/switchyard/pull/140) (`42a94d3`).
**Decision: NO-GO pending review of this complete evidence matrix and its CI run.**
This PR proposes the credential-free acceptance gate. It does not award GO,
close #96, enable Pi in the product, or authorize live effects.

## Exact candidate

| Component | Identity |
| --- | --- |
| Pi source | `e65b3b88c08412bb8ffbd21fb5dc5756064a6e5a`, five workspace packages at manifest version 1.4.0 |
| Upstream lock SHA-256 | `5acba9a5ff112c8dd4e62c5ac686199c3a0f99721b86302bb3bb0f35d640ecb1` |
| Catalog | `sha256-7fe1b41feaf43a7aad4bc832e6b306c7ced56e6d41bbc724b74e91eeacff6edf` |
| Node / npm / TypeScript | `22.23.4` / `11.9.0` / `7.0.2` |
| Rust / CI platform | `1.99.1` / `ubuntu-24.04` |
| Adapter and code revision | Fingerprint validated in both peers; workflow artifact records the tested Git commit/tree, dirty flag, lock/vector/manifest hashes and full pins |

This is the reviewed source-build remedy, not the mismatched published package
audited in [S2 feasibility](s2-feasibility.md). CI rebuilds all five packages and
checks committed hashes before installation. The installed-source tests check
admission source maps and reject an internal package falling back to npm.
No private Pi tables are read or written. Upstream signatures and cross-platform
artifact reproducibility are not newly claimed.

## Mandatory evidence matrix

Every row is proposed evidence requiring acceptance review. Exact executable
test names are in [`contract-cases.json`](../../runtime/pi-source/contract-cases.json).
The runner executes the entire Node and feature-enabled Rust workspace suites,
then requires every named case to have passed. Deleting/renaming a mandatory
case without updating its evidence fails the gate. Skipped, ignored, filtered,
cancelled and todo tests are refused.

| #96 requirement | Executable evidence | Scope / interpretation |
| --- | --- | --- |
| Reproducible source/API baseline | `source-integrity`: artifact/lock/source-map verification and handshake pin-drift rejection | Workflow rebuilds; missing prerequisites fail before a successful artifact |
| Public admission, reopen, deduplication | `admission-and-outbox`: transactional rollback, actual Rust outbox lost ACK over repeated generations, changed content, missing acknowledged submission | One mission/attempt/command/receipt; missing/corrupt original storage is retained or blocked |
| Missed observations and resync | `snapshot-resync`: actual wire `watch` across three owners, plus active Host recovery of interrupted tools | `resyncRequired` with current snapshot, no event replay promise; inspect/open are inert |
| Shared canonical identities | `canonical-identity`: the same checked-in Unicode, delimiter, UTF-16-order and safe-integer vectors in Rust and Node | Generation is authorization only; operation identity excludes it and payload arguments |
| Multi-effect / nested replay and conflict | `effect-replay`: real Pi SIGKILL after a real Host simulated receipt, two slots plus nested task, original receipt reuse, changed arguments conflict, uncertain replay blocked | The test-only mapper uses the real Host journal; it does not wire the product Pi Host to a backend |
| Scheduling, effects and stop interaction | `stop-composition`: pending Pause/Cancel suppress Continue and settle interrupted real tool tasks before replay; active control resumes the same task IDs; abort retains completed/uncertain Host effect evidence over two generations | Host/outbox tests and Host-effect/bridge tests cover their actual seams. The effect test driver sends abort explicitly; it is not a claim of one product Pi/ExecutionEnv path |
| Fixed tool registry and execution boundary | `registry-and-authority`: only write_file/write_pair/nested_write; unknown shell, eval, loader, MCP and publication calls fail; input cannot select tools/extensions/provider/model | All enabled effects forward through execution.request. In-process allowlist only; arbitrary trusted TypeScript is not OS-confined by this test |
| Answered / idle / aborted / interrupted | `snapshot-resync`, `stop-composition`, `bounds-and-timeout` | Input/continue `status=done` means answered; `unanswered` + `reason=aborted` means aborted. Interrupted pending work is observed with paused scheduling and retained tasks/receipts, never inferred complete from transport loss |
| Runtime has no mission authority | `registry-and-authority`: forged nested result authority, Host observation and stop-claim rejection | Pi settled/idle cannot establish verification, approval, publication or execution quiescence |
| Protocol, capabilities, generation and bounds | `bounds-and-timeout`, `registry-and-authority`: malformed/oversized frames, wrong binding, stale generation, unknown replies, busy retries, request overflow, effect flood, uncooperative timeout/reaping | Synchronous trusted receipt callbacks must return within the documented bound; no preemption of arbitrary Rust callback code is claimed |
| Storage failure / close interleavings | `storage-and-close`, all [S2-D cases](s2-storage-recovery.md) | Real SQLite refusal, fatal background failure, pending-close busy, stop intent/settlement and Rust ACK recovery; not torn-write/power-loss testing |
| Mandatory CI | `pi-bridge-contract` workflow + `contract.mjs` | Every PR, main push and merge queue entry; exact pins, no path filters, no skip/fallback, no continue-on-error |

The new Host scheduling fixture gates two real durable tool tasks immediately
before forwarding. Its entry log includes task identity and generation; it
stays armed during recovery. Pending stops leave the log unchanged through two
recoveries. The positive control proves the fixture can actually replay and that
active recovery uses the same task IDs. No test-only runtime effect callback is
added to the product Host.

The additional effect/abort composition covers completed effects with Pause and
Cancel and uncertain effects with Cancel. An uncertain effect has already moved
the mission to `needs_attention`, where Pause is not a legal command. Abort
settlement preserves the original receipt or uncertainty and admits no new
effects. Existing multi-effect/nested tests still prove completion replay.

## Reproduction and CI evidence

Use the committed Rust toolchain and put the exact Node executable on PATH:

```sh
node runtime/pi-source/install-npm.mjs
cd runtime/pi-source
node build.mjs
npm ci
cd ../..
node runtime/pi-source/contract.mjs
```

The runner checks versions and provenance, formatting, workspace/all-target
Clippy with warnings denied, TypeScript, every Node test and the feature-enabled
Rust workspace. It sets `SWITCHYARD_PI_NODE` to its own verified executable.
Logs and `evidence.json` are retained in the workflow's
`pi-bridge-contract-evidence` artifact, including failed runs. A pre-run build or
installation failure fails the job even if no runner artifact exists.
`conformance=passed` means tests passed; `acceptance` deliberately remains
`NO-GO until reviewed`. A local dirty checkout is recorded as dirty, not as
evidence for an exact committed revision. No source rebuild is implied by a
local run that reused cached artifacts; CI independently performs the rebuild.

Local validation of this change passed the runner with 41 Node tests and 218
Rust tests (196 core, 13 bridge, 9 CLI), no skipped/ignored/filtered tests,
formatting, Clippy and TypeScript checks. Source artifacts/dependencies were
reused locally with integrity checks. A negative run on Node 24.22.1 failed at
the exact-version check. The PR records the published head/tree and CI runs;
the local dirty-tree result does not stand in for clean CI revision evidence.

The existing `pi-bridge-probe`, `pi-source-candidate` and package-audit jobs remain
separate historical checks. This adds coverage and an evidence-aware gate; it
does not simply rename the partial probe. Creating YAML does not install a
required-check rule. After the first green run, a repository administrator must
require the exact `pi-bridge-contract` check in the applicable ruleset before
calling it merge-enforced. Ruleset enforcement is not changed by this PR.

## Review and downstream handoff

Review every matrix row against the exact head SHA and uploaded CI evidence,
including the credential-free source-build remedy and bounded protocol
refinements. If any mandatory row remains unproven, record its blocker in #96
and retain NO-GO. If all rows are accepted, record an explicit S2 GO with the
reviewed commit, source/Node pins and CI run, then update #94/#96/#97. Do not
treat merge, a green check, or a PR closing keyword as that decision.

S3 Track B still owns product Pi-to-Linux effect mapping, file/process handles,
bridge filesystem/process/network confinement, model-only egress, real process
tree quiescence and Pi stop delivery past blocked workspaces. Those limits must
remain visible when reviewing the composed test seams above. FakeRuntime stays
the product default. No provider credentials, live model, GitHub publication,
legacy TypeScript change, new ledger schema or silent session migration is
introduced. The trusted fixture change alters the adapter fingerprint; older
probe storage is retained and rejected rather than reopened under a new pin.
