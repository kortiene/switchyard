# Universal ADW workflow

ADW SDLC has two parts:

- **Kernel** — the deterministic orchestration in `src/orchestrator.ts`,
  the runner abstraction in `src/invoker.ts`, the secret boundary in
  `src/env.ts`, and the persistent run state in `src/state.ts`.
- **Project pack** — everything else a project provides through
  `.adw/config.json` and prompt/schema files.

The kernel is the same across projects. The project pack changes per repo.

## Universal contract

A project adopts ADW by adding:

```
.adw/
  config.json     # provider selection, phase chain, branching, gates, model tiers, …
  prompts/        # optional override prompts (default reads .pi/prompts)
  schemas/        # optional per-phase schema overrides
```

No edits to `adw_sdlc/src` are required. The kernel will:

1. Load `.adw/config.json` (or fall back to the built-in HealthTech defaults).
2. Resolve providers via `createProvidersFromConfig(config, …)`.
3. Use those providers for work-item/VCS/change-request/CI effects.
4. Compose prompts using `config.prompts.defaultRoot` and any
   runner-specific roots (e.g. `claude` → `.claude/commands`).

## Provider boundary

The orchestrator reasons in terms of:

- `WorkItemProvider` — fetch/state/postProgress/assignSelf/setStatus
- `VcsProvider` — branch/commit/push/diff
- `ChangeRequestProvider` — open/find/squash-merge + `pipelineStatus`
- `ProviderCli` — executable + repo detection

Built-in providers:

- **GitHub** (via `gh`) for work items and change requests
- **Git** for VCS

These are selected through `.adw/config.json`:

```json
"providers": {
  "cli": { "type": "github" },
  "workItems": {
    "type": "github",
    "closedStates": ["CLOSED"],
    "inProgressStatus": "In Progress",
    "doneStatus": "Done"
  },
  "vcs": { "type": "git" },
  "changeRequests": { "type": "github" }
}
```

`inProgressStatus` is applied when a run starts; the optional `doneStatus` is
applied (best-effort) once the run is merged **and** verified. GitHub already
auto-closes the issue via "closes #&lt;n&gt;", so `doneStatus` is for providers
or Projects boards that need an explicit terminal move and is unset by default.
For a non-GitHub provider whose verify gate reads the same status axis, list
`doneStatus` in `closedStates` so the post-merge verification recognises it.

Future provider plugins land here without orchestrator changes.

## Phase chain

The ordered agent-phase chain is project-configurable through an optional
`phases` array:

```json
"phases": [
  "classify", "plan", "implement", "tests",
  "resolve", "review", "patch"
]
```

Precedence is: an explicit `--phases` CSV (per-run) > the configured `phases`
chain > the full built-in catalog (`classify plan implement tests resolve e2e
review patch document`). Omitting `phases` keeps the full catalog, so this is
behavior-preserving for existing projects.

Every entry must name a **known catalog phase** — the kernel validates the
list against `AGENT_PHASES` and fails loudly otherwise. Phase *semantics* are
not configurable: the `resolve`/`patch` bounded loops, the `e2e`/`document`
conditional gates, and each phase's structured-output schema live in the
kernel and are keyed by phase name. A project may therefore **reorder or drop**
known phases (e.g. skip `e2e` on a repo with no end-to-end surface, or pin the
full chain explicitly so a future kernel-default change cannot silently reshape
its pipeline), but inventing a genuinely new phase name is done through
`customPhases` (below), not this field.

## Per-phase schema overrides

A project can override the structured-output schema of a phase by dropping a
JSON Schema at `.adw/schemas/<phase>.json` (or mapping `schemas.overrides[phase]`
to an explicit path):

```json
"schemas": { "root": ".adw/schemas", "overrides": { "tests": ".adw/schemas/tests.v2.json" } }
```

The schema feeds native-schema runners directly and validates the runner payload
(via ajv) on the parent side; the prompt's fenced-JSON contract is generated
from it. Only phases whose result fields the orchestrator does **not** read are
overridable — `tests`, `e2e`, `document`. Overriding a load-bearing phase
(classify/plan/implement/review/resolve/patch) is rejected loudly, because the
kernel's control flow depends on those built-in shapes.

## Custom phases

A project can register genuinely new, plain phases (no loop, no conditional
gate) and place them in the `phases` chain:

```json
"customPhases": ["audit"],
"phases": ["classify", "plan", "implement", "audit", "review"]
```

Each custom phase needs a prompt template at `<name>.md` (under the prompt
roots) and a result schema at `.adw/schemas/<name>.json`; its model tier comes
from `models.phaseTiers[<name>]` (else the default tier). By default it runs as a
plain sequential agent phase — its result is recorded but the kernel never
branches on it — unless it opts into a gate or loop (below). A name colliding
with a built-in phase, or a chain entry that is neither built-in nor registered,
is rejected loudly. Overriding `classify` remains out of scope (see
`DESIGN-schema-overrides.md`).

The whole resolved chain is preflighted at run start: every phase must have a
prompt template that resolves and a result schema that loads, so a custom phase
missing its `<name>.md` template or `.adw/schemas/<name>.json` schema (or a
broken/unsupported schema override) fails loudly up front — before any branch,
PR, or state is created — rather than mid-chain. A `--dry-run` runs the same
preflight, so it doubles as a config check.

## Custom-phase control flow (gates and loops)

A registered custom phase may opt into the two control-flow shapes that
generalize cleanly from the built-ins (see `DESIGN-custom-phase-control-flow.md`):

```json
"customPhases": ["audit", "verify"],
"phases": ["classify", "plan", "implement", "audit", "verify", "review"],
"gates": {
  "custom": {
    "audit": { "hints": ["auth", "payment"], "pathPrefixes": ["src/billing/"] }
  }
},
"loops": {
  "verify": { "command": "npm run verify", "maxAttempts": 3 }
}
```

- A **custom gate** (`gates.custom.<phase>`) runs the phase only when the change
  signal matches a `hints` word **or** a changed file matches an
  `exactFiles`/`pathPrefixes`/`fileExtensions` rule — the same matching as the
  built-in `document` gate. An empty predicate (the phase could never run) is
  rejected at startup.
- A **custom loop** (`loops.<phase>`) is resolve-style: the orchestrator runs
  `command`; a non-zero exit invokes the phase's agent (with the command output)
  to fix it and retries up to `maxAttempts` (default 3), stopping early if the
  agent reports `resolved: 0`. The loop command is run by the orchestrator with
  its own environment — the agent still receives no secrets — exactly like the
  built-in `--test-cmd` gate. The phase's result schema must declare `resolved`.

Both compose (a phase may be gated *and* looped). Control-flow config may target
**only** a registered custom phase: an entry on a built-in name (whose control
flow the kernel owns) or an unregistered name is rejected at startup. Built-in
`resolve`/`patch` loops and `e2e`/`document` gates are unchanged and keep their
own knobs (`--test-cmd`/`maxResolve`, `gates.e2e`/`gates.documentation`).

## Provider-neutral public types

| Old GitHub-shaped name | Provider-neutral name | Status |
| --- | --- | --- |
| `IssueContext` | `WorkItemContext` | compat alias preserved |
| `fetchIssue()` | `fetchWorkItem()` | compat function preserved |
| `CreatePrResult` | `CreateChangeRequestResult` | compat alias preserved |
| `CiStatus` | `PipelineStatus` | compat alias preserved |
| `FailingJob` | `PipelineJob` | compat alias preserved |
| `git.GitResult` (provider-shaped) | `OperationResult` | compat: `GitOperationResult` |
| `runIssue` (CliDeps) | `runWorkItem` (optional) | runIssue still required |
| `parsed.issue` (CLI) | `parsed.workItem` | both populated identically |

## State

Persistent run state at `agents/{adw_id}/state.json` is the cross-language
contract with the Python `adw/` pipeline. It carries v1 GitHub-shaped fields
(`issue_number`, `pr_number`, `pr_url`) for backward compatibility, plus
TS-additive provider-neutral metadata:

```json
"work_item": {
  "provider": "github",
  "type": "issue",
  "id": "5",
  "number": 5,
  "title": "..."
},
"change_request": {
  "provider": "github",
  "type": "pull_request",
  "id": "42",
  "number": 42,
  "url": "..."
}
```

Both halves are validated against `adw/state.schema.json`. The provider-neutral
fields are non-load-bearing for resume; v1 fields stay canonical.

## Secret boundary (non-configurable)

The deny-by-default env allowlist in `src/env.ts` and the static lint gate
`scripts/check-adw-sdlc-env.sh` are NOT project-configurable. They guarantee:

- `GH_TOKEN`, `MATRIX_*`, `MX_AGENT_*` keys are withheld from runner children
- No source file spreads `...process.env`
- The opencode SDK is reachable only via `@opencode-ai/sdk/v2/client`

These remain hardcoded for security reasons.

## Example: configuring ADW for a non-HealthTech project

Drop the example pack from `docs/examples/payments-api.config.json` into your
repository's `.adw/config.json` and adjust the values. The kernel will pick it
up automatically.
