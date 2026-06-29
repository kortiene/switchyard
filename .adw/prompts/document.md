---
description: Update project documentation after a reviewed implementation (phased ADW)
argument-hint: "<change-summary-and-files>"
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

Run the most relevant focused test(s) for your change first — e.g. `npx vitest run test/<file>.test.ts` from `adw_sdlc/`. Reserve the full `npm run verify` gate (typecheck, env lint, prompt-pack drift, tests, build) for broad or risky changes and for the final review; the ADW orchestrator runs the canonical gate at finalize, so re-running the whole suite in every phase wastes time and budget. If a check cannot be run, state exactly why and what command the maintainer should run.

Update the repository's documentation to reflect the implemented, reviewed change.

Change summary, files changed, and context:

$ARGUMENTS

## Scope and boundary

This is the standalone documentation pass, distinct from inline documentation already made during implementation.

- The implementation phase should have handled code-local comments, public API docstrings, and usage text tightly coupled to the changed code.
- Here, update broader prose that benefits from seeing the finished change: README, docs, architecture notes, developer guides, changelog, migration notes, API docs, or runbooks when relevant.
- Do not create noisy documentation for internal-only changes that do not affect users, operators, or maintainers.
- Do not invent product behavior, deployment assumptions, or compatibility promises that are not supported by the code.

## Workflow

1. Inspect the changed files and existing docs.
2. Identify docs that are stale, missing, or misleading because of this change.
3. Update only the documentation that should ship with the change.
4. Keep language precise, maintainable, and project-neutral.
5. Preserve documented domain, privacy, security, and operational constraints found in the repo.

## Output expectations

- Make the documentation edits directly.
- Summarize what changed and why.
- If no documentation update is warranted, say so clearly and explain the reason.
