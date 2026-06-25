---
description: Implement multiple work items sequentially using /issue semantics
argument-hint: "<work-item-id-or-range> [work-item-id-or-range ...] [-- notes]"
---
## Switchyard ADW project context

<!-- Generated project context. Edit .adw/pack.profile.json and run `npm run pack:generate`; do not hand-edit this block in .adw/prompts. -->

### Repository context to read

- `README.md` at the repository root, if present, for top-level project context.
- `adw_sdlc/README.md` — package overview, commands, config surface, and documentation map.
- `adw_sdlc/HANDOVER.md` — current architecture, operational invariants, and session-to-session state.
- `adw_sdlc/MVP-READINESS.md` and `adw_sdlc/PARITY.md` — MVP gates, live-run evidence, and parity criteria.
- `adw_sdlc/docs/LIVE-RUN-BATCH.md` — planned live `claude` issues and run-command templates.
- The specific GitHub issue being worked (`kortiene/switchyard`) and its acceptance criteria.
- The existing source tree and tests as they actually exist now; verify before assuming behavior.

### Project summary

`Switchyard` contains the ADW SDLC TypeScript control plane and related project-pack / Pi cockpit tooling. The control plane drives one work item through deterministic phases (`setup → classify → plan → implement → tests → resolve → e2e → review → patch → document → finalize → ci-fix → merge → report`) while runner adapters (`claude`, `codex`, `opencode`, `pi`) perform only the agentic code-editing phases. The near-term MVP goal is audited live reliability for the `claude` runner.

### Component map

- `adw_sdlc/src/orchestrator.ts`, `run-phase.ts`, `phases.ts`, `state.ts` — deterministic phase control, prompt composition, structured-output handling, and persisted run state.
- `adw_sdlc/src/runners/` and `invoker.ts` — runner adapters and the shared `AgentRunner.runPhase()` seam.
- `adw_sdlc/src/env.ts`, `env-vars.ts`, `exec.ts` — secret-boundary env allowlist, canonical `ADW_*` control-plane env aliases, and subprocess helpers.
- `adw_sdlc/src/providers*.ts`, `provider-descriptor.ts` — GitHub/git built-ins plus declarative `cli`/`rest` provider support.
- `adw_sdlc/tools/` — prompt-pack generation and parity-rate measurement.
- `.adw/config.json`, `.adw/pack.profile.json`, `.adw/prompts/` — project pack and generated runtime prompt templates.
- `.pi/extensions/adw-cockpit/` and `adw_sdlc/docs/PI-UI-EXTENSION-BACKLOG.md` — optional Pi TUI cockpit surface.

### Security and operational invariants

- The orchestrator owns all git/gh operations; ADW phases must not ask the coding agent to run git or gh.
- Runner child environments are deny-by-default: `GH_TOKEN`, `MATRIX_*`, canonical `ADW_*`, and legacy `MX_AGENT_*` control variables must not reach agents.
- Do not spread `process.env` into runner processes; `npm run lint:env` guards this boundary.
- Opencode integration must use the approved v2 client path and avoid factory calls that inherit parent env.
- Treat `PHASE_PREAMBLE_SHARED` and prompt contract wording as behavior-affecting; change only with explicit care and tests.
- State-schema fields used for cross-language compatibility must remain additive/non-breaking.

### Current status and assumptions

The package is TypeScript/Node and uses npm. Canonical runtime env knobs use `ADW_*` names, with deprecated `MX_AGENT_*` compatibility aliases. The canonical local gate is `npm run verify`, which runs typecheck, env lint, prompt-pack drift check, tests, build, and removes `dist/`. A live-run issue batch (#1–#8) exists to collect MVP evidence, but do not start costly live `claude` runs unless explicitly asked.

### Working rules

- Keep changes scoped to the GitHub issue and acceptance criteria.
- Prefer small, auditable changes with focused tests over broad rewrites.
- Update docs and prompt-pack sources when behavior or operator guidance changes.
- If `.adw/pack.profile.json` changes, regenerate `.adw/prompts` with `npm run pack:generate` and verify with `npm run pack:check`.
- Use `ADW_TEST_CMD="npm run verify"` for live ADW runs; do not chain multiple commands directly in `ADW_TEST_CMD`.

### Verification guidance

Run `npm run verify` from `adw_sdlc/` for the full local gate. For narrow changes, run the most relevant focused test first, then the full gate before reporting. If a check cannot be run, state exactly why and what command should be run by the maintainer.

Implement multiple work items sequentially for this repository, end to end, using `/issue` semantics for each item.

Work item selectors and shared notes:

$ARGUMENTS

`/issues` is a batch orchestrator. Process items one at a time, in normalized order, and return to a clean, updated base branch between items. Do not parallelize. Do not combine multiple work items into one branch or change request unless explicitly asked and the items genuinely require a shared implementation.

Workflow:

1. Parse and normalize selectors.
   - Accept single numeric IDs, e.g. `12`.
   - Accept inclusive hyphen ranges, e.g. `12-14` expands to `12, 13, 14`.
   - Accept inclusive dot ranges, e.g. `12..14` expands to `12, 13, 14`.
   - Treat content after `--` as shared notes for every item.

2. For each item, run the equivalent of `/issue` end to end.

3. Preserve isolation.
   - One work item → one branch/change request unless explicitly directed otherwise.
   - Do not carry uncommitted changes from one item into the next.
   - Stop on blockers instead of cascading failures into later items.

4. Produce a final batch report with completed items, skipped/blocked items, links or identifiers if available, and follow-up risks.
