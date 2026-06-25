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

### Provider registry (open switch, fail closed)

Provider *kind* (`type`) dispatches through a registry in `providers.ts` rather
than a closed switch, so a new in-tree provider (e.g. `gitlab`/`glab`) is a
one-line registration in the kernel — no config-schema change. Responsibilities
split the same way the phase chain does:

- `.adw/config.json` validates the type's **shape** (a non-empty string).
- The registry validates **membership** — the kinds it can build — and fails
  **closed** with a loud `AdwError` (`unsupported <role> provider type "<x>"
  (supported: …)`) before any provider is constructed. Because providers are
  built at run start (`defaultDeps`), an unknown kind is caught up front, even
  on `--dry-run`.

This keeps `config.ts` ⇄ `providers.ts` acyclic and means the secret boundary is
never weakened to add provider coverage: the registry only maps a name to an
**in-tree, reviewed** factory — it does not load project-supplied code.
`supportedProviderTypes()` reports the registered kinds per role. Built-in kinds
today: `github` (cli / workItems / changeRequests), `git` (vcs), and `cli` (a
declarative work-item provider, below). The staged path for genuinely external
providers (a declarative `rest`/`cli` driver, then an out-of-process plugin
broker) is designed in `docs/DESIGN-provider-plugins.md`; in-process loading of
config-supplied code is a rejected non-goal there.

### Declarative `cli` work items (no code)

A project can back its work items with a non-GitHub forge by *describing* the
provider — command templates + field mappings — rather than shipping code. The
kernel interprets the descriptor (`provider-descriptor.ts`) and shells the
project's forge CLI through a **scoped, one-credential env**:

```jsonc
"providers": {
  "workItems": {
    "type": "cli",
    "authEnv": "GITLAB_TOKEN",                 // a NAME; the kernel injects only this
    "routes": {
      "fetch": {
        "command": ["glab", "issue", "view", "{id}", "--output", "json"],
        "map": { "title": "$.title", "body": "$.description", "labels": "$.labels[*].name" }
      },
      "state": {
        "command": ["glab", "issue", "view", "{id}", "--output", "json"],
        "map": { "state": "$.state" }
      }
      // postProgress / assignSelf / setStatus: optional routes; absent ⇒ no-op
    }
  }
}
```

- `fetch`/`state` are required; the write routes are optional and no-op when
  absent (best-effort, like GitHub's no-op without `gh`).
- `map` values are a restricted path mini-language (`$.a.b`, `$.a[0]`, `$.a[*]`,
  `$.a[*].name`) — no JSONPath dependency, pure data walk.
- The CLI runs via `safeSubprocessEnv({ allowGhToken: false, extraAllow: [authEnv] })`:
  it gets `PATH`/`HOME` plus its one named token and **never `GH_TOKEN`** —
  tighter than the GitHub built-in, which inherits the ambient env. `authEnv`
  rejecting `GH_TOKEN`, deny-prefixed (`MATRIX_`/`MX_AGENT_`), or a model
  credential is enforced at load. With `authEnv` omitted, a CLI that stored its
  own auth under `~/.config/<tool>` (e.g. after `glab auth login`) still works
  (`HOME` is allowed).
- Pair with `closedStates` so the verify gate recognises the forge's terminal
  state (e.g. `"closedStates": ["closed"]`). The full descriptor design is in
  `docs/DESIGN-declarative-providers.md`.

### Declarative `rest` (HTTP) work items (no code)

The same fetch/state mapping over HTTP, for forges without a CLI. The kernel
performs the request against an **allowlisted https host**, injecting one named
credential as a header value:

```jsonc
"providers": {
  "workItems": {
    "type": "rest",
    "baseUrl": "https://gitlab.example.com/api/v4",
    "allowedHosts": ["gitlab.example.com"],   // required; exact host[:port]
    "authEnv": "GITLAB_TOKEN",                 // required; a NAME
    "authHeader": "Authorization",             // optional, default "Authorization"
    "authScheme": "Bearer",                    // optional, default "Bearer" ("" ⇒ raw token)
    "routes": {
      "fetch": { "method": "GET", "path": "/projects/{repo}/issues/{id}",
                 "map": { "title": "$.title", "body": "$.description", "labels": "$.labels[*].name" } },
      "state": { "method": "GET", "path": "/projects/{repo}/issues/{id}",
                 "map": { "state": "$.state" } }
    }
  }
}
```

- **Host allowlist + https** are enforced at load *and* re-checked per request;
  `{id}`/`{repo}` are percent-encoded into the path so they cannot change the
  host. `authEnv` carries the same guard as `cli` (no `GH_TOKEN`, deny-prefix, or
  model credential).
- `authHeader`/`authScheme` cover the forge variants — GitLab PATs use
  `"authHeader": "PRIVATE-TOKEN", "authScheme": ""`; GitHub/Gitea use
  `"authScheme": "token"`; Bearer is the default.
- Transport is a **kernel-owned synchronous one-shot fetch** (a fixed `node -e`
  helper spawned with a scoped env; the token is read by name inside the child,
  never on argv). No new dependency, no project code. Read routes only in this
  step (`fetch`/`state`); progress/assignment/status are not yet posted over
  `rest`.

### Declarative `rest` change requests (no code)

A non-GitHub forge can also back the **change request** (PR/MR) lifecycle over
HTTP. It reuses the same rest base (`baseUrl`/`allowedHosts`/`authEnv`/
`authHeader`/`authScheme`) and adds routes with **templated JSON request bodies**:

```jsonc
"providers": {
  "changeRequests": {
    "type": "rest",
    "baseUrl": "https://gitlab.example.com/api/v4",
    "allowedHosts": ["gitlab.example.com"],
    "authEnv": "GITLAB_TOKEN",
    "routes": {
      "findForBranch": { "method": "GET", "path": "/projects/{repo}/merge_requests?source_branch={branch}",
                         "map": { "url": "$[0].web_url" } },
      "create": { "method": "POST", "path": "/projects/{repo}/merge_requests",
                  "body": { "source_branch": "{branch}", "target_branch": "{base}", "title": "{title}", "description": "{body}" },
                  "map": { "number": "$.iid", "url": "$.web_url" } },
      "squashMerge": { "method": "PUT", "path": "/projects/{repo}/merge_requests/{id}/merge", "body": { "squash": true } },
      "pipelineStatus": { "method": "GET", "path": "/projects/{repo}/merge_requests/{id}",
                          "statusPath": "$.pipeline.status",
                          "stateMap": { "success": "success", "failed": "failure", "running": "pending" } }
    }
  }
}
```

- `findForBranch`/`create`/`squashMerge` are required; `pipelineStatus` is
  optional (omitted ⇒ `none`, i.e. no CI gate). `create`/`squashMerge` carry a
  JSON `body` whose string leaves are placeholder-substituted (`{branch}`,
  `{base}`, `{title}`, `{body}`; `{id}`/`{repo}` for merge).
- `pipelineStatus.stateMap` maps the forge's status string onto the kernel's
  `CiState` (`success`/`failure`/`pending`/`none`/`unknown`); unmapped ⇒
  `unknown`. Failing-job detail is populated by the optional `failingJobs` route
  (see "Declarative provider primitives" below); without it, `failingJobs` is `[]`.
- **`squashMerge` is the merge-authorized operation** — it is bound by the same
  host allowlist + https + scoped credential as every route, and the
  orchestrator still owns the **gating** (it merges only after the review/CI
  gates pass) and all git (branch/commit/push stay the `git` provider). A
  provider never receives `GH_TOKEN` or raw git/gh authority.

### Declarative provider primitives (step 2.5)

Three bounded primitives extend the declarative driver while staying **data, not
code** — every page and value is interpreted by kernel code over project data,
and every request still passes the host allowlist + https check. They are
independent and additive; absent ⇒ today's behavior exactly. (Token refresh,
2.5c, is spec'd but deferred until a concrete OAuth provider needs it —
`docs/DESIGN-declarative-providers-extensions.md`.)

**Transforms (2.5a)** — a *scalar* map value may carry a `|`-piped transform
chain after its path. A closed, eval-free vocabulary applied to the coerced
string after the data walk:

```jsonc
"map": { "state": "$.pipeline.status | lower" }      // normalize before a stateMap lookup
"map": { "number": "$.iid | default:0", "title": "$.title | trim" }
```

| Transform     | Effect                                                        |
| ------------- | ------------------------------------------------------------- |
| `lower`       | lowercase                                                     |
| `upper`       | uppercase                                                     |
| `trim`        | strip surrounding whitespace                                  |
| `default:<v>` | if the value is `""` (missing), substitute the literal `<v>`  |

Chains apply left-to-right (`$.s | trim | lower`). An unknown transform, or a
bare `default` with no argument, is a loud error at load. Array fields (`labels`)
keep the bare `[*]` form — per-element transforms are deferred.

**Pagination (2.5b)** — an optional `failingJobs` change-request route assembles
a multi-page list, populating `PipelineStatus.failingJobs` (which the ci-fix loop
consumes). It is fetched with the **same `{id}` as `pipelineStatus`** (the
change-request id), only when the pipeline is red:

```jsonc
"failingJobs": {
  "method": "GET", "path": "/projects/{repo}/merge_requests/{id}/jobs?scope=failed",
  "itemsPath": "$.jobs",
  "map": [ { "name": "$.name", "logExcerpt": "$.failure_reason | default:" } ],
  "paginate": { "next": { "style": "nextUrl", "path": "$.links.next" }, "maxPages": 10 }
}
```

- `itemsPath` locates the items array on a page (`"$"` ⇒ the body itself); `map`
  is a **one-element array** whose object templates each `{ name, logExcerpt }`
  job (the inner values are scalar mappings, so transforms apply).
- `paginate` (optional; omit ⇒ a single page) walks pages by one of two cursor
  styles: **`nextUrl`** (the next absolute URL from a body path) or
  **`pageParam`** (`{ style: "pageParam", param: "page", start: 1 }` — increment
  until a page yields zero items). `maxPages` is a **hard cap** (default 10) whose
  hit is logged — never a silent truncation.
- **Security:** a next-page URL comes from the (attacker-influenceable) response,
  so the kernel **re-asserts the host allowlist on every followed URL** — an
  off-allowlist next URL stops pagination (returns what was gathered) rather than
  being followed. A garbage page or a transport error ends the loop with the
  items gathered so far. (`Link`-header pagination is deferred — it would require
  response headers from the one-shot fetch helper.)

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
