# S3: Relocatable, digest-pinned confined Pi runtime

Refs #97, #94. This retires the compile-time `policy::source()` lookup that
resolved `runtime/pi-source` from `CARGO_MANIFEST_DIR`. An installed binary
could not see that checkout. The confined bridge now uses an explicit runtime
root on `ConfinedPiConfig`.

FakeRuntime remains the product and CLI default. There is no CLI flag, no
change to `Host::open`, and no new Cargo feature. Pi stays off unless a host
calls `Host::open_confined_pi`. Provider credentials, live models, TLS, and
production budgets stay out of scope. `test/bridge-child.ts` remains the
conformance entry. This is not S4–S8 or full M1.

## Host configuration

`ConfinedPiConfig.runtime` is an absolute path. The host must place it outside
workspaces and outside the mission control directory. Rust does not mount it
writable: every runtime bind is `--ro-bind`. The bridge never receives a
writable path to the install.

Before launch, Rust requires the path to be canonical. No component may be a
symlink, including the root itself. The root must be a directory owned by the
host uid with no group or world permission bits (mode `0700`). The root and
each required part must have no setuid, setgid, group-writable, or
world-writable bits. The required parts are:

- `src`
- `test/bridge-child.ts`
- `node_modules`
- `pins.json`
- `package.json`
- `package-lock.json`
- `confinement/guard.cjs`
- `.artifacts/provenance.json`

A missing, special, or unexpected part fails closed. Interior symlinks under a
real directory, such as npm's `node_modules/.bin` links, are not followed.
Their link text is part of the digest. A symlink that replaces one of the
required parts is refused.

## Pins and the runtime-root digest

The expected provenance is not a new compiled hash. `check` rebuilds the object
written by `runtime/pi-source/build.mjs` from the reviewed `pins.json` already
compiled into the crate:

- `sourceCommit`, `upstreamLockSha256`, `catalogRevision`, `node`, `npm`, and
  `packages` come from that pins file
- `decision` stays the build's fixed label `NEEDS-FULL-S2`

The installed `pins.json` must parse equal to those reviewed pins. The
installed `provenance.json` must parse equal to the rebuilt object. Either
mismatch returns before Node, Bubblewrap, or the bridge is executed.

The runtime-root digest is `sha256:` plus the SHA-256 of a manifest of those
parts. Each regular file is `f <relative-path> <sha256>\n`. Each interior
symlink is `l <relative-path> <target>\n`. Lines are sorted by the raw relative
path bytes. `scripts/runtime-root-digest.mjs` implements the same manifest for
the confinement evidence file. A unit test refuses a drift between the two.

`check` stores the canonical path and digest as `runtimeRoot` on the session
configuration. `ConfinedRuntime::configuration` stores the path as `runtime`
and the same digest as `runtimeRoot` on the attempt ledger. An opened session
still refuses a configuration change unless the previous stop is proven, never
reached `runtime.open`, and `state/` is empty. A later host open refuses a
different path or digest through the existing adapter-config comparison.
Replacing the install under an existing confined ledger therefore fails closed.

That fingerprint changes for every confined attempt. No product ledger uses
this adapter. Existing confined test ledgers will not reopen.

## TOCTOU

Validation and the first digest happen in `check`, before the bridge is
spawned. `argv` inspects the tree again and refuses to launch if the path or
digest changed. Bubblewrap then bind-mounts the canonical paths from that
second look, not the original strings. This narrows a swap between validation
and mount. It is not a claim that a concurrent root writer is impossible; the
root is host-private and not writable by the bridge.

## CI

`execution-confinement` copies the built `runtime/pi-source` tree to a mode
`0700` directory under `RUNNER_TEMP` and exports `SWITCHYARD_PI_RUNTIME`.
Every confined suite uses that variable. The evidence file
`bridge-environment.txt` records `runtime-root` and `runtime-root-digest`.
The checkout copy stays available for the policy compile. One host test makes
that checkout unreadable and still runs the `answer` and `process-hold`
fixtures from the relocated tree.

## Limits

This does not install a production bridge, select a model endpoint, or enable
Pi in the CLI. The native policy library remains `config.policy`. Workspace
confinement is unchanged. Reboot, power-loss, and a compromised host are not
claimed.
