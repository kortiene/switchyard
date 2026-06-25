# Design proposal — declarative providers (`rest`/`cli` driver), §11 #4 step 2

**Status:** sub-steps **2a (`cli`), 2b (`rest` work items), and 2c (`rest`
change-requests) implemented** (`HANDOVER.md` §8k, §8l, §8m); step 2 (declarative
driver) is complete for work items and change requests over `cli`/`rest`. This is the concrete design
for **step 2** of the staged provider-extensibility rollout in
`DESIGN-provider-plugins.md` §5 ("ship the declarative `rest`/`cli` driver,
Option B"). Step 1 — the fail-closed provider registry — shipped earlier
(`HANDOVER.md` §8j); this note specifies what plugs into it and resolves the
four open questions in `DESIGN-provider-plugins.md` §8.

**Two refinements made during 2b implementation (vs. §3.2 / §5.2 as drafted):**

- **Auth header is configurable**, not just the scheme. Real forges differ
  (GitLab PATs want `PRIVATE-TOKEN: <token>`, GitHub/Gitea want
  `Authorization: token <token>`, others Bearer), so the descriptor takes
  `authHeader` (default `Authorization`) + `authScheme` (default `Bearer`, `""`
  ⇒ raw token). Same one-named-credential boundary.
- **The fetch helper is an inline `node -e` script**, not a shipped
  `src/internal/rest-fetch.mjs` asset. Same security properties (token read by
  name inside the child's scoped env, never argv; request via stdin; child runs
  with only the scoped env), but no file to locate via `import.meta.url` or copy
  on build — the script is a constant string of kernel code.

**One-line thesis:** the existing GitHub provider is *already* a hardcoded
instance of a declarative provider — `fetchWorkItem` runs
`gh issue view <id> --json title,body,labels` and maps JSON fields onto
`WorkItemContext`. Generalize that one shape into a validated **descriptor** the
kernel interprets, so GitLab/Gitea/Jira/Linear become **data**, not code.

---

## 1. Goal & scope

Let a project back its work items (and later its change requests) with a
non-GitHub system **without code loading**, by describing the provider
declaratively in `.adw/config.json`. The kernel ships one audited driver per
transport; the project supplies endpoints/commands + field mappings as
validated data.

**In scope for step 2 (this note):**

- `WorkItemProvider` via two transports: `type: "cli"` (shell a forge CLI, sync,
  reuses `capture()`) and `type: "rest"` (HTTP against an allowlisted host).
- The descriptor schema, a restricted response-mapping mini-language, the
  credential boundary, the host allowlist, and fail-closed validation.

**Out of scope (later increments / other slices):**

- Declarative `ChangeRequestProvider` (create/find/merge) — it is the
  merge-authorized path; designed as sub-step **2c** with extra care (§13).
- `VcsProvider` — VCS stays the built-in `git`. Git is universal across forges;
  branch/commit/push are not a provider variable (cf. invariant: the
  orchestrator owns all git). No `type` other than `git` is planned for `vcs`.
- Out-of-process code plugins (Option C) — still a §10 hard stop, its own slice.
- Overriding `classify` — its in-process Zod path is untouched (cf.
  `DESIGN-schema-overrides.md`).

## 2. Where it plugs in

The §8j registry already dispatches provider *kind* through per-role tables in
`providers.ts` and fails closed on an unknown kind. This note adds two new
work-item kinds:

```
WORK_ITEM_PROVIDERS = {
  github: createGitHubWorkItemProvider,   // existing
  cli:    createCliWorkItemProvider,      // NEW — descriptor-driven, sync capture()
  rest:   createRestWorkItemProvider,     // NEW — descriptor-driven, HTTP
}
```

`config.ts` already shape-validates `type` as a non-empty string (§8j); the
registry already owns membership. So adding these is a registry entry plus the
driver + descriptor schema — **no further config-schema `type` change**. The
descriptor *body* (routes/map/auth) is new config surface, validated by a new
loader (below), kept off `AdwConfigSchema`'s hot path the same way
`schemas`/`customPhases` are validated lazily at resolve/preflight.

## 3. The descriptor

A work-item descriptor names, per provider method, how to obtain the data and
how to map the response onto the kernel's shape. `cli` and `rest` share the
`map` grammar and differ only in the `request` they describe.

### 3.1 `cli` (sync, reuses `capture()`)

```jsonc
"providers": {
  "workItems": {
    "type": "cli",
    "authEnv": "GITLAB_TOKEN",          // OPTIONAL credential NAME (see §7)
    "routes": {
      "fetch": {
        "command": ["glab", "issue", "view", "{id}", "--output", "json"],
        "map": { "title": "$.title", "body": "$.description", "labels": "$.labels[*]" }
      },
      "state": {
        "command": ["glab", "issue", "view", "{id}", "--output", "json"],
        "map": { "state": "$.state" }   // single scalar → the state string
      }
      // postProgress / assignSelf / setStatus: OPTIONAL routes; absent ⇒ no-op
    }
  }
}
```

`{id}`, `{repo}`, and (for write routes) `{status}`, `{body}` are the only
placeholders, substituted as **separate argv elements** (never shell-joined —
`capture()` uses `spawnSync` with no shell, so there is no word-splitting or
injection through the value). A placeholder may only appear as a whole argv
token or inside one; the substituted value is passed verbatim.

### 3.2 `rest` (HTTP)

```jsonc
"providers": {
  "workItems": {
    "type": "rest",
    "baseUrl": "https://gitlab.example.com/api/v4",
    "allowedHosts": ["gitlab.example.com"],   // REQUIRED (see §8)
    "authEnv": "GITLAB_TOKEN",                 // REQUIRED for rest (see §7)
    "authScheme": "Bearer",                    // OPTIONAL, default "Bearer"
    "routes": {
      "fetch": {
        "method": "GET", "path": "/projects/{repo}/issues/{id}",
        "map": { "title": "$.title", "body": "$.description", "labels": "$.labels[*].name" }
      },
      "state": {
        "method": "GET", "path": "/projects/{repo}/issues/{id}",
        "map": { "state": "$.state" }
      }
    }
  }
}
```

The resolved URL is `baseUrl + path` with placeholders percent-encoded. The
request carries exactly one header: `Authorization: <authScheme> <token>` (§7).
No request body for reads; write routes (later) add a JSON body template.

## 4. Response field mapping — resolves §8 q1 (no JSONPath dependency)

The `map` values are a **restricted path mini-language**, not full JSONPath, and
we ship a tiny audited evaluator rather than add a dependency (mirroring the
`ajv`-reluctance in `DESIGN-schema-overrides.md` §6.2 — and unlike ajv there is
no standard to be compatible with here, so a 30-line evaluator wins).

Grammar (validated at load; anything else is a loud `AdwError`):

| Form          | Meaning                                            |
| ------------- | -------------------------------------------------- |
| `$.a.b.c`     | nested object lookup                               |
| `$.a[0]`      | array index                                        |
| `$.a[*]`      | the array itself, each element coerced to string   |
| `$.a[*].name` | map each element to its `.name`, coerced to string |

- A mapping that targets a `WorkItemContext` **string** field (`title`, `body`,
  the `state` scalar) resolves to a string (missing ⇒ `""`, matching the GitHub
  provider's tolerant coercion).
- The `labels` field expects an array form (`$.x[*]` / `$.x[*].name`) and yields
  `string[]` (missing/non-array ⇒ `[]`, again matching today's behavior).
- No wildcards beyond `[*]`, no filters, no recursion, no `..`. This is
  deliberately under-powered: it covers every real forge issue shape while being
  trivially safe to evaluate (pure data walk, no `eval`, no code paths).

The evaluator and the `WorkItemContext` assembly live next to — and reuse the
coercion intent of — `fetchWorkItem` in `issue.ts`, so the built-in and
declarative paths produce byte-identical `WorkItemContext` objects for
equivalent inputs.

## 5. Transport

### 5.1 `cli` — `capture()` + scoped env

Substitute placeholders into the `command` argv, then run it through `capture()`
(sync `spawnSync`) **with a scoped env** built by the existing audited
`safeSubprocessEnv` (§7), parse stdout as JSON (reusing `ghJson`'s tolerant
parse), and apply `map`. This is the exact mechanism the GitHub provider uses,
minus the hardcoding — no new transport, no new dependency.

`capture()` today inherits the parent env (it has no `env` option). We add an
**optional** `capture(cmd, { env })` parameter (back-compatible: absent ⇒ inherit
as now). Only the declarative `cli` driver passes `env`; the GitHub `gh` path is
untouched. No `...process.env` spread is introduced (the `lint:env` gate and its
test stay green — `safeSubprocessEnv` is the only env builder).

### 5.2 `rest` — kernel-owned one-shot fetch helper — resolves §8 q3 (transport)

The provider interface is **synchronous** (the whole control plane is
`spawnSync`-sequential); Node has no synchronous `fetch`. Three options were
weighed:

| Option | Verdict |
| --- | --- |
| Make provider methods `async` | Rejected — ripples through the orchestrator, the legacy `OrchestratorDeps` adapter, and every call site for one provider kind. |
| Shell `curl` | Rejected — adds a runtime dependency and tends to put the token in argv (visible to `ps`); `curl --config -` mitigates but is awkward. |
| **Kernel-owned one-shot fetch helper** | **Recommended.** |

The kernel ships a tiny in-tree script (`src/internal/rest-fetch.mjs`) run via
`spawnSync(process.execPath, [helper], { env: scopedEnv, input: JSON.stringify(req) })`:
the child reads the request (method, url, authScheme) from **stdin**, reads the
token from its **scoped env** (never argv), performs one `await fetch()` with
Node's built-in client, and writes `{ status, body }` JSON to stdout. The parent
applies `map`. Synchronous from the parent's view; async `fetch` in the child;
**no new dependency**; the token is in neither argv nor the parent's exposure;
and the child runs with *only* the scoped env — process-level isolation, in the
spirit of the runner seam. The helper is **kernel code (in-tree, reviewed)** —
it is NOT a project plugin, so this is not the rejected Option A/C code-loading.

## 6. (reserved)

## 7. Credential boundary — one named credential

The descriptor names **at most one** credential, by env-var **name**
(`authEnv`), never a value. The kernel reads `process.env[authEnv]` and injects
it — into the `cli` child's env via `safeSubprocessEnv({ allowGhToken: false,
extraAllow: [authEnv] })`, or into the `rest` helper's `Authorization` header
(passed through that helper's scoped env). Guard rails, validated at load
(loud `AdwError`):

- `authEnv` MUST NOT be `GH_TOKEN` or `GH_BIN` (the orchestrator's GitHub
  authority is never handed to a declarative provider — invariant §3.7).
- `authEnv` MUST NOT match `ENV_DENY_PREFIXES` (`MATRIX_`, `MX_AGENT_`).
  `safeSubprocessEnv` already drops deny-prefixed `extraAllow` keys; we *also*
  reject them at load so the descriptor fails loudly rather than silently.
- `authEnv` MUST NOT be one of the runner credential keys (`ANTHROPIC_*`,
  `OPENAI_*`, `CODEX_*`, `OPENCODE_*`, `CLAUDE_*`, `PI_*`) — a provider gets its
  *own* forge token, not a model credential. (Validated against a small
  blocklist derived from `RUNNER_ENV_ALLOW`.)

A `cli` child therefore receives only `BASE_ENV_ALLOW` (`PATH`, `HOME`, …) plus
its one forge token — strictly **tighter** than today's `gh`, which inherits the
full ambient env. `HOME` is allowed, so a CLI that stores its own auth under
`~/.config/<tool>` (e.g. after `glab auth login`) works with `authEnv` omitted.

## 8. Host allowlist (`rest`) — resolves §8 q2 (per-provider)

`rest` requires a **per-provider** `allowedHosts: string[]` (exact host match,
no wildcards in v1). Before every request the kernel asserts the resolved URL's
host is in the list; otherwise loud `AdwError`, no request made. Per-provider
(not a global egress list) because providers are independent and a shared list
couples unrelated config; it also keeps the blast radius of a config edit to
that one provider. `baseUrl` and every route `path` must be `https://` (or
resolve under an `https` `baseUrl`); `http://` and any non-http(s) scheme are
rejected — the "no remote fetch of executable definitions / local-only code"
posture of `DESIGN-provider-plugins.md` §6 applied to data egress.

## 9. Fail-closed validation (load + preflight)

Mirror the `schema-override.ts` loader posture and `validatePhaseChain`:

- The descriptor is parsed by a dedicated Zod schema; unknown keys rejected
  (`strict`), every route's `map` parsed against the §4 grammar, `authEnv`
  checked against §7's blocklist, `rest` requires `allowedHosts` + https.
- Validation runs **eagerly at provider construction** (`createProvidersFromConfig`
  in `defaultDeps`, before the dry-run branch and before any side effect — the
  same run-start fail-closed point §8j already established). A `--dry-run`
  doubles as a descriptor check.
- `fetch` and `state` routes are **required**; `postProgress`/`assignSelf`/
  `setStatus` are optional and **no-op when absent** (best-effort, exactly like
  the GitHub provider no-ops without `gh`). This resolves §8 q4 (progress
  posting needs no special brokering in v1 — it is just an optional write route;
  if it later proves hot, a dedicated batched route is an additive change).

## 10. Interface fit & code layout

The driver implements the existing **synchronous** `WorkItemProvider` interface
unchanged — no interface churn, so `providerBackedDeps` and the orchestrator are
untouched. Proposed files:

- `src/provider-descriptor.ts` (new) — descriptor Zod schema, the §4 path
  evaluator, `WorkItemContext` assembly, the §7 credential guard, the §8 host
  guard. The lone "interpret descriptor data" module.
- `src/providers-rest-cli.ts` (new) — `createCliWorkItemProvider(descriptor)`
  and `createRestWorkItemProvider(descriptor)`, returning the standard
  `WorkItemProvider`. Registered in `providers.ts`'s `WORK_ITEM_PROVIDERS`.
- `src/internal/rest-fetch.mjs` (new) — the one-shot fetch helper (§5.2).
- `providers.ts` — two registry entries; reads the descriptor from
  `config.providers.workItems` (the registry factory signature gains the config
  slice it needs).

Acyclic: `providers.ts` → `providers-rest-cli.ts` → `provider-descriptor.ts` →
(`issue.ts` coercion, `env.ts`, `exec.ts`). No back-edges; `config.ts` still
imports nothing from these.

## 11. Invariants preserved

- **Orchestrator owns all git/gh** (§3.7): a work-item provider only *reads*
  issue data and posts best-effort progress; it has no git/gh authority and
  never receives `GH_TOKEN`. VCS stays built-in `git`.
- **Secret boundary** (`env.ts`): unchanged and reused, not reconfigured. A
  declarative provider gets one named, non-deny, non-`GH_TOKEN`, non-model
  credential via the existing `extraAllow`. No new `...process.env` spread.
- **Fail closed**: unknown kind (registry, §8j), malformed descriptor, bad
  `authEnv`, off-allowlist host, or non-https URL ⇒ loud `AdwError` at run start.
- **No code loading**: the descriptor is data; the only executed code is the
  in-tree driver and the in-tree fetch helper. Options A/D remain rejected.
- **Built-ins unchanged**: `github`/`git` paths and the committed config are
  byte-for-byte unaffected; the dry-run baseline stays identical.

## 12. Rollout sub-steps

1. **2a — `cli` work-item provider. ✅ DONE** (`HANDOVER.md` §8k). Descriptor
   schema + path evaluator + credential guard + `capture(cmd, {env})` +
   `createCliWorkItemProvider` + registry entry. Sync-native, no new dependency.
   Tests + docs landed; built-in `github`/`git` and the dry-run baseline
   unchanged.
2. **2b — `rest` work-item provider. ✅ DONE** (`HANDOVER.md` §8l). Adds the
   `allowedHosts`/https guard + percent-encoded path placeholders + the inline
   kernel fetch helper + `createRestWorkItemProvider` (read routes `fetch`/
   `state`; write routes deferred). Tests use an injected transport; the real
   transport is verified via a two-process loopback roundtrip. No new dependency.
3. **2c — declarative `ChangeRequestProvider`. ✅ DONE** (`HANDOVER.md` §8m).
   The merge-authorized path over `rest`: `findForBranch`/`create`/`squashMerge`
   (required) + optional `pipelineStatus`, with request-body templating and the
   same host allowlist + https + scoped credential as every rest route.
   `squashMerge` is bound by those same checks; the orchestrator still owns the
   gating and all git. **pipelineStatus** is a simple forge-status→`CiState`
   `stateMap` with empty `failingJobs` (job-log extraction deferred); a `cli`
   change-request provider is a symmetric follow-up (rest covers the forges).

## 13. Testing strategy

- `cli`: inject a fake `capture` (the test seam already used in
  `providers.test.ts`) returning canned JSON; assert `WorkItemContext` equality
  with the GitHub path for an equivalent payload; assert scoped env is built via
  `safeSubprocessEnv` (no `GH_TOKEN`, no deny-prefixed key).
- `rest`: unit-test the path evaluator and guards directly; integration-test the
  driver against a localhost `http`→ rejected and an allowlisted loopback
  `https` (or a mocked helper) → mapped.
- Fail-closed: a descriptor with a bad `map` form, `authEnv: "GH_TOKEN"`, an
  off-allowlist host, or an `http://` baseUrl each throws at construction.
- Parity: the `github` built-in and the committed dry-run output are unchanged.

## 14. Open questions

Resolved here: JSONPath dependency (→ no, tiny evaluator, §4); host allowlist
shape (→ per-provider, §8); `rest` transport (→ kernel-owned one-shot fetch
helper, §5.2); progress brokering (→ optional no-op route, §9).

Remaining for later sub-steps:

- **2c** change-request semantics across forges (GitLab MR vs Gerrit changeset)
  and whether `squashMerge` should require an extra explicit opt-in flag.
- Pagination for forges whose issue read needs multiple pages (none of
  `fetch`/`state` do today; a `rest` `paginate` hint is an additive future
  field).

## 15. Non-goals

- Not implementing now — this is the design gate (sign-off first, per the repo's
  design-first pattern for boundary work).
- Not a `vcs` provider other than `git`, not in-process code plugins, not a
  `classify` override.
- Not granting a provider git/gh or merge authority; change-request *effects*
  (2c) are still brokered under the orchestrator's ownership.
