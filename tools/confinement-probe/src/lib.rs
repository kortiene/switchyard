//! Deterministic test tooling only. No dependency from the product Host or CLI.
#![forbid(unsafe_code)]
#[cfg(all(feature = "linux-confinement", not(target_os = "linux")))]
compile_error!("linux-confinement requires Linux; there is no unconfined fallback");
#[cfg(all(feature = "linux-confinement", target_os = "linux"))]
pub mod probe;
