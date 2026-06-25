---
description: Update standalone documentation after a reviewed implementation (phased ADW)
argument-hint: "<change-summary-and-files>"
---
Update the repository's documentation to reflect the implemented, reviewed change.

Change summary, files changed, and context:

$ARGUMENTS

## Scope and boundary

This is the **standalone documentation pass**, distinct from the inline doc edits already made
during implementation:

- The `implement` phase already made the tight, code-local edits that must ship with the code
  (doc-comments on new public APIs, in-app/usage text, the focused references the change toggles).
  Do not redo or fight those.
- Here, update the broader prose that benefits from seeing the finished change: project docs that
  exist once the stack lands (e.g. a `docs/` tree, a `README`, developer guides), and — when a
  change shifts product scope, an epic, or an issue's status — the relevant entries in
  `BACKLOG.md`, plus any cross-references to `PRD_HealthTech.md` that are now stale. HealthTech is
  greenfield: if no `docs/` tree or README exists yet, the only durable docs are `PRD_HealthTech.md`
  and `BACKLOG.md`, so confine prose updates to those (and only when the change actually invalidates
  them).

## Instructions

- Only update documentation when the change is user-visible, alters a public API/CLI/protocol,
  or invalidates an existing doc/`PRD_HealthTech.md`/`BACKLOG.md` statement. If nothing needs
  updating, change nothing and report `docs_updated` false.
- Edit existing documentation in place. Do NOT create an `app_docs/` tree or a new
  per-feature documentation hierarchy.
- Describe only what this change actually implements; do not overstate planned or future behavior.
- Preserve HealthTech's invariants in any prose you write: local-first / zero-knowledge (the
  patient record is encrypted client-side with AES-256-GCM before any network transit; the server
  only ever holds opaque encrypted blobs keyed by anonymous UUIDs), ephemeral patient-controlled
  access (QR codes expire ~120s; the professional decrypts in RAM only and the session is wiped
  afterward), Ivorian data residency (ARTCI / loi n°2013-450), and degraded-network resilience
  (offline queue, plaintext record <= 500 KB, heavy medical images never stored on the patient
  device). Do not document anything that would contradict or weaken these.
- Do not document plaintext medical data, secrets, tokens, encryption keys, or PII; preserve
  existing redaction conventions.

Because this is the last authoring phase when it runs, also author the final commit message and
PR body (see the output instructions below) so they reflect all changes — code, tests, and docs.
