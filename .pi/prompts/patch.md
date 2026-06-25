---
description: Resolve blocking review findings in the phased ADW pipeline
argument-hint: "<blocker-findings-and-context>"
---
A self-review found blocking issues in the current implementation. Resolve them.

Blocking findings and context:

$ARGUMENTS

## Instructions

- Address every blocking finding above with the smallest correct change.
- Only fix the listed blockers; do not act on tech-debt or skippable items, and do not start
  unrelated work or broad rewrites.
- Keep tests meaningful — fix the cause, do not weaken assertions.
- Preserve HealthTech constraints: keep the local-first / zero-knowledge model intact — the medical
  record is encrypted client-side with AES-256-GCM before any network transit, and the server stores
  only opaque encrypted blobs keyed by anonymous UUIDs (it must never be able to read or decrypt them).
  Never weaken the cryptography; never log or persist plaintext medical data, encryption keys, or PII.
  Keep access ephemeral and patient-controlled (QR codes expire ~120s; the professional decrypts in RAM
  only and the session is wiped on end/inactivity). Keep data hosted on Ivorian soil (ARTCI / loi
  n°2013-450). Preserve degraded-network resilience (offline queue, tolerant of power/network cuts; the
  plaintext record stays <= 500 KB; heavy medical images are never stored on the patient device — only
  an ephemeral URL is embedded).
- Report how many blocking findings you fixed (`resolved`) and how many remain (`remaining`).

## Verify before finishing

If you changed code, before you report run the project's configured test gate (the command surfaced via
`MX_AGENT_TEST_CMD`) plus any format, lint, and build checks the project defines.

HealthTech has not finalized its stack or test command yet (backlog #1). If no test command is
configured, say so explicitly and recommend the exact command to run once the stack lands — do not
assume a particular language, build tool, or test runner, and do not invent a toolchain.

Fix anything these surface and rerun the relevant check. If a check cannot be run, say why and
give the exact command.
