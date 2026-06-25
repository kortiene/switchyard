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
(sst/opencode) · `pi` (pi-node) — selected with `--runner` / `ADW_RUNNER`.

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
| `--runner <id>` | `claude` (default) `\| codex \| opencode \| pi` (env: `ADW_RUNNER`) |
| `--phases <list>` | comma-separated phase subset/order (default: the configured chain) |
| `--dry-run` | print the resolved phase plan and exit without running |
| `--resume` + `--adw-id <id>` | resume a run from its saved state |
| `--timeout <s>` | per-phase timeout in seconds; `signal:'timeout'` fast-fails with no nudge |
| `--max-budget-usd <usd>` | native spend cap (claude only); `signal:'budget'` fast-fails with no nudge |
| `--test-cmd <cmd>` | the test-gate command (env: `ADW_TEST_CMD`) |
| `--repo <owner/repo>` | work-item/repo locator (env: `REPO`) |
| `-y, --yes` | do not prompt before the irreversible squash-merge |

Control-plane env vars are canonicalized under `ADW_*` (for example
`ADW_RUNNER`, `ADW_TEST_CMD`, `ADW_ASSUME_YES`, and
`ADW_PARITY_FORCE_FENCED_JSON`). The inherited `MX_AGENT_*` names remain as
deprecated compatibility aliases; if both names are set with different values,
the CLI fails loudly. Both `ADW_*` and legacy `MX_AGENT_*` are withheld from
runner subprocesses.

## Configuration

A project adopts adw_sdlc by adding a `.adw/config.json` at the repository
root. Every section is optional and layered over behavior-preserving defaults.
The validated surface (see `src/config.ts` for the authoritative Zod schema):

| Section | Configures |
| --- | --- |
| `project` | id / display name |
| `prompts` | template `defaultRoot` + per-runner roots. This repo points the HealthTech project pack at `../.adw/prompts`; `.pi/prompts` and `.claude/commands` are neutral fallback command templates. |
| `phases` | optional ordered agent-phase chain (reorder/drop known phases) |
| `schemas` | optional per-phase JSON Schema overrides for `tests`/`e2e`/`document` (`root` dir + `overrides` map) |
| `customPhases` | optional new phase names (each needs a `<name>.md` template + `.adw/schemas/<name>.json`); may opt into a `gates.custom` gate and/or a `loops` loop |
| `loops` | optional resolve-style loops for custom phases (`{ command, maxAttempts }`); the schema must declare `resolved` |
| `providers` | work-item / VCS / change-request / CLI provider selection, plus `closedStates`, `inProgressStatus`, and an optional terminal `doneStatus` |
| `progress` | the progress-comment tag |
| `branching` | branch prefix, label→prefix map, slug rules |
| `gates` | `e2e` / `documentation` conditional-gate hints / file rules, plus `custom` per-custom-phase gates |
| `models` | classify model, default tier, per-phase tiers, tier→runner→model map |
| `commands` | default test command and pre-merge finalize gates |

A non-HealthTech example pack lives at
[`docs/examples/payments-api.config.json`](./docs/examples/payments-api.config.json).
This repository's committed pack is at `../.adw/config.json` with HealthTech
prompt templates under `../.adw/prompts`.

### Prompt-pack generation

Runtime prompts under `../.adw/prompts` are generated, not hand-maintained. The
neutral source templates live in `../.pi/prompts` (mirrored byte-for-byte in
`../.claude/commands`) and the project profile lives at
`../.adw/pack.profile.json`.

```bash
npm run pack:generate       # regenerate ../.adw/prompts from templates + profile
npm run pack:check          # CI drift guard; fails if prompts are stale
npm run pack:generate -- --dry-run
```

The generator intentionally uses `{{var}}` and
`<!-- adw:block NAME -->…<!-- adw:endblock -->` so it cannot collide with the
runtime `$1` / `$ARGUMENTS` prompt-argument substitution. It also injects the
profile's project-context header after YAML frontmatter, with per-phase
exclusions such as `classify`.

An optional build-time metaprompt pass is available:

```bash
npm run pack:generate -- --llm --model claude-sonnet-4-6
```

This is for reviewed/offline prompt authoring only. The ADW runtime never calls
the metaprompt generator; it consumes the committed `.adw/prompts/*.md` files.

## Development

`npm run verify` is the **canonical local/CI quality gate**. It runs every check
below in order, exits non-zero if any fails, and removes the `dist/` build
artifact at the end:

```bash
npm run verify   # typecheck → lint:env → pack:check → test → build → rm -rf dist
```

ADW live runs use it as the single test command (the gate is shell-split and run
without a shell, so a chained `a && b` command will not work — one command is
required):

```bash
ADW_TEST_CMD="npm run verify"
```

See [`docs/LIVE-RUN-BATCH.md`](./docs/LIVE-RUN-BATCH.md) for the live-run
rationale and command templates.

The individual gates `verify` chains, for running a single stage during
development:

```bash
npm run typecheck     # tsc --noEmit
npm run lint:env      # static secret-boundary lint (fail-closed)
npm run pack:check    # generated prompt-pack drift guard
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
| [`MVP-READINESS.md`](./MVP-READINESS.md) | Open-risk counterweight to PARITY.md: what real MVP-readiness still requires (mostly live runs) |
| [`docs/LIVE-RUN-BATCH.md`](./docs/LIVE-RUN-BATCH.md) | Ready-to-create issue batch + live-run command templates for the 5–10 varied `claude` runs |
| [`docs/OBSERVED-LIVE-LEDGER.md`](./docs/OBSERVED-LIVE-LEDGER.md) | Live-run dashboard: tracks which PARITY.md Section-10 guarantees have been observed in a real `claude` run (not only under mocks) |
| [`docs/FAILURE-DRILLS.md`](./docs/FAILURE-DRILLS.md) | Live `claude` runbook for the timeout / budget fast-fail and kill-then-`--resume` failure drills (MVP-READINESS §1 evidence) |
| [`docs/SECRET-BOUNDARY-AUDIT.md`](./docs/SECRET-BOUNDARY-AUDIT.md) | Live-oriented secret-boundary audit: spawns a real child and asserts denied keys (`GH_TOKEN`/`MATRIX_*`/`ADW_*`/`MX_AGENT_*`) are absent, names/booleans only (the spawn-crossing complement to `lint:env` + `env.test.ts`) |
| [`MEMORY_STACK.md`](./MEMORY_STACK.md) | Decision record for the deferred cross-run memory feature |
| [`docs/DESIGN-schema-overrides.md`](./docs/DESIGN-schema-overrides.md) | Design + rollout for per-phase schema overrides / custom phases (implemented) |
| [`docs/DESIGN-custom-phase-control-flow.md`](./docs/DESIGN-custom-phase-control-flow.md) | Design for loop/gated custom phases (custom gates + resolve-style loops; implemented) |
| [`docs/DESIGN-provider-plugins.md`](./docs/DESIGN-provider-plugins.md) | Security/sandboxing design pass for provider plugin loading (registry + declarative `cli`/`rest` providers implemented; out-of-process code plugins deferred) |
| [`docs/DESIGN-declarative-providers.md`](./docs/DESIGN-declarative-providers.md) | Declarative `cli`/`rest` work-item and change-request providers (implemented) |
| [`docs/DESIGN-declarative-providers-extensions.md`](./docs/DESIGN-declarative-providers-extensions.md) | Declarative primitives — transforms + pagination (implemented); token refresh (deferred) |
| [`HANDOVER.md`](./HANDOVER.md) | Session-to-session universalization handover and roadmap |
| [`docs/examples/`](./docs/examples/) | Example non-HealthTech project packs |
