# Switchyard

Switchyard is the home of **`adw_sdlc`** — a TypeScript/Node control plane for a
phased **ADW (Agentic Developer Workflow) SDLC pipeline**. It drives one work
item (e.g. a GitHub issue) through a deterministic chain of single-purpose agent
phases, where each phase is one invocation of an interchangeable coding-agent
runner.

```
setup → classify → plan → implement → tests → resolve(loop) → e2e(gated)
      → review → patch(loop) → document(gated) → finalize → ci-fix(loop)
      → merge(optional) → report
```

The **orchestrator owns all git and GitHub work** and withholds secrets from the
coding agent through a deny-by-default environment allowlist: in phased mode the
agent never receives `GH_TOKEN`. The agent only edits the worktree and authors
the commit message and PR body. Four runner backends sit behind a single
`AgentRunner.runPhase()` seam — `claude` · `codex` · `opencode` · `pi` —
selected with `--runner` / `ADW_RUNNER`.

> The full architecture, CLI, configuration, and development workflow live in
> **[`adw_sdlc/README.md`](./adw_sdlc/README.md)**. This file is a
> repository-level map; that file is the source of truth for the package.

## Repository layout

| Path | What it is |
| --- | --- |
| [`adw_sdlc/`](./adw_sdlc/) | **The main package** (`adw-sdlc`): the orchestration kernel, runner seam, secret boundary, run state, and CLI. Start here. |
| [`.adw/`](./.adw/) | This repo's **project pack** — `config.json`, `pack.profile.json`, and the generated runtime `prompts/`. Configures the kernel for the Switchyard project. |
| [`.pi/`](./.pi/) | Pi integration: `extensions/adw-cockpit/` (the read-only TUI cockpit) and `prompts/` (the canonical, neutral command templates). |
| [`.claude/`](./.claude/) | `commands/` — a byte-for-byte mirror of `.pi/prompts` (drift-guarded by `npm run mirror:check`). |
| [`adw/`](./adw/) | Cross-language contract: `state.schema.json` (the on-disk `agents/{adw_id}/state.json` contract) and cross-language fixtures. |
| `agents/` | ADW runtime workspaces — per-run `state.json`, prompts, transcripts, and `metrics.json`. **Git-ignored** generated output; created by live runs and not present in a fresh clone. |
| [`scripts/`](./scripts/) | Repo scripts, including `check-adw-sdlc-env.sh` (the secret-boundary lint that backs `npm run lint:env`). |
| [`specs/`](./specs/) | Per-issue implementation specs that drove the work in this repo. |
| [`.github/workflows/`](./.github/workflows/) | CI — `verify.yml` runs the quality gate on a Node-version matrix. |
| `.impeccable.md` | Design context for the ADW Cockpit TUI (the project's only "frontend"). |

There is no root `package.json`; the npm package and all scripts live in
`adw_sdlc/`.

## Quick start

Requires Node `>=20.19` (the `pi` runner additionally needs Node `>=22.19`).
Run commands from the package directory:

```bash
cd adw_sdlc
npm install
npm run issue -- <work-item-id> --dry-run      # preview the phase plan, run nothing
npm run issue -- <work-item-id> --runner claude
```

`npm run issue` maps to `tsx src/cli.ts`; `-h` / `--help` prints every flag. See
[`adw_sdlc/README.md`](./adw_sdlc/README.md#quick-start) for the full flag table,
the `.adw/config.json` configuration surface, and prompt-pack generation.

## ADW Cockpit (TUI)

The project's only graphical surface is the **ADW Cockpit**, a read-only
[Pi](https://github.com/earendil-works/pi) TUI extension at
[`.pi/extensions/adw-cockpit/index.ts`](./.pi/extensions/adw-cockpit/index.ts).
It observes config, git, and `agents/*/state.json` to render a mission-control
dashboard (overview, latest run, pipeline) and never owns git/forge. Slash
commands include `/adw-menu`, `/adw-runs`, `/adw-config`, and `/adw-mvp`. Design
rationale is in [`.impeccable.md`](./.impeccable.md).

## Quality gate

`npm run verify` (from `adw_sdlc/`) is the canonical local **and** CI quality
gate. It runs every check in order and fails fast:

```bash
cd adw_sdlc
npm run verify   # typecheck → lint:env → pack:check → mirror:check → coverage → build → clean
```

CI runs the same command on a Node-version matrix (the `>=20.19` engines floor
and the Node 22 dev line) via
[`.github/workflows/verify.yml`](./.github/workflows/verify.yml), wired as a
required status check on `main`. ADW live runs use the same command as their test
gate (`ADW_TEST_CMD="npm run verify"`).

## Documentation

- **[`adw_sdlc/README.md`](./adw_sdlc/README.md)** — package overview, CLI,
  configuration, and development workflow (the documentation map to every design
  and operations doc lives at the end of that file).
- **[`adw_sdlc/docs/UNIVERSAL.md`](./adw_sdlc/docs/UNIVERSAL.md)** — the
  universal kernel / project-pack architecture.
- **[`adw_sdlc/HANDOVER.md`](./adw_sdlc/HANDOVER.md)** — session-to-session
  status and roadmap.

## License

Switchyard is available under the [MIT License](./LICENSE).
