# Switchyard

Switchyard turns a development task—usually a GitHub issue—into a tested,
reviewable pull request. A coding agent plans and edits the code; Switchyard
keeps the workflow moving, saves progress, runs checks, and controls Git,
GitHub, CI, and any optional merge.

Switchyard is unreleased and has no users. There is no long-term coexistence
or user-compatibility promise.

> [!IMPORTANT]
> Active development is the Rust mission host under `crates/`. The runnable
> capability in this checkout is S1: a local SQLite ledger driven by a fake
> runtime. The TypeScript package in `adw_sdlc/` is a frozen reference.

## Direction

The accepted long-term engine is Rust, including a native Rust agent runtime
([decision, 2026-10-29](./wiki/decisions/invariants.md)). Pi Durable is a
proof-of-concept adapter only, not that runtime. The planned M1 shape is Pi
Durable 1.4.0 behind a sandboxed TypeScript bridge. That bridge is not an
accepted integration.

S2 is **NO-GO**. See
[#96](https://github.com/kortiene/switchyard/issues/96) and the
[S2 feasibility audit](./docs/rust-first/s2-feasibility.md).

The TypeScript path and `runner-pi` are frozen (2026-10-29) and will be
retired with the TypeScript engine. Python compatibility is dropped. State v1
(`adw/state.schema.json`, `agents/<adw-id>/state.json`) is frozen as-is: no
further changes and no new consumers. Rust missions do not resume those runs.
Worktree lifecycle is owned by S3
([#97](https://github.com/kortiene/switchyard/issues/97)) in Rust. TypeScript
managed-worktree work is stopped.

Do not delete `adw_sdlc/` or `adw/` under these decisions. Cleanup of the Rust
tree is tracked in
[#109](https://github.com/kortiene/switchyard/issues/109). This page does not
repeat that queue. Normative mission transitions stay in the
[Rust-first architecture](./docs/rust-first/README.md#normative-mission-transitions).

## What you can run

S1 ([#95](https://github.com/kortiene/switchyard/issues/95),
[#105](https://github.com/kortiene/switchyard/pull/105)) is the implemented
host. `crates/switchyard-core` persists missions in an application-owned
SQLite ledger. The `switchyard` binary drives them with a fake runtime.

That host can create a mission from a local Git snapshot, admit commands,
recover after restart, and record fake-runtime observations. It does not call
a model, write to GitHub, confine a process, or resume frozen state v1.
`switchyard mission status` is read-only and does not take the owner lock.

```bash
cargo run --locked -p switchyard -- --help
./scripts/rust-verify.sh
```

`./scripts/rust-verify.sh` is the Rust check. It expects the committed Rust
1.99.1 toolchain in `rust-toolchain.toml` and runs `cargo fmt --check`,
Clippy, and `cargo test`. CI reports the same script as the `rust-core` job
on `ubuntu-24.04`. That job is not a required status check until a maintainer
adds the name to branch protection or rulesets.

The frozen TypeScript reference has a separate check. From `adw_sdlc/`:

```bash
npm run verify
```

There is no root `package.json`. The stage list, Node 20.19.0 / 22 matrix, and
the Pi audit jobs are in
[Testing and CI](./wiki/reference/testing-and-ci.md). Package-level detail is
in the [package development guide](./adw_sdlc/README.md#development).

## Where the work stands

| Work | Status |
| --- | --- |
| **S1** [#95](https://github.com/kortiene/switchyard/issues/95) | Implemented. Mission ledger, command outbox, fake runtime. |
| **S2** [#96](https://github.com/kortiene/switchyard/issues/96) | **NO-GO**. See [#96](https://github.com/kortiene/switchyard/issues/96) and the [S2 feasibility audit](./docs/rust-first/s2-feasibility.md). |
| **S3–S8** [#97](https://github.com/kortiene/switchyard/issues/97)–[#102](https://github.com/kortiene/switchyard/issues/102) | Planned. S3 owns native worktree lifecycle. Pi-dependent live work stays blocked while S2 is NO-GO. |
| **Native Rust agent runtime** | Accepted long-term target (2026-10-29). Not implemented. Replacing the proof-of-concept adapter needs its own reviewed gate. |

Product milestone:
[#94](https://github.com/kortiene/switchyard/issues/94).

## Architecture

Read these before changing mission, recovery, or runtime behavior:

- [Rust-first architecture and M1 scope](./docs/rust-first/README.md)
- [Delivery plan and gates](./docs/rust-first/delivery-plan.md)
- [Runtime protocol](./docs/rust-first/runtime-protocol.md)
- [S2 package audit (NO-GO, 2026-10-29)](./docs/rust-first/s2-feasibility.md)
- [Decision register](./wiki/decisions/invariants.md)

The product objective is one recoverable, verified change: freeze an
objective, approve a plan, restart and recover the same attempt, repair a
failed verification within limits, and publish one pull request only with an
explicit approval. That outcome is milestone #94. S1 does not perform it
against a model or GitHub.

## Repository map

| Path | Role | What it is |
| --- | --- | --- |
| [`crates/switchyard-core/`](./crates/switchyard-core/), [`crates/switchyard/`](./crates/switchyard/) | Active | S1 library and `switchyard` binary. One library crate and one executable. |
| [`docs/rust-first/`](./docs/rust-first/) | Active | Architecture, protocol, and gates. |
| [`scripts/rust-verify.sh`](./scripts/rust-verify.sh) | Active | Rust local gate. |
| [`wiki/`](./wiki/) | Active | OKF v0.1 map. Concepts cite canonical sources; they do not replace them. |
| [`.github/workflows/`](./.github/workflows/) | Active | `verify`, `rust-core`, `pi-package-audit`, and `pi-source-candidate`. |
| [`adw_sdlc/`](./adw_sdlc/) | Reference | Frozen TypeScript engine: CLI, kernel, runners, and package docs. Kept while useful. |
| [`adw/`](./adw/) | Reference | Frozen state v1 schema and fixtures. |
| [`.adw/`](./.adw/) | Reference | Project pack. `.adw/prompts/` is generated; `npm run pack:check` owns that drift gate. |
| [`.pi/prompts/`](./.pi/prompts/) | Reference | Hand-maintained neutral prompt source. |
| [`.claude/commands/`](./.claude/commands/) | Reference | Generated byte mirror of `.pi/prompts/`. `npm run mirror:check` owns that drift gate. |
| [`.pi/extensions/adw-cockpit/`](./.pi/extensions/adw-cockpit/) | Reference | Optional Pi TUI. Read-only by default. |
| [`runtime/pi/`](./runtime/pi/) | Experimental | Published Pi Durable 1.4.0 audit. NO-GO evidence, not a runtime. |
| [`runtime/pi-source/`](./runtime/pi-source/) | Experimental | Pinned source candidate (#107). `NEEDS-FULL-S2`. Not a product runtime. |
| [`specs/`](./specs/) | Reference | Historical implementation specifications. |
| `web/`, `tests/acceptance/` | Deferred | Named in the plan. Not packages or commands in this checkout. |
| [#42](https://github.com/kortiene/switchyard/issues/42), [#43](https://github.com/kortiene/switchyard/issues/43), [#44](https://github.com/kortiene/switchyard/issues/44) | Deferred | Demand-gated. No implementation in the cleanup track. |

`agents/` is git-ignored run output. Do not treat it as source.

## Legacy TypeScript reference

`adw_sdlc/` is frozen as of 2026-10-29: fixes only, and it will be retired.
`runner-pi` is part of that path. It is not the Pi Durable proof of concept
and not the native Rust runtime. The package is private and does not publish
a global CLI. Commands below run from `adw_sdlc/` after `npm ci`, with Node
`>=20.19`. Node `>=22.19` is required for the legacy Pi adapter.

The package default engine is `ts` (`DEFAULT_ENGINE`). `--engine py` fails
closed. Python compatibility is dropped; that default is not a promise that
the TypeScript engine remains the product.

A dry run does not look up the issue, load a runner, create run state, change
files, call GitHub, or spend model credits:

```bash
cd adw_sdlc
npm run issue -- 123 \
  --runner claude \
  --dry-run \
  --no-merge \
  --test-cmd "npm run verify"
```

The literal issue number `123` is safe in dry-run mode. A successful preview
only confirms that the local package and project pack load.

> [!WARNING]
> A live run can execute repository commands, edit files, post issue updates,
> push a branch, open a pull request, watch CI, and spend provider credits.
> Only target repositories you trust. Pass `--test-cmd` or `ADW_TEST_CMD`;
> this repository's default test command is empty.

Live-run prerequisites (`gh auth status`, a clean dedicated checkout, approval
to spend credits, and a `--no-merge` budget example) are in the
[package quick start](./adw_sdlc/README.md#quick-start), with the flag list and
managed-worktree commands. Historical evidence is in
[`adw_sdlc/PARITY.md`](./adw_sdlc/PARITY.md) and
[`adw_sdlc/MVP-READINESS.md`](./adw_sdlc/MVP-READINESS.md).

| Runner | Recorded role on this frozen path |
| --- | --- |
| **Claude** | Default runner for the reference package. Real issue-to-PR evidence is recorded. |
| **OpenCode** | Adapter with recorded real issue-to-PR runs. |
| **Codex** | Adapter implemented. The live ledger records an authentication blocker. |
| **Pi** (`runner-pi`) | Frozen legacy adapter. Run `babe0070` (issue #70, PR #73) is **partial** evidence only: it needed one scope fix from the owner. An unassisted Pi real-issue run is still owed. |

The reference pipeline is `setup → classify → plan → implement → tests →
resolve → e2e → review → patch → document → finalize → ci-fix → merge →
report`. Phase rules are in
[orchestration](./wiki/architecture/orchestration.md). The orchestrator, not
the agent, owns Git and GitHub. Runner children do not receive `GH_TOKEN` in
normal phased mode.

`--worktree` still exists on this path. Further TypeScript managed-worktree
development is stopped (2026-10-29). New lifecycle work is S3. See
[managed worktrees](./wiki/operations/managed-worktrees.md).

The [ADW Cockpit](./.pi/extensions/adw-cockpit/index.ts) is an optional
[Pi](https://github.com/earendil-works/pi) terminal dashboard on the same
frozen path. Its views are read-only by default. `/adw-run` is the explicit
state-changing command: it asks for confirmation and delegates to the
orchestrator. Design notes are in [`.impeccable.md`](./.impeccable.md).

## Documentation

The wiki is an
[OKF v0.1](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)
bundle rooted at [`wiki/index.md`](./wiki/index.md). Validate it from
`adw_sdlc/` with `npm run wiki:check`. Maintenance rules are in
[`wiki/contributing/wiki-maintenance.md`](./wiki/contributing/wiki-maintenance.md).

- [`wiki/index.md`](./wiki/index.md) — knowledge map.
- [`adw_sdlc/README.md`](./adw_sdlc/README.md) — TypeScript CLI and package reference.
- [`adw_sdlc/docs/UNIVERSAL.md`](./adw_sdlc/docs/UNIVERSAL.md) — kernel and project-pack architecture for that package.
- [`adw_sdlc/HANDOVER.md`](./adw_sdlc/HANDOVER.md) — historical status notes for the TypeScript port.

## License

Switchyard is available under the [MIT License](./LICENSE).
