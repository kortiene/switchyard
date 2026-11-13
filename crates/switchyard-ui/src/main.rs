//! Read-only mission dashboard for the Switchyard Rust ledger.
//!
//! `SWITCHYARD_DB=/path/to/switchyard.db switchyard-ui`
//!
//! Rune supplies the headless tooling flags (`--snap`, `--tree`, `--script`,
//! `--trace`, `--video`, `--list-scenes`), so the same binary that opens the
//! window also renders deterministic evidence in CI.

fn main() {
    switchyard_ui::run();
}
