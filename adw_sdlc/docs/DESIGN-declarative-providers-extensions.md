# Spec — declarative provider primitives (step 2.5)

**Status:** **2.5a (transforms) + 2.5b (pagination) IMPLEMENTED** (see HANDOVER
§8n); **2.5c (token refresh) DEFERRED** (build only against a concrete OAuth
provider — see the sub-section below and the rollout note). Extends the
implemented step-2 declarative driver (`DESIGN-declarative-providers.md`, §8k–§8m)
with three bounded primitives — **transforms**, **pagination**, **token
refresh** — that close most of the long-tail provider gap **while staying data,
not code**, and keeping the kernel-enforced host allowlist intact. It is the safe
alternative to step 3 (out-of-process code plugins,
`DESIGN-provider-plugins-out-of-process.md`): prefer exhausting 2.5 before
authorizing any code-loading surface.

> **Implementation notes (2.5a/2.5b).** Two refinements vs. this draft, both made
> for a well-defined non-paginated case and to avoid touching the security-reviewed
> fetch helper:
> - **`itemsPath` is a route-level field of `failingJobs`** (required), not nested
>   inside `paginate`. `paginate` carries only `next` + `maxPages`, so a
>   non-paginated single-page `failingJobs` (omit `paginate`) still has a defined
>   place to find its items array.
> - **`failingJobs` is fetched with the same `{id}` as `pipelineStatus`** (the
>   change-request id) and only when the pipeline is red. Resolving a separate
>   *pipeline* id first is a multi-step flow → step-3 territory, not this primitive.
> - **Cursor styles implemented: `nextUrl` (body path) + `pageParam`.** `linkHeader`
>   is deferred (it would need the fetch helper to return response headers); the
>   "Open questions" Link-header item is resolved toward body-path `nextUrl` first.

**Why 2.5 before 3:** every primitive here is interpreted by kernel code over
project *data*; none adds a code-execution surface, and every network request
(including each paginated page and the refresh exchange) still passes through
`assertAllowedHost` + https. Step 3's irreducible downside — a plugin doing its
own un-allowlisted network I/O (`DESIGN-provider-plugins-out-of-process.md` §4) —
does not apply here.

The primitives are independent and additive; absent ⇒ today's behavior exactly.
Recommended order is ascending complexity: **2.5a transforms → 2.5b pagination →
2.5c refresh.**

---

## 2.5a — Transforms (map mini-language extension)

**Gap:** a mapped value needs light, deterministic massaging — normalize a status
before a `stateMap` lookup, supply a fallback, trim whitespace.

**Design:** a scalar map value may carry a `|`-piped transform chain after the
path. A closed, eval-free vocabulary, validated at load:

```jsonc
"map": { "state": "$.pipeline.status | lower" }
"map": { "title": "$.title | trim", "number": "$.iid | default:0" }
```

| Transform     | Effect (on the scalar string from `evalScalar`)            |
| ------------- | --------------------------------------------------------- |
| `lower`       | lowercase                                                 |
| `upper`       | uppercase                                                 |
| `trim`        | strip surrounding whitespace                              |
| `default:<v>` | if the value is `""` (missing), substitute the literal `<v>` |

- Parsing: split the string on `|`; part 0 is the path (`$…`, parsed as today);
  each remaining part is `name` or `name:arg`. An unknown transform, or
  `default` without an arg, is a loud `AdwError` at load (fail-closed). No `|` ⇒
  a bare path (backward-compatible).
- **Scalar fields only** in 2.5a (`title`/`body`/`state`/`url`/`number`,
  `findForBranch.url`, `pipelineStatus.statusPath`). Array fields (`labels`) keep
  the `[*]` form; per-element transforms are a later, separate concern.
- Pure data: transforms are fixed string functions applied after the data walk —
  no expressions, no user code. `default` pairs especially well with the
  `number`/`state` mappings (e.g. `state: "$.x | lower"` so a `stateMap` keyed on
  canonical lowercase forge statuses always matches).

**Implementation touchpoints:** `provider-descriptor.ts` — extend the map-value
compile step to parse-and-validate a transform chain alongside the path; store
the compiled chain on the `PathSegment[]`-bearing field; `evalScalar` applies the
chain. ~1 small module addition; no schema change beyond the value *string*.

## 2.5b — Pagination (rest route modifier)

**Gap:** assembling a *list* that spans pages — most concretely, the
`pipelineStatus.failingJobs` detail deferred in 2c (list a pipeline's jobs,
filter to failing), or any "list all MRs / issues" route.

**Design:** an optional `paginate` on a `rest` route. The kernel fetches page 1,
extracts the page's items, finds the next page, re-requests until exhausted, and
hands the **accumulated items array** to the route's `map` (so array paths see
all items):

```jsonc
"failingJobs": {
  "method": "GET", "path": "/projects/{repo}/pipelines/{id}/jobs?scope=failed",
  "paginate": { "itemsPath": "$", "next": { "style": "nextUrl", "path": "$.links.next" }, "maxPages": 10 },
  "map": [ { "name": "$.name", "logExcerpt": "$.failure_reason | default:" } ]
}
```

Cursor styles (closed set):

- **`nextUrl`** — the next page's absolute URL comes from a body path
  (`{ style: "nextUrl", path: "$.links.next" }`) or a response **`Link` header**
  (`{ style: "linkHeader" }`); follow until absent.
- **`pageParam`** — increment a query param (`{ style: "pageParam", param:
  "page", start: 1 }`) until a page yields zero items.

Rules:

- **`maxPages` is a hard cap** (default 10). On reaching it the kernel `log()`s
  that pagination was truncated — **no silent truncation** (cf. the workflow
  "no silent caps" rule).
- **Security — the load-bearing obligation:** a `nextUrl`/`Link` value comes from
  the (attacker-influenceable) response, so the kernel **re-asserts
  `assertAllowedHost` on every followed/constructed page URL** before fetching.
  An off-allowlist next URL stops pagination (returns what was gathered) rather
  than following it. `pageParam` reuses the validated base path, so its host is
  already fixed.
- Empty/garbage page, or a transport error mid-loop, ends the loop with the items
  gathered so far (best-effort, like today's single-request fallbacks).

This unblocks a real 2c gap: with `paginate` + a `failingJobs` map, a `rest`
change-request provider can populate `PipelineStatus.failingJobs` (job
name + excerpt) that the ci-fix loop already consumes (`failingJobs[].name`).

**Implementation touchpoints:** `provider-descriptor.ts` — a `paginate` schema +
compile; `providers-rest-cli.ts` — a pagination loop wrapping `makeRestRequester`
that accumulates `itemsPath` items and re-checks the host per page; a new
optional `failingJobs` route on the change-request descriptor whose array map
yields `PipelineJob[]`.

## 2.5c — Token refresh (rest base modifier; heaviest, most demand-gated)

**Gap:** the API credential is not a static bearer token — an OAuth2
client-credentials / refresh-token flow must mint a short-lived access token
first.

**Design:** an optional `refresh` on the rest base. Before the first real call
(and again when the cached token expires), the kernel performs the refresh
request, extracts the access token, and uses **that** as the credential
(injected into the routes' `authHeader`/`authScheme`) for the rest of the run:

```jsonc
"refresh": {
  "method": "POST",
  "url": "https://auth.example.com/oauth/token",   // own URL; host MUST be in allowedHosts
  "credentialIn": { "field": "refresh_token" },     // where to inject the authEnv SECRET VALUE
  "body": { "grant_type": "refresh_token", "client_id": "my-app" },
  "tokenPath": "$.access_token",
  "expiresInPath": "$.expires_in"                   // optional; else refresh once per run
}
```

Flow & rules:

- The kernel reads `process.env[authEnv]` (the long-lived secret) and injects it
  into the refresh request at `credentialIn` (a body field or a header) — the
  descriptor names *where*, never holds the value. It runs the refresh against
  `refresh.url` (**host re-checked against `allowedHosts`**, https only),
  extracts `tokenPath`, and **caches the access token in kernel memory for the
  run** (re-minting when `expiresInPath` says it has expired).
- The minted access token becomes the credential for all other routes; their
  `authEnv` is then unused for the API calls (it is the refresh input).
- A failed refresh fails the affected calls closed (null/UNKNOWN/error), loudly
  noted; it never falls back to an unauthenticated call.

Refresh is the heaviest primitive (a pre-flight request, secret-value injection,
token caching/expiry, a second allowlisted host) and the least-demanded; spec it
as its own sub-step and build only against a concrete OAuth provider.

**Implementation touchpoints:** `provider-descriptor.ts` — a `refresh` schema on
`RestBase` (+ its host in the allowlist check); `providers-rest-cli.ts` — a
run-scoped token cache + a pre-call hook in `makeRestRequester` that swaps the
injected credential to the minted token.

## What 2.5 still does NOT cover (the residual step-3 demand)

Even with all three primitives, declarative data cannot express:

- **GraphQL** — constructing a query string and parsing a deeply *computed*
  response (beyond field-path + transforms).
- **Request signing** — HMAC / AWS SigV4 over the request bytes (a computation,
  not a value).
- **Conditional multi-step flows** — "create, then if X set labels, else …" with
  branching logic.

These remain the (still demand-gated) justification for step 3
(`DESIGN-provider-plugins-out-of-process.md`). 2.5 shrinks step 3's residual to
genuinely code-shaped providers.

## Rollout

1. **2.5a transforms** — ✅ DONE (HANDOVER §8n). Map-value `|`-chain grammar
   (`compileScalar`) + `evalScalarMapping` application + load-time validation;
   scalar fields now compile to `ScalarMapping { segments, transforms }`.
2. **2.5b pagination** — ✅ DONE (HANDOVER §8n). The `failingJobs` change-request
   route (`itemsPath` + one-element item `map` + optional `paginate`), the
   `nextUrl`/`pageParam` loop with the per-page host re-check (`isAllowedHost`,
   stop-don't-throw), `maxPages` logged truncation, and `failingJobs` populated
   into `PipelineStatus` when red.
3. **2.5c token refresh** — ⏸ DEFERRED. Own sub-step, build only against a
   concrete OAuth provider; the spec above stands as the build plan.

Each is additive and backward-compatible (absent ⇒ unchanged behavior); the
github/git built-ins and the dry-run baseline stay byte-identical.

## Testing

- **transforms:** unit-test the chain parser (valid + unknown-transform rejection
  + `default` arg required) and application (`lower`/`trim`/`default` on present
  vs missing values), incl. the `stateMap`-after-`lower` path.
- **pagination:** an injected transport returning multi-page cursors (`nextUrl`
  and `pageParam`); assert accumulation, `maxPages` truncation is logged, and an
  **off-allowlist next URL is refused** (the security test).
- **refresh:** an injected transport that returns a token from the refresh route
  then sees it on subsequent calls; assert the secret is injected only into the
  refresh request, the minted token into the others, and a failed refresh fails
  closed.
- Built-ins + dry-run unchanged; `lint:env` unaffected (no new env handling
  beyond the existing scoped-env path).

## Open questions

- Per-element transforms for array fields (`labels[*] | lower`) — defer unless
  demanded.
- `coalesce` (first non-empty of several paths) — useful but adds multi-path
  parsing; defer.
- `Link`-header pagination parsing (RFC 5988) — include in 2.5b or defer to
  body-path `nextUrl` only first?
- Refresh: client-credentials vs refresh-token vs basic-auth-exchange — pick the
  one the first real consumer needs; keep `credentialIn` general.

## Non-goals

- No code execution, no `eval`, no arbitrary transform functions — the transform
  vocabulary is a closed set.
- No un-allowlisted egress — every page and the refresh exchange are host-checked.
- Not a replacement for step 3's genuinely code-shaped cases (GraphQL/signing/
  branching) — 2.5 shrinks that residual, it does not erase it.
