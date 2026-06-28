# Spec: `rest` work-item write methods must not be silent no-ops

- **Work item:** GitHub issue #24 — `rest work-item write methods are silent no-ops — make them FAIL CLOSED`
- **Labels / class:** `issue_class:fix`, `area:providers`, `backlog`
- **Source:** `adw_sdlc/src/providers-rest-cli.ts:414-416`; `adw_sdlc/docs/UNIVERSAL.md:218-220`; backlog assessment (PLAN.md §11 / PARITY.md / MVP-READINESS.md)
- **Status:** specification only. **Do not implement as part of this phase.**

---

## 1. Goal

Close a genuine silent-loss trap in the declarative `rest` work-item provider. Today
`createRestWorkItemProvider` returns `postProgress`/`assignSelf`/`setStatus` as `() => {}`
no-ops (`adw_sdlc/src/providers-rest-cli.ts:414-416`). Because the orchestrator calls
`setStatus(doneStatus)` to perform the terminal board transition after a verified merge
(`adw_sdlc/src/orchestrator.ts:770-779`), a project configured with
`workItems: { type: "rest", … }` **and** a `doneStatus` will silently never mark its work
item done — a real correctness loss, not merely a deferred feature.

The fix must make this configuration **either work or fail loudly at run start**, never
drop the write silently.

---

## 2. Background — verify before writing

### 2.1 What is no-op'd today

`createRestWorkItemProvider` (`providers-rest-cli.ts:380-418`) ships only the read routes
(`fetch`/`state`); the three write methods are hard-coded no-ops:

```ts
postProgress: () => {},
assignSelf: () => {},
setStatus: () => {},
```

The descriptor schema reinforces this: `RawRestWorkItemSchema.routes` is `.strict()` and
only permits `{ fetch, state }` (`provider-descriptor.ts:510-512`), so a project cannot
currently even *declare* a rest write route — adding one is rejected as an unknown key.

### 2.2 Why this is a silent-loss trap (not just a missing feature)

The orchestrator invokes work-item status writes in two places:

| Call site | Status applied | Configurable? | Loss severity |
| --- | --- | --- | --- |
| `setupBranchAndStatus` (`orchestrator.ts:749`) | `inProgressStatus` | always set (default `'In Progress'`, `config.ts:128`) | cosmetic, best-effort (wrapped in try/catch, swallowed `orchestrator.ts:750-752`) |
| `transitionToDone` (`orchestrator.ts:770-779`) | `doneStatus` | **opt-in**, unset by default (`config.ts:138`) | **loss-bearing** — the verify gate reads this same status axis via `closedStates` |

`doneStatus` is the dangerous one: an operator who sets it has *explicitly asked* for a
terminal board transition (the GitHub provider doesn't need it because `closes #<n>`
auto-closes the issue — `config.ts:129-138`). For a non-GitHub `rest` provider the board
transition is the only "done" signal, and `transitionToDone` already swallows runtime
failures as best-effort (`orchestrator.ts:777-778`). With `setStatus` a no-op, that
swallow turns a *guaranteed never-applied* write into silence.

### 2.3 The asymmetry to remove

- The `cli` work-item provider **already** supports optional `postProgress`/`assignSelf`/
  `setStatus` routes, no-op only when absent (`providers-rest-cli.ts:117-145`;
  placeholders `provider-descriptor.ts:222-228`).
- The `rest` **change-request** provider **already** performs templated-body writes
  (`create`/`squashMerge`) using `substituteBody` + `requester.request(route, vars, body)`
  (`providers-rest-cli.ts:224-242, 284-289, 465-511`).

So body-templating for `rest` writes is already proven in-repo; only the `rest`
work-item role was left read-only ("write routes deferred", `DESIGN-declarative-providers.md:314-318`).

### 2.4 Reusable building blocks (already present)

- `substituteBody(template, vars)` — deep placeholder substitution into JSON body string
  leaves, not url-encoded (`providers-rest-cli.ts:224-242`).
- `makeRestRequester(base, transport).request(route, vars, bodyTemplate)` — resolves
  `baseUrl + path`, re-asserts host allowlist + https, substitutes the body, runs through
  the scoped one-credential transport with `GH_TOKEN` withheld (`providers-rest-cli.ts:265-292`).
- `restOk(res)` (`:294`), `formatProgress(adwId, phase, message)` (`exec.ts:59`),
  `note(message)` (`exec.ts:30`).
- `AdwError` for loud, run-start fail-closed errors (`src/errors.ts:8-13`).

---

## 3. Decision

Two acceptance options are offered by the issue; this spec **recommends a hybrid** that
satisfies all three acceptance bullets at once and matches the repo's "fail closed at run
start" posture and the universal-provider/parity trajectory.

### Recommended: implement `rest` write routes **and** add a run-start fail-closed guard

1. **Implement** optional `postProgress`/`assignSelf`/`setStatus` routes for the `rest`
   work-item provider via the body-templating already proven in the change-request
   provider (Option B). This brings `rest` work-items to parity with `cli` work-items and
   the `rest` change-request provider, and makes the configured transition actually happen.
2. **Guard** against the residual misconfig: if `workItems.doneStatus` is configured but
   the resolved descriptor has **no `setStatus` route**, throw a loud `AdwError` at provider
   construction (run start) (Option A). This is the only way to make the trap *loud* —
   throwing at the `setStatus` call site would be swallowed by `transitionToDone`'s
   best-effort `try/catch`, so the check must live before any work begins.

Why both: implementing writes alone still leaves a silent drop if an operator sets
`doneStatus` but forgets the `setStatus` route. The construction guard converts that into a
run-start error. Together they fully close the trap (and the guard generalizes to the `cli`
provider, which has the identical latent drop).

### Alternative (minimal, fail-closed only)

If the maintainer prefers the smallest change for a `fix`-class issue, ship **only** the
guard (§5.3) and leave the three write methods as no-ops. This satisfies acceptance bullet
1 and bullet 3, is lower-risk, but leaves `rest` work-items permanently unable to do status
transitions or progress (capability gap persists; operators must use `github`/`cli` for
boards or never set `doneStatus` on `rest`). See §11 open question O1.

### Scope decision on "progress" (acceptance bullet 1 says "doneStatus/progress")

Progress posting is **best-effort by the established provider contract** (the GitHub
provider no-ops without `gh`; the `cli` provider no-ops without a route —
`DESIGN-declarative-providers.md:265-269`) and is not separately configurable (it is always
*attempted*). This spec therefore gates the fail-closed guard on **`doneStatus`** (the
loss-bearing, opt-in write), and keeps `postProgress`/`assignSelf` as best-effort no-ops
when unrouted — but now *implementable* via an optional route. See §11 open question O2 if
the maintainer wants progress to also be loud.

---

## 4. Owning modules

- `adw_sdlc/src/provider-descriptor.ts` — descriptor schema + compile + guard helper.
- `adw_sdlc/src/providers-rest-cli.ts` — `createRestWorkItemProvider` write methods.
- `adw_sdlc/src/providers.ts` — registry factories wire the guard.
- `adw_sdlc/test/providers.test.ts` — coverage.
- `adw_sdlc/docs/UNIVERSAL.md`, `adw_sdlc/docs/DESIGN-declarative-providers.md` — docs.

No interface churn: `WorkItemProvider` (`providers.ts:49-57`) and the orchestrator are
untouched. `config.ts` is untouched (the loose `routes` record already accepts write routes
— `config.ts:152`).

---

## 5. Implementation steps

### 5.1 Descriptor schema + compile (`provider-descriptor.ts`)

1. Add placeholder sets for the write routes (mirror the `cli`
   `ALLOWED_PLACEHOLDERS`, `provider-descriptor.ts:222-228`):

   ```ts
   const WI_POST_PROGRESS_PLACEHOLDERS = ['id', 'repo', 'body'] as const;
   const WI_ASSIGN_PLACEHOLDERS = ['id', 'repo'] as const;
   const WI_SET_STATUS_PLACEHOLDERS = ['id', 'repo', 'status'] as const;
   ```

2. Add raw zod route schemas for the three optional write routes. Each carries an optional
   templated JSON `body` (same shape as `rawCrCreateRoute.body` / `rawCrMergeRoute.body`,
   `provider-descriptor.ts:611-621`). Sensible method defaults: `POST` for postProgress and
   assignSelf, `PUT` for setStatus.

   ```ts
   const rawRestPostProgressRoute = z
     .object({ method: restMethod.default('POST'), path: z.string().min(1), body: z.record(z.string(), z.unknown()).optional() })
     .strict();
   const rawRestAssignSelfRoute = z
     .object({ method: restMethod.default('POST'), path: z.string().min(1), body: z.record(z.string(), z.unknown()).optional() })
     .strict();
   const rawRestSetStatusRoute = z
     .object({ method: restMethod.default('PUT'), path: z.string().min(1), body: z.record(z.string(), z.unknown()).optional() })
     .strict();
   ```

3. Extend `RawRestWorkItemSchema.routes` (`provider-descriptor.ts:510-512`) to allow the
   three optional routes alongside the required `fetch`/`state`:

   ```ts
   routes: z.object({
     fetch: rawRestFetchRoute,
     state: rawRestStateRoute,
     postProgress: rawRestPostProgressRoute.optional(),
     assignSelf: rawRestAssignSelfRoute.optional(),
     setStatus: rawRestSetStatusRoute.optional(),
   }).strict(),
   ```

4. Extend `RestWorkItemDescriptor.routes` (`provider-descriptor.ts:515-520`) with the
   compiled optional write routes:

   ```ts
   postProgress?: { method: string; path: string; body?: Record<string, unknown> };
   assignSelf?: { method: string; path: string; body?: Record<string, unknown> };
   setStatus?: { method: string; path: string; body?: Record<string, unknown> };
   ```

5. In `parseRestWorkItemDescriptor` (`provider-descriptor.ts:528-556`), after the
   `fetch`/`state` path checks, validate + attach each present write route. For each:
   - `assertRestPath(route.path, <placeholders>, '<routeName>')` (percent-encoded path
     placeholders; reuses the existing https/path guard `:459-467`).
   - if `route.body !== undefined`, `assertBodyPlaceholders(route.body, <placeholders>, '<routeName>')`
     (reuses `:485-493`).
   - copy `{ method, path, body }` onto the returned `routes`.

   Use `WI_POST_PROGRESS_PLACEHOLDERS` for postProgress, `WI_ASSIGN_PLACEHOLDERS` for
   assignSelf, `WI_SET_STATUS_PLACEHOLDERS` for setStatus.

6. Add the shared, provider-agnostic guard helper (used by §5.3). It lives here because
   `provider-descriptor.ts` is the validation home and imports `AdwError` already
   (`:19`); it needs no `config.ts` import (the values are passed in):

   ```ts
   /**
    * Run-start fail-closed: an opt-in terminal board transition (`doneStatus`) is a
    * loss-bearing write. If it is configured but the declarative provider has no
    * `setStatus` route to honor it, the transition would be silently dropped — so
    * refuse to build the provider instead.
    */
   export function assertStatusTransitionRoutable(
     doneStatus: string | undefined,
     hasSetStatusRoute: boolean,
     kind: 'cli' | 'rest',
   ): void {
     if (doneStatus && !hasSetStatusRoute) {
       throw new AdwError(
         `workItems.doneStatus "${doneStatus}" is configured but the ${kind} work-item provider has no ` +
           `setStatus route; the terminal board transition would be silently dropped. ` +
           `Add a setStatus route or remove doneStatus.`,
       );
     }
   }
   ```

### 5.2 Provider write methods (`providers-rest-cli.ts`)

Replace the three no-op arrows (`providers-rest-cli.ts:414-416`) inside
`createRestWorkItemProvider`. Mirror the `cli` provider's optional-route shape
(`:117-145`) and use `requester.request(route, vars, route.body)`:

```ts
postProgress: (ctx, id, adwId, phase, message) => {
  const route = descriptor.routes.postProgress;
  if (!route) return;                                  // best-effort: unrouted ⇒ no-op
  const res = requester.request(route, {
    id: String(id),
    repo: ctx.repo,
    body: formatProgress(adwId, phase, message),
  }, route.body);
  if (!ok(res)) note(`could not post progress comment for #${id} (${phase})`);
},
assignSelf: (ctx, id) => {
  const route = descriptor.routes.assignSelf;
  if (!route) return;
  requester.request(route, { id: String(id), repo: ctx.repo }, route.body);
},
setStatus: (ctx, id, status) => {
  const route = descriptor.routes.setStatus;
  if (!route) return;                                  // construction guard (§5.3) makes
                                                       // doneStatus-without-route loud at run start
  const res = requester.request(route, { id: String(id), repo: ctx.repo, status }, route.body);
  if (!ok(res)) note(`could not update status for #${id} to "${status}"`);
},
```

Notes:
- `requester`, `ok` (`restOk`), `formatProgress`, `note` are all already in scope /
  imported (`providers-rest-cli.ts:21, 384-385`).
- Runtime failures stay **best-effort + observable** (`note`), consistent with the `cli`
  provider and `transitionToDone`'s swallow. They are *not* the trap — a runtime `setStatus`
  failure for `doneStatus` is independently caught by the post-merge verify gate, which
  re-reads the real work-item state and raises `AdwError` if it is not in `closedStates`
  (`orchestrator.ts:823-827`). Defense in depth: construction guard (misconfig) + verify
  (runtime).
- Update the function's doc comment (`providers-rest-cli.ts:377-379`) — it currently says
  "the write methods are no-ops here pending request-body templating".

### 5.3 Wire the fail-closed guard (`providers.ts`)

Update the `rest` (and `cli`) work-item factories (`providers.ts:246-247`) to call the
guard after parsing, before returning the provider:

```ts
cli: (config) => {
  const d = parseCliWorkItemDescriptor(config.providers.workItems);
  assertStatusTransitionRoutable(config.providers.workItems.doneStatus, d.routes.setStatus !== undefined, 'cli');
  return createCliWorkItemProvider(d);
},
rest: (config) => {
  const d = parseRestWorkItemDescriptor(config.providers.workItems);
  assertStatusTransitionRoutable(config.providers.workItems.doneStatus, d.routes.setStatus !== undefined, 'rest');
  return createRestWorkItemProvider(d);
},
```

Import `assertStatusTransitionRoutable` from `./provider-descriptor.js` (the file already
imports its other parse helpers — `providers.ts:30-34`). The `github` factory is **not**
guarded: it has no route concept and `doneStatus` is genuinely best-effort there (GitHub
auto-closes via `closes #<n>`).

This runs eagerly inside `createProvidersFromConfig` (`providers.ts:282-296`), the same
run-start fail-closed point the registry already uses — a `--dry-run` doubles as the check.

### 5.4 Docs

- `adw_sdlc/docs/UNIVERSAL.md:218-220` — change "Read routes only in this step
  (`fetch`/`state`); progress/assignment/status are not yet posted over `rest`." to document
  the optional `postProgress`/`assignSelf`/`setStatus` write routes (templated JSON body,
  same host-allowlist/https/scoped-credential guard as every rest route) and the
  `doneStatus`-without-`setStatus` fail-closed guard. Add a short example `setStatus` route.
- `adw_sdlc/docs/DESIGN-declarative-providers.md` — update §12 step 2b ("write routes
  deferred", `:314-318`) and §9 (`:265-269`) to note rest work-item writes landed and that
  `doneStatus` now fails closed without a `setStatus` route.
- If `adw_sdlc/HANDOVER.md` carries a "rest write routes deferred" line, reconcile it.
- No `.adw/pack.profile.json` change ⇒ no `npm run pack:generate` needed.

---

## 6. Test plan (`adw_sdlc/test/providers.test.ts`)

**Replace** the now-obsolete test "no-ops the write methods in 2b" (`providers.test.ts:224-234`,
which asserts the requester is *never* called). New/updated coverage:

1. **Unrouted writes stay best-effort no-ops** — a `rest` descriptor with only
   `fetch`/`state` (no write routes): `postProgress`/`assignSelf`/`setStatus` issue **no**
   transport call (assert the transport spy stays uncalled). (Keeps the best-effort
   contract; the loud failure is the construction guard, tested below.)
2. **Routed `setStatus` issues the templated request** — descriptor with a `setStatus`
   route (`{ method: 'PUT', path: '/projects/{repo}/issues/{id}', body: { state_event: '{status}' } }`):
   assert method `PUT`, percent-encoded URL, `body` substituted to `{ state_event: 'Done' }`,
   `env['GITLAB_TOKEN']` present and `env['GH_TOKEN']` undefined (scoped credential).
3. **Routed `postProgress` templates `{body}`** — route with `body: { body: '{body}' }`;
   assert the substituted body equals `formatProgress(adwId, phase, message)` output.
4. **Routed `assignSelf` issues its request** — assert method/URL.
5. **Fail-closed guard (rest)** — `createProvidersFromConfig(parseAdwConfig({ providers: {
   workItems: { …rest fetch/state…, doneStatus: 'Done' } } }), …)` with **no** `setStatus`
   route throws `AdwError` matching `/doneStatus .* no .* setStatus route/` (or the chosen
   message). Mirrors the existing off-allowlist fail-closed test (`providers.test.ts:236-242`).
6. **Guard passes when routed** — same config plus a `setStatus` route builds successfully.
7. **Fail-closed guard (cli)** — analogous test for the `cli` provider (the guard
   generalizes; closes the identical latent drop at `providers-rest-cli.ts:138-144`).
8. **Body placeholder validation** — `parseRestWorkItemDescriptor` with a `setStatus` body
   referencing an unknown placeholder (e.g. `{bogus}`) throws `AdwError` matching
   `/unknown placeholder/` (reuses `assertBodyPlaceholders`).
9. **Path placeholder validation** — a write-route `path` using a disallowed placeholder
   throws.

Run `npx vitest run test/providers.test.ts` first, then the full gate.

---

## 7. Acceptance criteria mapping

| Issue acceptance bullet | Satisfied by |
| --- | --- |
| `rest` config with `doneStatus` raises a loud `AdwError` (fail closed) instead of silently dropping writes | §5.1.6 guard helper + §5.3 wiring; test §6.5 |
| OR write routes implemented via the change-request body-templating | §5.1 schema + §5.2 provider methods (reusing `substituteBody`/`requester.request`); tests §6.2–6.4 |
| Add coverage for the chosen behavior | §6 (replaces `providers.test.ts:224-234`; adds 9 cases) |

Recommended path delivers **both** the implemented writes and the guard.

---

## 8. Security & invariant preservation

- **Orchestrator owns all git/gh** — unchanged. Write routes only issue the project's own
  forge API call against an allowlisted https host (`DESIGN-declarative-providers.md:294-305`).
- **Secret boundary** — writes go through the same `makeRestRequester` scoped
  one-credential env (`safeSubprocessEnv({ allowGhToken: false, extraAllow: [base.authEnv] })`,
  `providers-rest-cli.ts:266-267`): `GH_TOKEN` withheld, no `...process.env` spread. Assert
  this explicitly in tests §6.2.
- **Host allowlist + https re-checked per request** — `requester.request` calls
  `assertAllowedHost` on every resolved URL (`providers-rest-cli.ts:268-269`); write routes
  inherit it for free. Placeholders are percent-encoded into the path (cannot alter host);
  body leaves are raw (not in the URL).
- **Fail closed at run start** — the new guard throws inside `createProvidersFromConfig`,
  before the dry-run branch and any side effect, consistent with `:300-301`.
- **Built-ins unchanged** — `github`/`git` paths and the committed config are byte-for-byte
  unaffected (the github factory is not guarded; `config.ts` unchanged).

---

## 9. Risks

- **Behavior change for misconfigured rest projects.** A project that today sets `doneStatus`
  on a `rest` provider (silently no-op) will now fail at run start. This is the intended fix,
  but it is a breaking change for any such existing config. Mitigated by the clear error
  message (add a `setStatus` route or remove `doneStatus`). Low likelihood: rest work-item
  writes were never functional, so no working flow depended on the silent drop.
- **assignSelf has no universal "self" over REST.** Unlike `gh`/`glab` (`@me`), a generic
  forge has no kernel-known current-user id. The route is operator-templated (placeholders
  `id`/`repo` only) and remains best-effort; it is *not* a loss trap. Documented as such.
  (Open question O3.)
- **`closedStates` interaction.** For the post-merge verify gate to recognize `doneStatus`
  as terminal, the operator must include it in `closedStates` (existing behavior,
  `config.ts:125-138`). Out of scope here; mention in the UNIVERSAL.md example so operators
  don't trip on it.
- **Scope creep.** Implementing writes is larger than a pure guard. If the maintainer wants
  the minimal `fix`, take the §3 alternative (guard only).

---

## 10. Rollback

Self-contained and additive. Revert the changes to the five files in §4; no migration, no
persisted-state or schema-compat impact (`WorkItemProvider` and `config.ts` are untouched,
so cross-language state stays additive/non-breaking).

---

## 11. Open questions & assumptions

**Assumptions**
- A1. The recommended hybrid (implement writes + guard) is acceptable for an
  `issue_class:fix` item, given the issue explicitly sanctions the write-route option and
  the change-request provider already proves the mechanism. If not, ship §3 alternative.
- A2. Gating the guard on `doneStatus` (opt-in, loss-bearing) — not the always-defaulted
  `inProgressStatus` — is correct: failing every read-only rest config because the cosmetic
  "In Progress" cannot be set would break working 2b setups.
- A3. Generalizing the guard to `cli` (not just `rest`) is in-scope and desirable (same
  latent drop, trivial cost). Narrow to `rest` if strict issue scoping is preferred.

**Open questions**
- O1. Minimal (guard-only) vs. recommended (writes + guard)? Affects whether §5.1–5.2 land.
- O2. Should `postProgress` *also* fail closed somehow, or stay best-effort no-op when
  unrouted (this spec's choice, matching the GitHub-without-`gh` / cli-without-route
  contract)? There is no separate "progress enabled" config flag to gate on, so failing
  closed on progress would require a new flag.
- O3. Is `assignSelf` over `rest` worth shipping at all (no universal "self"), or should it
  remain a documented no-op and only `postProgress`/`setStatus` be implemented?

---

## 12. Verification

From `adw_sdlc/`:

1. Focused: `npx vitest run test/providers.test.ts`
2. Full gate: `npm run verify` (typecheck, `lint:env`, prompt-pack drift check, tests,
   build, `dist/` cleanup).

If a check cannot be run, state exactly why and the command the maintainer should run.
