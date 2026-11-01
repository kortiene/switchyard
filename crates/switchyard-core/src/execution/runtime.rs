use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde_json::{Value, json};

use crate::canonical::{canonicalize, sha256_hex};
use crate::domain::AttentionReason;
use crate::error::{Error, Result};

/// Mutating effect identity from the M1 formula. Generation, wire id, and the
/// argument hash are intentionally excluded.
pub fn operation_id(
    attempt_id: &str,
    session_id: &str,
    tool_task_id: &str,
    effect_key: &str,
) -> Result<String> {
    let canonical = canonicalize(&json!([
        "switchyard.exec.v1",
        attempt_id,
        session_id,
        tool_task_id,
        effect_key
    ]))?;
    Ok(format!("op-v1:{}", sha256_hex(canonical.as_bytes())))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeBinding {
    pub mission_id: String,
    pub attempt_id: String,
    pub session_id: String,
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub struct RuntimeRequest {
    pub binding: RuntimeBinding,
    pub request_id: String,
    pub payload_hash: String,
    pub method: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeReceipt {
    pub receipt_id: String,
    pub request_id: String,
    pub payload_hash: String,
}

/// Observations the fake runtime can emit. There is no phase or status field.
#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeNotification {
    PlanCompleted {
        plan: String,
        digest: String,
        quiescent: bool,
    },
    ImplementationSettled {
        quiescent: bool,
    },
    VerificationPass {
        content_digest: String,
        evidence: String,
        requirements: Vec<String>,
        completeness: String,
    },
    VerificationFail {
        content_digest: String,
        evidence: String,
        requirements: Vec<String>,
        completeness: String,
    },
    VerificationError {
        detail: String,
    },
    VerificationIncomplete {
        detail: String,
    },
    Uncertainty {
        reason: AttentionReason,
        detail: String,
    },
    PublicationObserved {
        content_digest: String,
        commit_sha: String,
        tree_sha: String,
        pr_id: String,
    },
    Idle,
}

pub fn parse_notification(value: &Value) -> Result<RuntimeNotification> {
    let kind =
        value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::UnknownNotification {
                kind: "missing".into(),
            })?;
    let text = |key: &str| -> Result<String> {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| Error::guard(format!("notification {kind} requires {key}")))
    };
    let requirements = || -> Result<Vec<String>> {
        value
            .get("requirements")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .ok_or_else(|| Error::guard("notification requires requirements"))
    };
    match kind {
        "plan_completed" => Ok(RuntimeNotification::PlanCompleted {
            plan: text("plan")?,
            digest: text("digest")?,
            quiescent: value
                .get("quiescent")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }),
        "implementation_settled" => Ok(RuntimeNotification::ImplementationSettled {
            quiescent: value
                .get("quiescent")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }),
        "verification_pass" => Ok(RuntimeNotification::VerificationPass {
            content_digest: text("contentDigest")?,
            evidence: text("evidence")?,
            requirements: requirements()?,
            completeness: required_completeness(value, kind)?,
        }),
        "verification_fail" => Ok(RuntimeNotification::VerificationFail {
            content_digest: text("contentDigest")?,
            evidence: text("evidence")?,
            requirements: requirements()?,
            completeness: required_completeness(value, kind)?,
        }),
        "verification_error" => Ok(RuntimeNotification::VerificationError {
            detail: text("detail")?,
        }),
        "verification_incomplete" => Ok(RuntimeNotification::VerificationIncomplete {
            detail: text("detail")?,
        }),
        "uncertainty" => Ok(RuntimeNotification::Uncertainty {
            reason: text("reason")?.parse()?,
            detail: value
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        }),
        "publication_observed" => Ok(RuntimeNotification::PublicationObserved {
            content_digest: text("contentDigest")?,
            commit_sha: text("commitSha")?,
            tree_sha: text("treeSha")?,
            pr_id: text("prId")?,
        }),
        "idle" => Ok(RuntimeNotification::Idle),
        other => Err(Error::UnknownNotification {
            kind: other.to_string(),
        }),
    }
}

impl RuntimeNotification {
    pub fn frame_json(&self) -> Value {
        match self {
            Self::PlanCompleted {
                plan,
                digest,
                quiescent,
            } => {
                json!({"kind": "plan_completed", "plan": plan, "digest": digest, "quiescent": quiescent})
            }
            Self::ImplementationSettled { quiescent } => {
                json!({"kind": "implementation_settled", "quiescent": quiescent})
            }
            Self::VerificationPass {
                content_digest,
                evidence,
                requirements,
                completeness,
            } => json!({
                "kind": "verification_pass",
                "contentDigest": content_digest,
                "evidence": evidence,
                "requirements": requirements,
                "completeness": completeness,
            }),
            Self::VerificationFail {
                content_digest,
                evidence,
                requirements,
                completeness,
            } => json!({
                "kind": "verification_fail",
                "contentDigest": content_digest,
                "evidence": evidence,
                "requirements": requirements,
                "completeness": completeness,
            }),
            Self::VerificationError { detail } => {
                json!({"kind": "verification_error", "detail": detail})
            }
            Self::VerificationIncomplete { detail } => {
                json!({"kind": "verification_incomplete", "detail": detail})
            }
            Self::Uncertainty { reason, detail } => {
                json!({"kind": "uncertainty", "reason": reason.as_str(), "detail": detail})
            }
            Self::PublicationObserved {
                content_digest,
                commit_sha,
                tree_sha,
                pr_id,
            } => json!({
                "kind": "publication_observed",
                "contentDigest": content_digest,
                "commitSha": commit_sha,
                "treeSha": tree_sha,
                "prId": pr_id,
            }),
            Self::Idle => json!({"kind": "idle"}),
        }
    }
}

fn required_completeness(value: &Value, kind: &str) -> Result<String> {
    let raw = value
        .get("completeness")
        .ok_or_else(|| Error::guard(format!("notification {kind} requires completeness")))?;
    let text = raw.as_str().ok_or_else(|| {
        Error::guard(format!("notification {kind} completeness must be a string"))
    })?;
    let parsed = text
        .parse::<crate::domain::Completeness>()
        .map_err(|_| Error::guard(format!("notification {kind} has invalid completeness")))?;
    Ok(parsed.as_str().to_string())
}

#[derive(Clone, Debug)]
struct JournalEntry {
    payload_hash: String,
    receipt_id: String,
    method: String,
    mission_id: String,
    attempt_id: String,
    session_id: String,
}

/// In-process fake [`AgentRuntime`](FakeRuntime) with a durable admission journal.
///
/// The journal is separate from the mission database. Same request id and same
/// payload hash return the original receipt. A different payload or method is
/// `command_conflict`. Notifications are an ephemeral queue; they are not
/// recovery authority and they cannot name a mission phase.
pub struct FakeRuntime {
    path: std::path::PathBuf,
    submissions: BTreeMap<String, JournalEntry>,
    admission_order: Vec<String>,
    notifications: VecDeque<RuntimeNotification>,
    fail_next_persist: bool,
    simulated_files: BTreeMap<String, String>,
    simulated_effect_count: usize,
    #[cfg(test)]
    fail_simulated_effect: bool,
}

impl FakeRuntime {
    pub fn open(path: &Path) -> Result<Self> {
        let submissions = if path.exists() {
            load_journal(path)?
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            path: path.to_path_buf(),
            submissions,
            admission_order: Vec::new(),
            notifications: VecDeque::new(),
            fail_next_persist: false,
            simulated_files: BTreeMap::new(),
            simulated_effect_count: 0,
            #[cfg(test)]
            fail_simulated_effect: false,
        })
    }

    pub fn push_notification(&mut self, notification: RuntimeNotification) {
        self.notifications.push_back(notification);
    }

    pub fn pop_notification(&mut self) -> Option<RuntimeNotification> {
        self.notifications.pop_front()
    }

    pub fn requeue_front(&mut self, notification: RuntimeNotification) {
        self.notifications.push_front(notification);
    }

    pub fn queued_notifications(&self) -> usize {
        self.notifications.len()
    }

    /// Test seam: the next journal write fails and does not keep the admission.
    pub fn fail_next_persist(&mut self) {
        self.fail_next_persist = true;
    }

    pub fn stored_receipt(&self, request_id: &str) -> Option<RuntimeReceipt> {
        self.submissions
            .get(request_id)
            .map(|entry| RuntimeReceipt {
                receipt_id: entry.receipt_id.clone(),
                request_id: request_id.to_string(),
                payload_hash: entry.payload_hash.clone(),
            })
    }

    pub fn admission_order(&self) -> &[String] {
        &self.admission_order
    }

    pub fn submission_count(&self) -> usize {
        self.submissions.len()
    }

    pub fn submit(&mut self, request: &RuntimeRequest) -> Result<RuntimeReceipt> {
        self.admit(request)
    }

    pub fn steer(&mut self, request: &RuntimeRequest) -> Result<RuntimeReceipt> {
        self.admit(request)
    }

    pub fn abort(&mut self, request: &RuntimeRequest) -> Result<RuntimeReceipt> {
        self.admit(request)
    }

    fn admit(&mut self, request: &RuntimeRequest) -> Result<RuntimeReceipt> {
        if request.binding.mission_id.is_empty()
            || request.binding.attempt_id.is_empty()
            || request.binding.session_id.is_empty()
            || request.request_id.is_empty()
        {
            return Err(Error::BindingMismatch {
                detail: "runtime request is missing mission, attempt, session, or request id"
                    .into(),
            });
        }
        if let Some(existing) = self.submissions.get(&request.request_id) {
            if existing.payload_hash != request.payload_hash || existing.method != request.method {
                return Err(Error::CommandConflict {
                    command_id: request.request_id.clone(),
                });
            }
            if existing.attempt_id != request.binding.attempt_id
                || existing.mission_id != request.binding.mission_id
                || existing.session_id != request.binding.session_id
            {
                return Err(Error::BindingMismatch {
                    detail: "runtime request binding does not match the admitted submission".into(),
                });
            }
            return Ok(RuntimeReceipt {
                receipt_id: existing.receipt_id.clone(),
                request_id: request.request_id.clone(),
                payload_hash: existing.payload_hash.clone(),
            });
        }
        let receipt_id = format!("r-{}", uuid::Uuid::new_v4());
        self.submissions.insert(
            request.request_id.clone(),
            JournalEntry {
                payload_hash: request.payload_hash.clone(),
                receipt_id: receipt_id.clone(),
                method: request.method.to_string(),
                mission_id: request.binding.mission_id.clone(),
                attempt_id: request.binding.attempt_id.clone(),
                session_id: request.binding.session_id.clone(),
            },
        );
        if let Err(error) = self.persist() {
            self.submissions.remove(&request.request_id);
            return Err(error);
        }
        self.admission_order.push(request.request_id.clone());
        Ok(RuntimeReceipt {
            receipt_id,
            request_id: request.request_id.clone(),
            payload_hash: request.payload_hash.clone(),
        })
    }

    fn persist(&mut self) -> Result<()> {
        if self.fail_next_persist {
            self.fail_next_persist = false;
            return Err(Error::storage("injected journal persistence failure"));
        }
        let mut submissions = serde_json::Map::new();
        for (request_id, entry) in &self.submissions {
            submissions.insert(
                request_id.clone(),
                json!({
                    "payloadHash": entry.payload_hash,
                    "receiptId": entry.receipt_id,
                    "method": entry.method,
                    "missionId": entry.mission_id,
                    "attemptId": entry.attempt_id,
                    "sessionId": entry.session_id,
                }),
            );
        }
        let body = serde_json::to_vec_pretty(&json!({
            "adapter": "fake",
            "credentials": "none",
            "submissions": submissions,
        }))
        .map_err(Error::storage)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(Error::storage)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp)
                .map_err(Error::storage)?;
            file.write_all(&body).map_err(Error::storage)?;
            file.sync_all().map_err(Error::storage)?;
        }
        fs::rename(&tmp, &self.path).map_err(Error::storage)?;
        Ok(())
    }
}

fn load_journal(path: &Path) -> Result<BTreeMap<String, JournalEntry>> {
    let text = fs::read_to_string(path).map_err(Error::storage)?;
    let value: Value = serde_json::from_str(&text).map_err(|error| Error::Storage {
        detail: format!("fake runtime journal is corrupt: {error}"),
    })?;
    let mut submissions = BTreeMap::new();
    let Some(map) = value.get("submissions").and_then(Value::as_object) else {
        return Err(Error::Storage {
            detail: "fake runtime journal is missing submissions".into(),
        });
    };
    for (request_id, entry) in map {
        let field = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| Error::Storage {
                    detail: format!("fake runtime journal entry {request_id} is missing {key}"),
                })
        };
        submissions.insert(
            request_id.clone(),
            JournalEntry {
                payload_hash: field("payloadHash")?,
                receipt_id: field("receiptId")?,
                method: field("method")?,
                mission_id: field("missionId")?,
                attempt_id: field("attemptId")?,
                session_id: field("sessionId")?,
            },
        );
    }
    Ok(submissions)
}

impl FakeRuntime {
    #[cfg(test)]
    pub(crate) fn fail_next_simulated_effect(&mut self) {
        self.fail_simulated_effect = true;
    }

    /// Effects applied during this process; completed replay does not increment it.
    pub fn simulated_effect_count(&self) -> usize {
        self.simulated_effect_count
    }

    /// In-memory simulated file map. Replay of an abandoned effect must not change it.
    #[cfg(test)]
    pub(crate) fn simulated_file_map(&self) -> Vec<(String, String)> {
        self.simulated_files
            .iter()
            .map(|(path, content)| (path.clone(), content.clone()))
            .collect()
    }
}
impl crate::execution_env::ExecutionEnv for FakeRuntime {
    fn apply(&mut self, request: &crate::ExecutionRequest) -> Result<crate::ExecutionReceipt> {
        let crate::ExecutionOperation::WriteFile { path, content } = &request.operation;
        let receipt = crate::ExecutionReceipt {
            operation_id: request.operation_id()?,
            payload_hash: request.payload_hash()?,
            simulated: true,
            bytes_written: content.len(),
            content_hash: sha256_hex(content.as_bytes()),
        };
        self.simulated_files.insert(path.clone(), content.clone());
        self.simulated_effect_count += 1;
        #[cfg(test)]
        if std::mem::take(&mut self.fail_simulated_effect) {
            return Err(Error::storage("injected error after simulated effect"));
        }
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_identity_ignores_arguments_and_is_stable() {
        let canonical = r#"["switchyard.exec.v1","a-1","s-1","task-1","write-file:v1"]"#;
        assert_eq!(
            canonicalize(&json!([
                "switchyard.exec.v1",
                "a-1",
                "s-1",
                "task-1",
                "write-file:v1"
            ]))
            .unwrap(),
            canonical
        );
        let first = operation_id("a-1", "s-1", "task-1", "write-file:v1").unwrap();
        let again = operation_id("a-1", "s-1", "task-1", "write-file:v1").unwrap();
        let other = operation_id("a-1", "s-1", "task-1", "exec:start:v1").unwrap();
        assert_eq!(first, again);
        assert_eq!(
            first,
            "op-v1:0483300ffc3898ba240af9eb1cfc1655a4d0f1cffb7fdb561a32759e2b6103a4"
        );
        assert_ne!(first, other);
    }

    #[test]
    fn set_status_is_not_a_notification() {
        let error =
            parse_notification(&json!({"kind": "set_status", "status": "published"})).unwrap_err();
        assert_eq!(error.category(), "unknown_notification");
        let parsed = parse_notification(&json!({
            "kind": "plan_completed",
            "plan": "do the work",
            "digest": "sha256:abc",
            "quiescent": true,
            "status": "published"
        }))
        .unwrap();
        match parsed {
            RuntimeNotification::PlanCompleted {
                plan, quiescent, ..
            } => {
                assert_eq!(plan, "do the work");
                assert!(quiescent);
            }
            other => panic!("status field must be ignored, got {other:?}"),
        }
    }
}
