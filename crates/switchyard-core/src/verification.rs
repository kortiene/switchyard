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
    /// Extra `provenance_json` fields merged into the collector object.
    pub provenance_extra: Option<Value>,
}

/// Who collected a new evidence row. Historical rows are not rewritten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationCollector {
    /// Notify-injected pass or fail.
    RuntimeObservation,
    /// `mission verify` on this host.
    HostVerifier,
}

impl VerificationCollector {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeObservation => "runtime-observation",
            Self::HostVerifier => "switchyard-host-verifier",
        }
    }
}

impl NewVerification {
    pub fn provenance(&self) -> Value {
        match self.collector {
            VerificationCollector::RuntimeObservation => json!({
                "collector": "runtime-observation",
                "runtimeClaim": true
            }),
            VerificationCollector::HostVerifier => {
                let mut document = json!({
                    "collector": "switchyard-host-verifier"
                });
                if let (Some(extra), Some(dest)) =
                    (self.provenance_extra.as_ref(), document.as_object_mut())
                    && let Some(src) = extra.as_object()
                {
                    for (key, value) in src {
                        dest.insert(key.clone(), value.clone());
                    }
                }
                document
            }
        }
    }

    pub fn requirements_json(&self) -> String {
        serde_json::to_string(&self.requirements).unwrap_or_else(|_| "[]".to_string())
    }
}
