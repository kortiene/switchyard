//! Operator decisions are separate from backend receipts and mission recovery.
use crate::{Result, RuntimeBinding, payload_hash};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionResolutionDecision {
    Completed,
    NotApplied,
    /// Audit an unresolved outcome. The effect and mission remain blocked.
    Abandon,
}
impl ExecutionResolutionDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NotApplied => "not_applied",
            Self::Abandon => "abandoned",
        }
    }
}

/// Operator-only Host API. Generation authorizes the call but is not its identity.
/// Reuse command_id and every other field after a lost acknowledgment.
#[derive(Clone, Debug)]
pub struct ExecutionResolutionRequest {
    pub command_id: String,
    pub binding: RuntimeBinding,
    pub operation_id: String,
    pub payload_hash: String,
    pub expected_revision: u64,
    pub actor: String,
    pub rationale: String,
    pub decision: ExecutionResolutionDecision,
}
impl ExecutionResolutionRequest {
    pub(crate) fn payload(&self) -> Value {
        json!({"version":1, "missionId":self.binding.mission_id,
            "attemptId":self.binding.attempt_id,"sessionId":self.binding.session_id,
            "operationId":self.operation_id,"payloadHash":self.payload_hash,
            "expectedRevision":self.expected_revision,"actor":self.actor,
            "rationale":self.rationale,"decision":self.decision.as_str()})
    }
    pub(crate) fn hash(&self) -> Result<String> {
        payload_hash(&self.payload())
    }
}
