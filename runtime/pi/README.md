# Pi Durable package audit (S2)

This directory is a **feasibility probe, not the runtime bridge**. The decision
for npm `@earendil-works/pi-durable@1.4.0` against the reviewed source baseline is
**NO-GO** (2026-10-29, merged in #106). See the [evidence and remedy](../../docs/rust-first/s2-feasibility.md).
The pinned source candidate merged in #107 lives in `../pi-source/` and is
`NEEDS-FULL-S2`, not an approval of this package. Draft #108 is not this
directory and is not on main. [#96](https://github.com/kortiene/switchyard/issues/96)
remains NO-GO until its complete gate passes. The mission CLI still uses S1's
fake runtime. There is no model-provider spend, workspace execution, GitHub
publication, or claim of confinement here.

Use exactly Node **22.23.4**, recorded in `.node-version`:

```bash
cd runtime/pi
npm ci
npm run verify
npm run --silent audit:package
npm run --silent gate
```

`verify` typechecks and tests the observed package behavior, including a real
SIGKILL/reopen test over Pi's SQLite storage. Those tests intentionally confirm
the incompatibilities and therefore can pass while the integration is NO-GO.
The crash probe uses a **passive Pi entry**, not Rust's outbox or a coding tool.

`audit:package` emits JSON evidence. `gate` emits the same evidence and exits
**1**: package inspection alone can never approve S2. The separate CI job is
named `pi-package-audit`; it is not the planned `pi-bridge-contract` check and
cannot unblock S3-S8. No acceptance checkbox is satisfied by its green result.

The lockfile fixes the package and every dependency by integrity. Install scripts
are disabled. Runtime probes import only public Pi APIs and explicitly configure
the faux provider. Source-map files are read only to compare the published
source with Git blob IDs from the two exact upstream commits; there are no
private runtime imports or Pi table queries.

Tests and the gate run without API keys. The package audit records the installed
Node version and rejects a different version in `verify`. It checks two relevant
source files, not byte-for-byte reproducibility of the whole upstream build or
independent verification of npm's provenance signatures.
