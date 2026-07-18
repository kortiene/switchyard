---
type: CLI Reference
title: Command-line interface
description: The supported command families, common run flags, and safety-sensitive CLI behavior.
resource: https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/cli.ts
tags: [reference, cli, commands, flags]
timestamp: "2026-07-18T13:26:10Z"
---

# Entry points

Run commands from `adw_sdlc/`:

```bash
npm run issue -- <work-item-id> --dry-run
npm run issue -- <work-item-id> --runner claude
npm run issue -- <work-item-id> --resume --adw-id a1b2c3d4
```

`issue` is retained as a backward-compatible name even when the configured provider uses a different kind of work item. The TypeScript engine is the default and only bundled engine; selecting `py` fails closed because the Python sibling is not part of this distribution.

# Common run flags

| Flag | Effect |
| --- | --- |
| `--runner <id>` | Select `claude`, `codex`, `opencode`, or `pi`. |
| `--phases <csv>` | Override the configured/default phase order for this run. |
| `--resume --adw-id <id>` | Resume a known run and skip completed phases. |
| `--test-cmd <cmd>` | Set the deterministic repair/finalization test gate. |
| `--project-root <dir>` | Target another trusted repository's config, prompts, state, and worktree. |
| `--timeout <seconds>` | Abort each runner invocation after the limit. |
| `--max-budget-usd <amount>` | Apply the supported native or parent-side run budget behavior. |
| `--no-merge` | Leave a green change request open and resumable. |
| `--dry-run` | Validate and show the plan without running phases or mutating external state. |

Use `npm run issue -- --help` for the complete current flag list.

# Safety-sensitive flags

* `--inherit-env` bypasses the deny-by-default runner environment and can expose parent secrets.
* `--allow-dirty` bypasses the ordinary clean-working-tree precondition.
* `--force` permits work on an already closed work item.
* `--no-verify` skips the post-run closed-state verification.
* `--yes` suppresses merge confirmation and is mutually exclusive with `--no-merge`.

# Managed-worktree commands

The current source also exposes conservative inspection and cleanup commands for opt-in managed runs:

```bash
npm run issue -- worktree list --json
npm run issue -- worktree status <adw-id> --json
npm run issue -- worktree remove <adw-id>
npm run issue -- worktree prune --dry-run
```

See the [managed-worktree playbook](/operations/managed-worktrees.md) before removal.

# Precedence

Explicit CLI values generally override canonical `ADW_*` environment variables, which override validated project-pack defaults. Deprecated `MX_AGENT_*` aliases remain compatibility inputs; conflicting canonical and legacy values fail loudly.

# Citations

[1] [CLI parser and usage text](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/cli.ts)

[2] [CLI behavior tests](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/test/cli.test.ts)

[3] [Environment alias handling](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/env-vars.ts)
