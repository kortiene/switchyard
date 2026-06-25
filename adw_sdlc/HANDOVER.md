# ADW SDLC — Universalization handover

This document captures the state of the universalization effort so a future
session can pick up where we stopped.

## 1. Repository context

- Path: `/Users/sekou/TAC/pi-gh-issue`
- Package: `adw_sdlc/` — TypeScript control plane for the phased ADW
  (Agentic Developer Workflow) SDLC pipeline.
- Pipeline:
  `setup → classify → plan → implement → tests → resolve(loop) → e2e(gated)
  → review → patch(loop) → document(gated) → finalize → ci-fix(loop)
  → merge → report`
- Runners: `claude` | `codex` | `opencode` | `pi` behind a single
  `AgentRunner.runPhase()` seam.
- Cross-language state contract: `adw/state.schema.json` plus the fixtures
  in `adw/fixtures/cross_language/`. The Python sibling is not bundled in
  this standalone port.
- Architectural plan & parity criteria: `adw_sdlc/PLAN.md`,
  `adw_sdlc/PARITY.md`, `adw_sdlc/HEALTHTECH_PORT.md`.
- Universal architecture reference (new): `adw_sdlc/docs/UNIVERSAL.md`.
- Example non-HealthTech project pack (new):
  `adw_sdlc/docs/examples/payments-api.config.json`.
- Neutral package entry point (new): `adw_sdlc/README.md`.
- Schema-overrides / custom-phases design + rollout (new):
  `adw_sdlc/docs/DESIGN-schema-overrides.md`.
- Custom-phase control-flow design (new):
  `adw_sdlc/docs/DESIGN-custom-phase-control-flow.md`.
- Provider-plugin security design (new):
  `adw_sdlc/docs/DESIGN-provider-plugins.md`.
- Declarative-providers design — #4 step 2 (2a `cli` + 2b `rest` work items +
  2c `rest` change-requests implemented):
  `adw_sdlc/docs/DESIGN-declarative-providers.md`.

**Repository state:** all of this session's work is **merged to `main`**
(no remote — local merge). `main` HEAD is the merge commit
`07b90f6 merge: provider extensibility — registry + declarative cli/rest providers (#4)`,
with the session's commits in history:

```
07b90f6 merge: provider extensibility — registry + declarative cli/rest providers (#4)
0ac57a5 feat(adw_sdlc): provider extensibility — registry + declarative providers (§8j–§8m)
9bbf755 docs(adw_sdlc): record the local merge to main in HANDOVER
3af37e3 merge: provider-plugin security design doc (#4)
9cde820 feat(adw_sdlc): loop/gated custom phases            (§8i)
5cc4462 docs(adw_sdlc): security design pass for #4         (§11 security doc)
d5ec440 feat(adw_sdlc): preflight phase chain at run start  (§8h)
2892c7a feat(adw_sdlc): universalize phase chain, schema overrides, custom phases, done-status
```

The provider-extensibility work (#4 step 1 + step 2: §8j registry, §8k/§8l/§8m
declarative `cli`/`rest` work-item + `rest` change-request providers) is a single
`feat` commit (`0ac57a5`) — the four slices are intertwined across the same files,
so they were not split. The per-feature branch (`feat/provider-extensibility`,
and the earlier `feat/custom-phase-*`, `docs/provider-plugin-security`) were
deleted after merging; their content is preserved in `main`'s history above. The
working tree is clean.

## 2. Session goal

Make this ADW workflow universal: reusable across different repositories,
languages, product domains, CI systems, issue trackers, VCS hosts, and
agent runners — while preserving secret isolation, deterministic
orchestration, human approval gates, structured outputs, resumability, and
cross-language state compatibility.

The session was driven by the "strong" universalization prompt, which is
recorded in the chat history but boils down to:

> Universalize the policy surface. Keep the safety guarantees inside the
> kernel. Make providers, prompts, gates, branching, models, schemas, and
> wording project-pack configuration.

## 3. Operational invariants — do NOT change without a separate design pass

These cross-cut everything. Any future session should refuse to touch them
without an explicit security/LLM/API-break review.

1. **Secret boundary in `src/env.ts`** — `BASE_ENV_ALLOW`,
   `ENV_DENY_PREFIXES = ['MATRIX_', 'MX_AGENT_']`, and `RUNNER_ENV_ALLOW`
   are NOT project-configurable.
2. **Static lint gate** — `scripts/check-adw-sdlc-env.sh` (run via
   `npm run lint:env`) enforces:
   - No `...process.env` spread anywhere in `src/`
   - No banned opencode factory calls (`createOpencodeServer`,
     `createOpencode`, `createOpencodeTui`)
   - opencode imports only via `@opencode-ai/sdk/v2/client`
3. **Phase preamble wording** in `src/phases.ts` (`PHASE_PREAMBLE_SHARED`)
   is LLM-facing prompt content; rewording risks behavior drift the test
   suite cannot detect.
4. **CLI command name `issue`** is documented as a backward-compatible
   GitHub alias and must not be renamed.
5. **`MX_AGENT_*` env var prefixes** are load-bearing in `ENV_DENY_PREFIXES`
   and the static lint; renaming them weakens the secret boundary.
6. **State v1 fields** (`issue_number`, `pr_number`, `pr_url`, etc.)
   remain canonical for resume and cross-language interoperability with
   the Python `adw/` reader. New fields must be additive and
   non-load-bearing.
7. **Orchestrator owns all git/gh** — runners are never granted git/gh
   tools and never receive `GH_TOKEN` in phased mode.
8. **The user's orchestrator owns ALL git/gh operations** — we do not run
   `git` or `gh` ourselves from this session.

## 4. Architecture as of this handover

```
┌────────────────────────────────────────────────────────────────────┐
│ KERNEL (do not touch for project-specific universalization)        │
│ ──────────────────────────────────────────────────────────────────│
│ src/orchestrator.ts   deterministic phase orchestration            │
│ src/invoker.ts        AgentRunner seam + capability matrix         │
│ src/run-phase.ts      structured-output invoker (single nudge)     │
│ src/env.ts            deny-by-default env allowlist (load-bearing) │
│ src/state.ts          persistent run state                         │
│ src/structured-call.ts shared classify call (Anthropic SDK)        │
│ src/schemas.ts        per-phase Zod schemas                        │
│ src/phases.ts         phase catalog, prompt composition, gates     │
│ src/registry.ts       lazy runner registry                         │
│ src/runners/*.ts      claude/codex/opencode/pi adapters + mock     │
├────────────────────────────────────────────────────────────────────┤
│ PROVIDER BOUNDARY                                                   │
│ ──────────────────────────────────────────────────────────────────│
│ src/providers.ts      WorkItemProvider/VcsProvider/                │
│                       ChangeRequestProvider/ProviderCli +          │
│                       GitHub/Git built-in adapters                 │
│ src/work-item.ts      provider-neutral work-item helpers (NEW)     │
│ src/issue.ts          historical GitHub-issue impl (compat)        │
├────────────────────────────────────────────────────────────────────┤
│ PROJECT PACK                                                        │
│ ──────────────────────────────────────────────────────────────────│
│ .adw/config.json      project pack (committed HealthTech version)  │
│ .pi/prompts/*.md      shared prompt templates (14 files)           │
│ .claude/commands/*.md byte-identical mirror of .pi/prompts         │
└────────────────────────────────────────────────────────────────────┘
```

`run()` in the orchestrator now uses providers directly. Legacy
`OrchestratorDeps` seams (`deps.git.*`, `deps.fetchIssue`, etc.) remain as
a compatibility adapter for tests; the runtime path is provider-first.

Kernel modules added by the follow-up sessions (not in the box above):
`src/config.ts` (project-pack config schema + loader), `src/schema-registry.ts`
(the single per-phase schema seam — built-in / override / custom), and
`src/schema-override.ts` (the lone ajv user: override loader, validator, and
fenced-JSON example generator). `OUTPUT_CONTRACT` now lives in `src/schemas.ts`
(re-exported from `src/phases.ts`).

## 5. Universal-config surface (`.adw/config.json`)

Currently configurable, validated by `AdwConfigSchema` (Zod):

```
project              { id, name }
prompts              { defaultRoot, runnerRoots }
phases               (optional ordered chain; validated against the kernel
                      catalog by parsePhases — reorder/drop known phases only)
schemas              (optional { root, overrides }; per-phase JSON Schema
                      overrides for tests/e2e/document, ajv-validated — §8f)
customPhases         (optional new plain phase names; each needs a <name>.md
                      template + .adw/schemas/<name>.json result schema — §8g)
providers
  cli                { type: 'github' }
  workItems          { type: 'github', closedStates, inProgressStatus,
                       doneStatus (optional terminal status), statusFieldName }
  vcs                { type: 'git' }
  changeRequests     { type: 'github' }
progress             { tag }
branching
  defaultPrefix
  labelPrefixes      (lowercased on load)
  slug               { maxLength, stripDiacritics, stripPhaseIssuePrefix }
gates
  e2e                { hints }
  documentation      { hints, exactFiles, pathPrefixes, fileExtensions }
models
  classifyModel
  defaultTier
  phaseTiers         (per-phase → 'cheap'|'mid'|'capable')
  tiers              (tier → runner → model id)
commands
  defaultTestCommand
  defaultFinalizeGates
```

Missing config falls back to behavior-preserving defaults (the committed
HealthTech setup).

## 6. Public type aliases (provider-neutral preferred; legacy kept)

| Provider-neutral (new) | Legacy alias (preserved)         |
| ---------------------- | -------------------------------- |
| `WorkItemContext`      | `IssueContext`                   |
| `fetchWorkItem()`      | `fetchIssue()`                   |
| `ChangeRequest`        | (no prior shape)                 |
| `CreateChangeRequestResult` | `CreatePrResult`            |
| `PipelineStatus`       | `CiStatus`                       |
| `PipelineJob`          | `FailingJob`                     |
| `PipelineState`        | `git.CiState`                    |
| `OperationResult`      | `GitOperationResult`             |
| `pipelineStatus()` (provider) | `ciStatus()` (optional shim) |
| `parsed.workItem` (CLI)| `parsed.issue`                   |
| `CliDeps.runWorkItem`  | `CliDeps.runIssue` (still required) |
| `workItemBranchPrefix` | `branchPrefix`                   |
| `slugifyWorkItemTitle` | `slugifyTitle`                   |
| `deriveWorkItemBranch` | `deriveBranch`                   |

All legacy names continue to compile, type-check, and behave identically
for GitHub provider configs.

## 7. State schema additions (additive only, non-load-bearing)

`adw/state.schema.json` now documents:

- `work_item`  — provider-neutral work-item metadata
- `change_request` — provider-neutral change-request metadata

V1 GitHub-shaped fields (`issue_number`, `pr_number`, `pr_url`, …) remain
canonical. The Python reader drops the additive fields per the existing
schema contract.

The committed TS-produced fixture
(`adw/fixtures/cross_language/ts-produced-state.json`) now carries the new
fields; the py-produced fixture stays v1-only.

## 8. Verification gates (run these to resume)

```bash
cd adw_sdlc

# 1) Typecheck — no emit
npm run typecheck

# 2) Static secret-boundary lint
npm run lint:env

# 3) Full test suite (current: 391 tests, 31 files)
npm test

# 4) Build (then clean — dist/ is a build artifact)
npm run build && rm -rf dist

# 5) CLI dry-run smoke
npx tsx src/cli.ts 42 --dry-run
```

Also confirm:

```bash
# prompt mirrors byte-identical
diff -rq .pi/prompts .claude/commands

# .adw config present
test -f .adw/config.json && echo ok

# no leftover build artifact
test ! -d adw_sdlc/dist && echo ok
```

Expected dry-run output:

```
[dry-run] phased run for GitHub issue #42 via claude
[dry-run] phases: setup(ts) -> classify -> plan -> implement -> tests -> resolve -> e2e -> review -> patch -> document -> finalize(ts) -> ci-fix(ts) -> merge(ts) -> report(ts)
[dry-run] agent env: GH_TOKEN withheld (allowGhToken=false)
[dry-run] test gate: (none configured)
```

## 8b. Follow-up session — configurable phase chain (§11 #1)

Delivered the optional `phases` chain on top of the green baseline:

- `adw_sdlc/src/config.ts` — added optional `phases` to `AdwConfigSchema`
  (shape-only: non-empty list of non-empty names). Absent in
  `DEFAULT_ADW_CONFIG`, so the default stays the full catalog.
- `adw_sdlc/src/phases.ts` — `parsePhases(csv, config?)` now resolves the
  chain by precedence (CSV > config.phases > `DEFAULT_PHASES`) and validates
  every name against `AGENT_PHASES` via `assertKnownPhases` (kernel owns
  membership; avoids a config↔phases import cycle).
- `adw_sdlc/src/orchestrator.ts` — passes the resolved `config` into
  `parsePhases(opts.phases, config)`.
- `adw_sdlc/docs/examples/payments-api.config.json` — pins the full chain
  explicitly (demonstrates the knob; behavior-preserving).
- `adw_sdlc/docs/UNIVERSAL.md` — new "Phase chain" section.
- Tests: `test/phases.test.ts` (+3 — config-driven default, CSV-overrides-
  config, unknown-config-phase rejection) and `test/config.test.ts` (+1
  shape test, plus a drift assertion on the example's `phases`).

Verified: typecheck, `lint:env`, build+clean, full suite (now **347**),
plus a live config round-trip — a custom chain dropping `e2e`/`document`
showed up in the dry-run, an unknown phase failed loudly, and the restored
committed config still prints the full chain.

## 8c. Follow-up session — terminal done-status transition (§11 #3)

Added an opt-in terminal status move on verified merge:

- `adw_sdlc/src/config.ts` — optional `providers.workItems.doneStatus`
  (unset by default; GitHub auto-closes via "closes #<n>", so this is for
  providers/Projects boards needing an explicit terminal move).
- `adw_sdlc/src/orchestrator.ts` — `transitionToDone()` helper, called
  (best-effort) right after the merge is recorded and again on the
  merge-already-done resume short-circuit. Mirrors setup's best-effort
  status move; a failed update never undoes a merge, and the verify gate
  re-reads real state independently. No-op without `doneStatus`.
- `adw_sdlc/docs/examples/payments-api.config.json` — adds `doneStatus: "Done"`.
- `adw_sdlc/docs/UNIVERSAL.md` — documents the in-progress/done status axis
  and the `closedStates` pairing for non-GitHub providers.
- Tests: `test/orchestrator.test.ts` (+2 — moves to doneStatus on merge;
  no move when unset) and `test/config.test.ts` (+1 optional-field test,
  plus a drift assertion on the example's `doneStatus`). An `afterEach`
  now resets `setAdwConfigForTests(null)` so per-test config overrides
  cannot leak.

No provider-interface change — `WorkItemProvider.setStatus` already existed.
GitHub behavior is unchanged (committed config carries no `doneStatus`).

## 8d. Follow-up session — universal README (§11 #5)

- `adw_sdlc/README.md` (new) — neutral package entry point routing to the
  existing docs; reflects the universal config surface including the
  `phases` chain (§8b) and `doneStatus` (§8c). Pure docs; no code/test
  changes, suite unchanged at 350.

## 8e. Follow-up session — schema-registry indirection (§11 #2, rollout step 1)

Pure refactor establishing the single seam for the deferred per-phase
schema-override feature; no behavior change.

- `adw_sdlc/src/schema-registry.ts` (new) — `resolvePhaseSchema(phase)`
  returns a handle `{ jsonSchema, validate, outputContract, requiredKeys }`,
  all delegating to the built-in Zod schema / `OUTPUT_CONTRACT` /
  `phaseJsonSchema` / `parsePhaseResult`.
- `adw_sdlc/src/schemas.ts` — `OUTPUT_CONTRACT` moved here from `phases.ts`
  (its logical home beside `PHASE_SCHEMAS`).
- `adw_sdlc/src/phases.ts` — re-exports `OUTPUT_CONTRACT` (source compat for
  `phases.test.ts` + `index.ts`); `buildFooter` now resolves the contract via
  the registry instead of indexing the constant.
- `adw_sdlc/src/run-phase.ts` — the native-schema (`jsonSchema`) and
  validate touchpoints route through the registry handle.
- `adw_sdlc/src/index.ts` — exports `resolvePhaseSchema` / `PhaseSchemaHandle`.
- `adw_sdlc/test/schema-registry.test.ts` (new, +3) — pins built-in
  delegation (jsonSchema/outputContract/requiredKeys equality, validate
  coercion + loud rejection).

Dependency graph stays acyclic: `run-phase`/`phases` → `schema-registry` →
`schemas`. classify's in-process Zod path is intentionally untouched. Zero
existing-test changes — the proof it is behavior-preserving. The contract-drift
guard still passes through the `OUTPUT_CONTRACT` re-export.

## 8f. Follow-up session — schema overrides, capability A (§11 #2, rollout step 2)

Projects can now override the structured-output schema of a non-load-bearing
phase. First dependency added to the package since the port.

- `adw_sdlc/src/schema-override.ts` (new) — the only ajv user: loads + sanity-
  checks an override JSON Schema (object-typed; rejects non-JSON, non-object,
  and remote `$ref`), compiles a validator (`strict:false`, fresh instance per
  compile to dodge ajv's duplicate-`$id` cache), formats errors, derives
  `requiredKeys`, and generates a bounded fenced-JSON footer example.
- `adw_sdlc/src/schema-registry.ts` — `resolvePhaseSchema(phase, config?)` now
  resolves an override file (`schemas.overrides[phase]` > `schemas.root/<phase>.json`)
  and returns an ajv-backed handle for `OVERRIDABLE_PHASES`
  (`tests`/`e2e`/`document`); any other phase with an override file is a loud
  error (load-bearing or `classify`-excluded).
- `adw_sdlc/src/config.ts` — optional `schemas: { root?, overrides? }` field.
- `adw_sdlc/src/index.ts` — exports `OVERRIDABLE_PHASES`.
- `adw_sdlc/package.json` + `package-lock.json` — `ajv@^8.20.0` promoted to a
  direct dependency (it was already present transitively; lock change is the
  single root-deps line, node_modules untouched). Imported as the named
  `{ Ajv }` export (this package has no `esModuleInterop`).
- `adw_sdlc/test/scaffold.test.ts` — the D3 package-invariant guard updated to
  expect ajv as the third unconditional runtime dep (with rationale).
- `adw_sdlc/test/schema-overrides.test.ts` (new, +10) — override routing,
  loud rejections, missing-path error, loader guards, example generation,
  and built-in preservation when no override file is present.

Verified end-to-end through the real `getAdwConfig()` path: an override file
makes `buildFooter` emit the generated contract, and a load-bearing override
throws — both through `resolvePhaseSchema`. Built-in Zod + coercion path
untouched, so the parity suite is unaffected. classify's in-process Zod path
stays out of scope (§6.6).

## 8g. Follow-up session — custom phases, capability B (§11 #2, rollout step 3)

Projects can register genuinely new, plain (non-loop, non-gated) phases.

- `adw_sdlc/src/config.ts` — optional `customPhases: string[]`.
- `adw_sdlc/src/phases.ts` — `knownPhaseNames(config)` = built-ins ∪ custom
  (rejects a custom name colliding with a built-in); `parsePhases` validates
  the chain against it and now returns `string[]`; `composePhasePrompt`
  defaults a custom phase's template basename to its own name and adds no
  reframing; `CONDITIONAL_PHASES`/`LOOP_PHASES`/`ARTIFACT_PHASES` and
  `composePhasePrompt`/`buildFooter` widened to `string`.
- `adw_sdlc/src/schema-registry.ts` — `resolvePhaseSchema` overloaded
  (built-in-generic + string); a registered custom phase resolves to an
  ajv handle from its required `.adw/schemas/<name>.json`; an unregistered or
  schemaless custom name is a loud error.
- `adw_sdlc/src/orchestrator.ts` — phase loop variable is now `string`;
  `phaseArgs`/`applyResult` widened; the custom phase runs through the generic
  `runAgentPhase` path (recorded via `markDone`, never branched on). A single
  documented `phase as AgentPhase` cast carries a custom name through the
  SchemaPhase-typed seam; the impls (template-basename fallback, registry
  custom routing) are robust to it.
- `adw_sdlc/test/custom-phases.test.ts` (new, +8) — chain membership,
  collision/unknown rejection, schema resolution (incl. schemaless error),
  and full prompt composition (template + generated contract).
- `adw_sdlc/test/orchestrator.test.ts` (+1) — a registered custom phase runs
  through the generic path and is recorded.

Built-in typing/behavior unchanged (the generic overloads preserve precise
built-in result types); proven end-to-end via the CLI dry-run (a custom phase
appears in the plan; an unregistered name fails loudly). Tier for a custom
phase comes from `models.phaseTiers[name]` (modelForPhase already tolerates
unknown phases → default tier).

## 8h. Follow-up session — custom-phase startup validation (§11 candidate)

Hardened capability B (§8g): the resolved phase chain is now preflighted at run
start, so a misconfigured custom phase (or broken/unsupported schema override)
fails loudly before any side effects instead of mid-chain.

- `adw_sdlc/src/phases.ts` — new `validatePhaseChain(phases, runner, config)`:
  for each phase in the chain it asserts the prompt template resolves
  (`templatePath` + `existsSync`, custom phases default the basename to their own
  name) and that `resolvePhaseSchema(phase, config)` loads — which compiles an
  override/custom schema eagerly, so a missing custom schema, a broken/
  unsupported override, or an unknown name throws here. Built-ins without an
  override are a no-op (their templates ship with the package).
- `adw_sdlc/src/orchestrator.ts` — calls `validatePhaseChain(phases, runner.id,
  config)` right after `parsePhases`, **before** the `--dry-run` branch, so a
  dry-run doubles as a config check and a real run fails before minting any
  branch/PR/state.
- `adw_sdlc/docs/UNIVERSAL.md` — custom-phases section documents the preflight.
- `adw_sdlc/docs/DESIGN-schema-overrides.md` — §9 records the follow-up.
- Tests: `test/custom-phases.test.ts` (+5 — stock chain passes, fully-wired
  custom phase passes, missing template / missing schema fail, unsupported
  load-bearing override surfaced during the walk) and `test/orchestrator.test.ts`
  (+1 dry-run preflight rejects an unwired custom phase before the plan prints).
  The pre-existing custom-phase orchestrator test (§8g) was updated to supply a
  real `audit.md`/`audit.json` — its config was previously invalid and only
  passed because validation was lazy.

No new dependency, no config-surface change, no provider-interface change. Built-in
behavior is unchanged (no-op for the stock chain; the committed dry-run output is
byte-identical). Proven end-to-end through the real CLI: a dry-run with an unwired
custom phase fails with `phase "audit" is missing its prompt template: …`, and a
fully-wired one shows `… implement -> audit -> review …` in the plan.

## 8i. Follow-up session — loop/gated custom phases (§11 candidate)

The deferred remainder of capability B: a registered custom phase may now opt
into a conditional gate or a resolve-style loop. Design doc:
`docs/DESIGN-custom-phase-control-flow.md`.

- `adw_sdlc/src/config.ts` — optional `gates.custom` (record → gate predicate,
  same shape as the `documentation` gate) and top-level `loops` (record →
  `{ command, maxAttempts=3 }`), both defaulting to `{}`. Default config carries
  the empty maps; existing configs are unaffected.
- `adw_sdlc/src/phases.ts` — `gateCustom` (factored from `gateDocument`'s
  matching) + `CustomGateRule`; `gateConditional` dispatches `e2e`/`document`/
  custom; `isConditionalPhase(phase, config)`; `validatePhaseChain` extended:
  control-flow keys must target a registered custom phase (built-in/unregistered
  rejected, key check runs first for precise messages), a custom gate needs ≥1
  matcher, and a custom loop phase's schema must declare `resolved`.
- `adw_sdlc/src/orchestrator.ts` — `resolveLoop`'s config gains an optional
  `phase` (default `'resolve'`), used for its progress tags and to pick which
  agent to invoke (one documented `phase as 'resolve'` cast carries a custom name
  through, reading the `resolved` field the schema is validated to declare); the
  gate check uses `isConditionalPhase` + passes `config`; the loop dispatch runs
  the generalized loop for `loops[phase]` custom phases. Built-in `resolve`/gates
  pass no new params → byte-for-byte unchanged.
- Docs: `docs/UNIVERSAL.md` (new "Custom-phase control flow" section), README
  config table + doc map.
- Tests: `test/custom-phase-control-flow.test.ts` (new, +9 — gate matching/
  dispatch, `isConditionalPhase`, and the four startup-validation rejections),
  `test/config.test.ts` (+1 — gates.custom/loops parsing + shape failures),
  `test/orchestrator.test.ts` (+3 — `resolveLoop` phase override, a gated phase
  skipping/running by signal, a looped phase red→green).

Proven end-to-end through the real CLI: a loop on a built-in fails with
`custom loop for "tests" is not allowed: built-in phases own their control flow`;
a custom loop phase whose schema omits `resolved` fails loudly; a fully-wired
gated+looped custom phase shows `… implement -> verify -> review …` in the plan.
Loop/gated custom phases are no longer a non-goal. Still out of scope:
`patch`-style findings loops and overriding `classify`.

## 8j. Follow-up session — provider-kind registry (§11 #4, rollout step 1)

First implementation slice of **#4 (provider plugin loading)**, following the
agreed staged path in `docs/DESIGN-provider-plugins.md` §5. **Step 1: open the
factory switch internally** — the design's explicitly safe, no-new-trust-surface
first step. Pure kernel work; no new dependency; **no code loading** (the §10
hard stop / Options A/D are untouched). Behavior-preserving for the `github`/
`git` built-ins.

- `adw_sdlc/src/config.ts` — provider `type` fields (cli / workItems / vcs /
  changeRequests) widened from `z.literal('github'|'git')` to `z.string().min(1)`
  (**shape** only). A doc comment records that membership is the provider
  registry's job — the same shape/membership split the `phases` chain uses, and
  it keeps `config.ts` ⇄ `providers.ts` acyclic. Defaults unchanged (github/git).
- `adw_sdlc/src/providers.ts` — replaced the closed if/else in
  `createProvidersFromConfig` with a per-role registry (`CLI_PROVIDERS`,
  `WORK_ITEM_PROVIDERS`, `VCS_PROVIDERS`, `CHANGE_REQUEST_PROVIDERS`) + a generic
  `resolveProviderFactory` that **fails closed** with a loud `AdwError`
  (`unsupported <role> provider type "<x>" (supported: …)`). Removed
  `neverProvider` (plain `Error`). Added/exported `supportedProviderTypes()`
  (read-only introspection of registered kinds per role). Adding an in-tree
  provider later is now a one-line map entry + factory — no config-schema change.
- `adw_sdlc/src/index.ts` — exports `supportedProviderTypes`.
- `adw_sdlc/docs/UNIVERSAL.md` — new "Provider registry (open switch, fail
  closed)" subsection under the provider boundary.
- `adw_sdlc/docs/DESIGN-provider-plugins.md` — header status → "staged rollout
  underway"; §5 step 1 marked DONE (the in-tree GitLab/Gitea *adapters* it
  mentioned are deferred to step 2's declarative driver / their own slice; the
  seam now exists).
- Tests: `test/providers.test.ts` (+2 — `supportedProviderTypes` snapshot;
  unknown kind fails closed naming role + supported types) and
  `test/config.test.ts` (+1 — provider `type` shape-validates, a non-empty
  unknown kind parses; the prior `svn` parse-rejection became a blank-type shape
  guard). 391 → **394**.

Because providers are built at run start (`defaultDeps` → `createProvidersFromConfig`,
before the dry-run branch and `validatePhaseChain`), an unknown kind is caught up
front, even on `--dry-run`. Verified: typecheck, `lint:env`, full suite (394),
build+clean, and a CLI dry-run (byte-identical to the documented baseline — the
committed `github`/`git` config still prints the full plan). Committed and merged
to `main` in `0ac57a5` (see §1).

Next for #4: **step 2 — the declarative `rest`/`cli` driver (Option B)**. Its
concrete design is in `docs/DESIGN-declarative-providers.md` (sub-steps 2a `cli`
→ 2b `rest` → 2c change-requests); **2a is now implemented — see §8k**.

## 8k. Follow-up session — declarative `cli` work-item provider (§11 #4, step 2a)

First implementation slice of **step 2** (the declarative driver, Option B),
per `docs/DESIGN-declarative-providers.md` §12 sub-step 2a. A project can back
its work items with a non-GitHub forge by *describing* the provider — command
templates + field mappings — instead of shipping code. **No new dependency, no
code loading**; the built-in `github`/`git` paths and the dry-run baseline are
byte-for-byte unchanged.

- `adw_sdlc/src/provider-descriptor.ts` (new) — the lone interpreter of
  descriptor *data*: a strict Zod descriptor schema, a **dependency-free** path
  mini-language (`parsePath`/`evalScalar`/`evalArray` over `$.a.b`, `$.a[0]`,
  `$.a[*]`, `$.a[*].name`), placeholder validation, and the credential guard
  (`authEnv` rejected if `GH_TOKEN`/`GH_BIN`, deny-prefixed, or a model
  credential — built from `ENV_DENY_PREFIXES` + `RUNNER_ENV_ALLOW`). Imports
  only zod/errors/env + the `WorkItemContext` type — no config/providers edge.
- `adw_sdlc/src/providers-rest-cli.ts` (new) — `createCliWorkItemProvider(descriptor,
  captureFn?)`: substitutes `{id}`/`{repo}`/… into the route argv, runs it via
  `capture()` with a **scoped one-credential env** from `safeSubprocessEnv({
  allowGhToken: false, extraAllow: [authEnv] })`, and maps JSON →
  `WorkItemContext`. `fetch`/`state` required (`state` ⇒ `UNKNOWN` on
  failure); `postProgress`/`assignSelf`/`setStatus` optional no-op routes. The
  back-edge to `providers.ts` is type-only (erased) → graph stays acyclic.
- `adw_sdlc/src/exec.ts` — `capture(cmd, { env })` optional param (spawnSync
  `env` replace semantics). Only the declarative driver passes it; gh/git
  callers inherit as before. No parent-env spread (lint:env stays green).
- `adw_sdlc/src/providers.ts` — registered `cli` in `WORK_ITEM_PROVIDERS`; the
  work-item factory type now receives the resolved `config` (github ignores it,
  cli reads `providers.workItems` through `parseCliWorkItemDescriptor`).
- `adw_sdlc/src/config.ts` — optional loose `authEnv` / `routes` on
  `providers.workItems` (preserved as shape; semantics validated in
  provider-descriptor.ts at construction). Defaults unchanged.
- `adw_sdlc/src/index.ts` — exports `createCliWorkItemProvider`,
  `parseCliWorkItemDescriptor`, `parsePath`, `evalScalar`, `evalArray`, and the
  `CliWorkItemDescriptor`/`PathSegment` types.
- `adw_sdlc/docs/UNIVERSAL.md` — new "Declarative `cli` work items" subsection.
- Tests: `test/provider-descriptor.test.ts` (new, +10 — path grammar, evaluator,
  and the descriptor guards: missing/extra fields, scalar/array mismatch,
  unknown placeholder, reserved `authEnv`) and `test/providers.test.ts` (+5 —
  cli build via `createProvidersFromConfig`, fetch mapping with the scoped env
  asserting `GITLAB_TOKEN` in / `GH_TOKEN` withheld, `UNKNOWN`/null fallbacks,
  write-route no-op, and fail-closed-at-construction). The §8j
  `supportedProviderTypes` snapshot + fail-closed message were updated to
  include `cli`. 394 → **409**.

Fail-closed at run start: `defaultDeps` → `createProvidersFromConfig` validates
the descriptor before the dry-run branch and any side effect, so a misconfigured
`cli` provider fails on `--dry-run` too. Verified end-to-end through the real
ESM graph (acyclic load; config → registry → mapped `WorkItemContext`;
`authEnv: "GH_TOKEN"` rejected), plus typecheck, `lint:env`, full suite (409),
build+clean, and the byte-identical github dry-run. Committed and merged to
`main` in `0ac57a5` (see §1).

Next for #4: **2b — the declarative `rest`/HTTP provider** (per-provider host
allowlist + the kernel-owned one-shot fetch helper) — **now implemented, see
§8l**; then **2c —** declarative change-requests (the merge-authorized path).
Step 3 (out-of-process plugin, Option C) remains a §10 hard stop.

## 8l. Follow-up session — declarative `rest`/HTTP work-item provider (§11 #4, step 2b)

Second implementation slice of step 2: the declarative driver over HTTP, for
forges without a CLI. Read routes (`fetch`/`state`) only; **no new dependency,
no code loading, no interface change**; built-ins and the dry-run baseline
unchanged.

- `adw_sdlc/src/provider-descriptor.ts` — added `parseRestWorkItemDescriptor`
  (+ `RestWorkItemDescriptor`): a strict Zod descriptor, reusing the shared
  `fetch`/`state` map schemas + compile helpers factored out of the cli path.
  New guards: `assertAllowedHost(url, allowedHosts)` (https + exact host[:port]
  allowlist), `assertRestPath` (plain `/path`, no scheme/authority, only
  `{id}`/`{repo}`), bare-host validation, and the shared `assertSafeAuthEnv`
  (now required for `rest`). `authHeader` (default `Authorization`) + `authScheme`
  (default `Bearer`, `""` ⇒ raw token) cover forge auth variants.
- `adw_sdlc/src/providers-rest-cli.ts` — `createRestWorkItemProvider(descriptor,
  transport?)`: resolves `baseUrl + path` with **percent-encoded** placeholders
  (so `{repo}`/`{id}` cannot alter the host), **re-asserts** host+https per call
  (defense in depth), maps JSON → `WorkItemContext`, `UNKNOWN`/null on
  non-2xx/error/garbage. Default `restTransportViaNode` spawns a **kernel-owned
  inline `node -e` one-shot fetch** (`REST_FETCH_SCRIPT`) via `spawnSync` with a
  scoped one-credential env + the request on **stdin**; the token is read by
  **name inside the child** (never argv). `RestRequest`/`RestResponse`/
  `RestTransport` are exported (transport is an injectable test seam). Write
  methods no-op in 2b (rest body templating deferred to 2c).
- `adw_sdlc/src/providers.ts` — registered `rest` in `WORK_ITEM_PROVIDERS`.
- `adw_sdlc/src/config.ts` — added loose `baseUrl`/`allowedHosts`/`authHeader`/
  `authScheme` to `providers.workItems` (validated by the rest loader).
- `adw_sdlc/src/index.ts` — exports the rest provider, transport, descriptor
  parser, `assertAllowedHost`, and the new types.
- `adw_sdlc/docs/UNIVERSAL.md` — new "Declarative `rest` (HTTP) work items"
  subsection. `docs/DESIGN-declarative-providers.md` — status + the two
  implementation refinements (configurable `authHeader`; inline helper vs. a
  shipped `.mjs`).
- Tests: `test/provider-descriptor.test.ts` (+6 — rest compile/defaults,
  authHeader override, https/host rejection, path rejection, missing/reserved/
  malformed, plus `assertAllowedHost`) and `test/providers.test.ts` (+4 — rest
  driver url-encoding + scoped env (GH_TOKEN withheld), non-2xx/error/garbage
  fallbacks, write no-ops, build-via-config + off-allowlist fail-closed). The
  §8j/§8k snapshots were updated to include `rest`. 409 → **419**.

Two refinements vs. the design draft (both documented): the auth **header** is
configurable (not just the scheme), needed for GitLab PAT (`PRIVATE-TOKEN`) vs.
GitHub/Gitea (`token`) vs. Bearer; and the fetch helper is an **inline `node -e`
script** (kernel constant) rather than a shipped `.mjs` asset — same security
properties, no path-resolution/build-copy concern.

Verified: typecheck, `lint:env`, full suite (419), build+clean, byte-identical
github dry-run, and a **live two-process loopback roundtrip** of the real
transport (helper read the token from its scoped env and sent
`Authorization: Bearer <token>`; status 200; body mapped). Committed and merged
to `main` in `0ac57a5` (see §1).

Next for #4: **2c — declarative change-requests** — **now implemented, see §8m**;
then **step 3** the out-of-process plugin broker (Option C, still a §10 hard stop).

## 8m. Follow-up session — declarative `rest` change-request provider (§11 #4, step 2c)

The merge-authorized declarative path, completing step 2 for both provider roles
over `rest`. A non-GitHub forge (GitLab/Gitea MRs, …) can now back the change-
request lifecycle as validated data. **No new dependency, no code loading, no
interface change**; the orchestrator keeps the gating and all git. Built-ins and
the dry-run baseline unchanged.

- `adw_sdlc/src/provider-descriptor.ts` — factored a shared rest **base**
  (`restBaseFields`/`RestBase`/`resolveRestBase`) out of the 2b work-item path;
  parameterized `assertRestPath` by allowed placeholders; added
  **request-body placeholder validation** (`assertBodyPlaceholders` over a JSON
  template's string leaves). New `parseRestChangeRequestDescriptor` +
  `RestChangeRequestDescriptor`: `findForBranch`/`create`/`squashMerge` required,
  `pipelineStatus` optional with a forge-status→`CiState` `stateMap`. Per-route
  placeholder sets (create gets `{branch,base,title,body,repo}`; merge/pipeline
  get `{id,repo}` — `{id}` is intentionally NOT bound at create time).
- `adw_sdlc/src/providers-rest-cli.ts` — `createRestChangeRequestProvider`
  (+ `substituteBody` deep JSON templating, a shared `makeRestRequester`
  reused by both rest roles). `create` substitutes/sends the JSON body and maps
  `number`/`url`→`{id,number,url}`; `squashMerge` issues the templated write and
  reports ok/failure; `pipelineStatus` maps via `stateMap` (absent route ⇒
  `none`, fetch failure ⇒ `unknown`, `failingJobs: []`); `findForBranch` →
  url|null. `RestRequest` gained an optional JSON `body`; the inline `node -e`
  helper now sends it with `content-type: application/json`.
- `adw_sdlc/src/providers.ts` — registered `rest` in `CHANGE_REQUEST_PROVIDERS`
  (factory now receives the resolved `config`, like work items).
- `adw_sdlc/src/config.ts` — loose `baseUrl`/`allowedHosts`/`authEnv`/
  `authHeader`/`authScheme`/`routes` on `providers.changeRequests`.
- `adw_sdlc/src/index.ts` — exports the CR provider/parser + `RestBase`/
  `RestChangeRequestDescriptor`.
- `adw_sdlc/docs/UNIVERSAL.md` — new "Declarative `rest` change requests"
  subsection. `docs/DESIGN-declarative-providers.md` — status + rollout 2c done.
- Tests: `test/provider-descriptor.test.ts` (+4 — CR compile/defaults, optional
  pipelineStatus, body/path placeholder rejection, https/host/credential/
  required-route guards) and `test/providers.test.ts` (+5 — create body
  substitution + number/url/id mapping + scoped env (GH_TOKEN withheld),
  findForBranch url|null, squashMerge templated PUT ok/failure, pipelineStatus
  stateMap + absent⇒none, build-via-config + off-allowlist fail-closed). The
  §8j/§8k/§8l snapshot updated to include CR `rest`. 419 → **428**.

**`squashMerge` security review (the merge-authorized op):** it is bound by the
same host allowlist + https + scoped one-credential env as every rest route; the
provider never receives `GH_TOKEN` or raw git/gh. The worst a hostile descriptor
can do is one templated request to its own allowlisted forge host with the user's
own scoped forge token — the trust the user already extends by configuring the
provider (the same posture as `gh` + `GH_TOKEN` for the github built-in). The
orchestrator still calls `squashMerge` only after the review/CI gates pass, and
all git (branch/commit/push) stays the built-in `git` VcsProvider.

Verified: typecheck, `lint:env`, full suite (428), build+clean, byte-identical
github dry-run, and a **live two-process loopback roundtrip** of the real
transport doing a `create`-shaped **POST with a templated JSON body** —
the server received `Authorization: Bearer <token>` (from the child's scoped env)
and the substituted body. Committed and merged to `main` in `0ac57a5` (see §1).

Scope note: 2c is `rest`-only; a `cli` change-request provider (`glab mr …`) is a
symmetric follow-up (rest covers the forges). Next for #4: **step 3** the
out-of-process plugin broker (Option C) — still a §10 hard stop for its own slice.

## 9. Files created/modified this session

### Priming (restored to make the baseline green)

- `node_modules/.bin/*` — regenerated 12 symlinks (had been flattened to
  copies during a non-symlink-preserving extraction)
- `.claude/commands/*.md` — restored 14 byte-identical mirrors of
  `.pi/prompts/*.md`
- `scripts/check-adw-sdlc-env.sh` — restored the fail-closed
  secret-boundary lint gate

### Universalization

- `.adw/config.json` (new) — committed HealthTech project pack
- `adw_sdlc/src/config.ts` (new) — validated config schema + loader +
  `isClosedWorkItemState()` helper
- `adw_sdlc/src/providers.ts` (new) — provider interfaces + GitHub/Git
  adapters + `createProvidersFromConfig()` + `providerBackedDeps()`
- `adw_sdlc/src/work-item.ts` (new) — provider-neutral work-item module
- `adw_sdlc/docs/UNIVERSAL.md` (new) — universal architecture reference
- `adw_sdlc/docs/examples/payments-api.config.json` (new) — non-HealthTech
  example project pack
- `adw_sdlc/HANDOVER.md` (new, this file)

### Migrated for provider/neutral types and config wiring

- `adw_sdlc/src/cli.ts` — `workItem` parsed field, `runWorkItem` dispatch
  alias, provider-neutral help text
- `adw_sdlc/src/common.ts` — REPO_ROOT doc comment updated
- `adw_sdlc/src/exec.ts` — `formatProgress` reads `progress.tag` from
  config
- `adw_sdlc/src/index.ts` — exports universal types
- `adw_sdlc/src/issue.ts` — renamed core types to `WorkItemContext` /
  `fetchWorkItem` with backward-compatible aliases, added
  configurable status field name
- `adw_sdlc/src/models.ts` — `modelForPhase` reads tiers from config;
  `classifyModel()` helper added
- `adw_sdlc/src/orchestrator.ts` — provider-first runtime path,
  configurable in-progress / closed states, provider-neutral work-item
  metadata recording, work-item-label-aware prompt blob
- `adw_sdlc/src/phases.ts` — template root + gate hints + file rules
  read from config
- `adw_sdlc/src/state.ts` — additive `work_item` / `change_request`
  metadata fields, tolerant load
- `adw_sdlc/src/structured-call.ts` — classify model from config

### Schema + cross-language fixtures

- `adw/state.schema.json` — documents `work_item` and `change_request`
- `adw/fixtures/cross_language/ts-produced-state.json` — TS-produced
  fixture refreshed with the new additive metadata

### Tests added / updated

- `adw_sdlc/test/config.test.ts` (new) — config parsing, deep-merge,
  drift guard for the example pack, closed-states + in-progress
  configurable behavior
- `adw_sdlc/test/providers.test.ts` (new) — provider adapter, no-`gh`
  fallbacks, legacy shim equivalence, pipeline-status migration
- `adw_sdlc/test/work-item.test.ts` (new) — work-item module
  compatibility surface
- `adw_sdlc/test/orchestrator.test.ts` — provider-first regression
  test, work-item/change-request metadata assertions
- `adw_sdlc/test/issue.test.ts` — `WorkItemContext` ↔ `IssueContext`
  alias test
- `adw_sdlc/test/cli.test.ts` — `workItem` alias, `runWorkItem`
  dispatch preference
- `adw_sdlc/test/cross-language-state.test.ts` — TS_ADDITIVE_KEYS now
  includes `work_item`, `change_request`
- `adw_sdlc/test/state.test.ts` — additive metadata serialization +
  loading

## 10. Hard stops we hit (do not breach without a separate design pass)

The session stopped at a natural diminishing-returns point. The next
plausible slices each cross a real boundary:

1. **External provider plugin loading from a config-supplied module path**
   — would introduce a new code-loading attack surface on the
   secrets-owning CLI. Needs its own security/sandboxing design.
2. **Phase preamble wording changes** — LLM-facing prompt content;
   rewording risks behavior drift the suite cannot detect.
3. **`MX_AGENT_*` env var rebranding** — load-bearing in
   `ENV_DENY_PREFIXES` and the static lint; weakens the secret boundary.
4. **CLI command rename (`issue` → `work-item`)** — documented backward
   compatibility alias; rename would break downstream and the Python
   engine handoff.
5. **File rename `issue.ts` → `work-item.ts`** — would force external
   importers to migrate without a grace period. The `work-item.ts`
   wrapper module already covers new code paths.

If a future session wants any of these, treat it as a new design pass,
not a continuation slice.

## 11. Recommended next steps (if continuing)

Ordered by ratio of value to risk:

1. **Phase catalog config-driven** — ✅ DONE (this session). `.adw/config.json`
   now takes an optional `phases` array. Precedence: `--phases` CSV >
   configured `phases` > full built-in catalog. The kernel still owns
   `AGENT_PHASES` and all phase semantics (loops, conditional gates,
   per-phase schemas); `parsePhases` validates the configured list against
   the catalog and fails loudly on an unknown name. Projects may reorder or
   drop known phases only. Inventing genuinely NEW phase names remains out of
   scope and depends on #2 (per-phase schema overrides). Config validates
   shape; the kernel validates membership — this deliberately avoids a
   `config.ts` ⇄ `phases.ts` import cycle.
2. **Schema overrides per phase** — ✅ FULLY DONE (design + all 3 rollout
   steps, this session): `docs/DESIGN-schema-overrides.md`. Key reframe: custom
   schemas/phases have no Python counterpart, so D4 parity applies only to the
   9 built-in shapes — the coercion layer stays hardcoded and a separate
   ajv-validated path handles overrides and custom phases. Rollout: **(1)
   registry indirection** (`src/schema-registry.ts` — §8e); **(2) capability A**
   (override safe phases `tests`/`e2e`/`document` via ajv; load-bearing/
   `classify` overrides rejected — §8f); **(3) capability B** (register plain
   new phases via `config.customPhases` + `phases` chain + `<name>.md` template
   + `.adw/schemas/<name>.json` — §8g). Still out of scope: loop/gated custom
   phases and overriding `classify` (its in-process Zod path).
3. **Provider-neutral `Done` / `Resolved` transition on success** — ✅ DONE
   (this session). Optional `providers.workItems.doneStatus` moves a work
   item to its terminal status on verified merge, best-effort, via the
   existing `setStatus` provider method (`transitionToDone` in
   `orchestrator.ts`). Unset by default, so GitHub (which auto-closes via
   "closes #N") is unchanged; non-GitHub providers set `doneStatus` and add
   it to `closedStates` for the verify gate. See §8c.
4. **Provider plugin loading (security-reviewed)** — 🚧 IN PROGRESS (staged).
   Design done (`docs/DESIGN-provider-plugins.md`; step-2 detail in
   `docs/DESIGN-declarative-providers.md`). **Step 1 (open the factory switch
   internally) ✅ DONE (§8j)**: a fail-closed per-role provider registry with a
   config shape/registry-membership split. **Step 2 (declarative driver) IN
   PROGRESS — sub-step 2a (`cli` work-item provider) ✅ DONE (§8k)**: descriptor
   data + a dependency-free response-mapping mini-language + a scoped
   one-credential env. **Sub-steps 2b (`rest` work items, §8l) and 2c (`rest`
   change-requests, §8m) ✅ DONE**: host-allowlisted + https-only HTTP via a
   kernel-owned inline fetch helper, percent-encoded path placeholders, templated
   JSON request bodies, configurable `authHeader`/`authScheme`, and the
   `squashMerge` merge-authority review. Step 2 is complete for work items and
   change requests; all of 2a–2c add no new dependency and no code loading.
   Remaining: an optional `cli` change-request provider (symmetric follow-up),
   then **step 3** the out-of-process plugin broker (Option C) — still a §10 hard
   stop for its own slice. In-process `import` of config-supplied code (Options
   A/D) stays a rejected non-goal.
5. **Universal README at package root** — ✅ DONE (this session).
   `adw_sdlc/README.md` is now the neutral entry point: pipeline overview,
   kernel/project-pack split, quick start + key flags, the config-surface
   table (including the new `phases` and `doneStatus` knobs), development
   commands, the secret-boundary note, and a documentation map linking
   `docs/UNIVERSAL.md`, `HEALTHTECH_PORT.md`, `PLAN.md`, `PARITY.md`,
   `MEMORY_STACK.md`, and `HANDOVER.md`. Pure docs; no code/test changes.

**Status:** #1, #2, #3, #5 are done. **#4 (provider plugin loading) is now in
progress** — its security design landed earlier (`5cc4462`) and **step 1 of the
staged rollout (the fail-closed provider registry) shipped this session (§8j)**.
Steps 2–3 (declarative `rest`/`cli` driver, then the out-of-process plugin
broker) remain; step 3 is still a §10 hard stop for its own slice. New candidate
slices identified while landing #2 (none on the original list, all optional):

- **Loop/gated custom phases** — ✅ DONE (this session, §8i). A registered custom
  phase may opt into a conditional gate (`gates.custom.<phase>`) and/or a
  resolve-style loop (`loops.<phase>`), reusing the `gateDocument` matching and
  the generalized `resolveLoop`. Built-in control flow is unchanged.
  `docs/DESIGN-custom-phase-control-flow.md`. Still out of scope: `patch`-style
  findings loops and overriding `classify`.
- **Custom-phase startup validation** — ✅ DONE (this session, §8h). The whole
  resolved chain is preflighted at run start via `validatePhaseChain`
  (template resolves + `resolvePhaseSchema` loads), so a missing `<name>.md`
  template, missing `.adw/schemas/<name>.json` schema, or broken/unsupported
  override fails loudly before any side effects; a `--dry-run` runs the same
  check.
- **Security design doc for #4** — ✅ DONE (this session; merged to `main` as
  `5cc4462`). `docs/DESIGN-provider-plugins.md` drafts the threat model + four
  isolation options for provider *code* loading and recommends declarative
  providers + an out-of-process broker over any in-process `import`. No
  implementation; #4 itself remains a §10 hard stop.

## 12. How to resume in a new session

A future agent should:

1. Read this file (`adw_sdlc/HANDOVER.md`) first.
2. Read `adw_sdlc/docs/UNIVERSAL.md` for the universal architecture, then
   `HEALTHTECH_PORT.md` for the current project's status.
3. Run the verification gates in §8 to confirm a green baseline.
4. Check §3 (operational invariants) before changing anything.
5. Pick from §11 (recommended next steps) or take a fresh direction
   from the user.

Test count baseline after this session: **428 passing across 32 files**
(343 at the original handover, +4 for the configurable phase chain, +3 for
the terminal done-status transition, +3 for the schema-registry indirection,
+10 for schema overrides capability A, +9 for custom phases capability B, +6
for custom-phase startup validation, +13 for loop/gated custom phases, +3 for
the provider-kind registry — §8j, +15 for the declarative `cli` work-item
provider — §8k, +10 for the declarative `rest`/HTTP work-item provider — §8l,
+9 for the declarative `rest` change-request provider — §8m). The session left no
build artifact, no temporary files, and no untracked binary churn. The ADW
orchestrator code path still runs no `git`/`gh` itself; the commits and the local
merge to `main` recorded in §1 were performed only at the user's explicit request
(there is no remote — the merge is local). **The §8j provider-registry and
§8k/§8l/§8m declarative-provider (`cli`/`rest` work-items + `rest`
change-requests) slices are committed as `0ac57a5` and merged to `main`
(`07b90f6`)** — see §1. The working tree is clean.
