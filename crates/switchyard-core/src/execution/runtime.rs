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
            completeness: text("completeness").unwrap_or_else(|_| "complete".into()),
        }),
        "verification_fail" => Ok(RuntimeNotification::VerificationFail {
            content_digest: text("contentDigest")?,
            evidence: text("evidence")?,
            requirements: requirements()?,
            completeness: text("completeness").unwrap_or_else(|_| "complete".into()),
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
    notifications: VecDeque<RuntimeNotification>,
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
            notifications: VecDeque::new(),
        })
    }

    pub fn push_notification(&mut self, notification: RuntimeNotification) {
        self.notifications.push_back(notification);
    }

    pub fn drain_notifications(&mut self) -> Vec<RuntimeNotification> {
        self.notifications.drain(..).collect()
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
        self.persist()?;
        Ok(RuntimeReceipt {
            receipt_id,
            request_id: request.request_id.clone(),
            payload_hash: request.payload_hash.clone(),
        })
    }

    fn persist(&self) -> Result<()> {
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
