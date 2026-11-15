use std::fmt;

/// Stable failure categories for the S1 host.
///
/// Names are the wire/CLI values. Library display text is diagnostic and is not
/// itself a compatibility contract.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Additional diagnostics without changing the primary failure semantics.
    #[error("{source}; {detail}")]
    WithContext { source: Box<Error>, detail: String },

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

    #[error("execution deadline expired before applying {operation_id}")]
    ExecutionDeadlineExceeded { operation_id: String },

    #[error("new execution blocked for mission {mission_id}: {detail}")]
    ExecutionBlocked { mission_id: String, detail: String },

    #[error(
        "execution was proven not applied for {operation_id}; original identity cannot be replayed"
    )]
    ExecutionNotApplied { operation_id: String },

    #[error("execution outcome is uncertain for {operation_id}")]
    ExecutionUncertain { operation_id: String },

    #[error("operation conflict for {operation_id}")]
    OperationConflict { operation_id: String },

    #[error("stale generation: expected {expected}, actual {actual}")]
    StaleGeneration { expected: u64, actual: u64 },

    #[error("repair limit exhausted ({count}/{max})")]
    RepairLimitExhausted { count: u32, max: u32 },

    /// `approve_plan` on an attempt that left draft before frozen inputs existed.
    /// The command receipt is stored and mission state does not change.
    /// Continuing requires a successor attempt.
    #[error("frozen inputs are absent; a successor attempt is needed")]
    FrozenInputsMissing,

    /// Live attempt inputs differ from the stored freeze. The command is not
    /// applied. The first refusal still commits `needs_attention`. Later
    /// refusals leave the revision unchanged. The way out is `cancel`, then
    /// `confirm_cancelled`, then `mission successor` if scope must change.
    #[error("frozen attempt inputs drifted; the command was not applied")]
    FrozenInputDrift,

    /// The approval named a digest other than the current plan digest.
    #[error("plan digest does not match the current plan")]
    StalePlanDigest,

    /// `request_revised_plan` at the frozen `max_plan_revisions` cap.
    /// Mission state does not change. Approve, reject, and cancel stay open.
    #[error("plan revision limit reached ({count}/{max}); a higher cap needs a successor attempt")]
    PlanRevisionLimitReached { count: u32, max: u32 },

    /// `mission successor` on a source that is not published, rejected, or cancelled.
    #[error("successor requires a terminal source; cancel, then confirm_cancelled")]
    SuccessorSourceNotTerminal,

    /// This source attempt already has a successor. The id is the existing mission.
    #[error("a successor already exists ({mission_id}); continue with that mission")]
    SuccessorExists { mission_id: String },

    /// The source revision or phase changed before the successor insert.
    /// The transaction rolls back, so nothing is written.
    #[error("the source mission changed during successor creation; nothing was written; retry")]
    SourceChangedDuringCreate,

    /// `--verifier` was not an exact canonical match for a document this host can run.
    #[error("verifier is not supported by this host; supported: {supported}")]
    VerifierNotSupported { supported: String },

    /// Notify `verification_pass` or `verification_fail` while the frozen verifier executes.
    #[error("verification must be collected by the host verifier")]
    VerificationNotHostRun,

    /// The path gate passed. This slice does not run the verifier.
    #[error("verifier confinement is not available")]
    VerifierConfinementUnavailable,

    /// `mission verify` on a verifier that has no host path gate.
    #[error("this verifier has no host path gate")]
    VerifierGateUnavailable,

    /// The index hides paths from `git diff`. Clear the flag, then verify again.
    #[error(
        "assume-unchanged or skip-worktree hides verifier paths; clear them with git update-index --no-assume-unchanged <path> or --no-skip-worktree <path>, then run verify again"
    )]
    VerifierIndexFlagsSet,

    /// Pi refused to admit the command and the protocol marks that refusal as
    /// definite. Transport loss and `admission: uncertain` stay [`Self::GuardFailed`]
    /// or [`Self::Storage`]. Effect outcomes use `PiEffectOutcome` separately
    /// under the `pi-contract` feature; they do not settle command admission.
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

    /// `exit_code` is `None` when Git was killed by a signal, or the failure
    /// was not a completed process exit. Signal death stays retryable.
    /// A real non-zero exit is evidence when a workspace check converts it.
    #[error("git inspection failed: {detail}")]
    GitFailed {
        detail: String,
        exit_code: Option<i32>,
    },

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
            Self::ExecutionUncertain { .. } => "execution_uncertain",
            Self::ExecutionNotApplied { .. } => "execution_not_applied",
            Self::ExecutionDeadlineExceeded { .. } => "execution_deadline_exceeded",
            Self::ExecutionBlocked { .. } => "execution_blocked",
            Self::StaleGeneration { .. } => "stale_generation",
            Self::RepairLimitExhausted { .. } => "repair_limit_exhausted",
            Self::FrozenInputsMissing => "frozen_inputs_missing",
            Self::FrozenInputDrift => "frozen_input_drift",
            Self::StalePlanDigest => "stale_plan_digest",
            Self::PlanRevisionLimitReached { .. } => "plan_revision_limit_reached",
            Self::SuccessorSourceNotTerminal => "successor_source_not_terminal",
            Self::SuccessorExists { .. } => "successor_exists",
            Self::SourceChangedDuringCreate => "source_changed_during_create",
            Self::VerifierNotSupported { .. } => "verifier_not_supported",
            Self::VerificationNotHostRun => "verification_not_host_run",
            Self::VerifierConfinementUnavailable => "verifier_confinement_unavailable",
            Self::VerifierGateUnavailable => "verifier_gate_unavailable",
            Self::VerifierIndexFlagsSet => "verifier_index_flags_set",
            Self::AdmissionRejected { .. } => "admission_rejected",
            Self::GuardFailed { .. } => "guard_failed",
            Self::UnknownNotification { .. } => "unknown_notification",
            Self::NotFound => "not_found",
            Self::ActiveMissionExists => "active_mission_exists",
            Self::GitFailed { .. } => "git_failed",
            Self::Canonical { .. } => "canonical_rejected",
            Self::Storage { .. } => "storage_failed",
            Self::WithContext { source, .. } => source.category(),
        }
    }

    /// Attach diagnostics while preserving the primary category and retry semantics.
    pub fn with_context(self, detail: impl Into<String>) -> Self {
        Self::WithContext {
            source: Box::new(self),
            detail: detail.into(),
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
            Self::WithContext { source, .. } => source.rejected_admission(),
            _ => None,
        }
    }

    pub fn storage(detail: impl fmt::Display) -> Self {
        Self::Storage {
            detail: detail.to_string(),
        }
    }

    pub fn git(detail: impl Into<String>, exit_code: Option<i32>) -> Self {
        Self::GitFailed {
            detail: detail.into(),
            exit_code,
        }
    }

    /// Completed Git exit, as opposed to a signal kill. Evidence checks turn
    /// only this case into a permanent workspace mismatch.
    pub fn git_exit_code(&self) -> Option<i32> {
        match self {
            Self::GitFailed { exit_code, .. } => *exit_code,
            Self::WithContext { source, .. } => source.git_exit_code(),
            _ => None,
        }
    }

    /// Storage and IO failures can be retried. Semantic rejections cannot.
    pub fn is_transient(&self) -> bool {
        if let Self::WithContext { source, .. } = self {
            return source.is_transient();
        }
        matches!(
            self,
            Self::Storage { .. } | Self::GitFailed { .. } | Self::WorkspaceBusy { .. }
        )
    }

    /// Workspace problems that must not block command admission or observation
    /// polling or FakeRuntime stop delivery. Work dispatch stays gated until
    /// the workspace is ready again.
    /// Stale generation and other host-level failures are not included.
    pub fn is_isolated_workspace_failure(&self) -> bool {
        if let Self::WithContext { source, .. } = self {
            return source.is_isolated_workspace_failure();
        }
        matches!(self, Self::WorkspaceUncertain { .. }) || self.is_transient()
    }
}

pub type Result<T> = std::result::Result<T, Error>;
