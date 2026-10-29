use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::error::{Error, Result};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    Draft,
    Planning,
    AwaitingPlanApproval,
    Implementing,
    Verifying,
    AwaitingReview,
    Approved,
    Publishing,
    Published,
    Rejected,
    Cancelled,
    Pausing,
    Paused,
    Cancelling,
    NeedsAttention,
}

impl Phase {
    pub const fn all() -> [Phase; 15] {
        [
            Self::Draft,
            Self::Planning,
            Self::AwaitingPlanApproval,
            Self::Implementing,
            Self::Verifying,
            Self::AwaitingReview,
            Self::Approved,
            Self::Publishing,
            Self::Published,
            Self::Rejected,
            Self::Cancelled,
            Self::Pausing,
            Self::Paused,
            Self::Cancelling,
            Self::NeedsAttention,
        ]
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Planning => "planning",
            Self::AwaitingPlanApproval => "awaiting_plan_approval",
            Self::Implementing => "implementing",
            Self::Verifying => "verifying",
            Self::AwaitingReview => "awaiting_review",
            Self::Approved => "approved",
            Self::Publishing => "publishing",
            Self::Published => "published",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::Pausing => "pausing",
            Self::Paused => "paused",
            Self::Cancelling => "cancelling",
            Self::NeedsAttention => "needs_attention",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Published | Self::Rejected | Self::Cancelled)
    }

    pub const fn is_normal(self) -> bool {
        matches!(
            self,
            Self::Draft
                | Self::Planning
                | Self::AwaitingPlanApproval
                | Self::Implementing
                | Self::Verifying
                | Self::AwaitingReview
                | Self::Approved
                | Self::Publishing
        )
    }

    /// Phases from which `pause` is legal. Publishing is intentionally absent.
    pub const fn is_pause_source(self) -> bool {
        matches!(
            self,
            Self::Draft
                | Self::Planning
                | Self::AwaitingPlanApproval
                | Self::Implementing
                | Self::Verifying
                | Self::AwaitingReview
                | Self::Approved
        )
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Phase {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        Phase::all()
            .into_iter()
            .find(|phase| phase.as_str() == text)
            .ok_or_else(|| Error::guard(format!("unknown phase {text}")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandKind {
    StartPlanning,
    PlanCompleted,
    RequestRevisedPlan,
    ApprovePlan,
    RequestVerification,
    CompletePass,
    AuthorizeBoundedRepair,
    RequestChanges,
    ApproveReview,
    Publish,
    ReconcileDeliveryComplete,
    Reject,
    Pause,
    ConfirmStopped,
    Resume,
    Cancel,
    ConfirmCancelled,
    DiscoverPublicationCompleted,
    DetectUncertainty,
    ExplicitRecovery,
    Steer,
}

impl CommandKind {
    pub const fn all() -> [CommandKind; 21] {
        [
            Self::StartPlanning,
            Self::PlanCompleted,
            Self::RequestRevisedPlan,
            Self::ApprovePlan,
            Self::RequestVerification,
            Self::CompletePass,
            Self::AuthorizeBoundedRepair,
            Self::RequestChanges,
            Self::ApproveReview,
            Self::Publish,
            Self::ReconcileDeliveryComplete,
            Self::Reject,
            Self::Pause,
            Self::ConfirmStopped,
            Self::Resume,
            Self::Cancel,
            Self::ConfirmCancelled,
            Self::DiscoverPublicationCompleted,
            Self::DetectUncertainty,
            Self::ExplicitRecovery,
            Self::Steer,
        ]
    }

    pub const fn transition_kinds() -> [CommandKind; 20] {
        [
            Self::StartPlanning,
            Self::PlanCompleted,
            Self::RequestRevisedPlan,
            Self::ApprovePlan,
            Self::RequestVerification,
            Self::CompletePass,
            Self::AuthorizeBoundedRepair,
            Self::RequestChanges,
            Self::ApproveReview,
            Self::Publish,
            Self::ReconcileDeliveryComplete,
            Self::Reject,
            Self::Pause,
            Self::ConfirmStopped,
            Self::Resume,
            Self::Cancel,
            Self::ConfirmCancelled,
            Self::DiscoverPublicationCompleted,
            Self::DetectUncertainty,
            Self::ExplicitRecovery,
        ]
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StartPlanning => "start_planning",
            Self::PlanCompleted => "plan_completed",
            Self::RequestRevisedPlan => "request_revised_plan",
            Self::ApprovePlan => "approve_plan",
            Self::RequestVerification => "request_verification",
            Self::CompletePass => "complete_pass",
            Self::AuthorizeBoundedRepair => "authorize_bounded_repair",
            Self::RequestChanges => "request_changes",
            Self::ApproveReview => "approve_review",
            Self::Publish => "publish",
            Self::ReconcileDeliveryComplete => "reconcile_delivery_complete",
            Self::Reject => "reject",
            Self::Pause => "pause",
            Self::ConfirmStopped => "confirm_stopped",
            Self::Resume => "resume",
            Self::Cancel => "cancel",
            Self::ConfirmCancelled => "confirm_cancelled",
            Self::DiscoverPublicationCompleted => "discover_publication_completed",
            Self::DetectUncertainty => "detect_uncertainty",
            Self::ExplicitRecovery => "explicit_recovery",
            Self::Steer => "steer",
        }
    }

    /// Operator commands that must be delivered to the runtime before they are
    /// acknowledged. Runtime-originated observations are not dispatched again.
    pub const fn is_runtime_bound(self) -> bool {
        matches!(
            self,
            Self::StartPlanning
                | Self::RequestRevisedPlan
                | Self::ApprovePlan
                | Self::AuthorizeBoundedRepair
                | Self::RequestChanges
                | Self::Pause
                | Self::Cancel
                | Self::Steer
        )
    }
}

impl fmt::Display for CommandKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for CommandKind {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        CommandKind::all()
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| Error::guard(format!("unknown command {text}")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandState {
    Pending,
    Acknowledged,
    Reconciled,
    Superseded,
}

impl CommandState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Acknowledged => "acknowledged",
            Self::Reconciled => "reconciled",
            Self::Superseded => "superseded",
        }
    }
}

impl FromStr for CommandState {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        match text {
            "pending" => Ok(Self::Pending),
            "acknowledged" => Ok(Self::Acknowledged),
            "reconciled" => Ok(Self::Reconciled),
            "superseded" => Ok(Self::Superseded),
            other => Err(Error::storage(format!("unknown command state {other}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandOrigin {
    Operator,
    RuntimeObservation,
}

impl CommandOrigin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::RuntimeObservation => "runtime_observation",
        }
    }
}

impl FromStr for CommandOrigin {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        match text {
            "operator" => Ok(Self::Operator),
            "runtime_observation" => Ok(Self::RuntimeObservation),
            other => Err(Error::storage(format!("unknown command origin {other}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttentionReason {
    Uncertainty,
    InvalidApproval,
    VerificationError,
    VerificationIncomplete,
    ExhaustedLimits,
    FailedStop,
    ExecutionUncertain,
}

impl AttentionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Uncertainty => "uncertainty",
            Self::InvalidApproval => "invalid_approval",
            Self::VerificationError => "verification_error",
            Self::VerificationIncomplete => "verification_incomplete",
            Self::ExhaustedLimits => "exhausted_limits",
            Self::FailedStop => "failed_stop",
            Self::ExecutionUncertain => "execution_uncertain",
        }
    }
}

impl fmt::Display for AttentionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for AttentionReason {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        match text {
            "uncertainty" => Ok(Self::Uncertainty),
            "invalid_approval" => Ok(Self::InvalidApproval),
            "verification_error" => Ok(Self::VerificationError),
            "verification_incomplete" => Ok(Self::VerificationIncomplete),
            "exhausted_limits" => Ok(Self::ExhaustedLimits),
            "failed_stop" => Ok(Self::FailedStop),
            "execution_uncertain" => Ok(Self::ExecutionUncertain),
            other => Err(Error::guard(format!("unknown attention reason {other}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationOutcome {
    Pass,
    Fail,
    Error,
    Incomplete,
}

impl VerificationOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Error => "error",
            Self::Incomplete => "incomplete",
        }
    }
}

impl FromStr for VerificationOutcome {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        match text {
            "pass" => Ok(Self::Pass),
            "fail" => Ok(Self::Fail),
            "error" => Ok(Self::Error),
            "incomplete" => Ok(Self::Incomplete),
            other => Err(Error::storage(format!(
                "unknown verification outcome {other}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completeness {
    Complete,
    Incomplete,
    Truncated,
}

impl Completeness {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::Truncated => "truncated",
        }
    }
}

impl FromStr for Completeness {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        match text {
            "complete" => Ok(Self::Complete),
            "incomplete" => Ok(Self::Incomplete),
            "truncated" => Ok(Self::Truncated),
            other => Err(Error::storage(format!("unknown completeness {other}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationState {
    Prepared,
    Running,
    Completed,
    Uncertain,
}

impl OperationState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Uncertain => "uncertain",
        }
    }

    pub const fn blocks_quiescence(self) -> bool {
        !matches!(self, Self::Completed)
    }
}

impl FromStr for OperationState {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        match text {
            "prepared" => Ok(Self::Prepared),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "uncertain" => Ok(Self::Uncertain),
            other => Err(Error::storage(format!("unknown operation state {other}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectProof {
    Reconciled,
    Quiescent,
    Unresolved,
}

impl FromStr for EffectProof {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        match text {
            "reconciled" => Ok(Self::Reconciled),
            "quiescent" => Ok(Self::Quiescent),
            "unresolved" => Ok(Self::Unresolved),
            other => Err(Error::guard(format!("unknown effect proof {other}"))),
        }
    }
}

impl EffectProof {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reconciled => "reconciled",
            Self::Quiescent => "quiescent",
            Self::Unresolved => "unresolved",
        }
    }
}

/// Authorized command body. This enum has no mission-phase field. Runtime
/// notifications are parsed into these variants and then evaluated by Rust.
#[derive(Clone, Debug, PartialEq)]
pub enum CommandBody {
    StartPlanning,
    PlanCompleted {
        plan: String,
        digest: String,
        quiescent: bool,
    },
    RequestRevisedPlan {
        feedback: String,
    },
    ApprovePlan {
        plan_digest: String,
    },
    RequestVerification {
        quiescent: bool,
    },
    CompletePass,
    AuthorizeBoundedRepair {
        rationale: String,
    },
    RequestChanges {
        feedback: String,
        within_frozen: bool,
    },
    ApproveReview {
        digest: String,
        rationale: String,
    },
    Publish {
        target_branch: String,
        base_branch: String,
        authorized: bool,
    },
    ReconcileDeliveryComplete {
        content_digest: String,
        commit_sha: String,
        tree_sha: String,
        pr_id: String,
    },
    Reject {
        rationale: String,
    },
    Pause,
    ConfirmStopped,
    Resume {
        authorized: bool,
    },
    Cancel,
    ConfirmCancelled,
    DiscoverPublicationCompleted {
        content_digest: String,
        commit_sha: String,
        tree_sha: String,
        pr_id: String,
    },
    DetectUncertainty {
        reason: AttentionReason,
        content_digest: Option<String>,
    },
    ExplicitRecovery {
        expected_target: Phase,
        ownership_proven: bool,
        effects: EffectProof,
    },
    Steer {
        content: String,
    },
}

impl CommandBody {
    pub fn kind(&self) -> CommandKind {
        match self {
            Self::StartPlanning => CommandKind::StartPlanning,
            Self::PlanCompleted { .. } => CommandKind::PlanCompleted,
            Self::RequestRevisedPlan { .. } => CommandKind::RequestRevisedPlan,
            Self::ApprovePlan { .. } => CommandKind::ApprovePlan,
            Self::RequestVerification { .. } => CommandKind::RequestVerification,
            Self::CompletePass => CommandKind::CompletePass,
            Self::AuthorizeBoundedRepair { .. } => CommandKind::AuthorizeBoundedRepair,
            Self::RequestChanges { .. } => CommandKind::RequestChanges,
            Self::ApproveReview { .. } => CommandKind::ApproveReview,
            Self::Publish { .. } => CommandKind::Publish,
            Self::ReconcileDeliveryComplete { .. } => CommandKind::ReconcileDeliveryComplete,
            Self::Reject { .. } => CommandKind::Reject,
            Self::Pause => CommandKind::Pause,
            Self::ConfirmStopped => CommandKind::ConfirmStopped,
            Self::Resume { .. } => CommandKind::Resume,
            Self::Cancel => CommandKind::Cancel,
            Self::ConfirmCancelled => CommandKind::ConfirmCancelled,
            Self::DiscoverPublicationCompleted { .. } => CommandKind::DiscoverPublicationCompleted,
            Self::DetectUncertainty { .. } => CommandKind::DetectUncertainty,
            Self::ExplicitRecovery { .. } => CommandKind::ExplicitRecovery,
            Self::Steer { .. } => CommandKind::Steer,
        }
    }

    pub fn payload(&self, mission_id: &str, attempt_id: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind().as_str(),
            "missionId": mission_id,
            "attemptId": attempt_id,
            "body": self.body_json(),
        })
    }

    fn body_json(&self) -> serde_json::Value {
        match self {
            Self::StartPlanning
            | Self::CompletePass
            | Self::Pause
            | Self::ConfirmStopped
            | Self::Cancel
            | Self::ConfirmCancelled => {
                serde_json::json!({})
            }
            Self::PlanCompleted {
                plan,
                digest,
                quiescent,
            } => serde_json::json!({
                "plan": plan,
                "digest": digest,
                "quiescent": quiescent,
            }),
            Self::RequestRevisedPlan { feedback } => serde_json::json!({"feedback": feedback}),
            Self::ApprovePlan { plan_digest } => serde_json::json!({"planDigest": plan_digest}),
            Self::RequestVerification { quiescent } => serde_json::json!({"quiescent": quiescent}),
            Self::AuthorizeBoundedRepair { rationale } => {
                serde_json::json!({"rationale": rationale})
            }
            Self::RequestChanges {
                feedback,
                within_frozen,
            } => serde_json::json!({
                "feedback": feedback,
                "withinFrozen": within_frozen,
            }),
            Self::ApproveReview { digest, rationale } => serde_json::json!({
                "digest": digest,
                "rationale": rationale,
            }),
            Self::Publish {
                target_branch,
                base_branch,
                authorized,
            } => serde_json::json!({
                "targetBranch": target_branch,
                "baseBranch": base_branch,
                "authorized": authorized,
            }),
            Self::ReconcileDeliveryComplete {
                content_digest,
                commit_sha,
                tree_sha,
                pr_id,
            }
            | Self::DiscoverPublicationCompleted {
                content_digest,
                commit_sha,
                tree_sha,
                pr_id,
            } => {
                serde_json::json!({
                    "contentDigest": content_digest,
                    "commitSha": commit_sha,
                    "treeSha": tree_sha,
                    "prId": pr_id,
                })
            }
            Self::Reject { rationale } => serde_json::json!({"rationale": rationale}),
            Self::Resume { authorized } => serde_json::json!({"authorized": authorized}),
            Self::DetectUncertainty {
                reason,
                content_digest,
            } => serde_json::json!({
                "reason": reason.as_str(),
                "contentDigest": content_digest,
            }),
            Self::ExplicitRecovery {
                expected_target,
                ownership_proven,
                effects,
            } => serde_json::json!({
                "expectedTarget": expected_target.as_str(),
                "ownershipProven": ownership_proven,
                "effects": effects.as_str(),
            }),
            Self::Steer { content } => serde_json::json!({"content": content}),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoIdentity {
    pub toplevel: String,
    pub head_sha: String,
    pub branch: String,
    pub porcelain_hash: String,
    pub origin_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceIssue {
    pub provider: String,
    pub number: u64,
    pub title: String,
    pub snapshot_at: i64,
}

#[derive(Clone, Debug)]
pub struct MissionRecord {
    pub id: String,
    pub revision: u64,
    pub phase: Phase,
    pub objective: String,
    pub repo: RepoIdentity,
    pub source_issue: SourceIssue,
    pub resume_target: Option<Phase>,
    pub prior_phase: Option<Phase>,
    pub attention_reason: Option<AttentionReason>,
    pub permitted_recovery_target: Option<Phase>,
    pub reconcile_only: bool,
    pub content_digest: String,
    pub plan_digest: Option<String>,
    pub plan_body: Option<String>,
    pub reviewed_digest: Option<String>,
    pub successor_of_mission_id: Option<String>,
    pub task_id: String,
    pub acceptance_criteria: String,
    pub attempt: AttemptRecord,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug)]
pub struct AttemptRecord {
    pub id: String,
    pub task_id: String,
    pub base_sha: String,
    pub session_id: String,
    pub runtime_adapter: String,
    pub policy_json: String,
    pub policy_hash: String,
    pub verifier_json: String,
    pub repair_count: u32,
    pub max_repairs: u32,
    pub plan_revision_count: u32,
    pub max_plan_revisions: u32,
    pub successor_of: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CommandRecord {
    pub id: String,
    pub mission_id: String,
    pub attempt_id: String,
    pub kind: CommandKind,
    pub payload_hash: String,
    pub expected_revision: u64,
    pub resulting_revision: Option<u64>,
    pub state: CommandState,
    pub request_id: String,
    pub origin: CommandOrigin,
    pub runtime_receipt_json: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DeliveryRecord {
    pub id: String,
    pub mission_id: String,
    pub attempt_id: String,
    pub target_branch: String,
    pub base_branch: String,
    pub approved_digest: String,
    pub tree_sha: Option<String>,
    pub commit_sha: Option<String>,
    pub pr_identity: Option<String>,
    pub publication_state: String,
}

#[derive(Clone, Debug)]
pub struct VerificationFact {
    pub outcome: VerificationOutcome,
    pub content_digest: String,
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationProof {
    pub content_digest: String,
    pub commit_sha: String,
    pub tree_sha: String,
    pub pr_id: String,
}

impl PublicationProof {
    pub fn matches_digest(&self, approved_digest: &str) -> bool {
        self.content_digest == approved_digest
            && !self.commit_sha.is_empty()
            && !self.tree_sha.is_empty()
            && !self.pr_id.is_empty()
    }
}

pub fn content_drift(mission: &MissionRecord) -> bool {
    mission
        .reviewed_digest
        .as_ref()
        .is_some_and(|reviewed| reviewed != &mission.content_digest)
}

/// Recorded recovery target when entering `needs_attention`.
///
/// `cancelling` is included so a pending cancel can settle. It is not a normal
/// phase and recovery to it does not resume work. In-flight delivery stays on
/// `publishing` so reconciliation is not skipped. Invalid approval and
/// error/incomplete verification recover to `verifying`, never `approved`.
pub fn permitted_recovery_target(
    from: Phase,
    reason: AttentionReason,
    delivery_in_flight: bool,
) -> Phase {
    if from == Phase::Cancelling {
        return Phase::Cancelling;
    }
    if from == Phase::Publishing || delivery_in_flight {
        return Phase::Publishing;
    }
    if from == Phase::Pausing || from == Phase::Paused {
        return Phase::Paused;
    }
    if matches!(
        reason,
        AttentionReason::InvalidApproval
            | AttentionReason::VerificationError
            | AttentionReason::VerificationIncomplete
    ) {
        return Phase::Verifying;
    }
    from
}

pub fn validated_recovery_target(mission: &MissionRecord) -> Result<Phase> {
    let permitted = mission
        .permitted_recovery_target
        .ok_or_else(|| Error::guard("needs_attention has no recorded recovery target"))?;
    if permitted == Phase::Publishing || mission.reconcile_only {
        return Ok(Phase::Publishing);
    }
    if permitted == Phase::Cancelling || permitted == Phase::Paused {
        return Ok(permitted);
    }
    if content_drift(mission) && matches!(permitted, Phase::Approved | Phase::AwaitingReview) {
        return Ok(Phase::Verifying);
    }
    Ok(permitted)
}

/// Resume target to store when recovery returns to `paused`.
///
/// A paused mission keeps its stopped phase. If the saved resume target was a
/// review phase and the content has drifted, that target is repaired to
/// `verifying` so the next resume does not depend on the stale approval.
pub fn repaired_resume_target(mission: &MissionRecord) -> Option<Phase> {
    let resume = mission.resume_target?;
    if matches!(resume, Phase::Approved | Phase::AwaitingReview) && content_drift(mission) {
        Some(Phase::Verifying)
    } else {
        Some(resume)
    }
}
