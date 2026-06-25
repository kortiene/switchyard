---
description: Prime the agent with HealthTech repository architecture and contribution rules
argument-hint: "[task/context]"
---
Prime yourself for working on the `HealthTech` platform before taking action.

Optional task/context from me: $ARGUMENTS

First, read and internalize the repository context:
- `PRD_HealthTech.md` (product requirements: vision, personas, epics/user stories, security spec, NFRs)
- `BACKLOG.md` (milestones M0–M4, epics, GitHub issues #1–#31, dependency graph, recommended implementation order)
- the specific GitHub issue being worked (kortiene/HealthTech) and its acceptance criteria
- the existing application source tree for the requested task — note: this is a GREENFIELD project, so no application code exists yet (the stack itself is undecided, backlog issue #1). If a source tree has since landed, read the relevant packages and files for the task.

Project summary:
`HealthTech` is a decentralized, local-first / zero-knowledge digital health platform for Côte d'Ivoire, serving patients and health professionals. The patient carries their medical record in their smartphone and grants ephemeral, controlled access to professionals via a dynamic QR code, without depending on a permanent internet connection. The cloud stores only opaque encrypted blobs keyed by anonymous UUIDs and can never read or decrypt them.

Target component map (per the PRD/BACKLOG; not yet built):
- Patient app (mobile-first, Android focus on entry-level devices): local account creation, master-key generation, record encryption, QR generation, zero-knowledge cloud backup, key recovery.
- Health-professional interface (web & mobile): QR scan, RAM-only decryption, note/prescription editing, end-of-session re-encryption + cloud sync + RAM wipe, offline queue.
- Zero-knowledge backend: minimal blob store (`PUT/GET /blob/{uuid}`), hosted on Ivorian soil; never sees plaintext or keys.
- Shared crypto core: AES-256-GCM authenticated encryption, master-key management, PBKDF2 key derivation/recovery.
- Infra & compliance: sovereign hosting, secrets management, CI/CD, ARTCI homologation artifacts.

Architecture and security constraints:
- Local-first / zero-knowledge: the patient's medical record is encrypted CLIENT-SIDE with AES-256-GCM before any network transit. The server stores only opaque encrypted blobs keyed by anonymous UUIDs and must never be able to read or decrypt them.
- Never weaken the cryptography; never log or persist plaintext medical data, encryption keys, or PII.
- Ephemeral, patient-controlled access: access QR codes expire (~120s); a professional decrypts the record in RAM only and the session is wiped at the end (or after inactivity).
- Data residency: data must stay hosted on Ivorian soil to satisfy ARTCI / loi n°2013-450.
- Degraded-network resilience: must work offline (e.g. SQLCipher queue) and tolerate power/network cuts; the plaintext record stays <= 500 KB; heavy medical images are never stored on the patient device (only an ephemeral URL is embedded).
- The master key is generated on-device and sealed in the hardware keystore; it is never exported in clear.

Current status to preserve:
This is a greenfield project: only the PRD and BACKLOG exist, and the technical stack (patient-app framework, professional interface, backend language, object storage, build tool, test command) has NOT been chosen yet (backlog issue #1). Do not assume a particular language, build tool, or test command, and do not imply that any feature, app, or backend exists yet — none has been implemented. Work against the PRD, the BACKLOG, and the specific issue being worked.

Working rules:
- Identify the owning component (patient app / professional interface / backend / crypto core / infra) and any existing patterns before editing.
- Keep changes focused, idiomatic, and testable.
- Respect the dependency order in `BACKLOG.md` — the crypto core (#10) and zero-knowledge backend (#9) gate most downstream work; the consultation loop (#16→#19) is the core demonstrable value.
- Preserve the zero-knowledge boundary and the local-first / offline-resilient design in every change.
- Avoid broad rewrites unless explicitly requested.
- Update the relevant docs (ADRs, schema, compliance/threat-model artifacts) when behavior changes.
- The orchestrator owns ALL git/gh operations; do not run git or gh yourself.

Before finalizing code changes, run or clearly recommend:
- The project's configured test gate (the command surfaced via `MX_AGENT_TEST_CMD`) plus any format/lint/build checks the project defines.
- Note: HealthTech has NOT finalized its stack or test command yet (backlog #1). If no test command is configured, say so explicitly and recommend the exact command(s) to run once the stack lands — do NOT assume a particular toolchain (no Cargo/Rust, no npm) or invent one.

After reading the relevant files, summarize the repository context in a few bullets, identify the likely component(s) involved in the task/context above, and propose a short plan before making code changes.
