#!/usr/bin/env bash
# Rust core gate for the S1 mission ledger. This is separate from
# `npm run verify` in adw_sdlc/. It does not call a model or GitHub.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ ! -f rust-toolchain.toml ]]; then
  echo "rust-toolchain.toml is missing; the Rust gate requires the committed toolchain." >&2
  exit 1
fi

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
