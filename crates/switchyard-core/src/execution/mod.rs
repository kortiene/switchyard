mod adapter;
mod git;
mod owner;
#[cfg(feature = "pi-contract")]
mod pi_bridge;
#[cfg(feature = "pi-contract")]
pub(crate) mod pi_runtime;
mod runtime;
pub(crate) mod workspace;

pub use git::inspect_repo;
pub use owner::{OwnerLock, lock_path, try_acquire};
#[cfg(feature = "pi-contract")]
pub use pi_bridge::{PiBridge, PiEffectOutcome};
pub use runtime::{
    FakeRuntime, RuntimeBinding, RuntimeNotification, RuntimeReceipt, RuntimeRequest, operation_id,
    parse_notification,
};

pub(crate) use adapter::{RecoveryProfile, RuntimeAdapter, ToolEffect};

/// Stop-admission fixture. It is not on a production allowlist.
#[cfg(feature = "pi-contract")]
pub(crate) fn is_busy_once_fixture(fixture: &str) -> bool {
    fixture == "busy-once"
}
#[cfg(feature = "pi-contract")]
pub(crate) use pi_runtime::{AcknowledgedCheck, PiRuntime};

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
pub(crate) mod confined_pi;

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
pub(crate) mod confined_runtime;
