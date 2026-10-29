use serde_json::{Value, json};

use crate::domain::{Completeness, VerificationOutcome};

/// Host-recorded verification input. Recording this does not change mission phase.
#[derive(Clone, Debug)]
pub struct NewVerification {
    pub outcome: VerificationOutcome,
    pub content_digest: String,
    pub evidence_body: String,
    pub completeness: Completeness,
    pub requirements: Vec<String>,
}

impl NewVerification {
    pub fn provenance() -> Value {
        json!({
            "collector": "switchyard-host",
            "runtimeClaim": false
        })
    }

    pub fn requirements_json(&self) -> String {
        serde_json::to_string(&self.requirements).unwrap_or_else(|_| "[]".to_string())
    }
}
