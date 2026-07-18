# Switchyard

Switchyard turns a development task—usually a GitHub issue—into a tested,
reviewable pull request. A coding agent plans and edits the code; Switchyard
keeps the workflow moving, saves progress, runs checks, and controls Git,
GitHub, CI, and any optional merge.

> [!IMPORTANT]
> Switchyard is a developer preview. It currently runs from a source checkout:
> the `adw_sdlc` package is private and does not publish a global CLI. Claude
> with GitHub is the current MVP path. Adapters for Codex, OpenCode, and Pi are
> included, but their live-run readiness differs.

## What Switchyard does

Give Switchyard one work item and it will:

1. Read and classify the requested change.
2. Ask the selected coding agent to plan, implement, test, and review it in
   focused stages.
3. Run deterministic project checks and repair loops between agent stages.
4. Create a branch and pull request, then watch and repair CI failures.
5. Save enough state to inspect or resume an interrupted run.
6. Merge only when you explicitly allow it—or leave the pull request open for
   human review.

This split is deliberate: the agent focuses on the code, while deterministic
Switchyard code owns workflow state and forge operations.

## Try it safely

The preview below needs Git, npm, and Node.js `>=20.19`. Node `>=22.19` is
recommended if you want every runner adapter, including Pi. You do **not** need
GitHub or model-provider credentials for the dry run.

```bash
git clone https://github.com/kortiene/switchyard.git
cd switchyard/adw_sdlc
npm ci
npm run issue -- --help
npm run issue -- 123 \
  --runner claude \
  --dry-run \
  --no-merge \
  --test-cmd "npm run verify"
```

The literal issue number `123` is safe here. In dry-run mode, Switchyard does
not look up the issue, load a runner, create run state, change files, call
GitHub, or spend model credits. It validates the local configuration and
prompt assets, then prints the resolved project root, phases, secret policy,
and test gate.

You should see output similar to:

```text
[dry-run] phased run for GitHub issue #123 via claude
[dry-run] project root: /path/to/switchyard
[dry-run] agent env: GH_TOKEN withheld (allowGhToken=false)
[dry-run] test gate: npm run verify
```

A successful preview confirms that the local package and project pack load. It
does not confirm GitHub access, runner authentication, or whether the project's
tests will pass during a live run.

## Run a real issue

A live run is intentionally more demanding. Before starting, make sure you
have:

- An open issue in a repository you are authorized to change.
- An authenticated GitHub CLI (`gh auth status`).
- Access and authentication for the selected runner. For the beginner path,
  authenticate Claude using either its login flow or an Anthropic API key.
- A clean, preferably dedicated target checkout. A live run creates and
  switches branches in that checkout.
- A deterministic test command that is correct for the target repository.
- Approval to spend model-provider credits and run project commands locally.

> [!WARNING]
> A live run can execute commands from the target repository, edit files, post
> issue updates, create and push a branch, open a pull request, watch CI, and
> spend provider credits. Only target repositories you trust.

For an issue in this Switchyard checkout, replace `123` with a real open issue
number and run from `adw_sdlc/`:

```bash
gh auth status
npm run issue -- 123 \
  --runner claude \
  --no-merge \
  --test-cmd "npm run verify" \
  --timeout 1800 \
  --max-budget-usd 10
```

The example runs in the current clean checkout, limits each runner call to 30
minutes, and leaves the pull request open. For Claude, the budget flag sets a
native $10 cap on each agent call; Switchyard also stops before a later phase
once recorded cumulative cost exceeds $10. It is a guardrail, not a guaranteed
$10 ceiling for the whole run. Adjust the timeout and budget deliberately—a
low cap may stop a larger change before completion. The repository's default
test command is empty, so specifying `--test-cmd` is important; otherwise the
deterministic test gate is skipped.

To target another local repository without copying Switchyard into it, add
`--project-root /absolute/path/to/repository` and `--repo owner/repository`,
then replace the test command with that project's real quality gate. The target
repository may provide its own `.adw/` project pack. Because that pack can
define commands and prompt paths, treat it as executable configuration and use
it only from a trusted repository.

See the [package quick start](./adw_sdlc/README.md#quick-start) for all CLI flags,
configuration options, managed-worktree recovery, and runner-specific setup.

## How Switchyard keeps you in control

- **Forge authority stays in the orchestrator.** Switchyard, rather than the
  coding agent, performs Git and GitHub operations.
- **Secrets are denied by default.** Runner processes do not receive
  `GH_TOKEN` in normal phased mode. The advanced `--inherit-env` escape hatch
  weakens this boundary and should be used only when you understand and accept
  the risk.
- **Managed isolation is available.** Advanced users can add `--worktree` to
  give a run its own linked Git checkout. A new linked worktree does not inherit
  ignored dependencies such as `node_modules`, so its test and finalize gates
  must be runnable in that fresh checkout. The exact runner sandbox strength
  still depends on the selected backend.
- **Checks are explicit.** Tests and finalize gates are ordinary deterministic
  commands chosen by the operator or project pack, not claims made by the
  model.
- **Progress is durable.** Run state, transcripts, and metrics are keyed by an
  ADW id so failed or interrupted work can be inspected and resumed.
- **Merge is optional.** `--no-merge` always leaves the pull request open. If
  merge is enabled, Switchyard asks for confirmation unless an experienced
  operator deliberately supplies `--yes`.

## A few useful terms

| Term | Plain-language meaning |
| --- | --- |
| **Work item** | The requested change, usually a GitHub issue. |
| **ADW** | Agentic Developer Workflow: the complete phased run for one work item. |
| **Phase** | One focused stage, such as planning, implementation, testing, or review. |
| **Runner** | The coding-agent backend used for agent phases: Claude, Codex, OpenCode, or Pi. |
| **Orchestrator** | Deterministic Switchyard code that controls phase order, state, commands, Git, and the forge. |
| **Project pack** | A repository's `.adw/` configuration, prompts, and schemas. |
| **Gate** | A deterministic command or condition that must pass before the run advances. |

## How a run flows

The complete default pipeline is:

```text
setup → classify → plan → implement → tests → resolve (loop) → e2e (gated)
      → review → patch (loop) → document (gated) → finalize → ci-fix (loop)
      → merge (optional) → report
```

Agent phases call the selected runner. Setup, finalization, reporting, Git,
forge, and CI coordination remain deterministic kernel responsibilities. Gated
phases can be skipped when they do not apply, and repair phases can loop within
configured limits.

## Runner readiness

The four adapters share one interface, but they should not be assumed to have
identical live reliability.

| Runner | Current role |
| --- | --- |
| **Claude** | Default runner and adopted MVP path; real issue-to-PR evidence is recorded. |
| **OpenCode** | Adapter with recorded real issue-to-PR runs; provider setup differs from Claude. |
| **Codex** | Adapter implemented; the current live ledger records an authentication blocker. |
| **Pi** | Adapter implemented; a complete real-issue run is still owed and Node `>=22.19` is required. |

Use [`adw_sdlc/PARITY.md`](./adw_sdlc/PARITY.md) for the evidence ledger and
[`adw_sdlc/MVP-READINESS.md`](./adw_sdlc/MVP-READINESS.md) for the current
shipping criteria.

## Troubleshooting your first run

- **`package.json` is missing:** commands were run from the repository root.
  Change into `adw_sdlc/` first.
- **The Node version is rejected:** check `node --version`; use Node `>=20.19`,
  or `>=22.19` for Pi.
- **GitHub access fails:** run `gh auth status`, then `gh auth login` if needed.
- **The runner cannot authenticate:** verify the selected provider's login or
  API-key setup before retrying. Claude-specific billing and login behavior is
  documented in the [package troubleshooting guide](./adw_sdlc/README.md#troubleshooting-classify-and-anthropic-api-billing).
- **The checkout is dirty:** use a clean dedicated clone, or commit or stash
  intentional work. Managed `--worktree` mode is another option when the
  project's dependencies and gate commands work in a fresh linked checkout.
- **No deterministic tests run:** pass `--test-cmd` or set `ADW_TEST_CMD`; this
  repository intentionally has no default command configured.
- **A run was interrupted:** repeat the original command with `--resume` and
  `--adw-id` using the id printed by the run. Include `--worktree` again if the
  original run used managed-worktree mode.

## ADW Cockpit (optional)

The [ADW Cockpit](./.pi/extensions/adw-cockpit/index.ts) is an optional
[Pi](https://github.com/earendil-works/pi) terminal dashboard. Its inspection
views are read-only by default and summarize configuration, Git state, recent
runs, and pipeline progress. `/adw-run` is the explicit state-changing
exception: it asks for confirmation and delegates execution to the
orchestrator, which continues to own Git and forge operations.

Useful slash commands include `/adw-menu`, `/adw-runs`, `/adw-config`, and
`/adw-mvp`. Design context lives in [`.impeccable.md`](./.impeccable.md).

## Repository guide

There is no root `package.json`; the Node package and all npm scripts live in
`adw_sdlc/`.

| Path | What you will find there |
| --- | --- |
| [`adw_sdlc/`](./adw_sdlc/) | The main package: CLI, orchestration kernel, runner adapters, tests, and detailed docs. |
| [`.adw/`](./.adw/) | Switchyard's project pack: configuration, generated runtime prompts, and schemas. |
| [`wiki/`](./wiki/) | The OKF v0.1 knowledge bundle for architecture, workflows, operations, and decisions. |
| `agents/` | Git-ignored run state, prompts, transcripts, and metrics; created by live runs. |
| [`.pi/`](./.pi/) | Pi prompts and the optional ADW Cockpit extension. |
| [`.claude/`](./.claude/) | Claude command templates mirrored from the canonical Pi prompts. |
| [`adw/`](./adw/) | Cross-language state schema and fixtures. |
| [`scripts/`](./scripts/) | Repository-level validation scripts. |
| [`specs/`](./specs/) | Implementation specifications used to develop the repository. |
| [`.github/workflows/`](./.github/workflows/) | Continuous-integration workflows. |

## Development and quality gate

From `adw_sdlc/`, the canonical local and CI check is:

```bash
npm run verify
```

It runs, in order:

```text
typecheck → lint:env → pack:check → mirror:check → wiki:check → coverage → build → clean
```

## ADW Cockpit (TUI)

The project's only graphical surface is the **ADW Cockpit**, a read-only-by-default
[Pi](https://github.com/earendil-works/pi) TUI extension at
[`.pi/extensions/adw-cockpit/index.ts`](./.pi/extensions/adw-cockpit/index.ts).
It observes config, git, and `agents/*/state.json` to render a mission-control
dashboard (overview, latest run, pipeline). Its sole state-mutating entry point,
`/adw-run`, requires explicit confirmation and delegates to the orchestrator,
which owns all git and forge mutations.

Slash commands include `/adw-menu`, `/adw-runs`, `/adw-config`, `/adw-mvp`, and
`/adw-run`. Design rationale is in [`.impeccable.md`](./.impeccable.md).

## Quality gate

`npm run verify` (from `adw_sdlc/`) is the canonical local **and** CI quality
gate. It runs every check in order and fails fast:

```bash
cd adw_sdlc
npm run verify   # typecheck → lint:env → pack:check → mirror:check → wiki:check → coverage → build → clean
```

CI runs the same command on Node 20.19 and Node 22. When using this check as a
live-run test gate for the Switchyard repository, pass
`--test-cmd "npm run verify"` as shown above.

## Documentation

The repository wiki is an
[OKF v0.1](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)
bundle rooted at [`wiki/index.md`](./wiki/index.md). Directory `index.md` files
provide progressive disclosure, while concepts summarize and cite canonical
sources rather than replacing exact package and API documentation. Validate it
from `adw_sdlc/` with `npm run wiki:check`.

- [`wiki/index.md`](./wiki/index.md) — knowledge map for people and agents;
  maintenance policy lives in
  [`wiki/contributing/wiki-maintenance.md`](./wiki/contributing/wiki-maintenance.md).
- [`adw_sdlc/README.md`](./adw_sdlc/README.md) — complete CLI, configuration,
  runner, and development reference.
- [`adw_sdlc/docs/UNIVERSAL.md`](./adw_sdlc/docs/UNIVERSAL.md) — reusable kernel
  and project-pack architecture.
- [`adw_sdlc/HANDOVER.md`](./adw_sdlc/HANDOVER.md) — current status and roadmap.

## License

Switchyard is available under the [MIT License](./LICENSE).
