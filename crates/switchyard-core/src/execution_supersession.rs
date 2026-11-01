//! Operator supersession of an abandoned execution effect.
//!
//! The request carries no receipt, quiescence claim, or workspace digest. The
//! Host observes those at decision time. Generation authorizes the call and is
//! not part of the command identity.
use crate::{Result, RuntimeBinding, payload_hash};
use serde_json::{Value, json};

/// Operator-only Host API. Reuse `command_id` and every other field after a
/// lost acknowledgement; only the binding generation is refreshed.
#[derive(Clone, Debug)]
pub struct ExecutionSupersessionRequest {
    pub command_id: String,
    pub binding: RuntimeBinding,
    pub operation_id: String,
    pub payload_hash: String,
    pub abandonment_command_id: String,
    pub abandonment_request_hash: String,
    pub expected_revision: u64,
    pub actor: String,
    pub rationale: String,
}

impl ExecutionSupersessionRequest {
    pub(crate) fn payload(&self) -> Value {
        json!({
            "version": 1,
            "missionId": self.binding.mission_id,
            "attemptId": self.binding.attempt_id,
            "sessionId": self.binding.session_id,
            "operationId": self.operation_id,
            "payloadHash": self.payload_hash,
            "abandonmentCommandId": self.abandonment_command_id,
            "abandonmentRequestHash": self.abandonment_request_hash,
            "expectedRevision": self.expected_revision,
            "actor": self.actor,
            "rationale": self.rationale
        })
    }

    pub(crate) fn hash(&self) -> Result<String> {
        payload_hash(&self.payload())
    }
}
