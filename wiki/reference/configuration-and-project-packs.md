---
type: Configuration Reference
title: Configuration and project packs
description: The validated repository policy, prompt sources, generated prompt pack, and override precedence used by Switchyard.
tags: [reference, configuration, project-pack, prompts, schemas]
timestamp: "2026-07-18T13:26:10Z"
---

# Project pack

The active repository pack lives under `.adw/`.

| Path | Ownership | Purpose |
| --- | --- | --- |
| `.adw/config.json` | Hand-maintained | Project identity, providers, model tiers, branching, gates, commands, phase/schema extensions, and prompt roots. |
| `.adw/pack.profile.json` | Hand-maintained | Switchyard context inserted while generating runtime prompts. |
| `.adw/prompts/` | Generated and committed | Runtime templates consumed through the configured default prompt root. |
| `.adw/schemas/` | Optional, hand-maintained | Custom phase schemas or allowed built-in overrides when configured. |

# Major configuration groups

| Group | Controls |
| --- | --- |
| `project`, `prompts` | Project identity and prompt lookup roots. |
| `providers` | CLI, work-item, VCS, and change-request implementations or descriptors. |
| `phases`, `customPhases`, `loops` | Known phase order and registered custom control flow. |
| `schemas` | Optional schema root and explicit per-phase override paths. |
| `gates` | End-to-end, documentation, and custom change-sensitive predicates. |
| `models` | Cheap/mid/capable tiers and per-runner model IDs. |
| `branching`, `progress`, `commands` | Branch naming, progress tag, and deterministic command defaults. |

Configuration is validated before a run proceeds. Provider kinds, phase membership, prompt/schema availability, and load-bearing control-flow constraints receive additional subsystem validation.

# Prompt ownership pipeline

```text
.pi/prompts/ (canonical neutral templates)
        ├── byte mirror ──► .claude/commands/
        └── profile generation with .adw/pack.profile.json ──► .adw/prompts/
```

After changing `.pi/prompts/` or `.adw/pack.profile.json`, run:

```bash
cd adw_sdlc
npm run mirror:sync
npm run pack:generate
npm run mirror:check
npm run pack:check
```

Do not hand-edit generated prompt-pack output as the only change; drift checks will either reject it or the next generation will replace it.

# Precedence and hard boundaries

An explicit `--phases` list overrides configured phases, which override the built-in catalog. Runner-specific prompt roots override the default prompt root. Explicit schema mappings override convention-based schema files, which override built-in schemas only where the kernel permits.

The secret environment boundary and built-in phase semantics are kernel policy, not project-pack options. An external project root is trusted because its config can select commands and local prompt/schema paths.

# Citations

[1] [Configuration schema and loader](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/config.ts)

[2] [Switchyard project configuration](https://github.com/kortiene/switchyard/blob/main/.adw/config.json)

[3] [Prompt-pack generator](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/pack-generator.ts)

[4] [Schema registry](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/schema-registry.ts)
