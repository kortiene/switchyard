---
type: Testing Reference
title: Testing and CI
description: The focused-test workflow, canonical verification chain, coverage policy, and CI execution matrix.
tags: [reference, testing, coverage, ci, verification]
timestamp: "2026-10-30T22:00:00Z"
---

# Canonical gate

From `adw_sdlc/`, run:

```bash
npm run verify
```

The chain is fail-fast:

```text
typecheck → environment lint → prompt-pack check → prompt-mirror check
          → wiki check → coverage/tests → build → remove dist
```

| Command | Purpose |
| --- | --- |
| `npm run typecheck` | Strict TypeScript checking without emit. |
| `npm run lint:env` | Static enforcement of runner secret-boundary rules. |
| `npm run pack:check` | Reject generated runtime prompt drift. |
| `npm run mirror:check` | Reject drift between neutral Pi prompts and Claude command mirrors. |
| `npm run wiki:check` | Validate OKF structure and Switchyard-strict local links. |
| `npm run coverage` | Run Vitest with V8 coverage and enforce thresholds. |
| `npm run build` | Compile production source; `verify` removes `dist/` afterward. |

# Development loop

Run the narrowest affected tests first:

```bash
npx vitest run test/<name>.test.ts
```

Use `npm test` for a coverage-free full suite. Finish broad or risky changes with `npm run verify`; live ADW runs use the same command through `ADW_TEST_CMD="npm run verify"`.

# Coverage and CI

Coverage measures all `src/**/*.ts` except the public barrel and declaration files. Current enforced floors are 85% statements, 73% branches, 82% functions, and 85% lines.

GitHub Actions checks the same `npm run verify` command on Node `20.19.0` (the package engine floor) and the Node 22 line. CI does not duplicate the gate stages; the package script remains the TypeScript entry point.

# Rust core gate

S1 adds a separate Rust gate. From the repository root:

```bash
./scripts/rust-verify.sh
```

The script checks formatting, clippy, and `cargo test` with the committed Rust 1.99.1 toolchain. CI runs it as the stable job name `rust-core` on `ubuntu-24.04`. It does not replace `npm run verify`. Adding the workflow job does not make `rust-core` a required status check; a maintainer has to add that exact name to branch protection or rulesets.

# Pi package audit (S2 NO-GO)

The separate `pi-package-audit` job uses pinned Node 22.23.4 on Ubuntu 24.04 to
reproduce the [S2 package/API incompatibilities](../../docs/rust-first/s2-feasibility.md).
Its five probes and typecheck can pass while the integration remains NO-GO.
`npm run gate` in `runtime/pi/` exits 1. The diagnostic job is not the planned
`pi-bridge-contract` check and grants no downstream live-execution readiness.
The [audit package](../../runtime/pi/README.md) documents commands and limits.

# Pinned source candidate

[#107](https://github.com/kortiene/switchyard/pull/107) added `pi-source-candidate`. It rebuilds `runtime/pi-source/` when that path, its workflow, or `docs/rust-first/` changes. The job is not `pi-bridge-contract`. S2 is NO-GO; see [#96](https://github.com/kortiene/switchyard/issues/96) and the [S2 feasibility audit](../../docs/rust-first/s2-feasibility.md). A green audit or source rebuild is not live execution and is not S2 acceptance.

# Wiki validation policy

OKF consumers must tolerate missing links. Switchyard's producer-side `wiki:check` deliberately uses a stricter policy and rejects broken local concept and repository-source links before changes enter the repository. External URLs and heading anchors are not fetched or validated.

# Citations

[1] [Package scripts](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/package.json)

[2] [Vitest and coverage configuration](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/vitest.config.ts)

[3] [GitHub Actions verification workflow](https://github.com/kortiene/switchyard/blob/main/.github/workflows/verify.yml)

[4] [Rust core gate](../../scripts/rust-verify.sh)

[5] [Verification script invariants](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/test/scaffold.test.ts)
