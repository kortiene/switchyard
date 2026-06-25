# Design proposal — per-phase schema overrides (§11 #2)

**Status:** proposal, not implemented. This is the dedicated design pass the
handover (`HANDOVER.md` §10/§11) requires before touching the structured-output
path. It exists so the work can start from a settled shape rather than a
judgement call mid-implementation.

**Author context:** follows the universalization slices already landed —
configurable phase chain (§11 #1, `HANDOVER.md` §8b) and the terminal
done-status transition (§11 #3, §8c).

---

## 1. Goal

Let a project supply its own per-phase result schema instead of the built-in
one, through the project pack (`.adw/`), without forking the kernel. Two
distinct capabilities fall under this heading; they have very different risk:

- **A. Override an existing catalog phase's schema** — e.g. add a
  `risk_level` field to the `review` result, or tighten `classify`'s enum to a
  project's label taxonomy.
- **B. Register a genuinely new phase** — the capability §11 #1 explicitly
  deferred here. A new phase name needs a schema *and* a prompt template, a
  model tier, an output contract, and (if it loops or gates) kernel semantics.

## 2. Non-goals

- **Not** loop or conditional-gated custom phases. The `resolve`/`patch` bounded
  loops and the `e2e`/`document` gates are kernel control flow keyed to phase
  name (`orchestrator.ts` `LOOP_PHASES`/`CONDITIONAL_PHASES` handling); a
  config-injected new phase cannot acquire those without code. v1 supports only
  **plain, sequential, non-loop, non-gated** new phases.
- **Not** changing the 9 built-in phases' parity with the Python engine.
- **Not** provider/plugin *code* loading (that is §11 #4, an explicit hard stop).

## 3. How schemas flow today (the touchpoints to honor)

Four code paths consume per-phase schemas. Any override mechanism must reconcile
with all four.

1. **Parse + validate (Zod, parent-side, always on).**
   `schemas.ts` `PHASE_SCHEMAS` are Zod objects; `parsePhaseResult(phase, data)`
   (`schemas.ts:194`) runs Python-parity coercion tables (`BOOL_FIELDS`,
   `STRING_FIELDS`, `LIST_OF_STRING_FIELDS`, `FLOAT_TRUNC_FIELDS`, the special
   `review.findings` handling) and then `PHASE_SCHEMAS[phase].safeParse`. This is
   defense-in-depth — the parent always re-validates whatever the backend
   returns (`schemas.ts` header, `run-phase.ts:108` `extract`).

2. **Native-schema channel (JSON Schema, passthrough).**
   For backends with `caps.nativeSchema` (`run-phase.ts:68,79`), the parent hands
   the runner `phaseJsonSchema(phase)` = `z.toJSONSchema(PHASE_SCHEMAS[phase])`
   (`schemas.ts:124`). `JsonSchema` is just `Record<string, unknown>`
   (`invoker.ts:18`) — a plain JSON Schema object. **A custom JSON Schema can
   feed this channel directly with no conversion.**

3. **Fenced-JSON output contract (string, non-native backends).**
   `OUTPUT_CONTRACT[phase]` (`phases.ts:229`) is the example shape rendered into
   the prompt footer for backends without native schema. The **contract-drift
   guard** (`phases.test.ts:122`) asserts every `OUTPUT_CONTRACT` example parses
   via `parsePhaseResult` *and* that every documented key exists in the Zod
   schema's `.shape`. An override that changes the shape must change this string
   too, or the guard fails and the agent is told one shape but validated against
   another.

4. **classify's shared structured call (Zod-only).**
   classify can run in-process on the Anthropic SDK via `structuredCall`
   (`structured-call.ts:62`), which takes a **Zod** schema (`zodOutputFormat`,
   `schema.parse`) — not JSON Schema. This path is Zod-bound and is a wrinkle for
   overriding classify specifically (see §6.6).

Model routing is **not** a touchpoint: `modelForPhase` already resolves unknown
phases to the default tier (`models.ts` `PHASE_TIER` is `Record<string, Tier>`,
"Unknown phases resolve as 'mid'"), so a new phase gets a model for free.

## 4. The reframe that makes this safe

The Python engine is the only reason the coercion layer in `parsePhaseResult`
exists — it mirrors `adw/_phases.py` `to_result` so both engines read a runner
payload identically (PLAN.md D4, `HANDOVER.md` §3.6). **Custom schemas and
custom phases have no Python counterpart.** The Python reader already drops
fields it does not know (same contract as the additive `work_item` /
`change_request` state fields, `HANDOVER.md` §7). Therefore:

> The D4 byte-for-byte parity obligation applies **only** to the 9 built-in
> phases' built-in shapes. Anything a project adds on top is TS-only and carries
> no parity debt.

This collapses most of the perceived risk: the coercion tables do not need to
become configurable. They stay hardcoded for the built-in phases; custom
schemas are validated by a separate, coercion-free path.

## 5. The override contract: load-bearing result fields

The orchestrator reads specific result fields to drive control flow. An override
of an **existing** phase must preserve these (additive changes are free):

| Phase | Field(s) the kernel consumes | Where |
| --- | --- | --- |
| `classify` | `issue_class` | `orchestrator.ts` `applyResult` |
| `plan` | `plan_file` | `applyResult` |
| `implement` | `files_changed` | sets `files`/`signal` |
| `review` | `findings[].severity` / `.description` / `.location` | patch gate + persist |
| `resolve` / `patch` | `resolved`, `remaining` | bounded-loop exit |

`tests` / `e2e` / `document` carry no control-flow-bearing result fields (their
booleans are recorded but never branched on), so they are the safest to
override. The override loader (§6.3) validates that an override of a load-bearing
phase still declares the required fields, and **fails loudly at config load** if
not — the kernel must never silently lose a field it depends on.

## 6. Design

### 6.1 Config surface

Convention-first, mirroring `prompts`:

```
.adw/
  schemas/
    <phase>.json     # a JSON Schema; overrides the built-in for <phase>
```

Plus an optional explicit map for non-default locations (parallels
`prompts.runnerRoots`):

```json
"schemas": {
  "root": ".adw/schemas",          // optional; default ".adw/schemas"
  "overrides": {                    // optional explicit phase -> path
    "review": ".adw/schemas/review.v2.json"
  }
}
```

Resolution per phase: explicit `overrides[phase]` > `root/<phase>.json` >
built-in. Absent → today's behavior exactly (behavior-preserving).

### 6.2 Validation engine

The override is authored as **JSON Schema** (so the native channel is a
passthrough). The parent still must validate payloads defense-in-depth, but the
built-in Zod schema does not describe the custom shape. Options:

- **(R) Recommended: add `ajv` as a dependency, used only on the override
  path.** Built-in phases keep their Zod + coercion path unchanged; overridden
  phases validate via a compiled ajv validator. Clean separation, no parity
  risk, no lossy conversion. Cost: one well-audited dependency, compiled once per
  run. ajv must be configured strict and offline (no `$ref` to remote URLs) —
  this is data validation, not code loading, but remote `$ref` resolution would
  be a network/SSRF surface and must be disabled.
- (Rejected) JSON-Schema → Zod conversion: no first-party converter exists
  (`z.toJSONSchema` is forward-only); third-party converters are lossy and add
  the same dependency weight without the clarity.
- (Rejected) Restrict overrides to a Zod-expressible subset authored as JSON:
  surprising to users and still needs a parser.

### 6.3 The schema registry

Introduce a small indirection so the four touchpoints ask a registry instead of
indexing `PHASE_SCHEMAS` directly:

```
resolvePhaseSchema(phase, config) -> {
  jsonSchema(): JsonSchema           // for the native channel (§3.2)
  validate(payload): Result          // ajv (override) or parsePhaseResult (built-in)
  outputContract(): string           // §3.3 — see 6.4
  requiredKeys(): string[]           // for the §5 load-bearing check
}
```

Built-in phases return today's exact behavior (`phaseJsonSchema`,
`parsePhaseResult`, `OUTPUT_CONTRACT[phase]`). Overridden phases return the ajv
path. `run-phase.ts`, the native-channel wiring, and the footer composition call
the registry. This keeps the change localized and keeps `parsePhaseResult`
untouched for built-ins (so the parity suite is unaffected).

### 6.4 Output contract for non-native backends

For an overridden phase the fenced-JSON footer example must match the override.
Derive it mechanically from the JSON Schema (a minimal example generator over the
schema's properties), rather than asking the project to hand-write a second
copy. The drift guard (`phases.test.ts:122`) generalizes to: for every phase,
the resolved `outputContract()` example must pass the resolved `validate()`.
Built-in phases keep their exact hardcoded strings (the generator is not run for
them), so the existing guard semantics are preserved.

### 6.5 New phases (capability B)

A new phase name additionally needs:

- **Template** — already covered: `templatePath` (`phases.ts:81`) resolves
  `<name>.md` under the configured roots. Today `TEMPLATE` maps phase→basename;
  generalize so an unknown phase defaults its basename to its own name.
- **Output contract** — from §6.4's generator.
- **Model tier** — already covered (default tier fallback).
- **Orchestrator handling** — runs through the generic `deps.runAgentPhase`
  path (`orchestrator.ts:1138`). Its result is validated and `markDone`'d but
  **not** branched on (it is not in the §5 table). This is the "plain phase"
  contract.
- **Phase-chain membership** — `parsePhases` (`phases.ts`) currently rejects any
  name outside `AGENT_PHASES`. Extend the known set to `AGENT_PHASES ∪
  registeredCustomPhases(config)` so a custom phase can appear in the `phases`
  chain. Loop/gate phases remain kernel-only (§2 non-goal).

### 6.6 classify wrinkle

classify's in-process path (`structuredCall`) is Zod-bound. For v1, **disallow
overriding `classify`** (it is the one phase whose contract the kernel both
constrains to an enum and routes specially); document it as unsupported and fail
loudly if a `classify` override is present. Revisit only if a concrete need
appears — it would require either routing an overridden classify through the
runner channel unconditionally, or building JSON-Schema output support into the
shared structured call.

## 7. Risks & mitigations

| Risk | Mitigation |
| --- | --- |
| Override drops a load-bearing field → kernel breaks silently | §5 required-field check, fail-closed at config load |
| Remote `$ref` in a JSON Schema → network/SSRF on a secrets-owning CLI | ajv configured offline; reject non-local `$ref` at load |
| Override desyncs from the prompt footer | §6.4 derives the footer from the schema; drift guard generalized |
| Parity suite regression on built-ins | built-in path is byte-for-byte unchanged; registry only branches for overridden phases |
| New dependency surface (ajv) | widely-audited, validation-only, pinned; no code execution |
| Custom phase silently no-ops control flow | documented "plain phase" contract; result recorded, never branched |

## 8. Test plan

- Registry: built-in phases resolve to today's exact `jsonSchema`/`validate`/
  `outputContract` (snapshot equality).
- Override of a safe phase (`tests`): payload matching the override validates;
  one violating it fails loudly.
- Required-field guard: an override of `review` omitting `findings` fails at
  load with a clear message.
- Footer generation: generated contract for an override passes its own validate.
- New plain phase end-to-end: appears in the `phases` chain, runs through the
  generic path, result recorded, `markDone`; gated/loop name in a chain is
  rejected.
- classify override rejected loudly.
- Behavior-preserving: with no `.adw/schemas/`, every existing test is unchanged.

## 9. Phased rollout

1. **Registry indirection** (no behavior change): route the four touchpoints
   through `resolvePhaseSchema`, built-ins only. Pure refactor; suite stays green.
   ✅ **DONE.** `src/schema-registry.ts` exposes `resolvePhaseSchema(phase) ->
   { jsonSchema, validate, outputContract, requiredKeys }`, all delegating to
   the built-ins. `OUTPUT_CONTRACT` moved to `schemas.ts` (beside
   `PHASE_SCHEMAS`) and is re-exported from `phases.ts` for source
   compatibility, which keeps the dependency graph acyclic
   (`run-phase`/`phases` → `schema-registry` → `schemas`). `run-phase.ts`
   (validate + native jsonSchema) and `phases.ts` `buildFooter`
   (outputContract) now go through the registry. Zero existing-test changes;
   `test/schema-registry.test.ts` pins built-in delegation. classify's
   in-process Zod path (`structuredCall`) is intentionally left untouched
   (§6.6). Suite: 353 passing / 28 files.
2. **Capability A** (override existing, safe phases): ajv path + required-field
   guard + footer generator + drift-guard generalization. ✅ **DONE.** A project
   drops `.adw/schemas/<phase>.json` (or maps `schemas.overrides[phase]`) for a
   phase in `OVERRIDABLE_PHASES` (`tests`/`e2e`/`document`). `schema-override.ts`
   loads/sanity-checks the JSON Schema (object-typed, no remote `$ref`), compiles
   it with ajv (`strict:false`, fresh instance per compile), validates payloads,
   and generates the fenced-JSON footer example from the schema. The registry
   rejects an override of any load-bearing or excluded phase (incl. `classify`)
   loudly. `ajv@^8.20.0` is now a direct dependency. `test/schema-overrides.test.ts`
   (+10) covers it. Suite: 363 passing / 29 files. Scope held: the built-in
   Zod + coercion path is untouched, so the parity suite is unaffected.
3. **Capability B** (plain new phases): template/tier/contract generalization +
   `parsePhases` known-set extension. ✅ **DONE.** A project lists new names in
   `config.customPhases`, places them in the `phases` chain, and supplies a
   template (`<name>.md`) + schema (`.adw/schemas/<name>.json`); the tier comes
   from `models.phaseTiers[name]` (else default). `parsePhases` validates
   against built-ins ∪ custom (rejecting unknowns and built-in collisions);
   `composePhasePrompt` defaults a custom phase's template basename to its name
   and carries no reframing; `resolvePhaseSchema` returns an ajv handle from the
   required custom schema; the orchestrator runs it through the generic path
   (recorded, never branched on). Phase-identifier types widened to `string` at
   the seams (Sets, `parsePhases`, `composePhasePrompt`/`buildFooter`,
   `phaseArgs`/`applyResult`) with `resolvePhaseSchema` overloaded
   (built-in-generic + string); the built-in Zod path is byte-for-byte
   unchanged. Loop/gated custom phases stay out of scope (§2). `ajv` already
   landed in capability A. `test/custom-phases.test.ts` (+8) and an orchestrator
   integration test (+1). Suite: 372 passing / 30 files.

**Follow-up — startup preflight (capability B hardening).** Both overrides and
custom phases resolve their template/schema lazily, so a misconfiguration first
surfaced mid-chain (after branches/PRs existed). `validatePhaseChain(phases,
runner, config)` (`phases.ts`) now walks the resolved chain at run start —
checking each phase's template resolves and `resolvePhaseSchema` loads its schema
(compiling overrides/custom schemas eagerly) — and the orchestrator calls it
right after `parsePhases`, before the `--dry-run` branch, so a missing template,
missing custom schema, or broken/unsupported override fails loudly up front and a
dry-run doubles as a config check. Built-ins without an override are a no-op.
`test/custom-phases.test.ts` (+5) and an orchestrator dry-run preflight test (+1).

Each step is independently shippable and independently verifiable against the
full gate suite (`HANDOVER.md` §8).

## 10. Open questions

- Footer example generation from arbitrary JSON Schema: bounded to the shapes
  ADW phases actually use (flat objects, arrays of flat objects)? A general
  generator is overkill; a constrained one matching the built-in contracts is
  enough and safer.
- Should an override be allowed to *narrow* a built-in (e.g. classify enum) while
  keeping the Zod/coercion path? Out of scope for v1 (classify is excluded); for
  others, narrowing-only overrides could later reuse the Zod path instead of ajv.
- Do any runners reject a JSON Schema that `z.toJSONSchema` would not emit (e.g.
  unsupported keywords)? Needs a per-runner capability check before B ships
  broadly; until then, document the safe JSON Schema subset.
