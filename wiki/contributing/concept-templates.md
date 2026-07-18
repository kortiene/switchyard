---
type: Template
title: Wiki concept templates
description: Copyable OKF frontmatter and body structures for recurring Switchyard knowledge types.
tags: [wiki, templates, okf, authoring]
timestamp: "2026-07-18T13:26:10Z"
---

# Use

Copy the smallest matching template into a non-reserved `.md` file. Replace every angle-bracket placeholder, remove inapplicable optional fields or sections, add the concept to its parent index, and run `npm run wiki:check` from `adw_sdlc/`.

`resource` is optional and should identify the one underlying asset a concept represents; use citations, not `resource`, for a list of evidence.

# Architecture component

```markdown
---
type: Architecture Component
title: <Component name>
description: <One-sentence responsibility and boundary.>
resource: <Canonical URI for the component, if applicable>
tags: [architecture, <subsystem>]
timestamp: "<ISO 8601 datetime>"
---

# Responsibility

<What this component owns and does not own.>

# Interfaces

| Interface | Consumer | Contract |
| --- | --- | --- |
| <name> | <consumer> | <contract> |

# Invariants

* <Load-bearing rule.>

# Failure modes

* <Failure and containment behavior.>

# Citations

[1] [Canonical source](https://example.com/source)
```

# Workflow

```markdown
---
type: Workflow
title: <Workflow name>
description: <One-sentence trigger-to-outcome summary.>
tags: [workflow, <domain>]
timestamp: "<ISO 8601 datetime>"
---

# Trigger

<When and why this workflow starts.>

# Preconditions

* <Required state.>

# Flow

1. <Deterministic step.>
2. <Next step and linked concept.>

# Outcomes

| Outcome | Meaning |
| --- | --- |
| <outcome> | <meaning> |

# Citations

[1] [Canonical source](https://example.com/source)
```

# CLI or configuration reference

````markdown
---
type: Configuration Reference
title: <Surface name>
description: <One-sentence statement of what can be looked up here.>
resource: <Canonical source URI>
tags: [reference, configuration]
timestamp: "<ISO 8601 datetime>"
---

# Syntax or shape

```text
<command or configuration shape>
```

# Fields

| Name | Default | Meaning |
| --- | --- | --- |
| `<name>` | <default> | <meaning and constraints> |

# Precedence

<Explicit values, environment, configuration, and defaults.>

# Examples

<Minimal safe examples.>

# Citations

[1] [Canonical source](https://example.com/source)
````

# Operational playbook

```markdown
---
type: Operational Playbook
title: <Playbook name>
description: <One-sentence symptom and safe outcome.>
tags: [operations, <subsystem>]
timestamp: "<ISO 8601 datetime>"
---

# Trigger

<Observable symptom.>

# Safety

* <Action or data that must be preserved or avoided.>

# Diagnose

1. <Read-only check.>
2. <Evidence interpretation.>

# Recover

1. <Bounded recovery step.>
2. <Success verification.>

# Escalation

<When to stop and what evidence to retain.>

# Citations

[1] [Canonical source](https://example.com/source)
```

# Design decision

```markdown
---
type: Decision
title: <Decision title>
description: <One-sentence choice and motivation.>
tags: [decision, <subsystem>]
timestamp: "<ISO 8601 datetime>"
---

# Status

<Implemented | Proposed | Deferred | Superseded>

# Context

<Forces, constraints, and alternatives.>

# Decision

<The chosen rule.>

# Consequences

* <Positive, negative, and operational effects.>

# Revisit when

<Concrete trigger for reconsideration.>

# Citations

[1] [Implementation or design source](https://example.com/source)
```

# Glossary concept

```markdown
---
type: Glossary Concept
title: <Term>
description: <One-sentence definition in Switchyard context.>
tags: [terminology, <domain>]
timestamp: "<ISO 8601 datetime>"
---

# Definition

<Precise meaning and scope.>

# Usage

<How the term appears in code, configuration, or operations.>

# Relationships

* <Related concept and relationship.>

# Citations

[1] [Canonical source](https://example.com/source)
```

# Citations

[1] [Open Knowledge Format concept-document conventions](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md#4-concept-documents)
