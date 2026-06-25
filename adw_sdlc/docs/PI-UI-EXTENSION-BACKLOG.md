# Pi UI extension backlog — ADW Cockpit

This backlog turns the Pi UI extension brainstorm into an implementation plan for
this repository. The goal is a project-local Pi extension that makes the ADW SDLC
pipeline easier to inspect and operate from Pi without weakening ADW's control
plane or secret boundary.

Extension location:

```text
.pi/extensions/adw-cockpit/index.ts
```

Pi will auto-discover project-local extensions from `.pi/extensions/` after the
project is trusted. During development the extension can also be loaded with
`pi -e .pi/extensions/adw-cockpit/index.ts`.

## Design principles

1. **Read-only by default.** The extension should inspect config, git state, and
   `agents/{adw_id}/state.json`; it should not start ADW runs or mutate git in
   the first phase.
2. **Widget-first, overlay-second.** Persistent widgets provide ambient status;
   overlays are reserved for drill-down flows.
3. **Do not weaken the ADW secret boundary.** The ADW orchestrator remains the
   only owner of git/forge/merge authority for ADW runs. The extension must not
   forward `GH_TOKEN` to runners or bypass `src/env.ts`.
4. **Respect Pi modes.** Use widget/status APIs only when `ctx.hasUI`; reserve
   `ctx.ui.custom()` overlays for `ctx.mode === "tui"`.
5. **Avoid background work in the extension factory.** Start any future watchers
   in `session_start` and clean them up in `session_shutdown`.
6. **Cache expensive checks.** Do not run `npm test`, `npm run build`, or other
   long-running commands automatically from widget rendering.

## Phase 1 — Passive cockpit widget (implemented)

### Scope

Create a read-only ADW Cockpit widget that appears in Pi when this repository is
opened. It summarizes the active project pack, providers, git state, and latest
ADW run state.

### User-visible behavior

The extension registers:

```text
/adw
/adw-refresh
```

- `/adw` toggles the cockpit widget on/off. It accepts optional aliases:
  `on`, `show`, `off`, `hide`, and `toggle`.
- `/adw-refresh` re-reads repository state and refreshes the widget.

The widget is styled as a dark **mission-control dashboard** (modelled on the
reference screen the maintainer supplied — see `.impeccable.md`). It is a custom
component that draws numbered, titled, box-drawn panels sized to the live editor
width:

```text
ADW COCKPIT                                  updated 10:24:31  ● live
┌─ 1. OVERVIEW ─────────── pack ✓ ┐  ┌─ 2. LATEST RUN ───────── PR ┐
│ runtime   : Pi                  │  │ run     : a1b2c3d4          │
│ project   : HealthTech          │  │ issue   : #42               │
│ providers : github/git/github   │  │ runner  : claude            │
│ prompts   : .adw/prompts         │  │ pr      : open              │
│ branch    : main                │  │ cost    : $0.123            │
│ worktree  : clean               │  │                            │
│ test gate : none                │  │                            │
│ finalize  : 0 gates             │  └─────────────────────────┘
└────────────────────────┘
┌─ 3. PIPELINE ────────────────────────────────────── 3/11 ┐
│ ▕████░░░░░░░░░░░░░▏ 3/11  ·  ◉ IMPLEMENT                          │
│ ● setup      ● classify   ● plan       ◉ implement              │
│ ○ tests      ○ resolve    ○ e2e        ○ review                 │
│ ○ patch      ○ document   ○ merge                              │
└──────────────────────────────────────────────┘
last  ✓ typecheck · code 0 · 1.2s · 10:42:00
```

Instead of a one-line `completed <n> (last: <phase>)` summary, the cockpit
renders the **full phase pipeline** for the latest run as a `●`/`◉`/`○`
status-dot **grid** (done / current / pending) of every phase in the
`setup → <agent chain> → merge` chain, under a green `▕██░▏ n/total` completion
meter and a current-phase callout. `OVERVIEW`/`LATEST RUN`/`PIPELINE` are
numbered, titled, box-drawn panels (two-up when wide, stacked when narrow);
`key : value` rows pair a dim key column with status-coloured values. Colour
comes only from theme tokens (`accent` headings, `success` done/healthy,
`warning` in-progress/missing, `error` alerts, `border` frames), so it honours
the user's terminal theme.

The agent chain is read from `.adw/config.json` `phases` (else the built-in
default catalog); any completed phase not in the derived chain is appended
defensively so progress is never hidden. With no run yet, the grid is shown as
an all-pending preview so the configured phases are visible immediately. A
fully-completed run shows `✦ ALL CLEAR` in place of a current callout.

The count denominator is the rendered chain length (`setup` + agent chain +
`merge`), not a fixed ratio: the `finalize`/`ci-fix`/`report` wrappers are
never recorded in `completed_phases`, and conditional gates (`e2e`/`document`)
are recorded as completed even when skipped — so a skipped gate reads as done,
matching exactly what the orchestrator persists.

It also exposes an optional mission-control status bar (`/adw-footer`) that
mirrors the dashboard footer in the reference — a health dot, dim UPPERCASE
labels against status-coloured values, faint ` │ ` dividers, a pipeline
mini-meter, and a right-justified run/cost cluster:

```text
● ADW │ PROJECT HealthTech │ BRANCH main │ TREE clean │ PIPELINE ▕██░▏ 3/11 implement │ LAST ✓ typecheck      RUN a1b2c3d4  $0.123
```

### Data sources

Read-only local sources:

```text
.adw/config.json
agents/*/state.json
git branch --show-current
git status --short
```

### Acceptance criteria

- [x] Project-local extension exists at `.pi/extensions/adw-cockpit/index.ts`.
- [x] Extension uses `ctx.ui.setWidget()` and `ctx.ui.setStatus()`.
- [x] Extension guards UI work with `ctx.hasUI`.
- [x] Extension is read-only: it reads files and runs read-only git commands
      only.
- [x] Extension gracefully handles missing `.adw/config.json`.
- [x] Extension gracefully handles no `agents/` directory and no run state.
- [x] Extension exposes `/adw` and `/adw-refresh` commands.
- [x] TypeScript syntax is locally validated with a temporary `tsc` config.

### Non-goals

- No full ADW run execution.
- No git mutations.
- No automatic tests/builds.
- No file watchers yet.
- No overlays yet.

## Phase 2 — Run inspector overlay (implemented)

### Scope

Add `/adw-runs` to browse recent ADW run workspaces in a TUI overlay.

### User flow

```text
/adw-runs
```

Displays a selectable list:

```text
Recent ADW Runs
> a1b2c3d4  issue #42  claude  setup→classify→plan
  b7e8f901  issue #37  codex   completed / PR #123
  c932aa10  issue #35  claude  failed or stopped at ci-fix
```

Selecting a run shows details:

```text
run: a1b2c3d4
issue: #42
branch: feat/42-title-a1b2c3d4
runner: claude
completed: setup, classify, plan
pr: none
workspace: agents/a1b2c3d4
```

### Candidate APIs

- `ctx.ui.custom()` for the overlay.
- `SelectList` from `@earendil-works/pi-tui` for run selection.
- `ctx.ui.setEditorText()` or `ctx.ui.pasteToEditor()` to insert a safe resume
  command rather than execute it.

### Acceptance criteria

- [x] `/adw-runs` opens only in TUI mode, or falls back to a notification in
      non-TUI UI modes.
- [x] Recent runs are sorted by `state.json` mtime (newest first).
- [x] Selecting a run shows phase completion and paths.
- [x] There is an action to insert, not execute, a resume command.
- [x] Overlay closes with Escape (`SelectList.onCancel` resolves the overlay).

### Implementation notes

- The picker and detail views use `ctx.ui.custom()` overlays built from
  `SelectList` + `DynamicBorder` + `Text` (`getSelectListTheme()` for styling).
- The resume command is `cd adw_sdlc && npm run issue -- <issue> --resume
  --adw-id <id>`; it is only inserted via `ctx.ui.setEditorText()`, never run.
  A run with no recorded `issue_number` cannot form a valid resume command
  (the CLI requires the positional work-item id and a resumed run rejects a
  mismatched number), so the insert action is reported as unavailable for it.
- Non-TUI modes (`rpc`/`json`/`print`) get a one-line summary notification
  instead of an overlay.

## Phase 3 — Safe command helpers (implemented)

### Scope

Add explicit commands for safe, user-invoked checks and dry-runs.

### Commands

```text
/adw-dry-run <work-item-id>
/adw-check typecheck
/adw-check lint-env
/adw-check pack-check
/adw-check test
/adw-check build
/adw-check all
```

### Behavior

- `/adw-dry-run <id>` runs:

  ```bash
  cd adw_sdlc && npx tsx src/cli.ts <id> --dry-run
  ```

- `/adw-check ...` runs only the requested command.
- Results are cached and summarized in the cockpit widget.
- Long-running operations use a cancellable loader.

### Candidate APIs

- `pi.exec()` for commands.
- `ctx.ui.custom()` with `BorderedLoader` for cancelable progress.
- `ctx.ui.notify()` for completion/failure.

### Acceptance criteria

- [x] No check runs automatically on startup or render.
- [x] Results include command, exit code, timestamp, duration, and truncated output.
- [x] Failures are shown clearly without crashing the extension.
- [x] Widget displays the last command/check result.

### Implementation notes

- `/adw-dry-run <id>` runs `npx tsx src/cli.ts <id> --dry-run` from
  `adw_sdlc/` via `pi.exec()`; the ADW CLI dry-run path previews the plan and
  does not invoke the selected runner.
- `/adw-check <name>` supports `typecheck`, `lint-env` (also `lint:env`),
  `test`, `build`, and `all`. `all` runs the checks in that order and stops at
  the first failure.
- Commands run only when explicitly invoked by the user. They are never launched
  from widget rendering, `session_start`, or `agent_end`.
- In TUI mode, command execution is wrapped in a cancellable `BorderedLoader`;
  non-TUI modes run directly and report via notifications.
- The cockpit widget caches and displays the most recent command/check result.
  Full command details are truncated before notification output.

## Phase 4 — Project-pack inspector (implemented)

### Scope

Add `/adw-config` to inspect `.adw/config.json` in a structured overlay.

### User flow

```text
/adw-config
```

Opens a live **master/detail** overlay: a section list whose highlighted entry
re-renders its values beneath it (via `SelectList.onSelectionChange`), styled
in the mission-control `key : value` language (dim keys, status-coloured
values; nested objects indent, scalar arrays inline):

```text
ADW CONFIG · .adw/config.json
> 1. Project       HealthTech (id healthtech)
  2. Providers     github/git/github
  3. Commands ⚠    test gate none ⚠ · finalize 0 ⚠
  4. Phases        9 (default catalog)
  5. Models        classify claude-haiku-4-5 · tier mid
  6. Gates         e2e, documentation
  7. Branching     prefix feat
  8. Prompts       .adw/prompts
  9. Schemas       built-in only
────────────────────────────────────────────
COMMANDS
  defaultTestCommand   : (none) — resolve loop has no test gate
  defaultFinalizeGates : (empty) — no finalize gates configured
  docs: adw_sdlc/docs/ARCHITECTURE.md
────────────────────────────────────────────
↑↓ navigate · esc close
```

### Panels

```text
project  providers  commands  phases/customPhases
models   gates      branching prompts   schemas
```

### Acceptance criteria

- [x] Config sections are navigable.
- [x] Missing or malformed config is rendered as a readable error.
- [x] Empty default test/finalize gates are called out as warnings.
- [x] The overlay links the user to relevant docs such as
      `adw_sdlc/docs/ARCHITECTURE.md` and `adw_sdlc/docs/UNIVERSAL.md`.

### Implementation notes

- `parseJsonObject(.adw/config.json)` returns `null` for a missing *or*
  malformed file; `buildConfigSections` collapses that to a single readable
  `Status` error section, so the overlay never throws on bad config.
- Each section carries a plain-text summary (shown in the list), a `warn` flag
  (e.g. empty test/finalize gates, surfaced as a `⚠` in the list and an amber
  line in the detail), relevant doc paths, and a themed detail renderer.
- `renderJsonLines` pretty-prints any config subtree (objects nest with accent
  keys, scalar arrays inline, empty values flagged); detail is capped with a
  `… N more — see .adw/config.json` note so deep sections (models/gates) stay
  bounded.
- Non-TUI modes (`rpc`/`json`/`print`) get a one-line section/warning summary
  notification instead of the overlay.
- The phase section reflects `config.phases` when present, else the built-in
  default catalog, and lists `customPhases`.

## Phase 5 — Workflow assistant

### Scope

Add higher-level workflow affordances after the read-only and safe-command
surfaces are proven.

### Candidate features

- Issue autocomplete for `#123` references using `gh issue list`, modeled after
  Pi's `github-issue-autocomplete.ts` example.
- `/adw-menu` command palette with common actions.
- Guarded `/adw-run <work-item-id>` that requires an explicit confirmation and
  clearly states that real git/forge operations will happen.
- MVP readiness panel that summarizes `PARITY.md`, `MVP-READINESS.md`, and
  `HANDOVER.md`.

### Acceptance criteria

- [ ] Potentially mutating operations require confirmation.
- [ ] Real ADW runs are never started from ambient widget refresh.
- [ ] The extension explains when a command will use git, gh, or the network.
- [ ] The extension remains optional and project-local.

## Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Extension grows into a second orchestrator | Keep ADW execution in `adw_sdlc/src/orchestrator.ts`; the extension observes and launches only explicit commands. |
| Accidental expensive commands | Never run tests/builds from render/session hooks; run only from explicit commands. |
| Secret-boundary drift | Do not pass model/forge secrets into runner processes from the extension. Keep `lint:env` as the source of truth. |
| Stale widget data | Provide `/adw-refresh`; optionally add watchers in a later phase with session-scoped cleanup. |
| Non-TUI modes | Guard with `ctx.hasUI` and `ctx.mode === "tui"` before custom TUI overlays. |
| Malformed state/config files | Treat parse failures as displayable status, not extension crashes. |

## UI design pass (implemented)

A cross-phase polish pass takes fuller advantage of Pi's TUI widget APIs and
applies frontend-design guidance (clear hierarchy, status-coloured feedback,
progressive disclosure, terminal-width safety, no visual over-claiming).

### What changed

The design pass evolved in three iterations — (1) a component-factory widget
(`Container` of themed `Text` rows framed by `DynamicBorder`, replacing the
plain `string[]`), (2) a full phase-pipeline tracker (per-phase glyphs + meter +
`n/total`, replacing the one-line `completed <n>` summary), and (3) a bold
instrument-panel treatment — all **superseded by the current mission-control
dashboard** below, which is the shipped state. The phase-tracker semantics
(chain from `.adw/config.json` `phases` else the default catalog; all-pending
preview when there is no run) carried forward unchanged.

- **Mission-control dashboard pass.** The cockpit now adopts a dark
  mission-control dashboard look (modelled on a maintainer-supplied reference;
  recorded in `.impeccable.md`). `dashboardWidget` returns a **custom
  component** whose `render(width)` draws numbered, titled, box-drawn panels
  (`1. OVERVIEW`, `2. LATEST RUN`, `3. PIPELINE`) sized to the live editor
  width — two-up when wide, stacked when narrow. It uses `key : value` rows
  (dim key column, status-coloured values), a `●`/`◉`/`○` phase status-dot
  grid, green `▕██░▏` completion meters, a `updated HH:MM:SS  ● live` heading,
  and right-aligned title tags (`PR` badge, `n/total`). Box drawing is
  ANSI-aware via `truncateToWidth(…, pad=true)` + `visibleWidth`; colour is
  theme-token only (`accent`/`success`/`warning`/`error`/`border`).
- **Below-editor command hint.** A second widget (`placement: "belowEditor"`)
  lists the primary commands so the surface is discoverable without docs.
- **Mission-control status bar.** `/adw-footer [on|off|toggle]` installs a
  `ctx.ui.setFooter()` bar styled to match the dashboard/reference: a health
  dot (green clean / amber dirty), dim UPPERCASE labels against status-coloured
  values, faint ` │ ` dividers, a `PIPELINE ▕██░▏ n/total <current>` mini-meter,
  and a right-justified `RUN <id>  $cost` cluster (the reference's budget slot).
  A `FooterSnapshot` is built once per refresh event so the per-frame render is
  pure formatting (no git/disk per paint); it truncates gracefully and restores
  the built-in footer when disabled. Off by default so it never silently
  replaces the user's footer — enable with `/adw-footer on`.
- **Prompt-pack sync.** Tracks the repo's move to template-generated project
  prompt packs: OVERVIEW shows a `prompts : <defaultRoot>` row (now
  `.adw/prompts`) and a `pack ✓`/`pack ·` title tag reflecting whether
  `.adw/pack.profile.json` exists, and `/adw-check` gains the read-only
  `pack-check` (`npm run pack:check`, included in `all`). The phase catalog the
  widget mirrors (`DEFAULT_AGENT_PHASES`) was reconfirmed against
  `adw_sdlc/src/phases.ts`, and the emitted CLI flags (`--dry-run`, `--resume`,
  `--adw-id`) against `adw_sdlc/src/cli.ts`.
- **Consistent status language.** A `✓`/`✗` convention is shared across the
  widget, footer, and notifications.

### Design-skill note

The `impeccable` frontend-design skill's Context Gathering Protocol requires
project-provided design context (audience, brand, tone). That context now lives
in **`.impeccable.md`** at the repo root: the cockpit is a Pi TUI surface for
developers operating the ADW pipeline, with a "mission-control dashboard"
aesthetic direction. The bolder/dashboard passes were executed against that
context and the supplied reference screen, applying the transferable principles
(hierarchy, dominant colour with rare accents, intentional structure,
width-safe rendering) within terminal constraints — colours stay on theme tokens
so the panel adapts to the user's terminal theme rather than hardcoding a
palette.

### Acceptance criteria

- [x] Cockpit uses a component-factory widget (not only `string[]`).
- [x] A below-editor hint widget exists via `placement: "belowEditor"`.
- [x] `/adw-footer` toggles a custom footer and restores the default.
- [x] Widget lines stay within terminal width (`truncateToWidth` in the footer;
      `Text` wraps widget rows).
- [x] Behaviour stays read-only and safe; no new automatic execution.
- [x] Extension typechecks against installed Pi types; repo gates stay green.

## Current status

Phases **1–4 are implemented**:

- **Phase 1** — passive cockpit, now a **mission-control dashboard** custom
  component: numbered box panels (`1. OVERVIEW`/`2. LATEST RUN`/`3. PIPELINE`,
  two-up when wide), `key : value` rows, a `●`/`◉`/`○` phase status-dot grid +
  green completion meter, and a `updated … ● live` heading. `/adw`, `/adw-refresh`.
- **Phase 2** — `/adw-runs` run inspector overlay (insert, never run, a resume
  command).
- **Phase 3** — `/adw-dry-run` and `/adw-check typecheck|lint-env|pack-check|
  test|build|all` safe helpers.
- **Phase 4** — `/adw-config` live master/detail config inspector.
- **Cross-phase** — a below-editor command hint, an optional mission-control
  `/adw-footer` status bar (off by default), a `✓`/`✗` status language, and a
  prompt-pack sync (`prompts` row, `pack ✓` tag, `pack-check`).

The aesthetic direction is recorded in `.impeccable.md`; colour is theme-token
only so the cockpit honours the user's terminal theme. **Phase 5** (workflow
assistant — `/adw-menu`, guarded `/adw-run`, issue autocomplete, MVP-readiness
panel) remains the only backlog item.
