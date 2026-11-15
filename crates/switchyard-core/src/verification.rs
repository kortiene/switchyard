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
    pub collector: VerificationCollector,
    /// Extra `body_json` fields for a host-run row. Notify rows leave this empty.
    pub host_fields: Option<Value>,
}

/// Who collected a new evidence row. Historical rows are not rewritten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationCollector {
    /// Notify-injected pass or fail.
    RuntimeObservation,
    /// `mission verify` on this host.
    HostVerifier,
}

impl NewVerification {
    pub fn provenance(&self) -> Value {
        match self.collector {
            VerificationCollector::RuntimeObservation => json!({
                "collector": "runtime-observation",
                "runtimeClaim": true
            }),
            VerificationCollector::HostVerifier => json!({
                "collector": "switchyard-host-verifier"
            }),
        }
    }

    pub fn requirements_json(&self) -> String {
        serde_json::to_string(&self.requirements).unwrap_or_else(|_| "[]".to_string())
    }
}
