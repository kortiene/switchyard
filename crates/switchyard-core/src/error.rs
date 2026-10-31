use std::fmt;

/// Stable failure categories for the S1 host.
///
/// Names are the wire/CLI values. Library display text is diagnostic and is not
/// itself a compatibility contract.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("illegal transition from {from} via {command}")]
    IllegalTransition { from: String, command: String },

    #[error("terminal mission is immutable ({phase})")]
    TerminalImmutable { phase: String },

    #[error("stale revision: expected {expected}, actual {actual}")]
    StaleRevision { expected: u64, actual: u64 },

    #[error("binding mismatch: {detail}")]
    BindingMismatch { detail: String },

    #[error("a second active owner already holds the host lock")]
    SecondActiveOwner,

    #[error("workspace {workspace_id} still has an active creator")]
    WorkspaceBusy { workspace_id: String },

    #[error("workspace {workspace_id} is uncertain: {detail}")]
    WorkspaceUncertain {
        workspace_id: String,
        detail: String,
    },

    #[error("command conflict for {command_id}")]
    CommandConflict { command_id: String },

    #[error("operation conflict for {operation_id}")]
    OperationConflict { operation_id: String },

    #[error("stale generation: expected {expected}, actual {actual}")]
    StaleGeneration { expected: u64, actual: u64 },

    #[error("repair limit exhausted ({count}/{max})")]
    RepairLimitExhausted { count: u32, max: u32 },

    #[error("usage limit exhausted ({count}/{max})")]
    UsageLimitExhausted { count: u32, max: u32 },

    /// Pi refused to admit the command and the protocol marks that refusal as
    /// definite. Transport loss and `admission: uncertain` stay [`Self::GuardFailed`]
    /// or [`Self::Storage`]. Effect-level rejection is still #96.
    #[error("admission rejected: {category}")]
    AdmissionRejected { category: String },

    #[error("guard failed: {detail}")]
    GuardFailed { detail: String },

    #[error("unknown runtime notification kind {kind}")]
    UnknownNotification { kind: String },

    #[error("not found")]
    NotFound,

    #[error("an active mission already exists")]
    ActiveMissionExists,

    #[error("git inspection failed: {detail}")]
    GitFailed { detail: String },

    #[error("canonical JSON rejected: {detail}")]
    Canonical { detail: String },

    #[error("storage failed: {detail}")]
    Storage { detail: String },
}

impl Error {
    pub fn category(&self) -> &'static str {
        match self {
            Self::IllegalTransition { .. } => "illegal_transition",
            Self::TerminalImmutable { .. } => "terminal_immutable",
            Self::StaleRevision { .. } => "stale_revision",
            Self::BindingMismatch { .. } => "binding_mismatch",
            Self::SecondActiveOwner => "second_active_owner",
            Self::WorkspaceBusy { .. } => "workspace_busy",
            Self::WorkspaceUncertain { .. } => "workspace_uncertain",
            Self::CommandConflict { .. } => "command_conflict",
            Self::OperationConflict { .. } => "operation_conflict",
            Self::StaleGeneration { .. } => "stale_generation",
            Self::RepairLimitExhausted { .. } => "repair_limit_exhausted",
            Self::UsageLimitExhausted { .. } => "usage_limit_exhausted",
            Self::AdmissionRejected { .. } => "admission_rejected",
            Self::GuardFailed { .. } => "guard_failed",
            Self::UnknownNotification { .. } => "unknown_notification",
            Self::NotFound => "not_found",
            Self::ActiveMissionExists => "active_mission_exists",
            Self::GitFailed { .. } => "git_failed",
            Self::Canonical { .. } => "canonical_rejected",
            Self::Storage { .. } => "storage_failed",
        }
    }

    pub fn guard(detail: impl Into<String>) -> Self {
        Self::GuardFailed {
            detail: detail.into(),
        }
    }

    /// Category when Pi definitely refused admission. `None` for uncertain
    /// transport, unknown admission, and every other failure.
    pub fn rejected_admission(&self) -> Option<&str> {
        match self {
            Self::AdmissionRejected { category } => Some(category.as_str()),
            _ => None,
        }
    }

    pub fn storage(detail: impl fmt::Display) -> Self {
        Self::Storage {
            detail: detail.to_string(),
        }
    }

    /// Storage and IO failures can be retried. Semantic rejections cannot.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Storage { .. } | Self::GitFailed { .. } | Self::WorkspaceBusy { .. }
        )
    }
}

pub type Result<T> = std::result::Result<T, Error>;
