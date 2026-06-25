---
description: Resolve failing repository checks reported by the phased ADW test gate
argument-hint: "<failing-output-and-context>"
---
## HealthTech project context

<!-- Generated project context. Edit .adw/pack.profile.json and run `npm run pack:generate`; do not hand-edit this block in .adw/prompts. -->

### Repository context to read

- `PRD_HealthTech.md` — product requirements, personas, epics/user stories, security spec, and NFRs.
- `BACKLOG.md` — milestones M0–M4, issues #1–#31, dependency graph, and recommended implementation order.
- The specific GitHub issue being worked (`kortiene/HealthTech`) and its acceptance criteria.
- The existing source tree as it actually exists now. This project began greenfield; do not assume an app, backend, test command, or stack exists until you verify it on disk.

### Product summary

`HealthTech` is a decentralized, local-first / zero-knowledge digital health platform for Côte d'Ivoire, serving patients and health professionals. The patient carries their medical record on their smartphone and grants ephemeral, controlled access to professionals via a dynamic QR code, without depending on permanent internet connectivity. The cloud stores only opaque encrypted blobs keyed by anonymous UUIDs and must never read or decrypt medical data.

### Target component map

- Patient app: mobile-first, Android focus on entry-level devices; local account creation, master-key generation, record encryption, QR generation, zero-knowledge cloud backup, key recovery.
- Health-professional interface: web/PWA + mobile; QR scan, RAM-only decryption, note/prescription editing, end-of-session re-encryption + cloud sync + RAM wipe, offline queue.
- Zero-knowledge backend: minimal blob store (`PUT/GET /blob/{uuid}`), hosted on Ivorian soil; never sees plaintext or keys.
- Shared crypto core: AES-256-GCM authenticated encryption, master-key management, PBKDF2 key derivation/recovery.
- Infra/compliance: sovereign hosting, secrets management, CI/CD, ARTCI homologation artifacts.

### Security and privacy constraints

- Local-first / zero-knowledge is non-negotiable: the medical record is encrypted client-side with AES-256-GCM before any network transit.
- Never weaken cryptography; never log or persist plaintext medical data, encryption keys, or PII.
- Access is ephemeral and patient-controlled: QR codes expire around 120 seconds; professionals decrypt in RAM only; sessions are wiped on end/inactivity.
- Data residency: data must stay hosted on Ivorian soil to satisfy ARTCI / loi n°2013-450.
- Degraded-network resilience: tolerate power/network cuts; support offline queueing; plaintext record stays <= 500 KB; heavy medical images are never stored on the patient device, only ephemeral URLs.
- The master key is generated on-device and sealed in the hardware keystore; it is never exported in clear.

### Current status and assumptions

HealthTech started as a greenfield project. Verify the current state from the repository before acting. If the stack or test command is still unset, do not assume Cargo/Rust, npm, Flutter, backend language, object storage, or CI conventions. Work from the PRD, BACKLOG, and the specific issue.

### Working rules

- Identify the owning component before editing.
- Respect BACKLOG dependency order; crypto core (#10) and zero-knowledge backend (#9) gate much downstream work; the consultation loop (#16→#19) is core demonstrable value.
- Preserve the zero-knowledge boundary and local-first/offline-resilient design in every change.
- Keep changes focused, idiomatic, testable, and production-minded.
- Update relevant docs, ADRs, schemas, threat-model, or compliance artifacts when behavior changes.
- The orchestrator owns all git/gh operations; do not run git or gh yourself inside ADW phases.

### Verification guidance

Run the project's configured test gate when available (`MX_AGENT_TEST_CMD`) plus any format/lint/build checks the repo defines. If no test command is configured, state that clearly and recommend the exact command(s) to run once the stack lands; do not invent a toolchain.

The repository's test or verification gate is failing. Fix the failures.

Failing output and context (truncated):

$ARGUMENTS

## Instructions

- Investigate the failures and make the smallest correct change that fixes them.
- Fix the root cause in code, tests, config, or docs as appropriate.
- Do not weaken meaningful assertions, skip tests, remove coverage, or mask failures just to make the gate pass.
- Stay within the scope of the current change; do not start unrelated work.
- Preserve documented project constraints, especially security, privacy, data-handling, compatibility, and operational guarantees.
- Do not run git or forge CLI commands inside the ADW phased pipeline; the orchestrator owns those operations.

Run the failing gate again if safe and available. Report what failed, what changed, what was verified, and any remaining risk.
