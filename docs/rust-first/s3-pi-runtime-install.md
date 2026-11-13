# S3: Relocatable confined Pi runtime install

Refs #97, #94. This retires the compile-time `policy::source()` lookup that
resolved `runtime/pi-source` from `CARGO_MANIFEST_DIR`. An installed binary
could not see that checkout. The confined bridge now uses an explicit runtime
root on `ConfinedPiConfig`.

FakeRuntime remains the product and CLI default. `Host::open` is unchanged,
and there is no new Cargo feature. Pi is never selected implicitly. The CLI
flag `--runtime confined-pi` is the explicit opt-in: it calls
`Host::open_confined_pi` with the credential-free `s3-acceptance` fixture (the
composed scenario) and does not set a model endpoint. Linux confinement
prerequisites (`SWITCHYARD_PI_RUNTIME`, `SWITCHYARD_BWRAP`,
`SWITCHYARD_CGROUP_ROOT`, `SWITCHYARD_PI_NODE`, `SWITCHYARD_BRIDGE_LAUNCHER`,
`SWITCHYARD_BRIDGE_POLICY`, and `SWITCHYARD_EXECUTION_HELPER`) are required;
a missing one fails closed and does not fall back to FakeRuntime. Provider
credentials, live models, TLS, and production budgets stay out of scope.
`test/bridge-child.ts` remains the conformance entry. This is not S4–S8 or
full M1.

The command that runs the composed scenario, after `mission create` and
`mission workspace` were opened with the same flag and the attempt is in
`awaiting_plan_approval`, is:

```text
switchyard mission command --runtime confined-pi --db <db> --mission <id> --kind approve_plan --expected-revision <rev> --payload '{"planDigest":"<digest>"}'
```

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

## What is pinned before launch

Two different checks run before Node or Bubblewrap is executed.

Compile-time bytes, via `include_bytes!` in `policy.rs`. The installed file's
SHA-256 must match those bytes, and `src/` must be exactly this set:

- `src/forwarders.ts`, `src/protocol.ts`, `src/runtime.ts`, `src/transport.ts`
- `test/bridge-child.ts`
- `package.json`
- `package-lock.json`
- `confinement/guard.cjs`

`pins.json` must parse equal to the reviewed pins compiled into the crate.
A mismatch in any of these returns before a runtime binary is started.

`provenance.json` is not an independent build proof. `check` rebuilds the
object `build.mjs` writes from those pins (`sourceCommit`,
`upstreamLockSha256`, `catalogRevision`, `node`, `npm`, `packages`, plus the
build's fixed `decision` label `NEEDS-FULL-S2`) and requires the installed
file to parse equal to it. That catches a provenance file that disagrees with
the pins. It does not prove `node_modules` was produced by that build.

`node_modules` is not compile-time pinned. Its bytes are part of the
runtime-root digest below, so the attempt ledger refuses a later swap. A first
open of a fresh ledger therefore accepts whatever `node_modules` is present,
then records that digest.

Accepted known gap: confined Pi runtime node_modules are not verified against the lockfile.
The runtime-root digest records the tree present at first open and refuses a later
swap, but that digest does not prove the tree matches `package-lock.json`
(#161 follow-up, #97).

## Runtime-root digest

The runtime-root digest is `sha256:` plus the SHA-256 of a manifest of the
required parts. Each regular file is `f <relative-path> <sha256>\n`. Each
interior symlink is `l <relative-path> <target>\n`. Lines are sorted by the
raw relative path bytes. `scripts/runtime-root-digest.mjs` implements the same
manifest for the confinement evidence file. A unit test refuses a drift
between the two.

`check` stores the canonical path and digest as `runtimeRoot` on the session
configuration. `ConfinedRuntime::configuration` stores the path as `runtime`
and the same digest as `runtimeRoot` on the attempt ledger. On a later open,
a digest that does not match the one stored at host open is refused
immediately after `check`, before a recovery file is written and before
cgroup retention runs. An opened session still refuses a configuration change
unless the previous stop is proven, never reached `runtime.open`, and `state/`
is empty. A later host open also refuses a different path or digest through
the existing adapter-config comparison.

That fingerprint changes for every confined attempt. FakeRuntime ledgers do
not use this adapter. Existing confined test ledgers will not reopen.

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
Every confined suite uses that variable. The same job runs
`cargo test -p switchyard --test cli_confined` with `pi-contract` and
`linux-execution`, which drives the composed `s3-acceptance` scenario through
the CLI flag. The evidence file
`bridge-environment.txt` records `runtime-root` and `runtime-root-digest`.
The checkout copy stays available for the policy compile. One host test makes
that checkout unreadable and still runs the `answer` and `process-hold`
fixtures from the relocated tree.

## Limits

This does not install a production bridge or select a model endpoint. The CLI
selects this runtime only with `--runtime confined-pi`; FakeRuntime remains
the default. The native policy library remains `config.policy`. Workspace
confinement is unchanged. Reboot, power-loss, and a compromised host are not
claimed.
