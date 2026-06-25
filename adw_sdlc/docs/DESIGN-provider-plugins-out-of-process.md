# Scoping — out-of-process provider plugins (Option C, §11 #4 step 3)

**Status:** scoping note, **not** a build authorization. This is the dedicated
design gate for **step 3** of the provider-extensibility rollout
(`DESIGN-provider-plugins.md` §5 / §10 hard stop): letting a project register a
provider implemented as **project-supplied code**, run **out of process**. Step 1
(registry, `HANDOVER.md` §8j) and step 2 (declarative `cli`/`rest`,
`DESIGN-declarative-providers.md`, §8k–§8m) are done.

**Headline recommendation (read this first):**

> **Do not implement step 3 yet.** For the realistic provider space (GitLab,
> Gitea, Jira, Linear, …), #4 is functionally complete at step 2: a project can
> drive work items and change requests on a non-GitHub forge via CLI or HTTP, as
> validated data, with a kernel-enforced host allowlist and **zero code loading**.
> Out-of-process *code* plugins add a materially larger attack surface for a
> long-tail need that has not materialized. If the long tail grows, **extend the
> declarative descriptor first** (step 2.5 below) — it stays data and keeps the
> egress allowlist. Reserve out-of-process code plugins for providers that are
> genuinely *code-shaped*, and only with the trade-offs in §4 understood. Keep
> this design on file; build §6 step 3a only when a **concrete** provider that
> declarative cannot express appears.

---

## 1. The demand gate — is step 3 needed?

Step 3 is justified only by providers that **declarative data cannot express**.
What step 2 already covers: any single-request-per-method REST or CLI provider
with JSON field mapping, templated paths/bodies, one bearer-style credential, and
a static status→state map. That is the overwhelming majority.

What declarative cannot express today (the precise gap):

- **Stateful / multi-step auth** — OAuth refresh-token exchange before a call,
  AWS SigV4 or HMAC request signing, short-lived token minting.
- **Pagination loops** — "follow `next` cursor until exhausted" to assemble a
  list (e.g. all failing CI jobs).
- **GraphQL** — build a query string, parse a deeply nested / computed response.
- **Computed mappings** — derive a value from several fields with real logic
  (not a single JSONPath).
- **Multi-request flows** — create-then-set-labels-then-assign as one logical op.

No provider in scope today needs these. So **step 3 stays demand-gated**: design
now, build when a concrete provider forces it.

## 2. Prefer extending declarative first (step 2.5)

Before reaching for code, most of §1's gap can be closed with **bounded
declarative primitives** — still data, still kernel-executed, still
host-allowlisted, **no new code-execution surface**:

- a `paginate` hint on a `rest` route (`cursorPath` + `itemsPath`, kernel loops
  with a bounded page cap);
- a `refresh` sub-route (a token-exchange request the kernel runs and caches
  in-memory for the run, injecting the result as the credential);
- a small, fixed transform vocabulary in the map mini-language (e.g. `lower`,
  `join`, `coalesce`) — closed set, no `eval`.

This is strictly preferable to step 3 for anything expressible as "a few more
declarative knobs": it preserves every step-2 guarantee. Recommend exhausting
2.5 before authorizing 3.

## 3. If built — architecture

An out-of-process plugin is the **code** analogue of the declarative `rest`
driver: the kernel spawns a **separate process** that speaks a tiny JSON protocol
over stdio and returns provider-shaped data. It reuses the exact mechanism step
2b proved — `spawnSync(process.execPath-or-config-command, …, { input, env })`
with a **scoped env from `safeSubprocessEnv`** — except the spawned program is
**project code**, not the kernel's fixed helper.

- **Loading = spawning, never importing.** Config names a *local command*
  (`{ "type": "plugin", "command": ["node", "./.adw/plugins/forge.mjs"], "authEnv": "FORGE_TOKEN" }`),
  the same trust level as the already-trusted `commands.defaultTestCommand` /
  `loops.*.command`. **Never** in-process `import()` (Option A) or a `vm`/
  permission shim (Option D) — both rejected in `DESIGN-provider-plugins.md` §4.
  No remote module specifiers (local-only).
- **Sync constraint → one-shot per call.** Providers are synchronous (the whole
  control plane is `spawnSync`-sequential; step 2 rejected making them async).
  So the MVP is **stateless one-shot**: spawn per method call, method+args on
  stdin, result on stdout — identical to the step-2b rest helper. A plugin that
  needs cross-call state (cached OAuth token) self-caches to a file under `HOME`
  (in its scoped env). A **long-lived stateful broker** (spawn once, blocking
  sync stdio RPC) is a *later* increment, only if one-shot proves too costly.
- **No git/gh authority.** The RPC surface is provider-shaped only
  (fetch/state/create/squashMerge/findForBranch/pipelineStatus/postProgress). The
  plugin cannot run git/gh and never receives `GH_TOKEN`; the orchestrator still
  owns all local git (branch/commit/push) and the gating — invariant §3.7.
- **Per-call timeout + structured errors + a versioned protocol envelope**
  (`{ v, method, args }` → `{ ok, value | error }`); a crash/timeout/garbage
  reply fails that call closed (null/UNKNOWN), never the run silently.

## 4. The security trade-off vs declarative (the load-bearing finding)

Process + env isolation protects the **high-value assets**: a plugin never sees
`GH_TOKEN`, `ANTHROPIC_*`, `MATRIX_*`, `MX_AGENT_*` (scoped `extraAllow` +
deny-prefixes), cannot touch the orchestrator's heap, and has no merge/push
authority. That part is solid.

**But there is an asymmetry the kernel cannot close without OS-level
sandboxing:** the declarative `rest` driver routes *every* request through the
kernel's `assertAllowedHost` + https check, so a config edit can only redirect to
an *allowlisted* host. An out-of-process plugin makes its **own** network calls —
the kernel cannot constrain its egress. A hostile plugin (scenario 2:
supply-chain via a PR editing `.adw/config.json`) therefore runs as **trusted
code with network + filesystem (repo) access** — it can exfiltrate the working
tree, just like a hostile `defaultTestCommand` or a hostile agent runner already
could. It still cannot get `GH_TOKEN` or merge.

So **Option C is a genuine step down in assurance from step 2**, not a free
addition. State this plainly to anyone enabling it:

- Declarative (step 2) = no code, kernel-enforced egress. **Preferred.**
- Plugin (step 3) = arbitrary code, isolated from secrets/merge/heap, but
  **not network-sandboxed** in the MVP. Treat a plugin as trusted at the level
  of a configured build command.

Two ways to recover egress control (both heavier, both later):

- **(c-i) OS sandbox** — run the plugin process under a network namespace /
  seccomp / firejail-style jail. Platform-specific, out of scope for this TS
  port's MVP; a deployment concern, not a kernel feature.
- **(c-ii) Intent coroutine** — the plugin does *no* I/O; it is a pure state
  machine that, given `(method, args, lastResponse?)`, returns either a
  **request intent** (method/path/headers/body) the kernel executes against the
  **host allowlist**, or a final result. This keeps arbitrary plugin *logic*
  (pagination, GraphQL, conditional flows) while every byte of egress stays
  kernel-mediated and allowlisted — the elegant high-assurance shape. It only
  fully closes egress when paired with (c-i) to deny the plugin its own sockets;
  otherwise it is architectural hygiene, not a hard boundary. Recommended target
  *if* a high-assurance deployment ever needs code + egress control.

## 5. Invariants any implementation must preserve

Restating `DESIGN-provider-plugins.md` §6 in step-3 terms:

- Spawn through `safeSubprocessEnv` with a provider-scoped `extraAllow` of **one
  named credential** (the same guard as step 2: never `GH_TOKEN`/`GH_BIN`, never
  deny-prefixed, never a model credential). Never a wildcard; never the parent
  env spread.
- Plugin returns **data/intent**; the kernel performs every git effect. No raw
  git/gh in the RPC surface.
- **Fail closed** at run start: unknown/unreadable plugin config, a non-local
  command, or a missing credential is a loud `AdwError` from
  `createProvidersFromConfig` (before the dry-run branch), exactly like step 2.
- **Static check (lint:env extension):** assert the plugin host builds the child
  env via `safeSubprocessEnv` and never widens the allowlist or spreads
  `process.env` — the same fail-closed posture as `scripts/check-adw-sdlc-env.sh`.

## 6. Rollout (only on demand)

1. **3a — protocol + spawn harness + one method.** The JSON envelope, a
   `spawnSync`-per-call transport with scoped env + timeout, and `fetch` for a
   work-item plugin, proven against a reference mock-plugin fixture. Smallest
   thing that demonstrates the boundary end-to-end.
2. **3b — full work-item + change-request RPC surface** behind `type: "plugin"`
   in both registries.
3. **3c (optional) — long-lived broker** if one-shot per-call is too costly, or
   **the intent coroutine (c-ii)** if a deployment needs code + kernel-enforced
   egress.

## 7. Testing strategy (when built)

- A committed **reference plugin fixture** (a tiny mock that echoes/canned-
  responds) spawned in tests — protocol conformance, arg round-trip, result
  mapping.
- **Env-scoping assertions**: the spawned child's env carries exactly the one
  credential, never `GH_TOKEN`/deny-prefixed (poisoned-source test, mirroring the
  env-isolation suite).
- **Failure modes**: timeout, non-zero exit, malformed/garbage stdout, oversized
  output → each fails that call closed without aborting the run.
- Built-ins and the dry-run baseline unchanged; the `lint:env` extension green.

## 8. Open questions

- One-shot vs long-lived broker as the *default* (MVP says one-shot; revisit if
  OAuth-refresh cost dominates).
- Egress: ship MVP as trusted-code (§4) and document, or block step 3 until (c-i)
  or (c-ii) exists? (Recommend: ship trusted-code MVP **only** with prominent
  docs, or skip straight to (c-ii) for a high-assurance posture — decide with the
  first real consumer.)
- Protocol reuse: a bespoke minimal envelope vs. leaning on the runner seam's
  transport. (Recommend bespoke + minimal; the rest-helper precedent is closer.)
- Windows: `spawnSync` + the command form behave differently; declare supported
  platforms.

## 9. Non-goals

- **Not** building step 3 now — this is the gate, and the recommendation is to
  stay at step 2 (+ optional 2.5) until a concrete need.
- **Not** Option A (in-process `import` of config-supplied code) or Option D
  (`vm`/permission shim) under any phrasing.
- **Not** granting a plugin git/gh/merge authority or `GH_TOKEN`.
- **Not** OS-level network sandboxing in the MVP (a deployment concern).
