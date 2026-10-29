---
type: Operational Playbook
title: Managed worktrees
description: Inspection, retention, resume, and conservative cleanup for opt-in Switchyard-owned linked worktrees.
tags: [operations, worktree, concurrency, cleanup, recovery]
timestamp: "2026-10-29T01:08:00-04:00"
---

# Status and scope

TypeScript managed-worktree work is stopped (2026-10-29). This page is not an in-development checkout and not a release contract. S3 ([#97](https://github.com/kortiene/switchyard/issues/97)) owns worktree lifecycle natively in Rust as part of the sandbox. The commands below describe the TypeScript checkout as it exists. Verify `npm run issue -- --help` before use.

Managed mode isolates a run from the primary checkout. Durable run/registry data lives below the repository's common Git directory, while agent-writable artifacts remain inside the linked worktree under `agents/<adw-id>/`.

# Start and inspect

```bash
cd adw_sdlc
npm run issue -- <work-item-id> --worktree --dry-run
npm run issue -- <work-item-id> --worktree

npm run issue -- worktree list --json
npm run issue -- worktree status <adw-id> --json
```

Use `--worktree-root <directory>` only with `--worktree`. The manager rejects unsafe parents, paths inside the source checkout, symlink escapes, foreign branch ownership, and unignored artifact paths.

# Retention rules

Retain the lane when a run is failed, interrupted, dirty, conflicted, change-request-ready, or remotely uncertain. Durable state survives normal worktree removal so cleanup does not erase the audit trail.

Automatic or explicit non-force removal is eligible only when:

* the exact local and remote change-request head is authoritatively proven merged; or
* the allocation is pristine and the work item was already closed before meaningful phases ran.

# Resume

```bash
npm run issue -- <work-item-id> --worktree --resume --adw-id <id>
```

Resume validates the durable record, repository identity, configured managed root, registered worktree, branch, cleanliness/operation state, and saved work-item identity. If a merge may have succeeded before a crash, reconciliation queries the remote request and compares exact head identity before continuing or cleaning.

# Cleanup

Preview reconciliation without mutation:

```bash
npm run issue -- worktree prune --dry-run --json
```

Remove one proven-safe lane:

```bash
npm run issue -- worktree remove <adw-id>
```

Removal never uses force. If it refuses, preserve the lane and resolve the stated ownership, cleanliness, operation, or remote-proof condition. Do not run a broad `git worktree prune` or manually delete registry/control files as a substitute for reconciliation.

# Citations

[1] [Managed run context](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/run-context.ts)

[2] [Worktree manager](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/worktree-manager.ts)

[3] [Managed run supervisor](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/run-supervisor.ts)

[4] [Managed-worktree design and recovery model](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/docs/DESIGN-managed-git-worktrees.md)
