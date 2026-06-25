---
description: Resolve failing repository checks reported by the phased ADW test gate
argument-hint: "<failing-output-and-context>"
---
The repository's test/verification gate is failing. Fix the failures.

Failing output and context (truncated):

$ARGUMENTS

## Instructions

- Investigate the failures above and make the smallest correct change that fixes them.
- Fix the root cause in the code or the tests as appropriate. Do NOT weaken or delete
  meaningful assertions, skip tests, or mask failures to make the gate pass.
- Stay within the scope of the current change; do not start unrelated work.
- Preserve HealthTech constraints: keep the local-first / zero-knowledge model intact — the
  patient record is encrypted client-side with AES-256-GCM before any network transit and the
  server only ever stores opaque encrypted blobs keyed by anonymous UUIDs. Never weaken the
  cryptography, never log or persist plaintext medical data, encryption keys, or PII, and keep
  access ephemeral and patient-controlled (QR codes expire ~120s; the professional decrypts in
  RAM only and the session is wiped afterward). Respect Ivorian data residency (ARTCI / loi
  n°2013-450) and degraded-network resilience (offline queue, plaintext record <= 500 KB, heavy
  images never stored on the patient device).
- The orchestrator re-runs the gate after you finish. Report how many failing checks you
  fixed (`resolved`) and how many remain (`remaining`); if you could fix nothing, say so via
  the counts so the loop can stop.

## Verify before finishing

Before you report, re-run the failing check and the project's configured verification gate
(the command surfaced via `MX_AGENT_TEST_CMD`) plus any format/lint/build checks the project
defines, and confirm they pass.

HealthTech is greenfield: the application stack, build tool, and test command have NOT been
finalized yet (backlog issue #1). So:

- If a test/lint/format/build command IS configured, run it, fix anything it surfaces, and rerun
  the relevant check until it passes.
- If NO test command is configured, say so explicitly. Do NOT assume a particular language, build
  tool, or test command, and do NOT invent a toolchain. Recommend the exact command that should be
  run once the stack lands so the gate can be re-run.

If a check cannot be run, say why and give the exact command.
