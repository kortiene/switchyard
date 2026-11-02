# Pinned Pi source candidate

This is the bounded source-build remedy for [S2 / #96](https://github.com/kortiene/switchyard/issues/96).
The published-1.4.0 NO-GO evidence stays in `../pi/`. This candidate is
**NEEDS-FULL-S2**, not an integration approval or a usable coding runtime.

This directory is disposable S2 proof-of-concept evidence, with no long-term
maintenance or TypeScript runtime compatibility promise. Rust remains the target
agent runtime; the legacy TypeScript engine stays frozen.

## Reproduce

Use Node **22.23.4** and npm **11.9.0**, then from this directory:

```sh
node build.mjs
npm ci
npm run verify
```

CI selects npm through `node install-npm.mjs`: it downloads the exact tarball
from `pins.json`, checks its committed SHA-512 before installation, disables
lifecycle scripts and verifies the installed version. An optional prefix path
argument installs into an isolated directory for local validation. This is a
byte-integrity check; independent signature verification is not claimed.

The build needs Git, curl, npm registry access, the pinned upstream commit and
the immutable pi.dev catalog URL on a catalog-cache miss. It fails on missing inputs, version mismatch,
changed source/lock/catalog, or artifact drift. It never falls back to `latest`,
the published Pi packages, a live catalog, or regenerated expected hashes.

`pins.json` fixes source commit `e65b3b88`, workspace version 1.4.0, the upstream lockfile SHA-256,
TypeScript 7.0.2, and all five output tarball hashes. The production workspace
dependency closure is telemetry, Chord, code mode, AI and Durable, all compiled
from that same unmodified commit. Building the code-mode dependency does not
enable its tools or execution capabilities in a future bridge.

Upstream's ignored model data is hydrated with its own offline hydrator from
the content-addressed catalog in `nix/model-catalog.json`. The bytes are checked
before hydration. No provider discovery or paid model request is made. This is
the upstream Nix build's data path, used without requiring Nix. Verified catalog
bytes are stored in `.catalog-cache/<catalogRevision>.json`; CI restores that
directory using the exact revision as its cache key. A warm cache avoids pi.dev
access. Cached bytes are hash-checked on every build, and corrupt entries fail
closed. A cold cache still requires pi.dev availability.

Build dependencies are installed with the unchanged upstream lock and lifecycle
scripts disabled. Runtime dependencies have a separate committed lockfile;
direct file dependencies and overrides prevent an internal Pi package from
silently resolving to a same-version registry binary. Build artifacts and
temporary checkouts are ignored. The script promotes artifacts only when every
hash matches, and emits `.artifacts/provenance.json`.
The source-cleanliness check rejects tracked changes and unexpected untracked
files; upstream-ignored build outputs remain allowed. This CI workflow pins all
Actions to full commit SHAs and applies the same path filters to PRs and main
pushes.

## Evidence and limits

An initial build and a second fresh-checkout build produced byte-identical
tarballs for all five packages. The `pi-source-candidate` CI job independently
rebuilds against those committed hashes on Ubuntu 24.04. Cross-platform artifact
reproducibility and independent verification of upstream signatures are not
claimed.

Four candidate tests verify:

- installed source maps match the reviewed admission source, and every internal
  Pi dependency comes from the hash-checked local artifacts;
- idle steering throws the exported `ConversationIdle`, admits nothing and makes
  zero model calls;
- submission admission and an application document commit or roll back together;
- the public continue API settles an empty context as `unanswered`.

These remove the demonstrated package/API mismatch. They do not prove the
Rust process bridge, durable payload conflicts, tool replay, cancellation,
transport limits, operating-system confinement, or real issue-to-PR acceptance.
S2 stays open, and Pi-dependent live work stays blocked until the full gate.

The dependent [process-bridge probe](../../docs/rust-first/s2-bridge-probe.md)
adds a feature-gated Rust supervisor and credential-free wire/replay tests.
Its extra tests are partial S2 evidence; the regular mission CLI is unchanged.

The [S2-A Host/outbox probe](../../docs/rust-first/s2-host-outbox.md) uses the same
pinned faux child with the real Rust ledger. It adds deterministic admission-gap
crashes and recovery tests; S2 remains NO-GO.

The [S2-B effect probe](../../docs/rust-first/s2-effect-outcomes.md) adds typed
effect outcomes and tests lost replies and uncertain effects against the real
Host simulated journal. It keeps the same product and S2 restrictions.

The [S2-C cancellation probe](../../docs/rust-first/s2-cancellation.md) prevents
concurrent duplicate aborts from cancelling later work and adds stop crash,
timeout and bounded request-pressure evidence. The complete S2 gate stays open.

The [S2-D storage/close probe](../../docs/rust-first/s2-storage-recovery.md) adds
SQLite write refusals, failed Rust acknowledgements and close/reopen races.
It fences close against unfinished calls and stops, and terminates the peer
on background storage failure. S2 remains NO-GO.
