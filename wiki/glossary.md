---
type: Glossary
title: Switchyard glossary
description: Canonical meanings for terms used across the Switchyard control plane and wiki.
tags: [terminology, adw, reference]
timestamp: "2026-07-18T13:26:10Z"
---

# Terms

| Term | Meaning |
| --- | --- |
| ADW | Agentic Developer Workflow: the phased delivery process driven by Switchyard. |
| Agent phase | One bounded, named unit of agent work with its own prompt and structured result. |
| AgentRunner | The common `runPhase()` interface implemented by Claude, Codex, OpenCode, and Pi adapters. |
| Bundle | This self-contained hierarchy of OKF Markdown concepts under `wiki/`. |
| Change request | Provider-neutral term for a proposed repository change, such as a GitHub pull request. |
| Concept | One OKF Markdown document representing one unit of knowledge. |
| Control plane | Deterministic code that owns orchestration, state, gates, Git, forge operations, and runner capabilities. |
| Gate | A deterministic condition or command that decides whether work proceeds or a repair phase runs. |
| Managed run | An opt-in run allocated to a Switchyard-owned linked Git worktree with durable ownership metadata. |
| Phase result | Runner output normalized and validated against the phase's structured-output schema. |
| Project pack | Repository policy under `.adw/`: configuration, generated runtime prompts, optional schemas, and generation profile. |
| Provider | An implementation of a control-plane role: CLI locator, work items, VCS, or change requests and CI. |
| Runner | A coding-agent backend selected to perform agentic phases without owning Git or forge actions. |
| Work item | Provider-neutral unit of requested work, such as a GitHub issue. |

# Related concepts

See [system architecture](/architecture/system.md), [runners and providers](/architecture/runners-and-providers.md), [state and artifacts](/architecture/state-and-artifacts.md), and [configuration and project packs](/reference/configuration-and-project-packs.md).

# Citations

[1] [`AgentRunner` and runner contracts](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/invoker.ts)

[2] [Phase catalog](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/phases.ts)

[3] [Provider interfaces](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/providers.ts)

[4] [Persistent state implementation](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/state.ts)
