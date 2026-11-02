#!/usr/bin/env bash
# Test-only S3-B1 policy; no provider credentials, no fallback on missing support.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]]
. /etc/os-release
[[ "$ID" == ubuntu && "$VERSION_ID" == 24.04 ]]
[[ "$(dpkg-query -W -f='${Version}' bubblewrap)" == 0.9.0-1ubuntu0.3 ]]
[[ "$(node --version)" == v22.23.4 ]]
[[ "$(/usr/bin/prlimit --version)" == 'prlimit from util-linux 2.39.3' ]]
backend="${SWITCHYARD_BWRAP:-/usr/bin/bwrap}"
[[ "$backend" = /* && -x "$backend" && ! -u "$backend" && ! -g "$backend" ]]
[[ "$("$backend" --version)" == 'bubblewrap 0.9.0' ]]
cmp /usr/bin/bwrap "$backend"
mkdir -p target/bridge-policy "${SWITCHYARD_CONFINEMENT_EVIDENCE:?set evidence directory}"
cc -Wall -Wextra -Werror -O2 -shared -fPIC tools/confinement-probe/bridge/lockdown.c -o target/bridge-policy/lockdown.so
cc -Wall -Wextra -Werror -O2 -DLOCKDOWN_TEST tools/confinement-probe/bridge/lockdown.c -pthread -o target/bridge-policy/lockdown-test
target/bridge-policy/lockdown-test | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/bridge-native.log"
export SWITCHYARD_PI_NODE="$(readlink -f "$(command -v node)")"
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
  sha256sum "$SWITCHYARD_PI_NODE" "$SWITCHYARD_BRIDGE_POLICY" tools/confinement-probe/bridge/*
  ldd "$SWITCHYARD_PI_NODE"
  for library in libc.so.6 libm.so.6 libdl.so.2 libpthread.so.0 librt.so.1 libstdc++.so.6 libgcc_s.so.1; do
    sha256sum "/lib/x86_64-linux-gnu/$library"
  done
  sha256sum /lib64/ld-linux-x86-64.so.2
  cat runtime/pi-source/.artifacts/provenance.json
} > "$SWITCHYARD_CONFINEMENT_EVIDENCE/bridge-environment.txt"
cargo fmt --all -- --check
cargo clippy -p switchyard-confinement-probe --all-targets --features bridge-confinement --locked -- -D warnings
# A harmless positive environment control exists in the Rust host, then the
# Node fixture asserts it was removed by default-deny environment construction.
SWITCHYARD_CREDENTIAL_SENTINEL=host-only-fixture cargo test -p switchyard-confinement-probe --test bridge_confinement --features bridge-confinement --locked -- --test-threads=1 --nocapture 2>&1 | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/bridge-tests.log"
