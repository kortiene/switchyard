# Design — loop/gated custom phases (control flow for custom phases)

**Status:** design + implementation in the same slice (the shape is fully
determined by the existing gate/loop machinery, so a separate proposal-only pass
adds no value). This document records the settled shape and the rationale.

**Author context:** the deferred remainder of capability B in
`DESIGN-schema-overrides.md`. Capability B (`HANDOVER.md` §8g) shipped **plain,
sequential** custom phases and explicitly listed loop/gated custom phases as a
non-goal (§2 there), because the `resolve`/`patch` loops and the `e2e`/`document`
gates are kernel control flow keyed to phase name. This slice lets a *custom*
phase opt into the two control-flow shapes that generalize cleanly.

---

## 1. Goal

Let a project-registered custom phase (`config.customPhases`) opt into either:

- **A conditional gate** — run only when the change signal/files match a
  configured predicate, exactly like the built-in `e2e`/`document` gates.
- **A resolve-style loop** — run a project-supplied verification command;
  if it fails, invoke the phase's agent to fix it and retry up to a bound,
  exactly like the built-in `resolve` loop.

Both behaviors compose: a phase may be gated *and* looped (gate decides whether
it runs at all; the loop is what running it means).

## 2. Non-goals

- **Not** a `patch`-style findings loop. `patchLoop` consumes `review`'s
  `findings[]` and is meaningful only paired with `review`; there is no
  project-supplied analogue. Only the `resolve` shape (command → fix → retry)
  generalizes.
- **Not** new control flow for **built-in** phases. The built-in gates/loops stay
  kernel-owned and are configured through their existing knobs (`gates.e2e`,
  `gates.documentation`, `--test-cmd`/`maxResolve`). A control-flow entry keyed to
  a built-in name is rejected loudly.
- **Not** changing built-in `resolve`/`e2e`/`document` behavior. The built-in
  paths are byte-for-byte unchanged (the loop generalization is additive; see §6).
- **Not** Python parity surface. Like all of capability B, custom phases have no
  Python counterpart, so there is no parity debt (`DESIGN-schema-overrides.md` §4).

## 3. The machinery this reuses

- **Gate matching** — `gateDocument` (`phases.ts`) already matches a change
  signal against `hints` (whole-word) **or** changed files against
  `exactFiles`/`pathPrefixes`/`fileExtensions`. A custom gate is the same
  predicate with project-supplied lists; `gateCustom` factors that matching out.
- **The resolve loop** — `resolveLoop` (`orchestrator.ts`) runs a shell command;
  green → done without invoking the agent; red → invoke the `resolve` agent with
  the command output, retry to `maxAttempts`, and stop early if the agent reports
  `resolved: 0`. Generalizing it to a `phase` parameter (default `'resolve'`)
  makes it drive any phase's agent against any command. The orchestrator already
  runs project-supplied commands (the `--test-cmd` test gate), so a custom loop
  command is the same trust level — run by the orchestrator, never the agent.

## 4. Config surface

Two additive, optional maps keyed by **custom** phase name:

```jsonc
"gates": {
  "e2e": { "hints": [ ... ] },
  "documentation": { ... },
  "custom": {
    "audit": {                          // run the "audit" phase only when…
      "hints": ["auth", "payment"],     // …the change signal mentions these, OR
      "pathPrefixes": ["src/billing/"], // …a changed file matches these rules
      "exactFiles": [], "fileExtensions": []
    }
  }
},
"loops": {
  "verify": {                           // the "verify" phase is a resolve-style loop
    "command": "npm run verify",        // orchestrator runs this; non-zero → fix attempt
    "maxAttempts": 3                     // optional, default 3 (matches resolve)
  }
}
```

Resolution and defaults:

- Absent maps → today's behavior exactly (behavior-preserving; the default config
  carries empty `gates.custom` and `loops`).
- A custom gate predicate with **no** matchers is a misconfiguration (the phase
  could never run) → rejected at startup.
- A custom loop phase's result schema **must declare `resolved`** (integer), since
  the loop reads `outcome.data.resolved` to detect no-progress → rejected at
  startup if absent.

## 5. Semantics

For a chain phase `p`:

1. **Gate** (if `p` is built-in-conditional **or** `gates.custom[p]` exists):
   evaluate the gate; if it says skip, `markDone(p)` and continue (recorded as
   skipped with a reason, like `e2e`/`document`).
2. **Body**:
   - built-in `resolve` → the built-in resolve loop (unchanged);
   - built-in `patch` → the built-in patch loop (unchanged);
   - `loops[p]` exists → the **generalized** resolve loop with `command`/
     `maxAttempts`/`phase = p` (invokes `p`'s agent, reads `p`'s `resolved`);
   - else → the plain sequential agent phase (capability B).

A gated **and** looped custom phase runs the gate first, then the loop — falling
out of the gate skips the loop entirely.

## 6. Kernel changes (localized, built-ins unchanged)

- `config.ts` — add `gates.custom` (record → predicate) and top-level `loops`
  (record → `{command, maxAttempts=3}`), both defaulting to `{}`. Default config
  carries the empty maps.
- `phases.ts` —
  - `gateCustom(rule, signal, changedFiles)` factored from `gateDocument`'s
    matching; `gateConditional` dispatches `e2e`/`document`/custom and still
    throws for a non-conditional name.
  - `isConditionalPhase(phase, config)` = built-in-conditional ∨ has a custom gate.
  - `validatePhaseChain` extended: reject a control-flow entry on a built-in name
    or unregistered phase; reject an empty custom-gate predicate; require
    `resolved` in a custom loop phase's schema.
- `orchestrator.ts` —
  - the gate check uses `isConditionalPhase` and passes `config` to
    `gateConditional`;
  - `resolveLoop`'s config gains an optional `phase` (default `'resolve'`), used
    in its progress tags and to pick which agent to invoke; the built-in call
    passes no `phase`, so resolve is identical;
  - the loop dispatch runs the generalized loop for `loops[phase]` custom phases.

The built-in `resolve` path passes no `phase` and the built-in gates keep their
own knobs, so every built-in path is unchanged — the existing `resolveLoop`,
gate, and orchestrator tests pass untouched (the proof it is behavior-preserving).

## 7. Risks & mitigations

| Risk | Mitigation |
| --- | --- |
| Custom loop schema lacks `resolved` → loop can't detect progress | required-field check in `validatePhaseChain`, fail-closed at startup |
| Control-flow entry on a built-in name silently shadows kernel flow | rejected at startup (control flow targets custom phases only) |
| Empty gate predicate → phase silently never runs | rejected at startup |
| Loop command is untrusted code | same trust as the existing `--test-cmd` gate: run by the orchestrator with its own env, never by the agent; the agent still gets no secrets |
| Built-in resolve/gate regression | built-in paths pass no new params and are byte-for-byte unchanged; covered by the existing suite |

## 8. Test plan

- `gateCustom`: hint match, file-rule match, no match; parity with `gateDocument`
  on shared inputs.
- `isConditionalPhase`: built-in conditional, custom-gated, plain.
- `resolveLoop` generalization: a custom `phase` drives that phase's agent and
  reads its `resolved`; the built-in `'resolve'` default is unchanged.
- `validatePhaseChain`: reject built-in-name control-flow entry, unregistered
  key, empty gate predicate, and a loop phase whose schema omits `resolved`.
- Orchestrator integration: a gated custom phase skips when the signal misses and
  runs when it hits; a looped custom phase loops on a red command then goes green.
- Behavior-preserving: with no `gates.custom`/`loops`, every existing test passes.
