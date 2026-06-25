---
description: Add or improve focused non-e2e tests for a spec, PR, or working tree
argument-hint: "[spec-file|pr-url-or-number|notes]"
---
Add or improve focused non-e2e test coverage for this target:

$ARGUMENTS

This command is for unit tests, deterministic integration tests that do not require external services, argument/output tests, policy/protocol/schema tests, and negative/security regression tests. Do not add tests that require live infrastructure (real network sync, a running backend, or device hardware) here; use `/e2e_tests` for those.

Workflow:

1. Understand the testing target
   - If the argument is a spec file path, read it completely and identify the behavior that should be covered by tests.
   - If the argument is a PR URL or number, inspect PR metadata, changed files, commits, checks, and diff using `gh` (or `~/.local/bin/gh` if needed).
   - If the argument is notes/free text, treat it as testing goals for the current working tree.
   - If no argument is provided, inspect the current working tree and ask for clarification only if the target is genuinely unclear.

2. Read repository context before editing
   - `PRD_HealthTech.md` (product requirements: local-first / zero-knowledge model, ephemeral access, data-residency, degraded-network resilience)
   - `BACKLOG.md` (epics, issues, and their order)
   - the specific GitHub issue being worked (kortiene/HealthTech), if named in the target
   - the existing application source tree and any tests around the target behavior — note: the application stack is not yet established (backlog #1), so the source tree may not exist yet; if so, say so.

3. Identify coverage gaps
   - Summarize the behavior under test.
   - Identify existing tests that already cover it.
   - Identify missing edge cases, negative cases, error handling, encryption/decryption boundaries, access-control and expiry checks, schema/protocol compatibility, offline/degraded-network paths, and regression risks.
   - Prefer the smallest test layer that gives confidence: unit tests before integration tests, integration tests before e2e tests.

4. Add or improve tests
   - Add focused, deterministic tests that cover the gaps.
   - Do not implement new product behavior except minimal testability hooks when absolutely necessary.
   - Do not weaken assertions or delete meaningful coverage to make tests pass.
   - Do not introduce flaky sleeps, timing-sensitive assertions, network dependencies, or external service requirements.
   - Do not use real secrets, patient data, encryption keys, tokens, or private keys in fixtures; use synthetic, clearly-fake values.
   - Match whatever language and test framework the project has adopted; do not introduce a new toolchain.
   - Document public test helpers if they are public APIs; prefer private helpers when possible.

5. Preserve HealthTech constraints
   - Local-first / zero-knowledge: the patient's medical record is encrypted client-side with AES-256-GCM before any network transit; the server stores only opaque encrypted blobs keyed by anonymous UUIDs and must never be able to read or decrypt them. Tests must assert this, never circumvent it.
   - Never weaken the cryptography; never log or persist plaintext medical data, encryption keys, or PII — including in test fixtures, assertions, and snapshots.
   - Ephemeral, patient-controlled access: access QR codes expire (~120s); a professional decrypts the record in RAM only and the session is wiped at the end (or after inactivity). Add negative tests that an expired or revoked grant cannot decrypt.
   - Data residency: data must stay hosted on Ivorian soil to satisfy ARTCI / loi n°2013-450; do not add tests that depend on out-of-region services.
   - Degraded-network resilience: must work offline (e.g. SQLCipher queue) and tolerate power/network cuts; the plaintext record stays <= 500 KB; heavy medical images are never stored on the patient device (only an ephemeral URL is embedded). Cover the offline/queue and size-bound paths.
   - Do not imply unimplemented behavior exists; only test what is actually implemented.

6. Verify before finishing
   - Run the most relevant test first (the single test or module covering the changed behavior) before the full gate, when practical.
   - Then run the project's configured test gate (the command surfaced via `MX_AGENT_TEST_CMD`) plus any format, lint, and build checks the project defines.
   - HealthTech has NOT finalized its stack or test command yet (backlog #1). If no test command is configured, say so explicitly and recommend the exact command to run once the stack lands — do NOT assume a particular language, build tool, or test runner, and do NOT invent a toolchain.
   - If a check fails, fix the issue and rerun the relevant check when practical.
   - If a check cannot be run, explain why and recommend the exact command.

7. Final report
   - Testing target
   - Files changed
   - Tests added or updated
   - Coverage gaps closed
   - Bugs discovered, if any
   - Checks run and results
   - Remaining coverage gaps or follow-up recommendations

Important: focus on tests. Do not broaden the implementation scope or add e2e infrastructure unless explicitly asked.
