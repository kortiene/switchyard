# Design proposal — provider plugin loading, security pass (§11 #4)

**Status:** design agreed; **staged rollout underway**. This is the dedicated
security/sandboxing design pass the handover (`HANDOVER.md` §10 #1, §11 #4)
required *before* any code is written, mirroring the way
`DESIGN-schema-overrides.md` settled the structured-output path before
implementation.

Rollout progress against §5:

- **Step 1 — open the factory switch internally (no plugins): ✅ DONE**
  (`HANDOVER.md` §8j). `createProvidersFromConfig` now dispatches provider
  *kind* through a per-role registry in `providers.ts`; `config.ts`
  shape-validates the `type` string while the registry owns membership and
  fails closed with a loud `AdwError`. Pure kernel work, no new dependency, no
  code-loading surface — behavior-preserving for the `github`/`git` built-ins.
- **Step 2 — declarative `rest`/`cli` driver (Option B): ✅ DONE.** Concrete
  design in `docs/DESIGN-declarative-providers.md` (descriptor schema, a
  dependency-free response-mapping mini-language, the one-named-credential
  boundary via `safeSubprocessEnv`, a per-provider host allowlist, and a
  kernel-owned synchronous fetch helper; resolves the four §8 open questions).
  **All sub-steps landed**: 2a (`cli`) + 2b (`rest`) work-item providers and 2c
  (`rest`) change-requests (`HANDOVER.md` §8k, §8l, §8m), the `cli`
  change-request provider (`HANDOVER.md` §8o), and the step-2.5 primitives —
  transforms + pagination (`HANDOVER.md` §8n). Step 2 is complete for work items
  and change requests across **both** `cli` and `rest`; only step-2.5c token
  refresh remains deferred.
- **Step 3 — out-of-process plugin (Option C): scoped, demand-gated, NOT built.**
  Scoping in `docs/DESIGN-provider-plugins-out-of-process.md`. Recommendation:
  do not build it yet — step 2 covers the realistic provider space, and a code
  plugin cannot enforce the host allowlist a declarative provider can (it does
  its own egress; §4 there). Prefer extending the declarative descriptor first
  (**step 2.5**, `docs/DESIGN-declarative-providers-extensions.md`: transforms,
  pagination, token refresh — all still data). Reserve step 3 for genuinely
  code-shaped providers (GraphQL, request signing, branching flows).

The code-loading boundary (Options A/D) remains rejected; nothing below it ships
until the boundary is implemented as designed.

**Author context:** follows the universalization slices already landed —
configurable phase chain (§8b), terminal done-status (§8c), per-phase schema
overrides + custom phases (§8e–§8g), and custom-phase startup validation (§8h).
Those slices made *policy* (prompts, gates, schemas, phase names) project-
configurable while keeping *mechanism* in the kernel. Provider plugin loading is
categorically different: it asks the kernel to run **project-supplied code**, not
project-supplied data. That difference is the whole subject of this document.

---

## 1. Goal

Let a project register its own `WorkItemProvider` / `ChangeRequestProvider` /
`VcsProvider` / `ProviderCli` implementation — e.g. GitLab issues, Jira work
items, Linear, a Phabricator/Gerrit change-request flow — **without forking the
kernel**, by pointing `.adw/config.json` at a project-supplied module.

`createProvidersFromConfig` (`providers.ts`) was originally a closed switch over
`type: 'github' | 'git'`; step 1 (§5) has since opened it into a fail-closed
per-role **registry** of in-tree factories (now also `cli`/`rest`). The remaining
plugin goal is the one piece a registry cannot give: loading a *project-supplied*
provider — `type: 'plugin'` + a module path — without an in-tree edit. That code
path is the demand-gated, still-unbuilt step 3.

## 2. Why this is a hard stop, not a slice

Every universalization slice so far moved **data** across the boundary:

| Slice | Crosses as | Worst case if hostile |
| --- | --- | --- |
| prompts (`.pi/prompts`) | template text → an LLM | a bad prompt; the agent has no secrets (§D5 env wall) |
| phase chain / custom phases | strings validated vs. a catalog | a loud `AdwError` at parse/preflight |
| schema overrides | JSON Schema → ajv (data) | ajv rejects the payload; remote `$ref` already banned |

A provider plugin crosses as **executable code that runs in the orchestrator
process**. The orchestrator is precisely the component the entire secret boundary
exists to protect:

- It holds `GH_TOKEN` and reads `process.env` directly (the runners never do —
  `env.ts` withholds everything from agent children; the orchestrator is the
  trusted side of that wall).
- It performs all git/gh side effects (branch, push, PR create, **squash-merge**).
- It runs with the user's full ambient environment, filesystem, and network.

So a plugin is not "a new provider"; it is **arbitrary code execution inside the
trusted, secrets-owning, merge-authorized process**. `new Ajv().compile(schema)`
validates data and cannot reach the network or disk; `await import(pluginPath)`
runs whatever the module's top level and methods choose to. There is no
in-process configuration of ajv that turns code loading into data loading — they
are different operations. That is why the handover classified this as a §10 hard
stop and not a continuation slice, and why it gets its own threat model first.

## 3. Threat model

**Asset:** `GH_TOKEN` (and any other secret in the orchestrator's `process.env`),
plus the repository's git history and the authority to merge to the default
branch.

**Trust levels (today):**

- *Kernel code* — fully trusted, in-tree, reviewed, lint-gated (`lint:env`).
- *Runner children* — untrusted with secrets by construction (`env.ts` allowlist,
  no `GH_TOKEN` in phased mode); trusted only to edit the working tree.
- *Project pack data* (`.adw/*`, prompts, schemas) — trusted to shape policy,
  validated, but never executed.

**The new actor — a provider plugin — would by default be:** fully trusted (runs
in-process) while originating from the **least controlled** place (a path in a
repo's `.adw/config.json`, which a PR author or a compromised dependency can
edit). That inversion is the core risk.

**Attack scenarios if a plugin is loaded naively (`await import(configPath)`):**

1. **Secret exfiltration.** `WorkItemProvider.fetch()` reads
   `process.env.GH_TOKEN` (or `ANTHROPIC_API_KEY`) and POSTs it to an attacker
   host. Nothing in the current architecture stops this — the env wall protects
   *children*, and a plugin is not a child.
2. **Supply-chain via config.** A PR edits `.adw/config.json` to repoint the
   plugin at `./.adw/evil.js`; CI runs ADW on the PR branch and executes it
   before any human reads the diff. This is the classic "config is code" CI
   escalation.
3. **Malicious merge / history rewrite.** A plugin's `ChangeRequestProvider`
   methods run inside the merge-authorized process; it can `squashMerge` early,
   merge a different branch, or shell out to `git push --force`.
4. **Transitive dependency.** The plugin `require`s an npm package that is later
   compromised; the blast radius is the orchestrator's full trust.
5. **Import-time side effects.** Code at the module's top level runs the instant
   it is imported, before any method is called — so "we only call the documented
   methods" is not a mitigation.

**Out of scope for the threat model (already handled elsewhere):** the agent
children (covered by `env.ts`), and JSON-Schema data overrides (covered by
`DESIGN-schema-overrides.md` — remote `$ref` already rejected).

## 4. Options

Ordered roughly by isolation strength. The tension throughout: a provider must
*do effectful work* (call `gh`/`glab`, hit an HTTP API), so "no I/O" sandboxes
are not viable — the question is how to grant *scoped* I/O without granting
*ambient secret + merge* authority.

### (A) In-process `import()` of a config-supplied path — REJECTED

What "naive plugin loading" means. The plugin shares the orchestrator's heap,
`process.env`, fd table, and network. Every §3 scenario applies. No amount of
"only call documented methods" helps, because import-time code and `process`
access are unrestricted. **This is the option the hard stop exists to prevent;
it is documented here only to be explicitly rejected.**

### (B) Declarative providers (no code at all) — RECOMMENDED FIRST STEP

Most real targets (GitLab, Gitea, Jira, Linear) are **REST/CLI shaped**. Instead
of code, a project supplies a **declarative descriptor**: endpoint templates,
auth-env-var *names* (not values), and JSON field mappings. The kernel ships one
audited, generic HTTP/CLI driver that the descriptor parameterizes.

```jsonc
"providers": {
  "workItems": {
    "type": "rest",
    "baseUrl": "https://gitlab.example.com/api/v4",
    "authEnv": "GITLAB_TOKEN",            // a NAME; kernel reads it, plugin never sees others
    "routes": {
      "fetch":  { "method": "GET", "path": "/projects/{repo}/issues/{id}",
                  "map": { "title": "$.title", "body": "$.description", "labels": "$.labels[*]" } }
    }
  }
}
```

This keeps **mechanism in the kernel** (the same principle as every prior slice):
the kernel owns the fetch/exec, injects *only* the one named credential, and the
project owns *policy* (which endpoint, which fields) as **validated data**. No
arbitrary code, so §3 scenarios 1/3/4/5 vanish and scenario 2 degrades to "a
config change can point at a different *URL*" — reviewable in the diff, and
constrainable with an allowlist of hosts. Covers the majority of providers
without ever opening the code-loading door. Cost: a generic driver + a descriptor
schema (real work, but it is *data* work).

### (C) Out-of-process plugin (subprocess / worker) with a brokered, least-privilege channel

For providers genuinely needing code, run the plugin as a **separate process**
the kernel spawns, speaking a small JSON-RPC protocol over stdio — the same shape
as the runner seam, which already proves this model. Critically, the kernel
spawns it through the **existing `safeSubprocessEnv` allowlist** (`env.ts`) with a
provider-specific `extraAllow` of *only* that provider's credential — so the
plugin process never receives `GH_TOKEN`, `ANTHROPIC_*`, `MATRIX_*`, or
`ADW_*` or legacy `MX_AGENT_*` (the deny-prefixes already guarantee those). The plugin gets
scoped I/O but not ambient secrets; it cannot merge because it has no git/gh
authority (the orchestrator still owns all VCS effects — invariant §3.7). Cost:
protocol design, lifecycle/timeout management, and serialization of the provider
interface; heavier than (B) but the right tool when a provider needs real logic
(pagination, OAuth refresh, GraphQL).

### (D) In-process with a hardened VM/permission boundary — REJECTED for secrets

Node's `vm` module is **not a security boundary** (documented as such); `import`
maps + `--experimental-permission` are coarse and process-wide, not per-module.
None reliably stops a determined in-process module from reaching `process.env` or
the network. Rejected for a secrets-owning process: it offers the *appearance* of
isolation without the guarantee, which is worse than (C)'s honest process wall.

## 5. Recommendation

A staged path that delivers provider extensibility while keeping the code-loading
door shut until it is genuinely required:

1. **Open the factory switch internally (no plugins). ✅ DONE** — Refactored
   `createProvidersFromConfig` so provider *kind* dispatches through a per-role
   registry (`providers.ts`); `config.ts` shape-validates the `type` string and
   the registry owns membership, failing closed with a loud `AdwError`. The
   in-tree GitLab/Gitea *adapters* envisaged here are deferred to their own slice
   (they need real glab/REST logic and are better expressed via step 2's
   declarative driver for the REST/CLI-shaped majority); the seam they would plug
   into now exists. This was pure kernel work, fully reviewed, no new trust
   surface.
2. **Ship the declarative `rest`/`cli` driver (Option B).** Covers the long tail
   of REST/CLI providers as validated data. Host-allowlist the endpoints; read
   exactly one named credential per provider; reuse the `assertNoRemoteRefs`-style
   fail-closed posture from the schema-override loader for the descriptor.
3. **Only if a concrete provider cannot be expressed declaratively, design the
   out-of-process plugin (Option C)** as its own slice, with: a spawn through
   `safeSubprocessEnv` (provider-scoped `extraAllow`, never `GH_TOKEN`), a
   capability-scoped RPC surface (no raw git/gh — the orchestrator brokers VCS),
   per-call timeouts, and a `lint:env`-style static check that the plugin host
   never widens the allowlist. **Never Option A; never Option D for secrets.**
   This step is now scoped in `docs/DESIGN-provider-plugins-out-of-process.md`,
   which (a) recommends **not** building it until a concrete provider forces it,
   (b) records the egress asymmetry — a code plugin cannot be held to the host
   allowlist a declarative provider is — and (c) routes most remaining demand to
   **step 2.5** (`docs/DESIGN-declarative-providers-extensions.md`) first.

This ordering means the *secret boundary is never weakened to gain provider
coverage* — (1) and (2) are the 90% case and add zero code-execution surface, and
(3) preserves the boundary by putting the untrusted code on the far side of the
same process wall the runners already live behind.

## 6. Invariants any implementation must preserve

Restating the load-bearing rules from `HANDOVER.md` §3 in plugin terms:

- The orchestrator owns ALL git/gh. A provider (declarative or out-of-process)
  **never** receives git/gh authority or `GH_TOKEN`; it returns *data/intent*,
  and the kernel performs the effect (§3.7).
- `safeSubprocessEnv` / `BASE_ENV_ALLOW` / `ENV_DENY_PREFIXES` are not
  plugin-configurable. A plugin may receive *one named credential* via a
  reviewed `extraAllow`, never a wildcard, and never a deny-prefixed key.
- Fail closed: an unknown/unreadable/over-broad provider config is a loud
  `AdwError` at load (same posture as the schema-override loader and
  `validatePhaseChain`), never a silent fallback to a more-trusted path.
- No remote fetch of executable definitions: a provider descriptor or plugin
  path must be local to the repo; no `http(s)://` module specifiers.

## 7. Non-goals

- **Not** implementing any of this now — this is the design gate, not a slice.
- **Not** Option A (in-process `import` of config-supplied code) under any
  phrasing; it is incompatible with the secret boundary.
- **Not** granting a provider the merge/push capability; VCS effects stay in the
  kernel regardless of provider source.
- **Not** changing the GitHub/Git built-ins' behavior; this is purely additive
  surface for *other* providers.

## 8. Open questions

- Does the declarative `rest` driver need a templating/JSONPath dependency, and
  if so which audited one? (Parallels the `ajv` decision in
  `DESIGN-schema-overrides.md` §6.2 — prefer one well-known, validation-only lib.)
- Host allowlist: per-provider (`allowedHosts: [...]`) vs. a global egress list?
- For Option C, is stdio JSON-RPC enough, or do some providers need streaming
  (e.g. long pagination)? Reuse the runner transport if possible.
- Should `progress` posting (the one provider call that writes to the work
  tracker mid-run) be brokered specially, since it is the most frequent effect?
