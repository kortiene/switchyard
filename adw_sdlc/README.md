# adw_sdlc

A TypeScript/Node control plane for the **phased ADW (Agentic Developer
Workflow) SDLC pipeline**. It drives one work item (e.g. a GitHub issue)
through a deterministic chain of single-purpose agent phases, where each phase
is one invocation of an interchangeable coding-agent runner.

```
setup → classify → plan → implement → tests → resolve(loop) → e2e(gated)
      → review → patch(loop) → document(gated) → finalize → ci-fix(loop)
      → merge(optional) → report
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
  defaults when absent). The project pack can also live in an **external** repo:
  point the run at it with `--project-root` / `ADW_PROJECT_ROOT` to orchestrate
  a target repository without copying the kernel into it (prompts/schemas the
  target does not ship fall back to the bundled kernel defaults).

> **Trust caution:** an external project root makes that repo's
> `.adw/config.json` drive this process — its test/finalize gate commands,
> prompt/schema paths, and provider descriptors all come from the target. Only
> target repos you trust to run commands on your machine.

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
backward-compatible GitHub alias. Work-item ids may be numeric GitHub issue
numbers or provider-native strings such as `PROJ-123`, UUIDs, and slugs; each
provider validates its own shape, and the GitHub adapter remains numeric-only.

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
| `--project-root <dir>` | target repo root for config/prompts/state/worktree (env: `ADW_PROJECT_ROOT`) |
| `-y, --yes` | do not prompt before the irreversible squash-merge |
| `--no-merge` | run through PR creation and green CI, then report success with the change request left open |

`--no-merge` and `--yes` are mutually exclusive. A PR-only run records
`merge_skipped: "flag"` without completing the `merge` phase; merge it later
with an explicitly authorized resume, for example
`--resume --adw-id <id> --yes`.

Control-plane env vars are canonicalized under `ADW_*` (for example
`ADW_RUNNER`, `ADW_TEST_CMD`, `ADW_ASSUME_YES`, `ADW_PROJECT_ROOT`, and
`ADW_PARITY_FORCE_FENCED_JSON`). The inherited `MX_AGENT_*` names remain as
deprecated compatibility aliases; if both names are set with different values,
the CLI fails loudly. Both `ADW_*` and legacy `MX_AGENT_*` are withheld from
runner subprocesses.

`ADW_PROJECT_ROOT` (or `--project-root`) selects an explicit project root for
the run: the orchestrator loads **that** directory's `.adw/config.json`,
prompts/schemas, and `agents/` state, and edits/git-operates/gates in its
worktree. Omit it and behavior is unchanged — the project root defaults to this
repository. A relative value resolves against the invocation directory, and a
non-existent or non-directory path fails closed with an actionable error.

### Troubleshooting: classify and Anthropic API billing

When `ANTHROPIC_API_KEY` is set, the `classify` phase first runs in-process
against the **public Anthropic messages API**, which is billed pay-as-you-go
(it does *not* accept a Claude subscription / `claude login` OAuth token). If
that account is out of credit, mis-billed, rate-limited, or the key is invalid,
the run treats that as an expected API-boundary failure and falls back to
classifying through the selected runner.

For example, an unfunded API account produces a progress note like:

```
>> classify: shared Anthropic API classify failed (Your credit balance is too low to access the Anthropic API.); falling back to claude runner without ANTHROPIC_API_KEY
```

The same issue can appear later on a resumed run: Claude Code may print
`Credit balance is too low` for `plan`/`implement`/other runner phases because
pay-as-you-go Anthropic auth (`ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN`)
takes precedence over a local `claude login`. The orchestrator recognizes this
as runner authentication/account failure (not a JSON parse failure), removes
those pay-as-you-go credentials for the `claude` runner, and retries that phase
once so Claude Code can use `claude login` / OAuth subscription auth instead.
You can still choose the runner path up front by setting
`ADW_CLASSIFY_ON_RUNNER=1`; see [`docs/LIVE-RUN-BATCH.md`](./docs/LIVE-RUN-BATCH.md)
for the subscription run template.

## Configuration

A project adopts adw_sdlc by adding a `.adw/config.json` at the repository
root. Every section is optional and layered over behavior-preserving defaults.
The validated surface (see `src/config.ts` for the authoritative Zod schema):

| Section | Configures |
| --- | --- |
| `project` | id / display name |
| `prompts` | template `defaultRoot` + per-runner roots. This repo points the HealthTech project pack at `../.adw/prompts`; `.pi/prompts` and `.claude/commands` are neutral fallback command templates. |
| `runners` | runner-specific settings; `runners.opencode.config` supplies OpenCode server/provider config and optional `authEnv` names one provider credential |
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

### Local / OpenAI-compatible models with OpenCode

The self-spawned OpenCode server can receive an operator-supplied provider
block from `.adw/config.json`. For example, this partial project config routes
all OpenCode tiers to a local OpenAI-compatible Qwen server:

```json
{
  "runners": {
    "opencode": {
      "authEnv": "LOCAL_MODEL_API_KEY",
      "config": {
        "enabled_providers": ["local"],
        "small_model": "local/qwen",
        "provider": {
          "local": {
            "npm": "@ai-sdk/openai-compatible",
            "name": "Local vLLM",
            "options": {
              "baseURL": "http://127.0.0.1:8000/v1",
              "apiKey": "{env:LOCAL_MODEL_API_KEY}"
            },
            "models": {
              "qwen": {
                "name": "Qwen local",
                "limit": { "context": 131072, "output": 16384 }
              }
            }
          }
        }
      }
    }
  },
  "models": {
    "tiers": {
      "cheap": { "opencode": "local/qwen" },
      "mid": { "opencode": "local/qwen" },
      "capable": { "opencode": "local/qwen" }
    }
  }
}
```

Set `LOCAL_MODEL_API_KEY` in the parent environment before the run (a local
server that does not authenticate can use a non-secret placeholder). `authEnv`
is the only additional name admitted to the OpenCode child allowlist; it is not
forwarded to other runners. GitHub authority, known credentials/config knobs
belonging to other runners, `ADW_*` / `MX_AGENT_*` / `MATRIX_*`, and alternate
OpenCode config-channel names are rejected. OpenCode performs the `{env:NAME}`
substitution, keeping the value out of the project file.

The adapter always replaces any operator-supplied `permission` field with its
own headless policy, including the `bash` denials for `git` and `gh`. Provider,
model, and other server fields are preserved; the security policy is not
project-configurable.

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

The neutral `../.pi/prompts` ↔ `../.claude/commands` mirror has its own
recursive, all-file byte-identity guard (`../.pi/prompts` is canonical):

```bash
npm run mirror:check        # gate drift guard; fails if the two trees differ
npm run mirror:sync         # repair: make ../.claude/commands match ../.pi/prompts
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
npm run verify   # typecheck → lint:env → pack:check → mirror:check → coverage → build → rm -rf dist
```

ADW live runs use it as the single test command (the gate is shell-split and run
without a shell, so a chained `a && b` command will not work — one command is
required):

```bash
ADW_TEST_CMD="npm run verify"
```

See [`docs/LIVE-RUN-BATCH.md`](./docs/LIVE-RUN-BATCH.md) for the live-run
rationale and command templates.

CI runs `npm run verify` on a **Node-version matrix** (`.github/workflows/verify.yml`):
the package engines floor **`20.19.0`** (`"engines": { "node": ">=20.19" }`) and
**`22`** (local-dev line), with `fail-fast: false` so each leg is independent
signal. The floor leg exists because CI previously only ever ran Node 22, leaving
the `>=20.19` floor unexercised (issue #37). The `pi` runner requires **Node ≥
22.19** (its npm package declares `"engines": { "node": ">=22.19.0" }` and is an
optional dependency), so only the Node-22 leg can exercise pi; the Node-20.19.0
leg covers the claude/codex/opencode adapters (the suite is fully mocked, so both
legs are green with no pi binary present).

The individual gates `verify` chains, for running a single stage during
development:

```bash
npm run typecheck     # tsc --noEmit
npm run lint:env      # static secret-boundary lint (fail-closed)
npm run pack:check    # generated prompt-pack drift guard
npm run mirror:check  # .pi/prompts ↔ .claude/commands byte-identity guard
npm run coverage      # vitest suite + v8 coverage (modest thresholds; the verify test stage)
npm test              # vitest suite, coverage-free (fast focused/full runs)
npm run build         # tsc -p tsconfig.build.json  (dist/ is a build artifact)
```

`npm run coverage` is the test stage of `verify`: it runs the full suite once
with the V8 coverage provider and enforces a modest, measured threshold
(`vitest.config.ts` → `test.coverage`). It measures `src/**/*.ts` with
`all: true`, so a brand-new untested module reports at 0% and trips the gate.
Coverage collection is off by default, so a focused `npx vitest run <file>` (and
plain `npm test`) stays coverage-free and never false-fails on the threshold. The
v8 reporter writes `coverage/` (git-ignored); open `coverage/index.html` after a
run.

`npm run lint:env` enforces the non-negotiable secret boundary: no
`...process.env` spread in `src/`, no banned opencode factory calls, and
opencode imports only via `@opencode-ai/sdk/v2/client`. The env allowlist in
`src/env.ts` and this lint are hardcoded for security and cannot be broadly
relaxed by a project pack. The sole narrow extension is the validated
`runners.opencode.authEnv`: one exact provider-credential name, still filtered
through `safeSubprocessEnv`; it cannot name GitHub authority, a denied prefix,
another runner's known credential/config knob, or an alternate OpenCode config
channel.

## Documentation map

| Document | Purpose |
| --- | --- |
| [`docs/UNIVERSAL.md`](./docs/UNIVERSAL.md) | Universal kernel/project-pack architecture and the config surface |
| [`HEALTHTECH_PORT.md`](./HEALTHTECH_PORT.md) | This repository's setup and the standalone-port deltas from upstream |
| [`PLAN.md`](./PLAN.md) | Full migration plan and the settled D1–D6 design decisions |
| [`PARITY.md`](./PARITY.md) | Parity checklist mapping each guarantee to the test(s) that prove it |
| [`MVP-READINESS.md`](./MVP-READINESS.md) | Readiness counterweight to PARITY.md: completed operational evidence plus the remaining sampling/sign-off risks |
| [`docs/LIVE-RUN-BATCH.md`](./docs/LIVE-RUN-BATCH.md) | Ready-to-create issue batch + live-run command templates for the 5–10 varied `claude` runs |
| [`docs/OBSERVED-LIVE-LEDGER.md`](./docs/OBSERVED-LIVE-LEDGER.md) | Live-run dashboard and issues #20–#23 evidence index for PARITY.md's Section-10 guarantees |
| [`docs/FAILURE-DRILLS.md`](./docs/FAILURE-DRILLS.md) | Safe isolated `claude` runbook plus observed timeout / native-budget / kill-resume evidence |
| [`docs/COST-AND-DURATION.md`](./docs/COST-AND-DURATION.md) | Cost/duration levers (tier routing, nudge-retry rate, budget/timeout caps) + the per-run `agents/{adw_id}/metrics.json` artifact and measurement plan |
| [`docs/SECRET-BOUNDARY-AUDIT.md`](./docs/SECRET-BOUNDARY-AUDIT.md) | Generic spawn preflight and real Claude SDK boundary probe; denied-key names/booleans only, never values |
| [`MEMORY_STACK.md`](./MEMORY_STACK.md) | Decision record for the deferred cross-run memory feature |
| [`docs/DESIGN-schema-overrides.md`](./docs/DESIGN-schema-overrides.md) | Design + rollout for per-phase schema overrides / custom phases (implemented) |
| [`docs/DESIGN-custom-phase-control-flow.md`](./docs/DESIGN-custom-phase-control-flow.md) | Design for loop/gated custom phases (custom gates + resolve-style loops; implemented) |
| [`docs/DESIGN-provider-plugins.md`](./docs/DESIGN-provider-plugins.md) | Security/sandboxing design pass for provider plugin loading (registry + declarative `cli`/`rest` providers implemented; out-of-process code plugins deferred) |
| [`docs/DESIGN-declarative-providers.md`](./docs/DESIGN-declarative-providers.md) | Declarative `cli`/`rest` work-item and change-request providers (implemented) |
| [`docs/DESIGN-declarative-providers-extensions.md`](./docs/DESIGN-declarative-providers-extensions.md) | Declarative primitives — transforms + pagination (implemented); token refresh (deferred) |
| [`HANDOVER.md`](./HANDOVER.md) | Session-to-session universalization handover and roadmap |
| [`docs/examples/`](./docs/examples/) | Example non-HealthTech project packs |
