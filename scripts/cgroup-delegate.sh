#!/usr/bin/env bash
# Root-only setup for the S3-A4 delegated cgroup v2 subtree.
# The Host, Bubblewrap, and workloads stay unprivileged. The subtree is created
# under the runner's own cgroup so that user can migrate into a leaf. A sibling
# under the cgroup root is not enough: migration also requires write access to
# the common ancestor's cgroup.procs. Missing controllers or a failed join
# abort the job. There is no rlimit-only fallback.
set -euo pipefail
if [[ "$(id -u)" -ne 0 ]]; then
  echo "cgroup delegation setup must run as root" >&2
  exit 1
fi
parent="${1:-}"
if [[ ! -d "$parent" || ! -f "$parent/cgroup.controllers" ]]; then
  echo "runner cgroup is not a cgroup v2 directory: ${parent:-<unset>}" >&2
  exit 1
fi
case "$parent" in
  /sys/fs/cgroup | /sys/fs/cgroup/ | /sys/fs/cgroup/init.scope | /sys/fs/cgroup/system.slice)
    echo "refusing to rearrange the system cgroup $parent" >&2
    exit 1
    ;;
esac
available="$(cat "$parent/cgroup.controllers")"
for controller in cpu memory pids; do
  if ! grep -Eq "(^| )${controller}( |$)" <<<"$available"; then
    echo "runner cgroup $parent lacks controller $controller ($available)" >&2
    exit 1
  fi
done
owner="${SUDO_USER:-}"
if [[ -z "$owner" || "$owner" == root ]]; then
  echo "SUDO_USER must be the unprivileged runner" >&2
  exit 1
fi
group="$(id -gn "$owner")"

# A cgroup with processes cannot enable subtree_control. Park the job in a
# child, then delegate a sibling the runner owns.
if grep -Eq '[0-9]' "$parent/cgroup.procs"; then
  mkdir -p "$parent/init"
  for _ in 1 2 3 4 5 6 7 8; do
    while read -r pid; do
      [[ -n "$pid" ]] || continue
      echo "$pid" >"$parent/init/cgroup.procs" 2>/dev/null || true
    done <"$parent/cgroup.procs"
    if ! grep -Eq '[0-9]' "$parent/cgroup.procs"; then
      break
    fi
  done
  if grep -Eq '[0-9]' "$parent/cgroup.procs"; then
    echo "could not move processes out of $parent before enabling controllers" >&2
    exit 1
  fi
fi
current_subtree="$(cat "$parent/cgroup.subtree_control")"
needed=""
for controller in cpu memory pids; do
  if ! grep -Eq "(^| )${controller}( |$)" <<<"$current_subtree"; then
    needed+=" +${controller}"
  fi
done
if [[ -n "$needed" ]]; then
  echo "$needed" >"$parent/cgroup.subtree_control"
fi

delegate="$parent/switchyard"
mkdir -p "$delegate"
if grep -Eq '[0-9]' "$delegate/cgroup.procs"; then
  echo "refusing to delegate $delegate: it already contains processes" >&2
  exit 1
fi
echo '+cpu +memory +pids' >"$delegate/cgroup.subtree_control"
chown "$owner:$group" "$delegate" "$parent/cgroup.procs"

# The probe must join the leaf as the runner. Writing limits alone does not
# prove migration is allowed.
proof="$(sudo -u "$owner" -H mktemp -d "$delegate/probe-XXXXXX")"
sudo -u "$owner" -H tee "$proof/memory.max" <<<"268435456" >/dev/null
sudo -u "$owner" -H tee "$proof/memory.swap.max" <<<"0" >/dev/null
sudo -u "$owner" -H tee "$proof/memory.oom.group" <<<"1" >/dev/null
sudo -u "$owner" -H tee "$proof/pids.max" <<<"64" >/dev/null
sudo -u "$owner" -H tee "$proof/cpu.max" <<<"50000 100000" >/dev/null
sudo -u "$owner" -H python3 - "$proof" <<'PY'
import os
import pathlib
import sys

leaf = pathlib.Path(sys.argv[1])
(leaf / "cgroup.procs").write_text(str(os.getpid()))
print("joined", os.getpid())
PY
echo 1 >"$proof/cgroup.kill"
if ! grep -q 'populated 0' "$proof/cgroup.events"; then
  echo "delegation probe did not return to populated 0" >&2
  exit 1
fi
rmdir "$proof"

evidence="${SWITCHYARD_CONFINEMENT_EVIDENCE:-}"
if [[ -n "$evidence" ]]; then
  mkdir -p "$evidence"
  {
    echo "cgroup_root=$delegate"
    echo "cgroup_parent=$parent"
    echo "cgroup_controllers=$available"
    echo "cgroup_subtree=$(cat "$delegate/cgroup.subtree_control")"
    echo "cgroup_owner=$(stat -c '%U:%G' "$delegate")"
    echo "ancestor_procs_owner=$(stat -c '%U:%G' "$parent/cgroup.procs")"
    echo "boot_id=$(cat /proc/sys/kernel/random/boot_id)"
  } | tee "$evidence/cgroup-delegation.txt"
  printf '%s\n' "$delegate" >"$evidence/cgroup-root.txt"
  chown -R "$owner:$group" "$evidence" || true
fi
printf '%s\n' "$delegate"
