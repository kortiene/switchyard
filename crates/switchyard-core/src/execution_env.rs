//! S3-A2 simulation contract. No host filesystem or subprocess backend.
use crate::{Error, Result, RuntimeBinding, operation_id, payload_hash};
use serde_json::json;

pub const FAKE_WRITE_SCOPE: &str = "fake.workspace.write.v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionOperation {
    /// Simulated only: stores bytes in FakeRuntime memory, never on disk.
    WriteFile { path: String, content: String },
}

#[derive(Clone, Debug)]
pub struct ExecutionRequest {
    pub binding: RuntimeBinding,
    pub tool_task_id: String,
    pub effect_key: String,
    pub capability_scope: String,
    /// Absolute Unix milliseconds. Replay never extends this deadline.
    pub deadline_ms: i64,
    pub operation: ExecutionOperation,
}

impl ExecutionRequest {
    pub fn operation_id(&self) -> Result<String> {
        operation_id(
            &self.binding.attempt_id,
            &self.binding.session_id,
            &self.tool_task_id,
            &self.effect_key,
        )
    }
    pub(crate) fn payload_hash(&self) -> Result<String> {
        payload_hash(&json!({"version":1, "operation":self.operation.to_json(),
            "scope":self.capability_scope, "deadlineMs":self.deadline_ms}))
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if self.tool_task_id.is_empty()
            || self.effect_key.is_empty()
            || self.tool_task_id.len() > 1024
            || self.effect_key.len() > 1024
        {
            return Err(Error::guard("invalid execution task/effect identity"));
        }
        if self.capability_scope != FAKE_WRITE_SCOPE {
            return Err(Error::guard("unsupported simulated execution capability"));
        }
        let ExecutionOperation::WriteFile { path, content } = &self.operation;
        // This is input validation, not a filesystem confinement claim.
        if path.is_empty()
            || path.len() > 4096
            || path.contains('\0')
            || path.contains('\\')
            || path.contains(':')
            || path
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == ".." || p.eq_ignore_ascii_case(".git"))
            || content.len() > 65536
        {
            return Err(Error::guard("invalid or oversized simulated write"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionReceipt {
    pub operation_id: String,
    pub payload_hash: String,
    pub simulated: bool,
    pub bytes_written: usize,
    pub content_hash: String,
}

/// Private seam: only the in-process simulator implements it in this slice.
/// A real backend requires separate confinement and lifecycle review.
pub(crate) trait ExecutionEnv {
    fn apply(&mut self, request: &ExecutionRequest) -> Result<ExecutionReceipt>;
}

impl ExecutionOperation {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        let Self::WriteFile { path, content } = self;
        json!({"method":"write_file", "path":path, "content":content})
    }
}
impl ExecutionReceipt {
    pub fn to_json(&self) -> serde_json::Value {
        json!({"operationId":self.operation_id, "payloadHash":self.payload_hash,
            "simulated":self.simulated, "bytesWritten":self.bytes_written, "contentHash":self.content_hash})
    }
    pub(crate) fn from_json(value: &serde_json::Value) -> Result<Self> {
        let text = |key| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| Error::storage("invalid execution receipt"))
        };
        if value.get("simulated").and_then(serde_json::Value::as_bool) != Some(true) {
            return Err(Error::storage("invalid simulation receipt"));
        }
        Ok(Self {
            operation_id: text("operationId")?,
            payload_hash: text("payloadHash")?,
            content_hash: text("contentHash")?,
            simulated: true,
            bytes_written: value
                .get("bytesWritten")
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| Error::storage("invalid receipt size"))?,
        })
    }
}

/// Read-only ledger evidence. A prepared or uncertain row does not authorize replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionView {
    pub operation_id: String,
    pub state: String,
    pub payload_hash: String,
    pub started_generation: u64,
    /// An abandonment supersession exists. The state stays `uncertain`.
    pub superseded: bool,
}
impl ExecutionView {
    pub fn to_json(&self) -> serde_json::Value {
        json!({"operationId":self.operation_id, "state":self.state,
            "payloadHash":self.payload_hash, "startedGeneration":self.started_generation,
            "superseded":self.superseded, "simulated":true})
    }
}
