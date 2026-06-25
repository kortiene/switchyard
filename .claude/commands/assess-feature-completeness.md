---
description: Assess HealthTech feature completeness against the PRD, the BACKLOG milestones, code, and GitHub issues
argument-hint: "[focus area or milestone target]"
---
Think hard and perform a thorough repository assessment of `HealthTech`.

Optional focus area or milestone target from me: $ARGUMENTS

Read all files needed to accurately evaluate how close the project is to being fully feature-complete relative to its PRD (product vision, epics, NFRs), its BACKLOG milestones (M0–M4), and the GitHub issue state (#1–#31). This is a greenfield, local-first / zero-knowledge digital health platform for Côte d'Ivoire — do not assume any language, build tool, or test command has been chosen yet.

Start by reviewing at minimum:

- `PRD_HealthTech.md` (vision, personas, epics E0–E7, functional specs, NFRs, crypto/security constraints)
- `BACKLOG.md` (milestones M0–M4, issues #1–#31, dependency map, critical path, implementation order)
- the specific GitHub issue(s) relevant to your focus area
- the (greenfield) source tree as it actually exists on disk — the BACKLOG plans a monorepo of `/app-patient`, `/app-medecin`, `/backend`, `/crypto-core`, `/infra`, `/docs/adr/`, but verify which of these exist and contain real code vs. are still empty:
  - patient app (mobile-first, Android entry-level target)
  - health-professional interface (web/PWA + mobile)
  - backend zero-knowledge blob service
  - shared crypto core (AES-256-GCM, key derivation/recovery)
  - infrastructure / sovereign hosting config
  - ADRs and compliance docs

Also inspect GitHub issue state (issues #1–#31) as needed, including recently completed work and remaining open issues. The orchestrator owns all git/gh; do NOT run git or gh yourself — work from the issue text already provided to you and from what is on disk.

Evaluate feature completeness across these areas (mapped to the PRD epics and BACKLOG milestones):

1. Stack decision & ADRs (#1) — is the stack actually chosen and documented?
2. Monorepo structure & build conventions (#2)
3. CI/CD: lint, unit tests, app/backend build, dependency scan (#3)
4. Environments & secret management, IaC (#4)
5. Loi n°2013-450 / ARTCI compliance mapping (#5)
6. Threat model (STRIDE) & security policy (#6)
7. Consent flow & patient legal journey (#7)
8. Sovereign hosting provisioned on Ivorian soil (#8)
9. Zero-knowledge blob storage service: PUT/GET by anonymous UUID (#9)
10. AES-256-GCM crypto module + test vectors (#10)
11. Local master-key generation & hardware keystore sealing (#11)
12. Key derivation & recovery (PBKDF2 + culturally-adapted questions) (#12)
13. Encrypted account creation / patient onboarding (#13)
14. Zero-knowledge cloud backup of the record (#14)
15. Medical-record schema & the ≤ 500 KB plaintext budget (#15)
16. Temporary QR-code generation (URL + symmetric key, ~120s expiry) (#16)
17. QR scan + RAM-only decryption (no plaintext to disk) (#17)
18. Note/prescription editing & in-RAM merge (#18)
19. End-of-session re-encryption, cloud push & RAM wipe (#19)
20. End-to-end consultation-loop demo / integration test (#20)
21. Offline secure queue (SQLCipher) (#21)
22. Sync on network return / conflict handling (#22)
23. Heavy medical images off-device + ephemeral URL (#23)
24. Degraded-network (Edge/3G) optimization (#24)
25. Security audit / external pentest (#25)
26. Independent cryptographic review (#26)
27. Performance validation (decrypt+display < 3 s on 3G) (#27)
28. Doctor UX polish (usable in < 5 min, no training) (#28)
29. Accessibility & robustness on entry-level phones (#29)
30. ARTCI homologation dossier (#30)
31. Abidjan field pilot / beta (#31)

For each area, report:

- status: complete / partial / missing
- evidence from files or the issue text
- gaps or risks
- security implications (this is a zero-knowledge health platform — weigh every gap against the no-plaintext, client-side-only-crypto invariant)
- recommended next work
- whether the PRD/BACKLOG accurately reflect what is actually implemented

Important constraints:

- Do not assume behavior exists just because the PRD or BACKLOG describes it. This is greenfield: most of #1–#31 are likely unstarted.
- Distinguish implemented behavior from placeholders, stubs, and docs-only intent.
- Preserve and treat as non-negotiable the HealthTech security model when judging completeness:
  - Patient records are encrypted CLIENT-SIDE (AES-256-GCM) before any transit.
  - The server stores only opaque encrypted blobs keyed by anonymous UUIDs and can never decrypt them.
  - Ephemeral patient-controlled access: QR codes expire (~120s); the professional decrypts in RAM only, wiped at session end.
  - Data residency on Ivorian soil (ARTCI / loi n°2013-450); offline-resilient; record ≤ 500 KB plaintext.
  - No heavy medical images on the patient device — only an ephemeral URL in the text record.
  - Never weaken crypto; never log or persist plaintext medical data, keys, or PII.
- Respect the milestone dependency chain: M0 → M1 → M2 → M3 → M4. A later milestone cannot be "complete" if its blockers (e.g. #10, #9) are not.
- Do not make code changes unless explicitly asked.

End with:

- overall feature-completeness estimate (per milestone M0–M4, then overall)
- top blockers to feature complete (anchor to the critical path: #1 → #6 → #10 → #9 → #14 → #16 → #17 → #19 → #21 → #25 → #30 → #31)
- recommended GitHub issues to file or update (describe them; the orchestrator will create them)
- recommended validation commands:
  - run the configured project test gate (`MX_AGENT_TEST_CMD`) plus any project lint/format/build checks
  - if no stack/test tooling has been chosen yet (#1), say so explicitly and recommend that the test/lint/build/CI commands be defined once the stack lands (#1, #3) rather than assuming any specific toolchain
