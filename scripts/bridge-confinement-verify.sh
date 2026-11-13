#!/usr/bin/env bash
# Credential-free S3-B1 through B6 and composed evidence; no unsupported fallback.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]]
. /etc/os-release
[[ "$ID" == ubuntu && "$VERSION_ID" == 24.04 ]]
[[ "$(dpkg-query -W -f='${Version}' bubblewrap)" == 0.9.0-1ubuntu0.3 ]]
[[ "$(node --version)" == v22.23.4 ]]
[[ "$(/usr/bin/prlimit --version)" == 'prlimit from util-linux 2.39.3' ]]
# The confined suites mount this private copy, not the checkout path compiled
# into an older binary. The workflow installs it; a local run creates it here.
if [[ -z "${SWITCHYARD_PI_RUNTIME:-}" ]]; then
  root="$(mktemp -d "${RUNNER_TEMP:?set RUNNER_TEMP}/pi-runtime.XXXXXX")"
  chmod 0700 "$root"
  cp -a runtime/pi-source/. "$root/"
  find "$root" -xdev \( -type d -o -type f \) -exec chmod a-s,g-w,o-w {} +
  chmod 0700 "$root"
  SWITCHYARD_PI_RUNTIME="$(readlink -f "$root")"
fi
export SWITCHYARD_PI_RUNTIME
[[ "$SWITCHYARD_PI_RUNTIME" = /* && ! -L "$SWITCHYARD_PI_RUNTIME" ]]
[[ "$(stat -c %a "$SWITCHYARD_PI_RUNTIME")" == 700 ]]
checkout="$(readlink -f runtime/pi-source)"
[[ "$SWITCHYARD_PI_RUNTIME" != "$checkout" && "$SWITCHYARD_PI_RUNTIME" != "$checkout"/* ]]
repo="$(readlink -f "$PWD")"
[[ "$SWITCHYARD_PI_RUNTIME" != "$repo" && "$SWITCHYARD_PI_RUNTIME" != "$repo"/* ]]
backend="${SWITCHYARD_BWRAP:-/usr/bin/bwrap}"
[[ "$backend" = /* && -x "$backend" && ! -u "$backend" && ! -g "$backend" ]]
[[ "$("$backend" --version)" == 'bubblewrap 0.9.0' ]]
cmp /usr/bin/bwrap "$backend"
mkdir -p target/bridge-policy "${SWITCHYARD_CONFINEMENT_EVIDENCE:?set evidence directory}"
node scripts/runtime-root-digest.mjs "$SWITCHYARD_PI_RUNTIME" > target/bridge-policy/runtime-root-digest.txt
runtime_digest="$(tr -d '\n' < target/bridge-policy/runtime-root-digest.txt)"
[[ "$runtime_digest" == sha256:* ]]
# Never let a filtered/failed rerun reuse a preceding composed report.
rm -f "$SWITCHYARD_CONFINEMENT_EVIDENCE/s3-composed-acceptance.json" "$SWITCHYARD_CONFINEMENT_EVIDENCE/s3-composed-evidence.json"
cc -Wall -Wextra -Werror -O2 -shared -fPIC runtime/pi-source/confinement/lockdown.c -o target/bridge-policy/lockdown.so
cc -Wall -Wextra -Werror -O2 -DLOCKDOWN_TEST runtime/pi-source/confinement/lockdown.c -pthread -o target/bridge-policy/lockdown-test
cc -Wall -Wextra -Werror -O2 -DLOCKDOWN_ENDPOINT_TEST runtime/pi-source/confinement/lockdown.c -pthread -o target/bridge-policy/lockdown-endpoint-test
target/bridge-policy/lockdown-test | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/bridge-native.log"
# Unset keeps full socket denial. model-socket is the only other accepted mode.
if SWITCHYARD_BRIDGE_EGRESS=open target/bridge-policy/lockdown-test; then
  echo "unexpected egress mode must not install a filter" >&2
  exit 1
fi
SWITCHYARD_BRIDGE_EGRESS=model-socket target/bridge-policy/lockdown-endpoint-test | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/bridge-egress-native.log"
# Install a byte-identical private candidate: the shared CI tool cache may be
# group-writable and is not an acceptable trusted runtime installation.
install -m 0755 "$(readlink -f "$(command -v node)")" "$PWD/target/bridge-policy/node"
chmod 0755 target/bridge-policy/lockdown.so
export SWITCHYARD_PI_NODE="$PWD/target/bridge-policy/node"
export SWITCHYARD_BRIDGE_POLICY="$PWD/target/bridge-policy/lockdown.so"
{
  git rev-parse HEAD HEAD^{tree}
  # After the Pi rebuild and the policy compile above. Porcelain omits gitignored
  # build outputs (target/, runtime/pi-source artifacts), so those do not fail this.
  [[ -z "$(git status --porcelain)" ]] || {
    echo "dirty tree; refusing bridge confinement evidence" >&2
    git status --porcelain >&2
    exit 1
  }
  node --version
  rustc --version
  cc --version
  sha256sum "$SWITCHYARD_PI_NODE" "$SWITCHYARD_BRIDGE_POLICY" tools/confinement-probe/bridge/* runtime/pi-source/confinement/*
  ldd "$SWITCHYARD_PI_NODE"
  for library in libc.so.6 libm.so.6 libdl.so.2 libpthread.so.0 librt.so.1 libstdc++.so.6 libgcc_s.so.1; do
    sha256sum "/lib/x86_64-linux-gnu/$library"
  done
  sha256sum /lib64/ld-linux-x86-64.so.2
  cat runtime/pi-source/.artifacts/provenance.json
  printf 'runtime-root %s\nruntime-root-digest %s\n' "$SWITCHYARD_PI_RUNTIME" "$runtime_digest"
} > "$SWITCHYARD_CONFINEMENT_EVIDENCE/bridge-environment.txt"
cargo fmt --all -- --check
cargo clippy -p switchyard-confinement-probe --all-targets --features bridge-confinement --locked -- -D warnings
# A harmless positive environment control exists in the Rust host, then the
# Node fixture asserts it was removed by default-deny environment construction.
SWITCHYARD_CREDENTIAL_SENTINEL=host-only-fixture cargo test -p switchyard-confinement-probe --test bridge_confinement --features bridge-confinement --locked -- --test-threads=1 --nocapture 2>&1 | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/bridge-tests.log"
# S3-B2 composes both real boundaries through the public Host file-write mapper.
# The mapper unit cases also run here with both features; the S2-only job cannot
# compile this Linux-specific path. Missing cgroups/namespace support still fails.
cargo build -p switchyard --bin switchyard-execution-helper --features linux-execution --locked
export SWITCHYARD_EXECUTION_HELPER="$PWD/target/debug/switchyard-execution-helper"
cargo clippy -p switchyard-confinement-probe --all-targets --features pi-linux-process --locked -- -D warnings
cargo test -p switchyard-core --features 'pi-contract linux-execution' pi_linux_ --locked -- --nocapture 2>&1 | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/pi-linux-mapper.log"
cargo test -p switchyard-confinement-probe --test pi_linux_write --features pi-linux-write --locked -- --test-threads=1 --nocapture 2>&1 | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/pi-linux-write.log"

# S3-B3: process handles/status/cancel, operator stops, and process crash replay.
cargo test -p switchyard-confinement-probe --test pi_linux_process --features pi-linux-process --locked -- --test-threads=1 --nocapture 2>&1 | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/pi-linux-process.log"

# S3-B4: Rust-owned confined sessions; no product runtime/default change.
cargo build -p switchyard --bin switchyard-bridge-launcher --features "pi-contract linux-execution" --locked
chmod 0755 target/debug/switchyard-bridge-launcher
export SWITCHYARD_BRIDGE_LAUNCHER="$PWD/target/debug/switchyard-bridge-launcher"
cargo test -p switchyard-core --features "pi-contract linux-execution" confined_pi_ --locked -- --nocapture 2>&1 | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/bridge-storage.log"

# S3-B5/C: exercise Host composition before the long B4 wall-deadline case for
# fail-fast diagnostics. The full B4 lifecycle suite remains required below.
cargo test -p switchyard-confinement-probe --test pi_host_dispatch --features pi-linux-process --locked -- --test-threads=1 --nocapture 2>&1 | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/pi-host-dispatch.log"

# S3-B4 lifecycle, including the real wall deadline and retention/reopen cases.
cargo test -p switchyard-confinement-probe --test bridge_lifecycle --features pi-linux-process --locked -- --test-threads=1 --nocapture 2>&1 | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/bridge-lifecycle.log"

# S3-B6: one host-configured model endpoint through the session proxy.
# Full denial remains the path exercised by the suites above.
cargo test -p switchyard-confinement-probe --test bridge_egress --features pi-linux-process --locked -- --test-threads=1 --nocapture 2>&1 | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/bridge-egress.log"

# S3-C: the Host-driven suite above must include the composed scenario. These
# checks reject missing/filtered evidence and bind report/log/environment hashes.
# Collection remains PENDING_REVIEW, not a product-enable or S3-GO decision.
node --test scripts/s3-acceptance-check.test.mjs
node scripts/s3-acceptance-check.mjs

# Opt-in CLI flag. The confined binary runs the composed s3-acceptance scenario.
# Missing prerequisites fail the test; FakeRuntime is not a fallback.
cargo test -p switchyard --test cli_confined --features "pi-contract linux-execution" --locked -- --test-threads=1 --nocapture 2>&1 | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/cli-confined.log"
