---
description: Review the working-tree implementation in the phased ADW pipeline
argument-hint: "<spec-file-or-empty> <issue-and-change-context>"
---
Review the implementation currently in the working tree for this change. There is no pull
request yet — review the staged and uncommitted changes against the issue and, if one was
created, the spec.

Spec file, if any: $1

Issue and change context:

${@:2}

## What to do

1. Understand the change.
   - Inspect the working-tree diff against the base branch and read the changed files in
     context, not just the diff.
   - Read the issue/acceptance criteria and the spec (`$1`) when provided; treat them as the
     definition of done. For HealthTech background, consult `PRD_HealthTech.md`, `BACKLOG.md`,
     and the specific GitHub issue being worked. The application source tree may not exist yet
     (the stack is still being chosen in backlog #1) — review whatever artifacts the change
     touches.

2. Review for quality and correctness.
   - Correctness bugs, missing error handling, weak or missing tests, untested edge cases.
   - Scope control: the change should not exceed what the issue/spec asked for.
   - Docs updated when behavior changes; new public APIs documented.

3. Check HealthTech constraints.
   - Local-first / zero-knowledge: the patient's medical record must be encrypted client-side
     with AES-256-GCM before any network transit; the server stores only opaque encrypted blobs
     keyed by anonymous UUIDs and must never be able to read or decrypt them.
   - The cryptography is never weakened; plaintext medical data, encryption keys, and PII are
     never logged or persisted.
   - Ephemeral, patient-controlled access: access QR codes expire (~120s); a professional
     decrypts the record in RAM only and the session is wiped at the end (or after inactivity).
   - Data residency: data stays hosted on Ivorian soil to satisfy ARTCI / loi n°2013-450.
   - Degraded-network resilience: works offline (e.g. SQLCipher queue) and tolerates
     power/network cuts; the plaintext record stays <= 500 KB; heavy medical images are never
     stored on the patient device (only an ephemeral URL is embedded).

4. Grade every finding by severity:
   - `blocker` — must be fixed before merge. A later `patch` phase auto-resolves these.
   - `tech_debt` — should be addressed but is not blocking. Reported, not auto-fixed.
   - `skippable` — minor or nit. Reported only.

5. Author the release text.
   - This is the final authoring phase for most runs, so write a high-quality commit message
     (`commit_message.txt`) and PR body (`pr_body.md`) (see the output instructions below)
     describing the change, the tests/checks run, and any security considerations.
   - For tests/checks: run the project's configured test gate (the command surfaced via
     `MX_AGENT_TEST_CMD`) plus any format/lint/build checks the project defines. HealthTech has
     not finalized its stack or test command yet (backlog #1) — if no test command is
     configured, say so explicitly and recommend the exact command to run once the stack lands;
     do not assume a toolchain or invent one.

Do not modify code in this phase — only report findings; the `patch` phase fixes blockers.
