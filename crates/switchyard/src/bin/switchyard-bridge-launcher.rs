//! Trusted startup barrier; never mounted inside the bridge sandbox.
#![forbid(unsafe_code)]

fn main() {
    switchyard_core::confined_pi_launcher_main();
}
