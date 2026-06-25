---
description: Create a detailed implementation spec in specs/ without implementing it
argument-hint: "<prompt>"
---
Create a detailed implementation specification for this request:

$ARGUMENTS

Do not implement the requested feature. Only create a planning/spec document.

Workflow:
1. Read enough repository context to make the plan accurate:
   - `PRD_HealthTech.md` (product requirements: local-first / zero-knowledge architecture, crypto and compliance constraints)
   - `BACKLOG.md` (epics, milestones, issues #1–#31, dependencies, and recommended implementation order)
   - the specific GitHub issue being worked, if the request maps to one
   - the existing application source tree for the affected area — note: the project is greenfield, so if no source exists yet for this request, say so explicitly
2. Think through the request carefully and identify the owning package(s)/module(s), existing patterns, security and compliance constraints, and likely edge cases.
3. Create the `specs/` directory if it does not already exist.
4. Write a new Markdown spec file in `specs/`.
   - Derive a short, descriptive, kebab-case filename from the prompt when possible.
   - Prefer a stable name like `specs/<descriptive-slug>.md`.
   - If a file with that name already exists, choose a non-conflicting variant.
5. After writing the spec, report the spec path and a short summary. Do not make code changes beyond the spec file.

The spec must include these sections:

# <Descriptive Title>

## Problem Statement
Explain the user need and current gap.

## Goals
List concrete outcomes this implementation should achieve.

## Non-Goals
List related work that should remain out of scope.

## Relevant Repository Context
Summarize the relevant architecture, packages, modules, current status, and conventions. The stack is not finalized yet (backlog #1), so state which decisions are still open rather than assuming a language, framework, or build tool.

## Proposed Implementation
Describe the recommended implementation approach in enough detail for a coding agent to execute later.

## Affected Files / Packages / Modules
List likely files and modules to read or modify.

## API / Interface Changes
Describe any command-line, public API, network endpoint, or QR/access-token surface changes. State "none" if none are expected.

## Data Model / Protocol Changes
Describe record schema, encrypted-blob format, persistence, or serialization changes. State "none" if none are expected.

## Security & Compliance Considerations
Call out client-side AES-256-GCM encryption, zero-knowledge server guarantees (opaque blobs keyed by anonymous UUIDs), key handling, ephemeral QR access (~120s expiry) and in-RAM-only decryption with end-of-session wipe, data residency on Ivorian soil (ARTCI / loi n°2013-450), the ≤ 500 KB plaintext record budget, never storing heavy medical images on the patient device (only an ephemeral URL), and logging/redaction concerns (never log plaintext medical data, keys, or PII) as applicable.

## Testing Plan
List unit, integration, end-to-end, crypto-vector, resilience (offline/degraded-network), or documentation tests that should be added or updated.

## Documentation Updates
List PRD, BACKLOG, ADR, or help-text updates needed.

## Risks and Open Questions
Identify ambiguities, blockers, compatibility concerns, and decisions needing confirmation.

## Implementation Checklist
Provide a step-by-step checklist suitable for a coding agent to follow later.

Important constraints to preserve:
- Local-first / zero-knowledge: the patient's medical record is encrypted client-side with AES-256-GCM before any network transit; the server stores only opaque encrypted blobs keyed by anonymous UUIDs and must never be able to read or decrypt them.
- Never weaken the cryptography; never log or persist plaintext medical data, encryption keys, or PII.
- Ephemeral, patient-controlled access: access QR codes expire (~120s); a professional decrypts the record in RAM only and the session is wiped at the end (or after inactivity).
- Data residency: data must stay hosted on Ivorian soil to satisfy ARTCI / loi n°2013-450.
- Degraded-network resilience: must work offline (e.g. SQLCipher queue) and tolerate power/network cuts; the plaintext record stays ≤ 500 KB; heavy medical images are never stored on the patient device (only an ephemeral URL is embedded).
- The stack, build tool, and test command are not finalized yet (backlog #1): do not assume a particular language, framework, or toolchain in the plan; flag stack-dependent choices as decisions to confirm.
- Document new public APIs.
- Do not imply unimplemented behavior exists unless the later implementation actually adds it.
