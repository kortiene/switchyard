# adw_sdlc

A TypeScript/Node control plane for the **phased ADW (Agentic Developer
Workflow) SDLC pipeline**. It drives one work item (e.g. a GitHub issue)
through a deterministic chain of single-purpose agent phases, where each phase
is one invocation of an interchangeable coding-agent runner.

```
setup → classify → plan → implement → tests → resolve(loop) → e2e(gated)
      → review → patch(loop) → document(gated) → finalize → ci-fix(loop)
      → merge → report
```

Four runner backends sit behind a single `AgentRunner.runPhase()` seam —
`claude` (Claude Agent SDK) · `codex` (OpenAI Codex SDK) · `opencode`
(sst/opencode) · `pi` (pi-node) — selected with `--runner` / `MX_AGENT_RUNNER`.

The **orchestrator owns all git and GitHub work** and withholds secrets from
the runner (a deny-by-default env allowlist): in phased mode the coding agent
never receives `GH_TOKEN`. The agent only edits the worktree and authors the
commit message and PR body.

## Two layers: kernel + project pack

adw_sdlc is built to be reused across repositories, languages, domains, CI
systems, issue trackers, and VCS hosts:

- **Kernel** — deterministic orchestration, the runner seam, the secret
  boundary, and persistent run state. The same across every project; not
  project-configurable.
- **Project pack** — provider selection, the phase chain, branching rules,
  conditional-gate hints, model tiers, and prompt/schema files, supplied
  through `.adw/config.json` at the repository root (with behavior-preserving
  defaults when absent).

See **[`docs/UNIVERSAL.md`](./docs/UNIVERSAL.md)** for the universal
architecture, and **[`HEALTHTECH_PORT.md`](./HEALTHTECH_PORT.md)** for this
repository's specific configuration and standalone-port deltas.

## Quick start

Requires Node `>=20.19`. From this package directory:

```bash
npm install
npm run issue -- <work-item-id> --dry-run     # preview the plan, run nothing
npm run issue -- <work-item-id> --runner claude
```

`npm run issue` maps to `tsx src/cli.ts`. The command name `issue` is kept as a
backward-compatible GitHub alias.

Frequently used flags (`-h` / `--help` prints the full list):

| Flag | Meaning |
| --- | --- |
| `--runner <id>` | `claude` (default) `\| codex \| opencode \| pi` (env: `MX_AGENT_RUNNER`) |
| `--phases <list>` | comma-separated phase subset/order (default: the configured chain) |
| `--dry-run` | print the resolved phase plan and exit without running |
| `--resume` + `--adw-id <id>` | resume a run from its saved state |
| `--test-cmd <cmd>` | the test-gate command (env: `MX_AGENT_TEST_CMD`) |
| `--repo <owner/repo>` | work-item/repo locator (env: `REPO`) |
| `-y, --yes` | do not prompt before the irreversible squash-merge |

## Configuration

A project adopts adw_sdlc by adding a `.adw/config.json` at the repository
root. Every section is optional and layered over behavior-preserving defaults.
The validated surface (see `src/config.ts` for the authoritative Zod schema):

| Section | Configures |
| --- | --- |
| `project` | id / display name |
| `prompts` | template `defaultRoot` + per-runner roots |
| `phases` | optional ordered agent-phase chain (reorder/drop known phases) |
| `schemas` | optional per-phase JSON Schema overrides for `tests`/`e2e`/`document` (`root` dir + `overrides` map) |
| `customPhases` | optional new plain phase names (each needs a `<name>.md` template + `.adw/schemas/<name>.json`) |
| `providers` | work-item / VCS / change-request / CLI provider selection, plus `closedStates`, `inProgressStatus`, and an optional terminal `doneStatus` |
| `progress` | the progress-comment tag |
| `branching` | branch prefix, label→prefix map, slug rules |
| `gates` | `e2e` and `documentation` conditional-gate hints / file rules |
| `models` | classify model, default tier, per-phase tiers, tier→runner→model map |
| `commands` | default test command and pre-merge finalize gates |

A non-HealthTech example pack lives at
[`docs/examples/payments-api.config.json`](./docs/examples/payments-api.config.json).
This repository's committed pack is at `../.adw/config.json`.

## Development

```bash
npm run typecheck     # tsc --noEmit
npm run lint:env      # static secret-boundary lint (fail-closed)
npm test              # vitest suite
npm run build         # tsc -p tsconfig.build.json  (dist/ is a build artifact)
```

`npm run lint:env` enforces the non-negotiable secret boundary: no
`...process.env` spread in `src/`, no banned opencode factory calls, and
opencode imports only via `@opencode-ai/sdk/v2/client`. The env allowlist in
`src/env.ts` and this lint are **not** project-configurable — they are
hardcoded for security and must not be relaxed by a project pack.

## Documentation map

| Document | Purpose |
| --- | --- |
| [`docs/UNIVERSAL.md`](./docs/UNIVERSAL.md) | Universal kernel/project-pack architecture and the config surface |
| [`HEALTHTECH_PORT.md`](./HEALTHTECH_PORT.md) | This repository's setup and the standalone-port deltas from upstream |
| [`PLAN.md`](./PLAN.md) | Full migration plan and the settled D1–D6 design decisions |
| [`PARITY.md`](./PARITY.md) | Parity checklist mapping each guarantee to the test(s) that prove it |
| [`MEMORY_STACK.md`](./MEMORY_STACK.md) | Decision record for the deferred cross-run memory feature |
| [`docs/DESIGN-schema-overrides.md`](./docs/DESIGN-schema-overrides.md) | Design proposal for per-phase schema overrides / custom phases (not yet implemented) |
| [`docs/DESIGN-provider-plugins.md`](./docs/DESIGN-provider-plugins.md) | Security/sandboxing design pass for provider plugin loading (proposal; not implemented) |
| [`HANDOVER.md`](./HANDOVER.md) | Session-to-session universalization handover and roadmap |
| [`docs/examples/`](./docs/examples/) | Example non-HealthTech project packs |
