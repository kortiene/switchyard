---
description: Add or improve end-to-end tests for HealthTech patient/professional flows
argument-hint: "[spec-file|pr-url-or-number|notes]"
---
Add or improve end-to-end test coverage for this target:

$ARGUMENTS

This command is for heavier end-to-end scenarios, especially behavior crossing the patient app, the health-professional interface, the sovereign backend, client-side crypto, ephemeral access, session lifecycle, and offline/sync boundaries. Prefer `/tests` for unit tests and deterministic non-e2e integration tests.

Workflow:

1. Understand the e2e target
   - If the argument is a spec file path, read it completely and identify the end-to-end behavior that needs coverage.
   - If the argument is a PR URL or number, inspect PR metadata, changed files, commits, checks, and diff via the orchestrator-provided context. Do not run git or gh yourself; the orchestrator owns all git/gh.
   - If the argument is notes/free text, treat it as e2e testing goals for the current working tree.
   - If no argument is provided, inspect the current working tree and ask for clarification only if the target is genuinely unclear.

2. Read repository and test infrastructure context before editing
   - `PRD_HealthTech.md` for product requirements and the security/architecture model.
   - `BACKLOG.md` for the epics, issue ordering, and the specific GitHub issue (kortiene/HealthTech #1–#31) being worked.
   - The specific GitHub issue under test, if one is named.
   - The existing application source tree, existing tests, and any e2e harness — once they exist. The project is greenfield: there is no application source tree or e2e harness yet, so say so explicitly and base scenarios on the PRD and the issue.
   - Any ADRs under `docs/adr/` (issue #1) once the stack is chosen, since the e2e harness depends on that choice.

3. Decide whether e2e coverage is warranted
   - Summarize the behavior under test.
   - Identify what lower-level tests already cover.
   - Add e2e tests only when unit or non-e2e integration tests are insufficient.
   - Prefer a small number of high-value scenarios over broad, slow, flaky coverage.
   - Clearly separate live-backend / multi-process / device-emulator tests from default tests if the project convention requires gating.
   - High-value HealthTech end-to-end surfaces include:
     - Patient onboarding: local encrypted account creation, client-side master-key generation, no nominative data sent in cleartext, and key recovery on a new device (PBKDF2 passphrase / cultural security questions).
     - Ephemeral access loop: patient generates a time-boxed access QR code, professional scans it, downloads the encrypted blob, decrypts it in RAM only, edits/merges a note or prescription, ends the session, re-encrypts and uploads, and the professional's RAM is wiped. Cover QR expiry (~120s) and the inactivity timeout.
     - Offline queue + sync: a consultation validated while the network is down is queued in the encrypted local store (e.g. SQLCipher) and synchronized once connectivity returns.
     - Degraded-network behavior: the plaintext record stays <= 500 KB; heavy medical images are not stored on the patient device, only an ephemeral access URL is embedded.

4. Add or improve e2e tests
   - Use existing project infrastructure and patterns.
   - Drive the real end-to-end flow (patient app -> backend -> professional interface) rather than mocking past the boundary under test.
   - Do not require real production services, real patient identities, or live sovereign-backend credentials; use local/test instances and synthetic fixtures.
   - Avoid making the default test gate depend on emulators, external networks, or live services unless that is already the project convention.
   - Prefer gated/tagged tests, or clearly documented external prerequisites, for tests that need a backend instance, a device emulator, or a local object store.
   - Keep tests reproducible, deterministic where possible, and safe to run repeatedly.
   - Avoid arbitrary sleeps; prefer readiness checks, bounded retries, or existing synchronization helpers. For QR-expiry and inactivity timeouts, drive a controllable clock rather than wall-clock sleeps.
   - Ensure test logs and fixtures never expose plaintext medical data, encryption keys, or PII.

5. Preserve HealthTech constraints
   - Local-first / zero-knowledge: the medical record is encrypted client-side with AES-256-GCM before any network transit; the server stores only opaque encrypted blobs keyed by anonymous UUIDs and must never be able to read or decrypt them. E2E assertions must confirm the server sees only ciphertext.
   - Never weaken the cryptography; never log or persist plaintext medical data, encryption keys, or PII — including in test output and fixtures.
   - Ephemeral, patient-controlled access: access QR codes expire (~120s); the professional decrypts in RAM only and the session is wiped at the end or after inactivity. E2E tests must exercise expiry and wipe, not bypass them.
   - Data residency: data must stay on Ivorian soil to satisfy ARTCI / loi n°2013-450; tests must not route real data to foreign services.
   - Degraded-network resilience: must work offline (e.g. SQLCipher queue) and tolerate power/network cuts; the plaintext record stays <= 500 KB; heavy medical images are never stored on the patient device (only an ephemeral URL is embedded).
   - E2E tests must not create trust/access/residency bypasses just to pass.
   - Do not imply unimplemented behavior exists unless it is actually implemented.

6. Document how to run the e2e tests
   - Update nearby docs, test comments, or scripts when needed.
   - Clearly list external requirements such as a local backend instance, a device/web emulator, or a local object store.
   - Include exact commands for setup, execution, and cleanup.

7. Verify before finishing
   - Run the narrowest relevant e2e test first when practical.
   - Run the project's configured test gate (the command surfaced via `MX_AGENT_TEST_CMD`) plus any format, lint, and build checks the project defines.
   - HealthTech has not finalized its stack or test command yet (backlog #1). If no test command is configured, say so explicitly and recommend the exact commands to run once the stack lands — do not assume a particular language, build tool, or test runner, and do not invent a toolchain.
   - If a check cannot be run, explain why and recommend the exact command.

8. Final report
   - E2E target and scenario covered
   - Test infrastructure used
   - Files changed
   - Tests added or updated
   - Commands run and results
   - External requirements, if any
   - Bugs discovered, if any
   - Remaining gaps, flakes, risks, or follow-up recommendations

Important: focus on end-to-end coverage. Do not broaden product behavior beyond what is necessary to make the e2e scenario testable and safe.
