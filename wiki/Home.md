---
type: Wiki
okf_version: "0.1"
title: Home
description: Index page for the Switchyard OKF wiki bundle.
tags: [wiki, index]
---

# Switchyard

> **Autonomous worktree orchestration for multi-agent AI development.**

Switchyard is a TypeScript-based SDLC engine that runs AI agent work-streams as
Git worktrees, each managed through a complete lifecycle — from classification
and planning through implementation, review, and merge.  It is both a developer
tool and a coordination layer for teams (human or machine) that need traceable,
concurrent, conflict-free development.

## Quick links

| | |
|---|---|
| [⚡ Overview](overview.md) | Purpose, scope, and a map of the control plane |
| [📖 Glossary](glossary.md) | Shared definitions of every term in use |
| [🚀 README](https://github.com/kortiene/switchyard/blob/main/README.md) | Repository map and quick start |

## Browse by category

### Architecture
* [System overview](architecture/system.md)
* [Orchestration](architecture/orchestration.md)
* [Runners & providers](architecture/runners-and-providers.md)
* [Security boundaries](architecture/security-boundaries.md)
* [State & artifacts](architecture/state-and-artifacts.md)

### Reference
* [CLI reference](reference/cli.md)
* [Configuration & project-packs](reference/configuration-and-project-packs.md)
* [Testing & CI](reference/testing-and-ci.md)

### Workflows
* [Work-item lifecycle](workflows/work-item-lifecycle.md)
* [Contributing](workflows/contributing.md)

### Operations
* [Managed worktrees](operations/managed-worktrees.md)
* [Troubleshooting](operations/troubleshooting.md)

### Decisions
* [Implemented invariants](decisions/invariants.md)

### Contributing
* [Concept templates](contributing/concept-templates.md)
* [Wiki maintenance](contributing/wiki-maintenance.md)

## Format

This wiki uses [Open Knowledge Format v0.1](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md).
Each file may carry an `okf_version` front-matter field; the `log.md` tracks all structural changes.