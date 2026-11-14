#!/usr/bin/env bash
# Gate for the read-only Rune dashboard in crates/switchyard-ui.
#
# Deliberately separate from scripts/rust-verify.sh: this crate pulls the GPU
# stack (wgpu, Parley, Taffy, accesskit) and needs Linux graphics system
# libraries at build time. The core gate must stay lean and must not acquire
# those packages. See crates/switchyard-ui/README.md.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ ! -f rust-toolchain.toml ]]; then
  echo "rust-toolchain.toml is missing; the UI gate requires the committed toolchain." >&2
  exit 1
fi

UI=crates/switchyard-ui

# Rune links these at build time on Linux. Headless snapshot runs do not need
# a display, but the compile does need the dev packages. Probe pkg-config
# itself first: a missing binary is not a missing library.
if [[ "$(uname -s)" == "Linux" ]]; then
  if ! command -v pkg-config >/dev/null 2>&1; then
    echo "pkg-config is required to detect Rune's Linux libraries (Debian/Ubuntu: apt-get install pkg-config libxkbcommon-dev libwayland-dev)" >&2
    exit 1
  fi
  for lib in xkbcommon wayland-client; do
    if ! pkg-config --exists "$lib"; then
      echo "missing $lib dev package (Debian/Ubuntu: apt-get install libxkbcommon-dev libwayland-dev)" >&2
      exit 1
    fi
  done
fi

cargo fmt --manifest-path "$UI/Cargo.toml" --all -- --check
cargo clippy --manifest-path "$UI/Cargo.toml" --all-targets --locked -- -D warnings
cargo test --manifest-path "$UI/Cargo.toml" --locked

# Optional deterministic evidence: render every scene to PNG without a display
# or a database. Needs a Vulkan device; software rendering is fine:
#   SWITCHYARD_UI_SNAP=1 ./scripts/ui-verify.sh
if [[ "${SWITCHYARD_UI_SNAP:-0}" == "1" ]]; then
  cargo build --manifest-path "$UI/Cargo.toml" --release --locked
  bin="$UI/target/release/switchyard-ui"
  out="$UI/artifacts"
  mkdir -p "$out"
  for scene in unread planning awaiting-review needs-attention published empty; do
    "$bin" --snap "$out/$scene.png" --scene "$scene" --theme dark --size 1180x1000 --scale 2
  done
  "$bin" --snap "$out/planning-light.png" --scene planning --theme light --size 1180x1000 --scale 2
  "$bin" --tree "$out/needs-attention.tree.json" --scene needs-attention --theme dark
  echo "snapshots in $out"
fi
