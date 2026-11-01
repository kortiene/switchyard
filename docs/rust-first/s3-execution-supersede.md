# S3-A5: supersede an abandoned execution decision

Scope: [#133](https://github.com/kortiene/switchyard/issues/133) under
[#97](https://github.com/kortiene/switchyard/issues/97) Track A. It closes the
remaining open item of [#125](https://github.com/kortiene/switchyard/issues/125).
Depends on [#130](https://github.com/kortiene/switchyard/issues/130), delivered
by [#132](https://github.com/kortiene/switchyard/pull/132). Milestone
[#94](https://github.com/kortiene/switchyard/issues/94).

FakeRuntime stays the product adapter. There is no Linux execution CLI and no
Pi wiring. **S2 remains NO-GO.** This slice does not claim full S3, Track B,
reboot recovery, seccomp, or an egress allowlist.

## What supersession is

`Host::supersede_execution` appends one audit row for an effect that already
has a committed Abandon resolution. It applies to either journal. The effect
row, its intent, and the abandonment audit stay byte-for-byte unchanged. The
state stays `uncertain`. There is no receipt and no new state CHECK. The
outcome stays unknown.

The operation stops counting toward `execution_blocked`, and the mission
revision increments in the same owner-fenced transaction. Phase does not
change. Nothing is dispatched, reopened, verified, reviewed, or approved for
publication. Explicit recovery is still the operator's next step, and any
other unresolved effect or generic operation still blocks it. Replaying the
original operation id returns the abandoned unknown outcome and does not
execute again: no new cgroup leaf is allocated and the FakeRuntime file map
does not change. Later work needs a new op-v1 identity.

Unresolved, `completed`, and `not_applied` effects cannot be superseded, and a
supersession itself cannot be superseded. A lost acknowledgement retries the
same command id and payload, refreshing only the owner generation, and returns
the one stored audit before the Host asks for new evidence. A different
payload under that id conflicts. A new command id for the same effect is
refused. Command ids stay unique across both resolution tables and
`execution_supersessions`.

The request names the stable command id, current binding, exact operation id
and payload hash, the abandonment command id and request hash, the expected
revision, an actor, and a rationale. The actor is a local audit label. There
is no field for a receipt, a quiescence claim, or a workspace digest.

## Evidence the Host observes

Checks that do not need the workspace run first. A stale generation, wrong
binding or hash, stale revision, a phase other than `needs_attention`, or a
terminal mission is rejected and the block stays.

**Simulated.** The basis is the same as #126: an exclusive `&mut Host`,
synchronous apply, and no background workers. The simulated effect never
touched the worktree.

**Linux.** The delegated cgroup backend must be enabled, and this Host must
have no live supervisor for the operation or the mission. The recorded leaf is
killed again and must then read `populated 0` with the same path, device,
inode, and boot id. Quiescence stored at abandonment is not reused. Every
other Linux effect of the mission must already be settled (`completed` or
`not_applied`), already superseded, or re-proven empty the same way. A missing
leaf, a replaced inode, a different boot id, a nonempty leaf, or a path
outside the delegated root fails closed. After a reboot — a different boot id
or a leaf that is gone — the abandoned effect stays blocked.

**Workspace.** After that proof, a fresh S3-A1 `verify` must pass on the
attempt's existing worktree: identity, Git backlinks, a detached HEAD at or
descending from the frozen base, a readable index, and no `index.lock`. The
workspace must be `ready`. The worktree is kept. This slice does not create or
retire one, and it does not add an S5 verification gate.

The audit records a content digest of what is on disk. It is a Git tree id
built in a temporary index (`GIT_INDEX_FILE`), never the real index and never
`index.lock`. The index is seeded with `read-tree HEAD` and then
`git add --all --force`. **Ignored files are included**, so a leftover cannot
hide by being listed in `.gitignore`. That covers tracked files at their
on-disk bytes, untracked files, and `.sy-tmp-*` leftovers whether or not they
are ignored. On-disk bytes win when the real index holds a different staged
blob. New blobs and the tree are written only to a scratch
`GIT_OBJECT_DIRECTORY` beside the worktree, created mode `0700` and refused if
that path already exists. The repository object store is only an alternate.
The scratch directory is not the shared temp directory. Drop deletes it. A
killed Host can leave a `0700` `switchyard-digest-*` directory in the attempt
slot; the next digest on that slot removes those leftovers before creating a
new one. Ignored files do not become loose objects in the shared database.
The command does not move HEAD, the branch, or the real index.

The owner flock is per canonical database path. A copied or hardlinked ledger
gets its own lock, so supersession compares the workspace's recorded ledger
path with this Host's path, the same check workspace preparation uses, before
any cgroup kill.

The workspace `ready` check and S3-A1 verify run before any cgroup kill, and
again after quiescence together with the digest. A refusal after a kill keeps
the block and includes that kill evidence in the error. Sibling leaves are
re-killed only while this Host holds the exclusive owner flock
(`execution/owner.rs`); a second live Host cannot open the ledger.

## Ledger v9

Forward migration adds append-only `execution_supersessions`:

| Column | Constraint |
| --- | --- |
| `command_id` | primary key |
| `operation_id` | unique |
| `journal` | `simulated` or `linux` |
| `abandonment_command_id` | unique |
| `request_hash` | |
| `audit_json` | |

Triggers abort UPDATE and DELETE on this table and on both resolution tables.
The cross-journal command-id triggers also cover supersessions. A racing insert
that loses `UNIQUE(operation_id)` or `UNIQUE(abandonment_command_id)` is a
guard error, not a raw storage failure. v1–v8 rows are
not rewritten. Blocking queries exclude superseded operations. Read-only
inspection of a v1–v8 database shows `executionSupersessions` as an empty array
and does not migrate. A writer upgrades to v9.

## Operator surface

`mission status` adds `executionSupersessions`. Each effect view gains
`superseded`. State stays `uncertain`. `nextSteps` names the evidence a
supersession needs and says the outcome stays unknown.

```sh
switchyard mission supersede-execution --db mission.sqlite \
  --mission m-... --attempt a-... --session s-... \
  --operation-id op-v1:... --payload-hash sha256:... \
  --abandonment-command-id resolution-001 \
  --abandonment-request-hash sha256:... \
  --expected-revision 8 --command-id supersede-001 \
  --actor operator --rationale 'Leaf re-proven empty; outcome stays unknown'
```

The CLI covers the simulated journal only, the same boundary as
`abandon-execution`. A default build refuses a Linux row and explains that the
delegated cgroup backend is required. Linux supersession is Host API only.
Exit 0 means the audit committed, not that the effect completed. Copy the
abandonment id and request hash from `executionResolutions` or
`linuxExecutionResolutions`.

A superseded effect no longer blocks Pause or Cancel completion. Other
unresolved effects still do.

## Still open

- #97 Track B.
- Reboot recovery, including superseding an effect whose leaf is gone after a
  reboot.
- Seccomp, egress allowlists, and adversarial or power-loss quiescence.
- The S4 lifecycle, including retiring a workspace and a successor attempt.
