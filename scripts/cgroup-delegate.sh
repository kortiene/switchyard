#!/usr/bin/env bash
# Root-only setup for the S3-A4 delegated cgroup v2 subtree.
# The Host, Bubblewrap, and workloads stay unprivileged. If this cannot
# delegate cpu, memory, and pids, it fails. There is no rlimit-only fallback.
set -euo pipefail
if [[ "$(id -u)" -ne 0 ]]; then
  echo "cgroup delegation setup must run as root" >&2
  exit 1
fi
if [[ ! -f /sys/fs/cgroup/cgroup.controllers ]]; then
  echo "cgroup v2 is not mounted at /sys/fs/cgroup" >&2
  exit 1
fi
controllers="$(cat /sys/fs/cgroup/cgroup.controllers)"
for controller in cpu memory pids; do
  if ! grep -Eq "(^| )${controller}( |$)" <<<"$controllers"; then
    echo "cgroup v2 controller ${controller} is unavailable" >&2
    exit 1
  fi
done
subtree="$(cat /sys/fs/cgroup/cgroup.subtree_control)"
for controller in cpu memory pids; do
  if ! grep -Eq "(^| )${controller}( |$)" <<<"$subtree"; then
    mkdir -p /sys/fs/cgroup/init
    while read -r pid; do
      [[ -n "$pid" ]] || continue
      echo "$pid" >/sys/fs/cgroup/init/cgroup.procs 2>/dev/null || true
    done </sys/fs/cgroup/cgroup.procs
    echo '+cpu +memory +pids' >/sys/fs/cgroup/cgroup.subtree_control
  fi
done
delegate=/sys/fs/cgroup/switchyard
mkdir -p "$delegate"
if [[ -s "$delegate/cgroup.procs" ]]; then
  echo "refusing to delegate ${delegate}: it already contains processes" >&2
  exit 1
fi
echo '+cpu +memory +pids' >"$delegate/cgroup.subtree_control"
owner="${SUDO_USER:-}"
if [[ -z "$owner" || "$owner" == root ]]; then
  echo "SUDO_USER must be the unprivileged runner" >&2
  exit 1
fi
chown "$owner:$owner" "$delegate"
group="$(id -gn "$owner")"
chown "$owner:$group" "$delegate"
# The runner creates the leaves. Confirm that account can do so before CI
# continues as that user.
proof="$(sudo -u "$owner" -H mktemp -d "$delegate/probe-XXXXXX")"
sudo -u "$owner" -H tee "$proof/memory.max" <<<"268435456" >/dev/null
sudo -u "$owner" -H tee "$proof/memory.swap.max" <<<"0" >/dev/null
sudo -u "$owner" -H tee "$proof/memory.oom.group" <<<"1" >/dev/null
sudo -u "$owner" -H tee "$proof/pids.max" <<<"64" >/dev/null
sudo -u "$owner" -H tee "$proof/cpu.max" <<<"50000 100000" >/dev/null
sudo -u "$owner" -H tee "$proof/cgroup.kill" <<<"1" >/dev/null
if ! grep -q 'populated 0' "$proof/cgroup.events"; then
  echo "delegation probe did not report populated 0" >&2
  exit 1
fi
rmdir "$proof"
evidence="${SWITCHYARD_CONFINEMENT_EVIDENCE:-}"
if [[ -n "$evidence" ]]; then
  mkdir -p "$evidence"
  {
    echo "cgroup_root=$delegate"
    echo "cgroup_controllers=$(cat /sys/fs/cgroup/cgroup.controllers)"
    echo "cgroup_subtree=$(cat "$delegate/cgroup.subtree_control")"
    echo "cgroup_owner=$(stat -c '%U:%G' "$delegate")"
    echo "boot_id=$(cat /proc/sys/kernel/random/boot_id)"
  } | tee "$evidence/cgroup-delegation.txt"
  printf '%s\n' "$delegate" >"$evidence/cgroup-root.txt"
  chown -R "$owner:$group" "$evidence" || true
fi
printf '%s\n' "$delegate"
