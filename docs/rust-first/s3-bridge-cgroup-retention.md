# Bounded confined-bridge cgroup retention

Refs #151, #97, #94. This extends the non-default Rust-owned
[bridge lifecycle](s3-bridge-lifecycle.md). It does not change provider egress,
credentials, FakeRuntime defaults, reboot support, or full S3 acceptance.

## Ownership and retention bound

Cleanup runs while the session owner flock is held. The top-level `cgroup`
field in `current.json` remains a live recovery reference, even for a proven
stopped session. That leaf is never collected. Reopening still kills and
proves that exact device/inode/boot identity empty before inspecting Pi state.
A missing or replaced **current** leaf blocks recovery as before.

After a normal handoff, one current leaf remains per session. During allocation
and handoff there can be two: the current leaf and its replacement. An
interrupted allocation with unknown identity retains at most one additional
leaf and blocks further allocations. Cleanup does not guess that identity.

Pre-existing leaves with `recovered-<generation>.json` evidence are collected
in batches of at most eight historical retirements per cleanup pass, in addition
to settling pending allocation/deletion journals. A remaining historical
backlog blocks new allocation; a subsequent reopen continues draining it.
Historical lifecycle/recovery evidence is not a live cgroup reference after a
new current record has committed. Retirement receipts explicitly record this
handoff rather than erasing the old identity or proof.

No global `bridge-*` directory sweep is performed. Leaves with no journal or
recovery identity from before this change, foreign cgroups, and unrelated
sessions are not adopted or deleted. Such untracked legacy leaves are outside
the managed retention bound and need independent operator inspection. Audit
files themselves are retained; this is a cgroup bound, not a disk-retention
policy or terminal-session garbage collector.

## Durable ordering

All journals and receipts use the existing owner-only, no-follow control
storage: file fsync, atomic rename, then directory fsync. Journal removal also
validates an owned regular file, unlinks it, and fsyncs the directory.

1. Before creating a new leaf, persist `leaf-allocation.json` with the session
   identity, generation, and intended unique path. Persist its device/inode/boot
   identity immediately after creation. No launcher can run yet.
2. Before replacing the current record, retain the predecessor's proven
   identity in `recovered-<generation>.json`. Only a durable replacement
   `current.json` transfers live recovery authority to the new leaf.
3. Settle the allocation journal. If its leaf is now current, remove only the
   journal. If the leaf was never published as current, retire that unreleased
   allocation with the same identity/emptiness rules as a historical leaf.
4. Select an obsolete, recorded leaf and persist `leaf-retirement.json`.
   Check that it is not current and that its allocation/recovery source still
   matches. Validate device, inode, boot, and fresh `populated 0`.
5. Persist deletion authorization containing the exact identity and empty
   proof **before** `rmdir`. Recheck identity and emptiness at removal. Cleanup
   does not kill a nonempty or replaced obsolete cgroup to make deletion work.
6. Retain `retired-<identity-hash>.json` with the source generation, identity,
   authorization and completion. Only then clear the retirement journal. Keep
   the original recovery/lifecycle evidence.
7. Settle old leaves before launching the replacement bridge. The current
   leaf is retained after stop; it becomes collectible only after a later
   durable handoff.

## Interrupted and uncertain cases

- **Before mkdir:** an absent, unpublished allocation can be abandoned. The
  audit records that it was unreleased, not a process-stop proof.
- **After mkdir, before identity persistence:** if the intended path exists,
  retain it and block. Neither its name nor an empty reading licenses adoption.
- **Before replacing current:** only an identified unreleased replacement may
  be retired; the original current leaf remains protected.
- **After replacing current:** the new current leaf remains protected, while
  the predecessor can be retired using its durable recovery evidence.
- **Before rmdir:** retry only after checking the original source, identity and
  fresh emptiness again.
- **After rmdir, before receipt/journal cleanup:** absence is acceptable only
  for the obsolete identity whose deletion was already durably authorized.
  This exception cannot certify a missing current leaf or any tool effect.
- **Missing without authorization, changed identity, or nonempty:** retain the
  journal and fail closed. A reappeared path after completed retirement also
  blocks; it is not deleted as if it were the original leaf.

The control-root and delegated-cgroup ancestors retain the lifecycle's existing
trusted-Host/same-UID assumptions. This does not claim atomicity across SQLite,
control files and cgroupfs, or power-loss, reboot, or cross-host recovery.

## Evidence and gates

The `confined_pi_` unit suite uses the real durable control storage with a
modeled cgroup backend. It injects interruption before/after record handoff,
mkdir identity persistence, and removal/receipt completion; checks missing,
replaced and nonempty leaves on both sides of authorization; exercises legacy
backlog draining; and performs 33 handoffs without cgroup growth. These are
fault-model tests, not claims of real SIGKILL at each syscall boundary.

The existing required `execution-confinement` job also runs real Linux
`bridge_lifecycle` tests: six session reopens, current-leaf retention and missing
current refusal, missing/replaced/nonempty historical-leaf refusal, and an
explicit post-rmdir crash snapshot. The parent suite's real Host SIGKILL test
now verifies exact retained retirement evidence after the obsolete leaf is
removed. Existing B1–B6 and Host dispatch suites stay enabled. Native execution
requires the supported Linux x86_64 runner; ARM64 unit results alone do not
satisfy that gate.
