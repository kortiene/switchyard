#!/usr/bin/env bash
# Explicit Linux gate: never skip, remove isolation, or run a fallback.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]]
. /etc/os-release
[[ "$ID" == ubuntu && "$VERSION_ID" == 24.04 ]]
[[ "$(dpkg-query -W -f='${Version}' bubblewrap)" == 0.9.0-1ubuntu0.3 ]]
backend="${SWITCHYARD_BWRAP:-/usr/bin/bwrap}"
[[ "$backend" = /* && -x "$backend" && ! -u "$backend" && ! -g "$backend" ]]
[[ "$("$backend" --version)" == 'bubblewrap 0.9.0' ]]
# A copied backend must be byte-identical to the pinned distro binary.
cmp /usr/bin/bwrap "$backend"
mkdir -p "${SWITCHYARD_CONFINEMENT_EVIDENCE:?set evidence directory}"
{
  uname -a
  cat /etc/os-release
  dpkg-query -W bubblewrap apparmor util-linux libc6
  sha256sum /usr/bin/bwrap "$backend" /usr/bin/prlimit /usr/bin/setsid
  rustc --version
  git rev-parse HEAD
  git status --short
  printf 'runner_image=%s\nrunner_image_version=%s\n' "${ImageOS:-unknown}" "${ImageVersion:-unknown}"
  for setting in /proc/sys/kernel/apparmor_restrict_unprivileged_userns /proc/sys/user/max_user_namespaces; do
    if [[ -r "$setting" ]]; then
      printf '%s=%s\n' "$setting" "$(cat "$setting")"
    else
      printf '%s=unreadable\n' "$setting"
    fi
  done
  cat tools/confinement-probe/ci/bwrap.apparmor
} | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/environment.txt"
cargo fmt --all -- --check
cargo clippy -p switchyard-confinement-probe --all-targets --features linux-confinement --locked -- -D warnings
cargo test -p switchyard-confinement-probe --features linux-confinement --locked -- --test-threads=1 --nocapture 2>&1 | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/tests.log"
