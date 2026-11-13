---
type: Testing Reference
title: Testing and CI
description: The focused-test workflow, canonical verification chain, coverage policy, and CI execution matrix.
tags: [reference, testing, coverage, ci, verification]
timestamp: "2026-11-13T12:18:33Z"
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

The script checks formatting, clippy, and `cargo test` with the committed Rust 1.99.1 toolchain. This is the default-feature local gate. CI runs it as the `rust-core` job within `verify.yml`; `rust-core` is not currently required by the rulesets. The required `pi-bridge-contract` job runs workspace formatting, Clippy and tests with `pi-contract`; the separate required `execution-confinement` job builds and tests Linux execution and the confined bridge lifecycle. Legacy `npm run verify` remains separate.

# Pi package audit (published-package NO-GO)

The separate `pi-package-audit` job uses pinned Node 22.23.4 on Ubuntu 24.04 to
reproduce the [S2 package/API incompatibilities](../../docs/rust-first/s2-feasibility.md).
Its five probes and typecheck can pass while the published-package decision remains NO-GO.
`npm run gate` in `runtime/pi/` exits 1. The diagnostic job is not the full
`pi-bridge-contract` check and grants no downstream live-execution readiness.
The [audit package](../../runtime/pi/README.md) documents commands and limits.

# Pinned source candidate

[#107](https://github.com/kortiene/switchyard/pull/107) added `pi-source-candidate`. It rebuilds `runtime/pi-source/` when that path, its workflow, or `docs/rust-first/` changes. The job is not `pi-bridge-contract`. S2 GO for the source-built candidate was recorded on 2026-11-02; see [the acceptance record](../../docs/rust-first/s2-contract-gate.md). A green audit or source rebuild is not live execution and is not S2 acceptance.

# Partial bridge probe

The `pi-bridge-probe` job runs the feature-gated Rust transport and credential-free
recovery probes against the rebuilt source candidate. See the
[probe scope](../../docs/rust-first/s2-bridge-probe.md).
It does not replace `pi-bridge-contract` or enable live execution.

# Full contract and Linux confinement

After rebuilding/installing the pinned source, run `node runtime/pi-source/contract.mjs` from the repository root. This verifies the full credential-free S2 matrix and writes revision-bound evidence.

On the declared Ubuntu 24.04 x86_64 runner, `scripts/confinement-verify.sh` and `scripts/bridge-confinement-verify.sh` exercise real OS boundaries and recovery. They require the pinned backend, namespace support and writable delegated cgroups; missing support fails. The bridge script also runs the model-endpoint native filter checks and `bridge_egress`. See [bridge lifecycle](../../docs/rust-first/s3-bridge-lifecycle.md) and [model-endpoint egress](../../docs/rust-first/s3-model-egress.md).

A read-only ruleset audit on 2026-11-02 confirmed ruleset 26316769 requires `execution-confinement` and that `pi-bridge-contract` was not yet required. A later read-only ruleset audit on 2026-11-03 confirmed `pi-bridge-contract`, `execution-confinement`, `verify (node 20.19.0)` and `verify (node 22)` are required on main. Rulesets 26316513 and 26316769 enforce them; no status names are changed by S3-B4. These checks do not grant full S3 or live-provider acceptance.

# Check inventory

Each CI check maps to a local command; local and CI agree on what the check proves. Required status reflects the 2026-11-03 read-only ruleset audit recorded above.

| CI check (workflow) | Local command | What it proves | Required on main |
| --- | --- | --- | --- |
| `verify (node 20.19.0)`, `verify (node 22)` (`verify.yml`) | `cd adw_sdlc && npm ci && npm run verify` | Frozen TypeScript reference gate: typecheck, environment lint, prompt pack/mirror drift, wiki check, coverage thresholds, build. | Yes (rulesets 26316513 and 26316769) |
| `rust-core` (`verify.yml`) | `./scripts/rust-verify.sh` | Default-feature Rust gate: formatting, Clippy, and `cargo test` on the committed Rust 1.99.1 toolchain. | No |
| `pi-bridge-contract` (`pi-bridge-contract.yml`) | Workspace fmt/Clippy/`cargo test` with `pi-contract`, then `node runtime/pi-source/contract.mjs` | Full credential-free S2 contract matrix with revision-bound evidence. | Yes |
| `execution-confinement` (`execution-confinement.yml`) | `./scripts/confinement-verify.sh` and `./scripts/bridge-confinement-verify.sh` | Real OS confinement, bridge lifecycle, and model-endpoint egress on the declared Ubuntu 24.04 x86_64 runner. | Yes |
| `pi-package-audit` (`pi-package-audit.yml`) | `cd runtime/pi && npm ci && npm run verify && npm run audit:package` | Published-package diagnostic; reproduces the historical NO-GO evidence. Not `pi-bridge-contract`. | No (diagnostic) |
| `pi-source-candidate` (`pi-source-candidate.yml`) | `cd runtime/pi-source && node install-npm.mjs && node build.mjs && npm ci && npm run verify` | Reproducible rebuild of the pinned source candidate. | No |
| `pi-bridge-probe` (`pi-bridge-probe.yml`) | Feature-gated Rust transport and credential-free recovery probes | Partial probe scope only; does not replace `pi-bridge-contract`. | No |

All `uses:` references in `.github/workflows/` are pinned to immutable commit SHAs with human-readable version comments (C6, #115). `dtolnay/rust-toolchain` publishes `stable` as a branch, not a tag; its pin records the branch head and the resolution date in the comment. Re-resolve pins deliberately; do not follow movable tags.

# Wiki validation policy

OKF consumers must tolerate missing links. Switchyard's producer-side `wiki:check` deliberately uses a stricter policy and rejects broken local concept and repository-source links before changes enter the repository. External URLs and heading anchors are not fetched or validated.

# Citations

[1] [Package scripts](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/package.json)

[2] [Vitest and coverage configuration](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/vitest.config.ts)

[3] [GitHub Actions verification workflow](https://github.com/kortiene/switchyard/blob/main/.github/workflows/verify.yml)

[4] [Rust core gate](../../scripts/rust-verify.sh)

[5] [Verification script invariants](https://github.com/kortiene/switchyard/blob/main/adw_sdlc/test/scaffold.test.ts)
