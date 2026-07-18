---
type: Security Architecture
title: Security boundaries
description: The authority split and deny-by-default process boundary that keep forge secrets away from coding agents.
tags: [security, secrets, environment, git, github]
timestamp: "2026-07-18T13:26:10Z"
---

# Authority split

```text
control plane: work-item credentials + Git + change requests + CI + merge
runner child:  selected worktree + phase prompt + explicit safe environment
```

In phased mode, the orchestrator owns Git and forge work. Agent prompts prohibit Git and forge commands, runner-specific controls reinforce that policy, and the agent process does not receive `GH_TOKEN`.

# Environment boundary

`safeSubprocessEnv()` constructs a new environment from an explicit base and runner-specific allowlist. It does not copy the parent environment wholesale.

Always-denied control-plane prefixes include `MATRIX_`, `ADW_`, and legacy `MX_AGENT_`. GitHub authority is withheld in phased mode. Unknown future environment variables are withheld because absence from the allowlist is the default.

`--inherit-env` is an explicit isolation opt-out. It can expose forge credentials, control variables, and unrelated parent secrets to the runner and should not be used for unattended operation.

# Defense in depth

| Control | What it checks |
| --- | --- |
| Environment builder | Only expected base and selected-runner keys enter the child. |
| Static environment lint | Blocks `process.env` spreading and unsafe OpenCode construction/import patterns. |
| Runner tests | Assert adapter-specific environment replacement and tool-control behavior. |
| Prompt preamble | States that Git and forge actions belong to the orchestrator. |
| Audit playbook | Records names and booleans only; it never prints secret values. |

# Residual and trusted surfaces

`HOME` and runner-specific configuration directories may expose credentials already stored on disk; operators can point supported runner directories at scrubbed locations. An external project root is trusted input because its configuration controls commands, prompt roots, and schema paths. Transcripts and run artifacts may contain repository content and must be handled accordingly.

See [troubleshooting](/operations/troubleshooting.md) for safe authentication and boundary diagnostics.

# Citations

[1] [Deny-by-default environment implementation](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/env.ts)

[2] [Static secret-boundary lint](https://github.com/kortiene/switchyard/blob/main/scripts/check-adw-sdlc-env.sh)

[3] [Secret-boundary audit guide](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/docs/SECRET-BOUNDARY-AUDIT.md)

[4] [Environment isolation tests](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/test/env.test.ts)
