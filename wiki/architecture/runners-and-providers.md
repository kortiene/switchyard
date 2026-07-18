---
type: Architecture Component
title: Runners and providers
description: The interchangeable agent-runner and external-system provider seams used by the control plane.
tags: [architecture, runners, providers, adapters]
timestamp: "2026-07-18T13:26:10Z"
---

# Runner seam

`AgentRunner.runPhase()` accepts one normalized request: phase, prompt, model, worktree, explicit environment, optional schema and budget, transcript path, and abort signal. It returns a normalized structured result, transcript, usage, exit code, termination signal, and optional resumable session ID.

| Runner | Structured output | Isolation/control highlights |
| --- | --- | --- |
| Claude | Native schema | Explicit child environment, per-tool hook, native cost and budget support. |
| Codex | Native schema | Explicit child environment and workspace-write sandbox; parent-side pricing when possible. |
| OpenCode | Native schema | Switchyard-spawned server with an allowlisted environment and runner-owned permission policy. |
| Pi | Fenced JSON contract | Allowlisted subprocess environment and streamed JSON events. |

The kernel branches on declared capabilities, not runner identity. Model names are resolved through project-configured tiers.

# Provider seam

Providers separate runner choice from external-system choice.

| Role | Responsibility | Built-in or configured forms |
| --- | --- | --- |
| CLI | Resolve or validate a provider command-line dependency. | GitHub CLI built-in. |
| Work item | Read work, post progress, and update status. | GitHub plus declarative CLI and REST descriptors. |
| VCS | Inspect and mutate the repository under kernel control. | Git built-in. |
| Change request | Create/update requests, read pipeline state, merge, and verify remote status. | GitHub plus declarative CLI and REST descriptors. |

Declarative descriptors are data, not in-process plugins. REST descriptors enforce HTTPS and host rules; credential forwarding is limited to one validated environment-variable name. Arbitrary project-supplied provider code remains deferred.

# Relationship

A runner may edit files but does not receive forge authority. A provider may perform an orchestrator-requested external operation but does not decide phase flow. The [security boundary](/architecture/security-boundaries.md) applies across both seams.

# Citations

[1] [`AgentRunner` request, result, and capabilities](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/invoker.ts)

[2] [Runner registry](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/registry.ts)

[3] [Provider interfaces and built-ins](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/providers.ts)

[4] [Declarative provider validation](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/src/provider-descriptor.ts)
