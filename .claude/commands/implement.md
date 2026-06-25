---
description: Implement a spec file end-to-end
argument-hint: "<spec-file>"
---
Implement the specification in this file end-to-end:

$1

Extra context/notes from me (may be empty): ${@:2}

Do not stop after planning unless the spec is genuinely ambiguous, unsafe, impossible, or blocked by missing information. Read the spec, implement it, test it, and report the result.

Workflow:

1. Read and understand the spec
   - Read the spec file at `$1` completely.
   - Treat the spec as the source of truth for scope and acceptance criteria.
   - If the file does not exist, stop and report the missing path.
   - If the spec is ambiguous, state the ambiguity, make a reasonable assumption when safe, and proceed. Stop only for real blockers.

2. Read repository context before editing
   - `PRD_HealthTech.md` (product requirements and security/architecture constraints)
   - `BACKLOG.md` (epics, issues, milestone order, dependencies)
   - the specific GitHub issue the spec implements, if one is referenced
   - the existing application source tree and tests for the affected behavior — note that HealthTech is greenfield, so if no source tree exists yet, say so and work from the PRD/backlog and spec
   - existing docs (including any ADRs under `docs/adr/`) around the affected behavior

3. Summarize and plan briefly
   - Summarize the requested implementation in a few bullets.
   - Identify the owning component(s) (e.g. patient app, professional interface, backend, crypto core), modules, and existing patterns.
   - List the concrete implementation steps.
   - Then proceed with implementation.

4. Implement the spec completely
   - Make the smallest correct change that satisfies the spec.
   - Keep changes focused, idiomatic, and testable.
   - Preserve existing repository conventions and milestone boundaries.
   - Do not introduce broad rewrites unless the spec explicitly requires them.
   - Update docs when behavior changes.
   - Add or update tests that cover the new behavior.

5. Preserve HealthTech constraints
   - Local-first / zero-knowledge: the patient's medical record is encrypted client-side with AES-256-GCM before any network transit. The server stores only opaque encrypted blobs keyed by anonymous UUIDs and must never be able to read or decrypt them.
   - Never weaken the cryptography; never log or persist plaintext medical data, encryption keys, or PII.
   - Ephemeral, patient-controlled access: access QR codes expire (~120s); a professional decrypts the record in RAM only, and the session is wiped at the end (or after inactivity).
   - Data residency: data must stay hosted on Ivorian soil to satisfy ARTCI / loi n°2013-450.
   - Degraded-network resilience: must work offline (e.g. SQLCipher queue) and tolerate power/network cuts; the plaintext record stays <= 500 KB; heavy medical images are never stored on the patient device (only an ephemeral URL is embedded).
   - Do not imply unimplemented behavior exists unless this implementation actually adds it.

6. Verify before finishing
   - Run the project's configured test gate (the command surfaced via the `MX_AGENT_TEST_CMD` environment variable) plus any format, lint, and build checks the project defines.
   - HealthTech has not finalized its stack or test command yet (backlog #1). If no test command is configured, say so explicitly and recommend the exact command to run once the stack lands — do not assume a particular language, build tool, or test runner, and do not invent a toolchain.
   - Run any additional checks named in the spec.
   - If a check fails, fix the issue and rerun the relevant check when practical.
   - If a check cannot be run, explain why and recommend the exact command.

7. Final report
   - Spec implemented: `$1`
   - Files changed
   - Behavior implemented
   - Tests/checks run and results
   - Any assumptions made
   - Any remaining risks, limitations, or follow-up work

Important: do not merely create another plan. Implement the provided spec end-to-end.
