# Design proposal — managed Git worktrees

**Status:** implemented for the initial opt-in managed-worktree release. The
repository now includes immutable run contexts, a durable common-Git-dir
registry, per-run/lifecycle/merge leases, real linked-worktree allocation and
resume validation, separated durable state and worktree-local authored
artifacts, structured outcomes, conservative cleanup, and `worktree
list/status/remove/prune --dry-run` CLI controls. The bounded multi-run batch
dispatcher and cockpit UI described as later phases remain follow-up work.

**Author context:** generalizes the field-proven external/operator workflow in
[`PARALLEL-BATCH.md`](./PARALLEL-BATCH.md) and
[`run-batch-parallel.sh`](../run-batch-parallel.sh). The existing
`--project-root` seam already makes the kernel worktree-compatible; this design
adds native ownership, durability, recovery, cleanup, and concurrency controls.

**Decision summary:** adopt one managed Git worktree and one worker process per
ADW run. Keep the developer's primary checkout untouched, preserve the existing
branch naming and `--project-root` behavior, retain failed/interrupted/PR-only
worktrees, and remove only manager-owned worktrees whose exact change request is
authoritatively merged or whose skipped-closed allocation is proven pristine.
Use a durable run registry, strict ownership checks, short shared-repository
locks, and a merge-specific lock. Do not run concurrent `run()` calls inside
today's process-global runtime.

---

## 1. Goal

Allow one developer to run independent Switchyard work items against the same
Git repository concurrently without one run switching, dirtying, committing,
or cleaning another run's checkout.

The intended steady state is:

```text
primary checkout / shared Git repository
├── durable registry + control state
├── managed worktree <run-a> ── worker process A
├── managed worktree <run-b> ── worker process B
└── managed worktree <run-c> ── worker process C
```

The capability includes:

- worktree creation, discovery, reuse, and conservative cleanup;
- exclusive branch/worktree ownership per run;
- safe resume after process failure or cancellation;
- bounded concurrency without serializing agent phases or tests;
- base-freshness and merge coordination across sibling runs;
- durable observability after a successfully merged worktree is removed; and
- CLI and cockpit visibility into active and retained runs.

## 2. Non-goals

- **Not an in-process scheduler for the first release.** Each managed run gets
  a separate worker process and runner instance. The exported library can gain
  safe in-process concurrency only after process-global roots and caches are
  removed.
- **Not automatic parallelization of arbitrary dependencies.** A later batch
  supervisor may accept independent work items or an explicit dependency plan;
  it must not infer that two issues are conflict-free merely because their
  titles differ.
- **Not isolation of every external resource.** Worktrees isolate checked-out
  files, not fixed ports, databases, global package caches, API quotas, or
  subscription limits. Projects remain responsible for making their gates safe
  to run concurrently.
- **Not transparent recovery from a deleted checkout with uncommitted work.**
  ADW currently commits only during finalize. If a worktree disappears after an
  editing phase, `completed_phases` does not reconstruct those filesystem
  effects. Initial recovery must fail closed; filesystem checkpoints are a
  separate follow-up.
- **Not ownership of externally created worktrees.** Existing `--project-root`
  callers remain responsible for those checkouts unless they explicitly adopt
  them into the managed registry.
- **Not automatic mutation of the developer's primary branch.** A managed lane
  may fetch shared refs, but it never switches, rebases, resets, or pulls the
  primary checkout.

## 3. Existing capability and operational evidence

Worktrees are not greenfield in Switchyard:

- [`run-batch-parallel.sh`](../run-batch-parallel.sh) creates one detached
  linked worktree per issue and invokes the current kernel with
  `--project-root <worktree>`.
- [`PARALLEL-BATCH.md`](./PARALLEL-BATCH.md) records a 17-issue live batch and
  the constraints learned from it: explicit repository locators, capped and
  staggered concurrency, shared-`.git` lock contention, rebase conflicts,
  artifact archival, and branch/worktree cleanup.
- [`src/common.ts`](../src/common.ts) already gives config, prompts, state,
  agents, runners, and captured subprocesses a project-root seam. Inherited
  provider subprocesses that bypass its default cwd are a known explicit-cwd
  gap, not evidence that ambient cwd is safe.
- [`src/invoker.ts`](../src/invoker.ts) already gives every runner request an
  explicit editing `cwd`, and all four runner adapters honor it.
- [`src/git.ts`](../src/git.ts) already rebases a lane onto a moved
  `origin/<base>`, derives force-with-lease from actual remote divergence, and
  retries fetches that encounter sibling ref-lock contention. The orchestrator
  re-proves gates after a real rebase.
- Branches already include the eight-character ADW run ID
  (`{prefix}/{work-item}-{adw-id}-{slug}`), providing a useful ownership key.

The missing feature is therefore not basic Git compatibility. It is a generic,
safe lifecycle manager around behavior that an operator script currently owns.

## 4. Current assumptions that conflict with first-class worktrees

### 4.1 One process-global root owns unrelated concerns

`projectRootOverride` in [`src/common.ts`](../src/common.ts) is process-global.
It currently anchors:

- `.adw/config.json` and project prompt/schema resolution;
- `agents/<adw-id>/` state and phase artifacts;
- the runner's editing directory;
- the default cwd for captured Git, provider, and local gate commands; and
- the root-aware config cache.

`run()` sets that global once and intentionally does not reset it because the
CLI exits after one run. Two concurrent `run()` calls in one Node process can
therefore redirect each other's config, state, Git commands, and runner cwd.
Some inherited provider commands currently bypass that default entirely; those
call sites also need an explicit run cwd before managed execution can rely on
checkout isolation.

### 4.2 Durable state is inside a disposable checkout

[`src/state.ts`](../src/state.ts) derives `agentsDir()` from `projectRoot()`.
Deleting a linked worktree therefore deletes the run's state, prompts,
transcripts, metrics, and authored commit/PR text. The existing batch wrapper
compensates by copying those files into a batch archive before removal.

### 4.3 State and agent-writable artifacts share one directory

Review and document phases instruct the coding agent to write
`commit_message.txt` and `pr_body.md` under `state.workspace()`
([`src/phases.ts`](../src/phases.ts)). Moving that workspace wholesale under
the Git common directory or an XDG state directory would put those instructed
paths outside the worktree. Sandboxed runners, especially a workspace-write
backend, cannot be assumed to write there.

Durable control state and agent-writable artifacts must therefore become
separate concepts. An escaping symlink is not an acceptable substitute because
it weakens the worktree sandbox and is not portable.

### 4.4 Resume trusts state without proving checkout identity

A resumed run skips `setup` once it appears in `completed_phases`. It does not
verify that:

- the current repository is the repository that created the state;
- the current worktree is the registered worktree generation;
- `HEAD` is attached to `state.branch_name`;
- the branch is not checked out in another worktree; or
- a merge, rebase, or cherry-pick is not in progress.

The fresh-run dirty check is deliberately bypassed on resume because prior
uncommitted edits are expected. That is correct only when the dirty tree is the
same owned tree whose state is being resumed.

### 4.5 Existing branch reuse is ownership-blind

`createOrCheckoutBranch()` switches to any same-named local branch. The push
path's force-with-lease safety explicitly assumes that each ADW branch has one
writer, but no lease or manifest enforces that assumption. Managed mode must
never adopt a branch merely because its name matches.

### 4.6 The merge tail assumes a single checkout

After a successful remote merge, `pullRebase()` attempts `git switch <base>` in
the issue checkout. In a linked-worktree layout, the base is normally checked
out in the primary checkout, so the switch fails. The caller currently ignores
that result.

There is also a time-of-check/time-of-use gap: the last base sync happens before
interactive confirmation, and another lane can merge before the actual merge
command runs.

### 4.7 Exit code does not describe the terminal outcome

Return code `0` can currently mean:

- the run merged;
- `--no-merge` left a green change request open;
- the work item was already closed and skipped; or
- a resumed run found that merge was already recorded.

A worktree manager cannot decide retention or cleanup from the exit code alone.

## 5. Design invariants

The implementation must preserve these invariants:

1. One ADW run ID owns at most one live worktree generation and one branch.
2. Only one process lease may execute or resume a run ID at a time.
3. Different work items may run concurrently; duplicate active runs for the
   same repository/provider/work-item tuple are rejected by default.
4. Fresh branch, state, registry, or path collisions fail. Reuse requires an
   exact matching ownership record.
5. Every runner, Git command, provider command, and local gate receives an
   explicit per-run cwd; no managed-run effect depends on ambient cwd.
6. The primary checkout's current branch and working tree are never mutated by
   a managed lane.
7. A completed phase may be skipped only in the same validated worktree
   generation or from a future durable filesystem checkpoint.
8. Agent-writable lifecycle artifacts are contained inside the worktree but
   proven untracked and ignored before execution; commit staging never includes
   them.
9. State used for lifecycle or resume is written atomically. Corrupt state is a
   hard error, not equivalent to missing state.
10. Failure, interruption, conflict, or uncertain remote state retains the
   worktree. Uncertainty never triggers automatic deletion.
11. Automatic cleanup never uses `--force` and touches only manager-owned
    registrations under the configured managed root.
12. Merge is allowed only after gates and CI were proved against the base the
    lane is about to merge into, within the guarantees offered by the selected
    forge/provider.
13. Existing non-managed and external `--project-root` workflows retain their
    current behavior.

## 6. Per-run context and path model

Introduce an immutable context assembled before any run-specific effect:

```ts
interface RunContext {
  packageRoot: string;
  sourceRoot: string;
  worktreeRoot: string;
  projectRoot: string;
  stateRoot: string;
  artifactRoot: string;
  gitCommonDir: string;
  mode: 'primary' | 'external' | 'managed';
}
```

Meanings:

- **packageRoot** — the installed Switchyard kernel and bundled fallback
  prompts/schemas; today's `REPO_ROOT`.
- **sourceRoot** — the operator-selected project in the primary checkout. It is
  used for repository discovery and managed-worktree provisioning, not agent
  edits.
- **worktreeRoot** — the Git top-level of the linked worktree.
- **projectRoot** — the project directory inside the linked worktree. For a
  monorepo, preserve `sourceRoot`'s path relative to the source Git top-level
  instead of treating the worktree top-level as the project root.
- **stateRoot** — durable control-plane state and observability.
- **artifactRoot** — an ignored directory inside the worktree where a sandboxed
  agent may author commit/PR text and any future agent-owned artifacts.
- **gitCommonDir** — canonical shared Git metadata directory and repository
  identity anchor.

Recommended managed-mode defaults:

- durable registry/control state under `<git-common-dir>/switchyard/`;
- linked worktrees under a configurable sibling or operator state root, keyed
  by repository identity and ADW ID; and
- agent-authored files under the linked worktree's ignored
  `agents/<adw-id>/` execution mirror.

`artifactRoot` is a safety boundary, not a naming convention. Before any agent
runs, the manager must canonicalize it beneath `worktreeRoot`, create it, prove
that no target artifact path is tracked, and prove representative target files
are ignored (for example with `git ls-files` and `git check-ignore --no-index`).
Failure is a hard preflight error. Commit staging must also assert that nothing
under the owned artifact root entered the index and abort if that invariant is
violated; it cannot rely solely on every target repository having an
`agents/` ignore rule.

The exact durable-state root remains configurable because some operators may
prefer `$XDG_STATE_HOME` or a platform equivalent. It must not live under a
directory that routine cache cleanup can erase.

Legacy mode maps `stateRoot` and `artifactRoot` back to today's
`<projectRoot>/agents/<adw-id>` layout.

## 7. Durable registry and state

### 7.1 Registry record

Use a separately versioned, load-bearing registry record rather than making
new top-level fields in the cross-language `state.json` contract authoritative.
A record should contain at least:

```text
schema version
ADW run ID
repository identity (canonical common Git dir + provider repo locator)
work-item provider and exact ID
source root and project-relative path
managed worktree path and generation ID
branch, base, base/head OIDs when known
lifecycle state and structured run outcome
runner/kernel version or commit
process ID, host, lease ID, start time, heartbeat
change-request ID/URL/head and merge intent/outcome
retention/cleanup disposition
created/updated timestamps
```

Suggested lifecycle states:

```text
provisioning
├── skipped-closed → cleaned | retained
└── ready → running
            ├── merged → cleanup-needed → cleaned
            ├── pr-ready → retained
            ├── skipped-closed → cleaned | retained
            ├── failed → retained
            └── interrupted → retained
```

### 7.2 Atomicity and error handling

Managed lifecycle records and resume state must use unique temporary files plus
atomic rename, following the pattern metrics already use. A write failure must
fail the managed run safely and retain its checkout; it cannot be swallowed.

Readers distinguish:

- missing state — potentially a never-started/provisioning run;
- corrupt state — fail closed and require repair; and
- a valid but stale lease — mark interrupted after host/PID policy allows the
  lease to be reclaimed.

Only one per-run lease holder may write state. The registry/lifecycle lock is
separate from Git's own ref locks.

### 7.3 State versus agent artifacts

Control-plane state, prompts, metrics, and parent-written transcripts may live
in the durable state root. Paths explicitly handed to the coding agent must
remain under `projectRoot`/`artifactRoot`.

For review/document authoring:

1. The prompt names worktree-local artifact paths.
2. The parent absorbs their contents into `AdwState` as it does today.
3. The parent atomically checkpoints the updated state.
4. A merged cleanup may then remove the worktree without losing the authored
   text or run history.

If the process dies before the parent absorbs the files, the retained worktree
remains the recovery source and the incomplete phase runs again.

### 7.4 History transition checkpoints

The registry records both operation intent and result around every
history-changing transition: initial branch attachment, ordinary/finalize/CI-fix
commits, rebase, push or force-push, and merge. The result checkpoint includes
the current local branch OID and, when applicable, the observed remote branch
OID and tested base OID.

If the process crashes between intent and result, resume reconciles that named
transition from Git and provider state before accepting a new head. An
unexplained head change is an ownership violation; a stale checkpoint must not
make a legitimate completed rebase look foreign, and a branch name alone must
not make a foreign rewrite look owned.

## 8. Worktree and branch lifecycle

### 8.1 Fresh allocation

The parent/supervisor performs this sequence:

1. Resolve and validate the source repository, remote, provider locator, and a
   provisional base from explicit input or project configuration without
   mutating the primary checkout.
2. Mint an ADW ID before choosing the path or branch. Check that the ID does not
   collide with registry records, state, managed paths, or refs.
3. Write an atomic branch-unset `provisioning` intent under the repository
   lifecycle lock, then release the lock.
4. Fetch `origin/<base>` without holding the lifecycle lock.
5. Reacquire the lifecycle lock, allocate the managed worktree detached at the
   fetched OID, verify its top-level/common Git dir/detached head/clean status
   and project-relative root, checkpoint the allocation, then release the lock.
6. Load the authoritative configuration from that detached lane, confirm that
   its base agrees with the provisional base, fetch one work-item snapshot, and
   derive the deterministic branch name exactly once. A mismatch fails closed
   instead of silently mixing dirty/older primary-checkout configuration with
   the lane. If the work item is already closed, persist `skipped_closed` and
   use the pristine-cleanup rule in section 11.
7. Under the lifecycle lock, prove that the derived branch/ref is unowned and
   absent, attach it with `git switch -c <branch>`, and atomically checkpoint
   the branch, base/head OIDs, config digest, work-item snapshot, and worktree
   generation.
8. Prove the `artifactRoot` containment/tracked/ignore invariants, then lock the
   worktree with a reason such as `switchyard:<adw-id>` so generic Git pruning
   does not discard a retained lane.
9. Mark the registry record `ready`, release the repository lifecycle lock, and
   spawn the worker with that immutable setup snapshot.

Managed worker setup consumes and verifies the registered branch/config/work-item
snapshot. It must not re-fetch branch-driving data, re-derive the branch, or
call the ownership-blind legacy `createOrCheckoutBranch()` path. Provider status
can be refreshed at explicitly named race boundaries, but a changed title or
label never silently changes the already-owned branch.

The worker receives an internal recursion guard plus explicit roots. It must not
interpret its own managed worktree as another request to allocate a worktree.

`--dry-run` performs validation and prints the proposed policy/path/branch but
does not mint a durable ID, write registry state, create a ref, or add a
worktree.

### 8.2 Resume and reuse

`git worktree list --porcelain` is the authoritative source for Git
registrations; the Switchyard registry supplies ownership metadata. Directory
existence alone is never sufficient.

Before spawning a resumed worker, validate:

- the canonical common Git dir matches the registry;
- the worktree path is exactly the registered managed path;
- the path is a Git worktree for that common dir;
- `HEAD` is attached to the saved branch once setup is complete;
- the local branch resolves to the expected head or an explicitly reconciled
  successor;
- no other worktree has the branch checked out;
- no Git operation is in progress; and
- the per-run lease can be acquired.

Dirty/untracked files are preserved on a valid resume. A detached HEAD is
acceptable only during an explicitly recorded provisioning state. It is a hard
error after setup.

The "expected head" is the last completed history-transition checkpoint from
section 7.4, not merely the OID recorded at allocation. An in-progress intent
must be reconciled before the lease is granted.

If a registered directory is missing:

- with no completed editing phase and a safe known ref, the manager may
  reconstruct the lane explicitly on that ref;
- with completed but uncheckpointed editing phases, fail closed and present
  recovery/fresh-run options; and
- never recreate it detached at `origin/<base>` while leaving setup marked
  complete.

### 8.3 Branch collision policy

- A fresh run never switches to an existing same-named branch.
- A matching registry record may reuse its branch only for resume.
- A branch checked out elsewhere reports the owning path and refuses to run.
- A remote same-named branch without matching local ownership is foreign and
  causes a collision error.
- The existing `--force-with-lease` behavior remains, but the lease is now
  backed by enforced one-writer ownership.
- Two live runs for the same work item are rejected by default. An eventual
  `--allow-duplicate-run` must be explicit because it can create duplicate PRs
  and competing status transitions.

## 9. Locking and concurrency

Use distinct locks for distinct scopes:

| Lock | Lifetime | Protects |
| --- | --- | --- |
| Per-run execution lease | full worker lifetime | duplicate start/resume and concurrent state writers |
| Repository lifecycle lock | short critical sections | allocate/register/remove/repair and local ref deletion |
| Shared Git-metadata lock | individual operations where needed | manager and worker fetch/ref operations beyond Git's own retry behavior |
| Merge lock | final checked sequence only | sibling Switchyard merges between final base proof and merge |

Do not hold a repository lock over agent phases, gates, CI polling, or the full
worker lifetime. That would make worktrees nominally parallel but operationally
serial.

The existing bounded fetch retry remains useful even with manager locks because
other Git processes and external tools can touch the shared metadata.

### 9.1 Resource concurrency

A later batch supervisor should expose `--jobs <n>` with a conservative
default, not an unbounded `Promise.all`. The field-proven wrapper used a cap and
staggered starts because lanes share:

- one Git metadata store;
- API/provider rate limits and subscriptions;
- cold dependency/build work;
- global caches; and
- machine CPU, memory, and disk.

The worktree manager may inject a non-secret run identifier into orchestrator
gate preparation, but it must not relax the runner environment/secret boundary.

## 10. Merge correctness and recovery

### 10.1 Existing base-freshness defense

Keep the current sequence that:

1. commits the lane;
2. fetches and rebases onto `origin/<base>` if needed;
3. re-runs pre-merge gates after a real rebase;
4. force-pushes with lease when history diverged;
5. watches/fixes CI; and
6. rechecks the base after the CI window.

That path already handles most cross-lane movement and should stay in the
kernel rather than moving into a shell supervisor.

### 10.2 Final merge serialization

Move confirmation before the final protected sequence. After confirmation and
green CI:

1. acquire the repository's merge lock;
2. fetch the base and compare it with the base/head pair whose gates and CI are
   green;
3. if the base moved, release the lock, rebase/re-gate/re-push/re-watch, then
   retry;
4. persist a merge intent containing change-request identity and head/base OIDs;
5. merge; and
6. persist the provider-confirmed outcome before releasing the lock.

The local merge lock serializes Switchyard-managed lanes only. An external
actor can still merge in the final window. Stronger protection requires a
forge merge queue, strict up-to-date branch policy, or a provider operation that
supports an expected-head/base precondition. The implementation must document
which guarantee the configured provider actually offers.

### 10.3 Crash after remote merge

The remote merge currently occurs before `merge` is saved locally, and existing
PR discovery searches only open PRs. On startup/resume, a run with merge intent
or a saved change-request ID must query provider-neutral change-request status,
including merged/closed states, and verify the recorded head before deciding to
push, create another PR, or clean up.

For GitHub, the managed path also requires a non-empty explicit/detected
repository locator. The live wrapper established this as load-bearing for
`gh pr merge --delete-branch` inside a linked worktree.

### 10.4 Primary checkout after merge

Managed mode replaces the current `switch base; pull --rebase` tail with a
fetch-only refresh of remote refs. It may report that the developer's primary
branch is behind, but it never updates that checkout automatically.

## 11. Cleanup, stale worktrees, and force policy

### 11.1 Automatic cleanup eligibility

Automatic removal requires one of these terminal proofs:

- **Merged:** structured outcome is `merged`, the provider confirms the saved
  change request is merged, and the saved request head OID, provider-confirmed
  request head OID, and current local branch OID are exactly equal.
- **Skipped closed:** structured outcome is `skipped_closed`, the provider
  confirms the work item is closed, no editing phase began, `HEAD` still equals
  the recorded allocation/base OID, and the run created no change request or
  remote branch. A close discovered before allocation creates no worktree.

Both proof classes also require that:

- the execution lease is released and no supervised descendant remains;
- the registered path/common-dir/branch (when a branch was attached) still
  match ownership;
- no rebase/merge/cherry-pick is active; and
- tracked and untracked status is clean.

Ignored build outputs or secrets may still cause normal removal to fail even
when porcelain appears clean. That is `cleanup-needed`, not permission to force.

Because managed and retained worktrees are Git-locked, cleanup runs under the
repository lifecycle lock and rechecks the proof immediately before mutation.
It unlocks only the exact worktree with the matching Switchyard lock reason,
then attempts normal, non-force `git worktree remove`. After successful removal
it deletes the local branch, if one was attached, only for the proven merged or
pristine-skipped run; durable state and the terminal registry outcome remain
for observability. If normal removal fails, the manager re-locks the
still-present worktree when possible, marks it `cleanup-needed`, and reports
the reason. It never escalates automatically to force removal.

### 11.2 Retention policy

Default retention:

| Outcome | Default |
| --- | --- |
| merged and clean | remove worktree; retain durable record/artifacts |
| closed before allocation | record/return skipped; no worktree to retain |
| `skipped_closed` after allocation | remove only when pristine proof passes; otherwise retain |
| `--no-merge` / PR ready | retain |
| agent/gate/CI failure | retain |
| timeout/cancellation/process crash | retain |
| rebase conflict | retain for salvage |
| remote/provider state unknown | retain |
| foreign or ownership mismatch | never touch |

### 11.3 Reconciliation states

On `list`, `resume`, and explicit repair:

| Registry | Git registration | Directory | Action |
| --- | --- | --- | --- |
| present | present | present | validate ownership and report/reuse |
| present | present/prunable | missing | mark orphaned; explicit recreate or repair |
| absent | present | any | foreign; read-only |
| present | absent | present | quarantine; require explicit adopt/repair |
| present | absent | missing | if provisioning was interrupted, retry only when refs and recorded OIDs prove no foreign/partial branch; otherwise retain the record and require repair |
| present with stale lease | present | present | mark interrupted; retain and allow audited lease recovery |

Never run global `git worktree prune` automatically. A `prune` command previews
managed reconciliation and touches only records whose ownership is proven.

### 11.4 Explicit force cleanup

Force cleanup is a separate, confirmed operator action. Before destructive
removal it should:

1. show dirty/untracked/in-progress/ahead/open-PR conditions;
2. archive durable state;
3. capture a recovery patch or Git bundle when possible;
4. report the recovery path; and
5. unlock/remove only the exact canonical managed path.

No recursive deletion may target an unresolved environment variable, symlinked
escape, source checkout, workspace root, or path outside the configured managed
worktree parent.

## 12. Process and cancellation lifecycle

The supervisor owns one worker process group per run.

- First `SIGINT`/`SIGTERM`: atomically mark `cancelling`, signal the whole worker
  process tree, wait a bounded grace period, then escalate to kill.
- Second signal: immediate escalation.
- After exit: verify descendants are gone, mark `interrupted` unless a stronger
  terminal outcome was persisted, release the execution lease, and retain the
  worktree.
- Runner `stop()` methods must be idempotent, await actual process exit, and not
  mask the primary failure.

Today's CLI does not bridge OS signals into the phase `AbortSignal`, and local
gates use synchronous process execution. Safe background/TUI cancellation
eventually requires cancellable asynchronous gate execution as well as parent
process-group supervision.

## 13. Structured outcome and public API

Add a detailed outcome API without breaking the existing numeric return:

```ts
type RunOutcomeKind =
  | 'merged'
  | 'pr_ready'
  | 'skipped_closed'
  | 'failed'
  | 'interrupted';

interface RunOutcome {
  kind: RunOutcomeKind;
  adwId?: string;
  workItemId: string;
  branch?: string;
  changeRequestId?: string;
  changeRequestUrl?: string;
  error?: string;
}
```

Recommended compatibility shape:

- new `runDetailed(...): Promise<RunOutcome>` for the managed worker;
- existing `run(...): Promise<number>` remains a wrapper for current consumers;
- worker persists its outcome atomically before exit; and
- the manager makes cleanup decisions from the persisted outcome plus fresh
  Git/provider checks, never exit code alone.

Provider changes required for robust reconciliation:

- query a saved change request in open/closed/merged states;
- return status and head identity;
- expose provider-supported conditional/queued merge capability; and
- keep GitHub-specific `gh` behavior inside the provider adapter.

The Git VCS provider should be constructed with an explicit run context/cwd or
accept explicit checkout handles. Worktree provisioning itself belongs in a
Git-specific `WorktreeManager` above the universal phased kernel, not in a
generic work-item provider.

## 14. CLI surface

Initial opt-in commands:

```text
adw-sdlc issue <work-item-id> --worktree
adw-sdlc issue <work-item-id> --worktree --worktree-root <dir>
adw-sdlc issue <work-item-id> --worktree --resume --adw-id <id>

adw-sdlc worktree list
adw-sdlc worktree status <adw-id>
adw-sdlc worktree remove <adw-id>
adw-sdlc worktree prune --dry-run
```

Later, after the lifecycle is proven:

```text
adw-sdlc batch <work-item-id>... --jobs <n>
```

Semantics:

- `--worktree` opts into managed ownership.
- Without `--worktree`, `--project-root` and `ADW_PROJECT_ROOT` retain today's
  external-checkout semantics exactly.
- With both, user-facing `--project-root` identifies the source project; the
  internal child receives the allocated lane's project root and a recursion
  guard.
- Managed resume requires `--worktree` in the first release. It resolves the
  source repository from explicit `--project-root` or the current directory's
  canonical common Git dir, then searches only that repository's registry. It
  never scans machine-wide records by bare ADW ID.
- Without `--worktree`, `--resume --adw-id` retains today's legacy/external
  lookup under the selected project root and never silently switches to a
  managed record. A same-repository ID whose legacy and managed ownership
  metadata conflict fails closed and names both candidates.
- `--dry-run` prints source root, proposed managed root, branch, base, retention
  policy, and concurrency policy without mutation.
- `worktree list/status` support a stable `--json` form for the cockpit and
  automation.
- `remove` refuses active, dirty, foreign, ambiguous, or unmerged lanes with
  run-owned work; the pristine `skipped_closed` proof is the only no-merge
  exception.
- An eventual `adopt` is explicit and validates every ownership invariant; it
  is never performed as a side effect of `list` or `resume`.

Machine-local path defaults belong in CLI/env or local Git-common-dir config,
not committed `.adw/config.json`. Project-level preparation policy may live in
the trusted project pack because project commands already execute with that
trust level.

## 15. Cockpit/UI changes

The Pi cockpit currently reads config, Git status, and `agents/` only from
`ctx.cwd`. A registry-backed view should show all managed runs for the current
repository:

- ADW ID and work-item ID/title;
- running/retained/stale/cleanup-needed state;
- runner, phase, cost, and last heartbeat;
- branch, worktree path, clean/dirty/detached status;
- change-request URL/status; and
- the safe resume or cleanup action available.

Suggested rollout:

1. Read-only multi-worktree list/detail view.
2. Insert registry-aware resume commands.
3. Guarded start in managed mode.
4. Separately confirmed cleanup/repair actions.

Externally managed worktrees remain read-only in the cockpit unless explicitly
adopted. Ambient dashboard refresh never creates, removes, repairs, or prunes a
worktree.

## 16. Backward compatibility and migration

- Managed mode is opt-in initially.
- Preserve `--project-root`, `ADW_PROJECT_ROOT`, branch naming, and the current
  default `agents/<adw-id>/` location for non-managed runs.
- Keep `state.json`'s existing cross-language fields and schema version.
  Optional worktree metadata may be added only as advisory data; load-bearing
  ownership lives in the separately versioned registry.
- An old state file without registry metadata resumes only through legacy or
  explicitly supplied external-root behavior.
- Never move old state or claim old worktrees automatically.
- A future `worktree adopt` may import an external worktree after checking repo,
  branch, state, dirty status, and active processes.
- Keep `run-batch-parallel.sh` as an operational compatibility example until
  native managed mode reproduces its guarantees in automated and live evidence;
  deprecate it explicitly afterward.
- Public `run()` and current provider interfaces retain compatibility wrappers
  while explicit-context/detailed APIs are introduced.

## 17. Operational risks and mitigations

| Risk | Mitigation |
| --- | --- |
| shared `.git` ref/config lock contention | short lifecycle/metadata locks plus existing bounded Git retry |
| two writers adopt one branch | registry ownership + per-run lease + exact branch/worktree validation |
| deleted worktree loses completed uncommitted edits | retain by default; generation binding; fail closed until filesystem checkpoints exist |
| state torn during crash | unique temp + atomic rename; corrupt is an error |
| remote merge lands before local state save | merge intent + provider status/head reconciliation |
| base moves between CI and merge | confirmation first; merge lock + final base check; provider queue/CAS where available |
| primary checkout branch is already checked out | never switch/pull it from a lane; fetch-only post-merge |
| agent cannot write durable state path | keep agent-authored paths under worktree artifact root |
| process termination leaks runner descendants | supervised process groups, bounded TERM→KILL, awaited runner stop |
| new worktree lacks ignored dependencies/config | optional explicit preparation command; never copy untracked files or secrets automatically |
| tests/services collide on ports/global resources | bounded jobs; project-supplied per-run configuration; worktrees alone are not claimed as full isolation |
| disk use grows from retained/cold worktrees | list/status disk visibility; explicit cleanup; configurable retention alerts, never age-only deletion |
| submodules/LFS/sparse checkout/hooks differ | preflight and documented policy; cover supported modes with real integration tests |
| resumed lane uses a different kernel fallback revision | record kernel version/commit and warn/fail according to compatibility policy |
| path/symlink mistake makes cleanup destructive | canonical containment validation and exact managed-root ownership before any removal |

## 18. Test strategy

### 18.1 Unit tests

- Parse `git worktree list --porcelain`: attached, detached, locked, prunable,
  paths with spaces/unicode.
- Registry schema, lifecycle transitions, repo/path/branch validation, and
  monorepo-relative project roots.
- Atomic state/registry writes, corrupt-state failure, and unique temp names.
- Artifact-root containment plus tracked/ignore preflight, including a target
  repository with no `agents/` ignore rule and a staged-artifact refusal.
- Per-run/repository/merge lock acquisition, release, stale-lease rules, and
  exactly-one-winner behavior.
- Fresh collision versus exact owned resume.
- Cleanup decision table: clean, dirty, untracked, ignored-output, ahead,
  in-progress Git operation, open PR, merged PR, foreign registration, force.
- Structured outcome mapping for merged, PR-ready, skipped, failed, and
  interrupted runs.
- Idempotent reconciliation and cleanup.

### 18.2 Real-Git integration tests

Use a bare origin, one primary checkout, and at least two actual linked
worktrees—not two independent clones:

- allocate two issue branches concurrently without cross-contamination;
- prove the primary checkout never changes branch or working-tree state;
- commit and push both lanes independently;
- reject a branch checked out in another worktree;
- handle locked, missing-but-registered, registered-but-missing, and foreign
  worktrees;
- retain a conflicted rebase in a clean, salvageable state;
- preserve state after a merged worktree is removed;
- resume the exact attached branch and reject detached/wrong-branch resume;
- finish merge cleanup without attempting to switch the lane to `main`; and
- reject simultaneous resume of one run while allowing different runs.

### 18.3 Orchestrator and sandbox integration tests

- Every normal, loop, gate-heal, patch, and CI-fix agent call receives the
  correct explicit worktree cwd.
- Sandboxed runners can write review/document artifacts under `artifactRoot`
  while durable state lives elsewhere.
- Completed phases are tied to the expected worktree generation.
- A missing worktree with completed uncommitted edits refuses unsafe resume.
- Legacy `agents/` and external `--project-root` behavior remains unchanged.
- `runDetailed` and numeric `run` wrappers agree on compatibility semantics.

### 18.4 Fault-injection and process tests

Inject termination after:

- registry intent;
- worktree add;
- branch setup;
- phase checkpoint;
- commit;
- push;
- change-request creation;
- remote merge; and
- cleanup start.

After every injection, state must be parseable, ownership unambiguous, no lease
falsely live, and recovery deterministic.

Use a helper that spawns grandchildren to prove cancellation leaves no worker,
runner, server, or gate descendants. Cover first/second signal behavior and
TERM→KILL escalation.

### 18.5 Platform and project-shape coverage

- Linux, macOS, and Windows path/process behavior.
- Case-insensitive filesystems, long IDs, spaces/unicode, and symlinks.
- Dirty source checkout with a clean managed lane.
- Non-`main` default bases.
- Monorepo subdirectories.
- Submodules, LFS, sparse checkout, and checkout hooks according to the declared
  support policy.
- Disk exhaustion during provisioning and state checkpoint.
- Missing ignored dependencies and explicit preparation behavior.

## 19. Implementation map

Expected new modules:

| Component | Responsibility |
| --- | --- |
| `src/run-context.ts` | immutable package/source/worktree/project/state/artifact/common-Git roots |
| `src/run-registry.ts` | atomic lifecycle records, structured outcomes, leases, and reconciliation metadata |
| `src/worktree-manager.ts` | Git-specific allocate/validate/list/unlock/remove/repair operations |
| `src/run-supervisor.ts` | worker spawning, process-group signals, heartbeat, outcome collection, and bounded jobs |

Expected existing touchpoints:

| File/component | Change |
| --- | --- |
| `src/common.ts` | retain compatibility accessors while managed paths move to explicit `RunContext` |
| `src/config.ts` | load/cache config by explicit project root rather than mutable process-global state |
| `src/state.ts` | explicit durable state store, atomic managed checkpoints, corrupt-vs-missing reads, generation binding |
| `src/metrics.ts` | route metrics through the explicit durable state root |
| `src/phases.ts` | route agent-authored commit/PR paths through worktree-local `artifactRoot` |
| `src/exec.ts` | explicit cwd and eventually cancellable asynchronous gates |
| `src/run-phase.ts` | require/pass the worktree cwd instead of relying on the global default |
| `src/git.ts` | ownership-aware branch validation, worktree-safe post-merge behavior, OID/status helpers |
| `src/providers.ts` | context-bound VCS operations and provider-neutral merged/status/head reconciliation |
| `src/orchestrator.ts` | detailed outcomes, managed resume preflight, merge intent/lock/reconciliation, explicit context threading |
| `src/cli.ts` | managed flags, worker recursion guard, worktree subcommands, and later batch dispatch |
| `src/index.ts` | export new context, outcome, registry, manager, and supervisor APIs |
| `.pi/extensions/adw-cockpit/index.ts` | registry-backed multi-run visibility and guarded lifecycle actions |
| `adw/state.schema.json` | optional advisory metadata documentation only; no managed ownership fields become v1 load-bearing |
| `test/` | real linked-worktree, ownership, recovery, cancellation, compatibility, and fault-injection suites |

Runner adapters should require only targeted lifecycle hardening: they already
consume an explicit `PhaseRequest.cwd`. Each worker still owns a distinct
runner instance, so no adapter is shared across worktrees.

## 20. Phased rollout

### Phase 0 — safety foundation

- Introduce `RunContext` and explicit cwd/state/artifact seams while retaining
  legacy wrappers.
- Make managed state/registry writes atomic and distinguish missing from corrupt.
- Add structured outcomes and per-run leases.
- Validate branch/worktree/repository identity on resume.
- Remove/suppress worktree-unsafe post-merge base switching.
- Add real linked-worktree regression coverage.

Exit criterion: the current externally managed `--project-root` flow works with
explicit context and no behavior regression; unsafe resumes fail loudly.

### Phase 1 — one managed worktree

- Add registry, `WorktreeManager`, fresh allocation, exact reuse, list/status,
  conservative explicit removal, and repository lifecycle/merge locks.
- Run each lane in a supervised process group with a recursion guard, signal
  forwarding, bounded TERM→KILL escalation, descendant-exit verification, and
  no lease release while a worker or runner may still be editing.
- Keep managed mode opt-in. Until Phase 2 reconciliation is proven, retain
  terminal lanes by default and expose only proof-checked explicit removal.

Exit criterion: one issue can be started, interrupted, inspected, resumed, and
merged without touching the primary checkout; merged cleanup preserves durable
history when explicitly requested. Independently launched managed lanes
serialize shared mutations and merge correctly, and cancellation leaves no
editing descendant.

### Phase 2 — recovery and automatic cleanup

- Harden runner-specific idempotent shutdown and add cancellable asynchronous
  gates while retaining the Phase 1 process-group backstop.
- Add merge intent/provider reconciliation, including crash-after-merge.
- Add stale lease/registration repair and safe automatic cleanup for proven
  merged and pristine `skipped_closed` outcomes.
- Add optional preparation policy without copying untracked data.

Exit criterion: the fault-injection matrix has deterministic recovery at every
owned transition, and no uncertain state is deleted.

### Phase 3 — bounded multi-run supervisor

- Add `batch ... --jobs <n>` or equivalent multi-run controls.
- Exercise the existing shared-Git/merge locks under aggregate dispatch; add
  status/logging, staggering, and resource caps.
- Support explicit dependency lanes/barriers without inferring dependencies.

Exit criterion: two or more real linked-worktree runs complete concurrently,
including base movement, CI wait, merge serialization, and one retained failure.

### Phase 4 — cockpit and default decision

- Add registry-backed multi-run cockpit views and guarded actions.
- Collect live evidence equivalent to or stronger than the existing batch
  wrapper.
- Decide whether managed worktrees remain opt-in or become the default.
- Explicitly deprecate the hard-coded parallel wrapper only after parity.

## 21. Settled recommendations and open decisions

### Settled by this proposal

- One worker process and one worktree per run initially.
- Worktree path keyed by ADW ID, not raw work-item title.
- Existing ADW-ID branch naming retained.
- Durable control state separated from worktree-local agent artifacts.
- Process-wide repository lock rejected; short scoped locks instead.
- Dedicated merge lock inside finalize.
- Primary checkout never switched or pulled by a managed lane.
- Failure/interruption/conflict/unknown state retained.
- Automatic force removal rejected.
- Existing `--project-root` behavior preserved.

### Still requiring an implementation decision

1. **Durable state location:** Git common dir versus platform state dir.
   Recommendation: Git common dir by default, configurable for operators.
2. **PR-ready retention:** retain indefinitely versus remove after proving all
   work is committed/pushed. Recommendation: retain initially.
3. **Duplicate work-item runs:** hard prohibition versus explicit override.
   Recommendation: reject by default; add an explicit override only with a
   concrete use case.
4. **Missing-worktree recovery:** fail closed versus phase-level filesystem
   checkpoints/hidden refs. Recommendation: fail closed in the first release.
5. **External merge guarantees:** require branch protection/merge queue versus
   provider-specific conditional merge. Recommendation: expose provider
   capability and fail/document honestly when only local serialization exists.
6. **Preparation policy:** CLI-only command versus trusted project-pack field.
   Recommendation: project-pack command with CLI override, never implicit
   copying of ignored/untracked files.
7. **Managed-mode default:** opt-in versus default. Recommendation: opt-in until
   linked-worktree, fault-injection, and live-run evidence is complete.
8. **In-process API concurrency:** retain process isolation indefinitely versus
   finish the explicit-context refactor for `Promise.all(runDetailed(...))`.
   Recommendation: defer until a real embedding needs it; do not let it block
   the process-per-run product path.
