---
type: Contributor Workflow
title: Contributor workflow
description: A source-of-truth-first workflow for changing Switchyard code, prompts, configuration, tests, and documentation.
tags: [workflow, contributing, testing, prompts, documentation]
timestamp: "2026-07-18T13:26:10Z"
---

# Before editing

1. Read the root and package READMEs, then locate the implementation and tests that own the behavior.
2. Inspect the working tree and preserve unrelated changes; generated run, build, and coverage output is not source.
3. Identify whether the change touches a load-bearing boundary: runner environment, Git/forge ownership, prompt preamble, state compatibility, or managed cleanup.
4. Prefer a focused, auditable change and a focused test before broad verification.

# Change paths

| Change | Source to edit | Required follow-through |
| --- | --- | --- |
| Kernel behavior | `adw_sdlc/src/` | Update focused tests and affected reference/wiki concepts. |
| Project policy | `.adw/config.json` | Exercise config validation and document operator-visible behavior. |
| Neutral prompts | `.pi/prompts/` | Sync `.claude/commands/`, regenerate `.adw/prompts/`, run both drift checks. |
| Pack context | `.adw/pack.profile.json` | Regenerate `.adw/prompts/` and run `pack:check`. |
| State shape | `state.ts`, `adw/state.schema.json` | Keep additions compatible and update cross-language fixtures/tests deliberately. |
| CLI behavior | `cli.ts` | Update CLI tests, package docs, and [CLI reference](/reference/cli.md). |
| Wiki concept | `wiki/` | Update indexes/log when needed and run `wiki:check`. |

# Test and verify

From `adw_sdlc/`:

```bash
npx vitest run test/<affected>.test.ts
npm run wiki:check
npm run verify
```

The final command is the canonical local and CI gate. Real paid runner calls, live forge mutations, and live evidence drills are not part of ordinary automated tests.

# Documentation ownership

Existing package/API documentation remains canonical for exact usage and configuration. The wiki explains cross-cutting concepts and traversal. Summarize and cite canonical sources instead of copying long sections that will drift. Follow the [wiki maintenance policy](/contributing/wiki-maintenance.md).

# Citations

[1] [Repository development guide](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/README.md)

[2] [Prompt-pack generator](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/tools/pack-generate.ts)

[3] [Prompt mirror checker](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/tools/mirror-check.ts)

[4] [Canonical package scripts](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/package.json)
