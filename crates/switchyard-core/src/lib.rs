//! Switchyard mission authority for the Rust-first S1 slice.
//!
//! This crate owns mission transitions, the application SQLite ledger, the
//! single-host owner lock, and a default fake agent runtime. The optional
//! `pi-contract` feature adds a credential-free Host/Pi outbox probe. It does not read model
//! credentials, write to GitHub, resume legacy ADW state, or confine a Pi
//! process. Those boundaries belong to later slices.

#![forbid(unsafe_code)]

mod canonical;
mod delivery;
mod domain;
mod error;
mod execution;
mod execution_env;
mod execution_resolution;
mod execution_supersession;
mod linux_execution;
pub use execution_env::{
    ExecutionOperation, ExecutionReceipt, ExecutionRequest, ExecutionView, FAKE_WRITE_SCOPE,
};
pub use execution_resolution::{ExecutionResolutionDecision, ExecutionResolutionRequest};
pub use execution_supersession::ExecutionSupersessionRequest;
#[cfg(all(feature = "linux-execution", target_os = "linux"))]
pub use linux_execution::helper_main as linux_helper_main;
pub use linux_execution::{
    EDIT_SCOPE as LINUX_EDIT_SCOPE, EXEC_CANCEL_SCOPE as LINUX_EXEC_CANCEL_SCOPE,
    EXEC_START_SCOPE as LINUX_EXEC_START_SCOPE, EXEC_STATUS_SCOPE as LINUX_EXEC_STATUS_SCOPE,
    LIST_SCOPE as LINUX_LIST_SCOPE, LinuxExecutionConfig, LinuxExecutionOutcome,
    LinuxExecutionRequest, LinuxExecutionView, LinuxOperation, RANGE_SCOPE as LINUX_RANGE_SCOPE,
    READ_SCOPE as LINUX_READ_SCOPE, WRITE_SCOPE as LINUX_WRITE_SCOPE,
};
mod persistence;
mod policy;
mod service;
mod transitions;
mod verification;
mod workspace;

pub use workspace::WorkspaceView;

pub use canonical::{canonicalize, payload_hash};
pub use delivery::{proof_matches, publication_operation_id};
pub use domain::{
    AttentionReason, CommandBody, CommandKind, CommandOrigin, CommandState, EffectProof,
    OperationState, Phase, PublicationProof, content_drift, permitted_recovery_target,
    validated_recovery_target,
};
pub use error::{Error, Result};
#[cfg(feature = "pi-contract")]
pub use execution::PiBridge;
pub use execution::{
    FakeRuntime, RuntimeBinding, RuntimeNotification, inspect_repo, lock_path, operation_id,
    parse_notification,
};
pub use service::{
    AdmitRequest, AdmitResult, CommandView, CreateMission, DeliveryView, Host, MissionSnapshot,
    NotifyOutcome, journal_path,
};

#[cfg(test)]
mod acceptance;
