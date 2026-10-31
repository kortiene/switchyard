# S3-A1: owned worktrees and crash reconciliation

Scope: [#118](https://github.com/kortiene/switchyard/issues/118), under
[#97](https://github.com/kortiene/switchyard/issues/97) Track A. This is
Pi-independent workspace ownership. **S2 remains NO-GO.** FakeRuntime remains
the product adapter. A worktree is not a sandbox: this adds no agent execution
API, provider calls, Pi tool forwarding, verification or GitHub publication.

## Explicit preparation

Create a mission, then choose an existing canonical absolute directory outside
the repository. The operator chooses the root; the Host chooses the attempt
slot. Relative paths, dot components, symlink aliases, existing slots and overlap
with registered checkouts or Git administration are rejected.

```sh
mkdir -p /absolute/private/workspaces
cargo run --locked -p switchyard -- mission workspace \
  --db /absolute/control/mission.sqlite \
  --mission m-... --attempt a-... --root /absolute/private/workspaces
```

`Host::prepare_workspace(mission, attempt, root)` implements the same operation.
It creates `<root>/<attempt>/tree` at the mission's frozen commit with detached
HEAD. It does not switch a source branch, copy dirty source files, alter the
source index or follow a source branch that has advanced. Only host-owned
worktree administration is added to the shared Git directory.

The private attempt slot contains `owner.json`, `owner.lock`, `git-create.log`
and `tree/`. The marker is outside tracked content. Creation uses an exclusive
new directory (0700) and files (0600), and syncs the marker and directory entries
before authorizing Git. There is no automatic removal, pruning, force, path
repair or checkout/reset during recovery.

## Ledger and identity

Rust ledger v3 adds `workspaces`, with unique workspace ID, attempt and path. It
records mission/attempt, canonical ledger/workspace paths, source repository, frozen base,
filesystem identities, lifecycle, Git administration path, creation/verification
generations and uncertainty detail. The migration is additive from v2; v1
upgrades pass through v2. Historical v1/v2 ledgers remain inspectable without
migration. Frozen legacy TypeScript state v1 is unchanged.

The Host owner lock and generation fence guard every operation, including an
idempotent reopen. The filesystem marker and Git lock reason bind the workspace
to the same mission, attempt, base and repository. Recovery checks device/inode
identities, canonical paths, both Git backlinks, a detached HEAD, lock
reason, readable index and the frozen Git tree. While `creating`, HEAD must
equal the frozen base. Once `ready`, HEAD may be that base or a descendant. Missing or mismatched metadata
never authorizes adoption of an existing directory. A copied/moved ledger at a
different path cannot adopt the original workspace through a different owner lock.

Once ready, tracked, staged and untracked edits survive reopen. Readiness proves
identity, not verified content or OS writer quiescence. Preparation grants no
mission phase/revision, approval, digest or execution authority. Terminal
workspaces can be inspected and reconciled, but a terminal attempt cannot create
a new workspace. A successor gets a distinct slot; the old workspace is retained.

## Lifecycle and crash boundaries

| State/boundary | Restart behavior |
| --- | --- |
| No reservation | Opening/recovery does not create a workspace implicitly. |
| `reserved`, no slot | The persisted intent authorizes one exclusive slot creation. |
| `reserved`, slot with a matching `owner.json` and no other entries besides `owner.lock` | Reclaim that slot and continue creation. The marker is the ownership proof for the gap before `creating` is committed. |
| `reserved`, slot present without that proof | Ownership is ambiguous: retain it and record `uncertain`. |
| `creating`, a creator holds the lease | Return `workspace_busy`; keep the intent pending. |
| `creating`, complete matching clean checkout at the frozen base | Acknowledge the existing workspace as `ready`; do not repeat Git creation. |
| `creating`, absent/partial/changed checkout | Record `uncertain`, retain evidence and block. A clean Git failure with no tree and no worktree registration is the exception: it stays `creating` and can be retried. |
| `ready`, identity matches and detached HEAD descends from the frozen base | Return the original workspace, verified for this owner generation. Equality with the base still matches; later descendant commits do too. |
| `ready`, missing/replaced/rebound metadata, or HEAD that does not descend from the base | Record `uncertain`; never create a replacement. |
| `uncertain` | Stay blocked; this slice has no automatic repair/reset API. |

The Host commits `reserved` before filesystem changes, creates and syncs the
ownership slot, takes its lease, then commits `creating` before Git. A crash
after a matching `owner.json` is durable and before `creating` is committed
reclaims that slot. A crash that leaves no matching marker still blocks
conservatively. A crash after `creating` but before Git begins also blocks when
there is no complete checkout: absence alone cannot prove an old writer never
existed. An observed storage or Git failure does not record `uncertain` unless
it leaves workspace evidence; those errors stay retryable.

Creation uses `git worktree add --no-checkout --detach --lock --reason ...`,
checks the new binding, then performs a one-time `git reset --hard` only in that
new, otherwise empty directory. Filters are discovered and disabled again in
the new worktree, including drivers from conditional/per-worktree configuration.

Git inherits a duplicate of the locked workspace descriptor as stdin and logs
to a retained file, avoiding broken pipes when the Host dies. A surviving Git
process keeps the flock until exit. A replacement Host can own the ledger but
cannot reconcile/reuse the workspace while that creator holds the lease. The
Host closes its descriptor without explicitly unlocking the child's copy. This
protects the known creation process; it is not arbitrary process-tree supervision.

After Git finishes, the Host verifies the complete clean checkout before
committing `ready`. SIGKILL before that acknowledgment recovers the existing
workspace. Failures retain diagnostics/files and record durable uncertainty.
These are process-crash guarantees, not a claim of power-loss durability for all
Git working files.

`Host::dispatch_pending` reconciles a recorded workspace in the current owner
generation before its readiness gate, so `mission command` can dispatch a
workspace prepared by an earlier process. `Host::recover` does that same
reconciliation, then dispatch, poll, and snapshot. An uncertain or busy
workspace skips dispatch and is returned after polling and the snapshot, so
one workspace does not block runtime recovery. Direct dispatch still refuses a
workspace that is not ready for this generation. `mission status` reports
recorded state read-only, without Git operations or a workspace lease.

## Git boundary and remaining S3 work

Only fixed host operations run. Git inherits PATH for the trusted host executable;
Git override variables and model/forge credentials are cleared. System/global
config, hooks, fsmonitor, configured clean/smudge/process filters, submodule
recursion, sparse checkout, lazy fetch, replacement objects and network protocols
are disabled. Submodules are not initialized. Repository filters are not
materialized: an LFS pointer, for example, stays a pointer. The trusted operator's
executable, configuration and control directories must not change concurrently.

Canonical paths and inode checks address ordinary aliasing and replacement;
they do not defeat hostile filesystem races. OS confinement, inaccessible
Git/control metadata, mediated effects, arbitrary writer fencing, process-tree
supervision and safe workspace release remain subsequent #97 work. Pi-dependent
live work also requires S2 GO. The legacy TypeScript worktree manager is unused.

## Verification

`./scripts/rust-verify.sh` includes real Git tests for dirty source preservation
(branch, HEAD, index bytes, tracked/untracked content), frozen-base checkout,
idempotency, retained edits, successor isolation, collisions/aliases, stale
generations, SIGKILL before/after creation, a surviving Git creator's lease,
partial/missing/rebound metadata, hooks and conditional filters, ledger upgrades
and the CLI with inherited Git configuration/credential sentinels.

Git's [worktree documentation](https://git-scm.com/docs/git-worktree) describes
linked metadata and `--lock`. That administrative lock prevents pruning/moving;
it does not stop writers and is separate from the OS creation lease above.
