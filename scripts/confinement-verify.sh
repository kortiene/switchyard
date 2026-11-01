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
# S3-A4 reuses this job. Missing delegation fails the job; nothing is skipped.
cgroup="${SWITCHYARD_CGROUP_ROOT:-}"
if [[ -z "$cgroup" || ! -d "$cgroup" || ! -w "$cgroup" ]]; then
  echo "SWITCHYARD_CGROUP_ROOT is missing or not writable; refusing an unconfined run" >&2
  exit 1
fi
{
  echo "cgroup_root=$cgroup"
  echo "cgroup_subtree=$(cat "$cgroup/cgroup.subtree_control")"
  echo "cgroup_owner=$(stat -c '%U:%G' "$cgroup")"
  echo "boot_id=$(cat /proc/sys/kernel/random/boot_id)"
} | tee -a "$SWITCHYARD_CONFINEMENT_EVIDENCE/environment.txt"
cargo build -p switchyard --bin switchyard-execution-helper --features linux-execution --locked
cargo clippy -p switchyard-core -p switchyard --all-targets --features linux-execution --locked -- -D warnings
# S3-A4 execution and S3-A5 supersede tests. Missing delegation fails above;
# this suite does not skip or run unconfined.
cargo test -p switchyard --test linux_execution --features linux-execution --locked -- --test-threads=1 --nocapture 2>&1 | tee "$SWITCHYARD_CONFINEMENT_EVIDENCE/linux-execution.log"
