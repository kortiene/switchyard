# adw_sdlc — HealthTech standalone port

This package is a port of the **ADW (Agentic Developer Workflow) SDLC** control plane from the
`mx-agent` monorepo into the HealthTech project. It drives a GitHub issue through a phased,
multi-agent software-delivery pipeline:

```
setup → classify → plan → implement → tests → resolve(loop) → e2e(gated)
      → review → patch(loop) → document(gated) → finalize → ci-fix(loop) → merge → report
```

The orchestrator owns **all** git/gh and withholds secrets from the agent (deny-by-default env
allowlist); each phase runs on one of four interchangeable runner backends
(`claude` | `codex` | `opencode` | `pi`) behind a single `AgentRunner.runPhase()` seam. See
[`PLAN.md`](./PLAN.md) for the full architecture and [`PARITY.md`](./PARITY.md) for the parity
checklist.

## What changed for the standalone HealthTech port

The original was a pnpm-workspace member with a sibling Python `adw/` engine. This port is
**TypeScript-only and self-contained**. Changes from upstream:

| Area | Upstream (mx-agent) | HealthTech port |
| --- | --- | --- |
| Default engine | `--engine py` (delegates to `python3 adw/issue.py`) | **`--engine ts`** (the Python sibling is not bundled; `py` stays a recognized id but is **not available here** — selecting it raises an explicit "not available in this standalone distribution" `AdwError` at dispatch, with no spawn and no `python3` dependency) |
| Test gate (`DEFAULT_TEST_CMD`) | `cargo test --all` | **empty** — skipped until configured (set `ADW_TEST_CMD`) |
| Pre-merge gates (`DEFAULT_FINALIZE_GATES`) | hardcoded `cargo fmt/clippy/build` | **empty/configurable** via `ADW_FINALIZE_GATES` (newline-separated); empty repo can merge |
| Branch prefixes (`TYPE_PREFIX`) | `type:bug`/`type:docs`/… | also maps HealthTech's plain labels (`bug`, `docs`, `tech-debt`, `infra`, …), case-insensitive |
| Branch slugs | ASCII only | **de-accented** (French issue titles slug cleanly) |
| Phase preamble | "mx-agent ADW pipeline… Python performs all git/gh" | engine-neutral ("the ADW pipeline… the orchestrator performs all git/gh") |
| Conditional-gate hints | mx-agent vocab (ipc, daemon, matrix…) | + HealthTech domain (crypto, encryption, auth, consent, qr, offline…) |
| Prompt templates | Rust/Cargo + Matrix/daemon context in shared command roots | HealthTech prompts live in the project pack at `.adw/prompts` (local-first / zero-knowledge, AES-256-GCM, ARTCI, ≤500 KB, PRD/BACKLOG context); `.claude/commands` and `.pi/prompts` are neutral byte-identical fallback command templates |
| Universalization config | no standalone project-pack seam | `.adw/config.json` now expresses the current project pack: prompt roots, branch label mapping, e2e/doc gate hints, model tiers, progress tag, and default gate commands; missing config falls back to the same defaults |
| Provider seam | GitHub/git effects wired directly into orchestration deps | `src/providers.ts` defines provider interfaces for CLI resolution, work items, VCS, and change requests/CI, plus behavior-preserving Git/GitHub adapters; `run()` now uses providers directly, with legacy `OrchestratorDeps` seams kept only as an adapter for tests/incremental migration |
| Provider selection | implicit GitHub/git defaults in code | `.adw/config.json` explicitly selects the built-in providers (`github` for CLI/work-items/change-requests, `git` for VCS); `createProvidersFromConfig()` is the future plugin switchpoint |

The **cross-language state contract** is preserved at `../adw/state.schema.json` (+ fixtures under
`../adw/fixtures/cross_language/`) — JSON-only, no Python code. Provider-neutral `work_item` and
`change_request` metadata are TS-additive, non-load-bearing fields; the v1 compatibility fields
`issue_number`, `pr_number`, and `pr_url` remain canonical for resume and Python interoperability.

## Status

- `npm install && npm run typecheck` → clean.
- `npm test` → **466 tests pass** (35 files).
- `npm run lint:env` → secret-withholding lint gate passes.

## Usage

```bash
cd adw_sdlc
npm install

# Preview the plan for issue #N (no runner SDK needed):
npx tsx src/cli.ts <N> --dry-run

# Run the full pipeline on issue #N with the claude runner:
ADW_TEST_CMD="<your test command>" \
  npx tsx src/cli.ts <N> --runner claude --yes
```

Requires `gh` authenticated for the `kortiene/HealthTech` repo. Optionally set `PROJECT_NUMBER=2`
so the setup phase can move the issue's card on the GitHub Project board.

Project-specific policy is loaded from `.adw/config.json` when present. Runtime env knobs use the canonical
`ADW_*` namespace; inherited `MX_AGENT_*` names are still accepted as deprecated compatibility aliases.
The committed HealthTech config is behavior-preserving and currently covers prompt roots (`.adw/prompts`), provider selection, branch prefixes,
conditional-gate hints, model-tier routing, progress-comment tag, and default test/finalize gates. Git/GitHub effects now have a
first provider boundary in `src/providers.ts`; `run()` uses providers directly while the legacy `OrchestratorDeps`
shape is preserved as an adapter for test parity and incremental migration. Run state now records provider-neutral
`work_item`/`change_request` metadata additively alongside the compatibility `issue_*`/`pr_*` fields. CLI/help and
runtime progress wording now describe the universal work-item workflow while keeping the `issue` command name as a
backward-compatible GitHub alias. Public context types now prefer `WorkItemContext`, with `IssueContext` retained as
a compatibility alias and `fetchWorkItem()` as the neutral wrapper over the current GitHub implementation. Change-request
providers now return provider-neutral `CreateChangeRequestResult` / `ChangeRequest` metadata including an `id`, while
the legacy `createPr` adapter still exposes the old `{number,url,error}` shape. CI/check terminology is now provider-neutral
at the boundary: `ChangeRequestProvider.pipelineStatus(...)` returns `PipelineStatus` with `PipelineJob`s (`ciStatus`/`CiStatus`/
`FailingJob` remain as compatibility aliases). VCS/change-request mutating actions return the provider-neutral `OperationResult`
(GitOperationResult alias). Work-item helpers are now reachable via a neutral `work-item.ts` module, with `deriveWorkItemBranch`,
`slugifyWorkItemTitle`, and `workItemBranchPrefix` as neutral aliases. The CLI parser exposes a neutral `workItem` field alongside
`issue`, and `CliDeps` supports an optional `runWorkItem` hook preferred over `runIssue` when set. The terminal/closed work-item
states are now configurable via `providers.workItems.closedStates` in `.adw/config.json` (default `['CLOSED']`), so non-GitHub
providers can declare their own terminal values without orchestrator changes. The setup-phase status applied to a fresh
work item is configurable via `providers.workItems.inProgressStatus` (default `'In Progress'`), and the GitHub Projects
status field name is configurable via `providers.workItems.statusFieldName` (default `'Status'`). Universal-architecture
notes live in `docs/UNIVERSAL.md`, and `docs/examples/payments-api.config.json` shows a non-HealthTech project pack. The
secret boundary remains hardcoded in `src/env.ts` and guarded by `npm run lint:env`; do not make runner env inheritance
project-configurable.

HealthTech prompts are now generated from neutral source templates plus
`.adw/pack.profile.json`: run `npm run pack:generate` to refresh
`.adw/prompts`, and `npm run pack:check` as the drift guard. The optional
`--llm` metaprompt pass is build-time only; runtime uses committed prompt files.

### Auth — API key *or* Anthropic subscription

The `claude` runner works with either:

- **Pay-as-you-go API key:** `export ANTHROPIC_API_KEY=sk-ant-…`. The cheap `classify` phase runs
  in-process on the Anthropic SDK (haiku).
- **Claude Pro/Max subscription:** run `claude login` once (credentials in `~/.claude`), or
  `export CLAUDE_CODE_OAUTH_TOKEN=…`. **No API key needed.** When `ANTHROPIC_API_KEY` is unset, the
  pipeline auto-routes `classify` through the runner (the Claude Code executable honors the
  subscription) instead of the API SDK — no flag required. `ADW_CLASSIFY_ON_RUNNER=1` forces
  this routing even when a key is present.

The subscription token / on-disk login reach the runner child through the env allowlist
(`CLAUDE_CODE_OAUTH_TOKEN` + `HOME`); secrets like `GH_TOKEN` are still withheld.

## Test gate (live — stack chosen, monorepo scaffolded)

Backlog #1 (stack, see `docs/adr/`) and #2 (scaffold) are done. The pipeline test gate is:

- **`ADW_TEST_CMD="just test"`** — a root `justfile` target aggregating `cargo test --workspace`
  + the web `vitest` + the Flutter `flutter test`. Run from the repo root, e.g. `just issue <N> …`.
- **`ADW_FINALIZE_GATES`** (newline-separated) for extra pre-merge gates, e.g.
  `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo deny check`.
