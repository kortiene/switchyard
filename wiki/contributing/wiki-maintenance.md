---
type: Maintenance Workflow
title: Wiki maintenance
description: Ownership, source hierarchy, update triggers, validation, and stale-content controls for the Switchyard OKF bundle.
tags: [wiki, maintenance, okf, governance, validation]
timestamp: "2026-07-18T13:26:10Z"
---

# Ownership

The contributor changing a documented behavior owns the corresponding wiki update. Exact API/CLI/configuration facts stay owned by implementation and package documentation; the wiki owns cross-cutting explanation, navigation, relationships, and operational summaries.

# Source hierarchy

When sources disagree, use this order:

1. Current implementation, `.adw/config.json`, and executable tests.
2. Package scripts, schemas, tool configuration, and CI workflow.
3. Root and package READMEs.
4. Design, handover, readiness, parity, and operational records as rationale or evidence.
5. Never use generated runtime/build/coverage output as canonical knowledge.

Label unresolved disagreement or in-development behavior; do not silently choose an attractive claim.

# Update triggers

| Change | Review these concepts |
| --- | --- |
| Phase order, gates, retries, schemas | [Orchestration](/architecture/orchestration.md), [work-item lifecycle](/workflows/work-item-lifecycle.md) |
| Runner/provider capabilities | [Runners and providers](/architecture/runners-and-providers.md) |
| Environment, tool, Git, or forge authority | [Security boundaries](/architecture/security-boundaries.md), [decisions](/decisions/invariants.md) |
| State or artifact paths/meaning | [State and artifacts](/architecture/state-and-artifacts.md) |
| CLI flags or commands | [CLI reference](/reference/cli.md), affected playbook |
| Config, prompts, schemas, model tiers | [Configuration and project packs](/reference/configuration-and-project-packs.md) |
| Test/coverage/build/CI gate | [Testing and CI](/reference/testing-and-ci.md) |
| Managed-run lifecycle | [Managed worktrees](/operations/managed-worktrees.md), [decisions](/decisions/invariants.md) |

# Authoring workflow

1. Start from a [concept template](/contributing/concept-templates.md).
2. Keep one concept focused on one unit of knowledge and use structural Markdown.
3. Add or update its parent `index.md` entry using the concept description.
4. Cite canonical source and tests for substantive implementation claims.
5. Update `timestamp` only for a meaningful concept change, not a mechanical sweep.
6. Add a newest-first `log.md` entry for bundle-level additions, removals, or reorganizations.
7. Run the focused validator, then the canonical gate.

```bash
cd adw_sdlc
npm run wiki:check
npm run verify
```

# Validation policy

Every non-reserved Markdown concept must contain parseable YAML frontmatter with a non-empty `type`. Nested indexes have no frontmatter; the root index alone declares `okf_version`. Logs use ISO dates newest first.

`wiki:check` also validates recommended metadata shapes and local links. This is deliberately stricter producer policy than OKF baseline conformance: OKF consumers still must tolerate unknown types/keys, missing optional metadata, and broken links in external or incomplete bundles.

Concept links beginning `/` are resolved from the bundle root, per OKF. Index entries use relative links for progressive browsing. External citations are not fetched, and heading anchors are not checked.

# Freshness controls

The verification gate catches structural and link drift. Review-based triggers catch semantic drift. Future automation may map changed source paths to affected concepts and flag old timestamps, but it should not rewrite prose or timestamps automatically. Split a page only when independent ownership or retrieval value justifies a new concept.

# Privacy

Do not copy tokens, private work-item content, transcripts, user paths, or live `agents/` data into the wiki. Operational examples use placeholders and names-only secret diagnostics.

# Citations

[1] [Open Knowledge Format v0.1](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)

[2] [Switchyard wiki validator](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/tools/wiki-validate.ts)

[3] [Documentation change gate configuration](https://github.com/kortiene/switchyard/blob/main/.adw/config.json)
