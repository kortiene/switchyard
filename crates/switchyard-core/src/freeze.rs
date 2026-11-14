//! Host admission for the `draft → planning` freeze.
//!
//! This is not an adapter authority check. `AuthorityProfile.certifies_mission_facts`
//! does not decide whether these inputs are complete, canonical, or unchanged.
use serde_json::{Map, Value, json};

use crate::canonical::{canonicalize, payload_hash};
use crate::domain::MissionRecord;
use crate::error::{Error, Result};

/// Canonical frozen-input document for one attempt.
///
/// The encoding is RFC 8785 JSON (`canonicalize`): object keys sorted by
/// UTF-16 code units, no insignificant whitespace, integers only. The
/// decision digest is `payload_hash` of this value (`sha256:` plus hex
/// SHA-256). `sessionInitialized` is a session marker, not model or runtime
/// configuration, and is omitted.
pub(crate) fn frozen_document(mission: &MissionRecord) -> Result<Value> {
    require_text(&mission.objective, "objective")?;
    require_text(&mission.acceptance_criteria, "acceptance criteria")?;
    require_text(&mission.attempt.base_sha, "base SHA")?;
    require_text(&mission.source_issue.provider, "issue snapshot")?;
    Ok(json!({
        "acceptanceCriteria": mission.acceptance_criteria,
        "baseSha": mission.attempt.base_sha,
        "executionLimits": {
            "maxPlanRevisions": mission.attempt.max_plan_revisions,
            "maxRepairs": mission.attempt.max_repairs,
        },
        "issueSnapshot": {
            "number": mission.source_issue.number,
            "provider": mission.source_issue.provider,
            "snapshotAt": mission.source_issue.snapshot_at,
            "title": mission.source_issue.title,
        },
        "objective": mission.objective,
        "runtimeConfig": runtime_configuration(&mission.attempt.runtime_config_json)?,
        "sandboxPolicy": sandbox_policy(
            &mission.attempt.environment_json,
            &mission.attempt.policy_json,
        )?,
        "verifier": parse_object(&mission.attempt.verifier_json, "verifier definition")?,
    }))
}

pub(crate) fn frozen_canonical(mission: &MissionRecord) -> Result<String> {
    canonicalize(&frozen_document(mission)?)
}

/// Body the following slice will hash as the plan digest.
///
/// Not stored or checked by plan approval in this slice. The hash is
/// `payload_hash` (RFC 8785, then `sha256:` plus hex SHA-256) of this object:
/// the plan text plus the frozen base SHA, sandbox policy, verifier, and
/// execution limits. The unit test is the caller until that slice lands.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_digest_document(plan: &str, frozen: &Value) -> Value {
    json!({
        "baseSha": frozen.get("baseSha").cloned().unwrap_or(Value::Null),
        "executionLimits": frozen.get("executionLimits").cloned().unwrap_or(Value::Null),
        "plan": plan,
        "sandboxPolicy": frozen.get("sandboxPolicy").cloned().unwrap_or(Value::Null),
        "verifier": frozen.get("verifier").cloned().unwrap_or(Value::Null),
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_digest_hash(plan: &str, frozen: &Value) -> Result<String> {
    payload_hash(&plan_digest_document(plan, frozen))
}

fn runtime_configuration(raw: &str) -> Result<Value> {
    let mut value = parse_object(raw, "runtime configuration")?;
    if let Some(object) = value.as_object_mut() {
        object.remove("sessionInitialized");
    }
    Ok(value)
}

fn sandbox_policy(environment_json: &str, policy_json: &str) -> Result<Value> {
    let environment = parse_object(environment_json, "sandbox policy")?;
    let policy = parse_object(policy_json, "sandbox policy")?;
    let mut sandbox = Map::new();
    if let Some(object) = environment.as_object() {
        for (key, value) in object {
            sandbox.insert(key.clone(), value.clone());
        }
    }
    if let Some(object) = policy.as_object() {
        for (key, value) in object {
            if key == "maxRepairs" || key == "maxPlanRevisions" {
                continue;
            }
            sandbox.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(sandbox))
}

pub(crate) fn parse_object(raw: &str, label: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|_| Error::guard(format!("{label} is not valid JSON")))?;
    if !value.is_object() {
        return Err(Error::guard(format!("{label} must be a JSON object")));
    }
    Ok(value)
}

fn require_text(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(Error::guard(format!(
            "{label} is required to start planning"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{AttemptRecord, MissionRecord, Phase, RepoIdentity, SourceIssue};
    use crate::policy::plan_digest_for;

    fn mission() -> MissionRecord {
        MissionRecord {
            id: "m-test".into(),
            revision: 1,
            phase: Phase::Draft,
            objective: "Ship the freeze".into(),
            repo: RepoIdentity {
                toplevel: "/repo".into(),
                head_sha: "abc".into(),
                branch: "main".into(),
                porcelain_hash: "sha256:00".into(),
                origin_url: None,
            },
            source_issue: SourceIssue {
                provider: "github".into(),
                number: 172,
                title: "S4-A".into(),
                snapshot_at: 10,
            },
            resume_target: None,
            prior_phase: None,
            attention_reason: None,
            permitted_recovery_target: None,
            reconcile_only: false,
            content_digest: "digest".into(),
            plan_digest: None,
            plan_body: None,
            reviewed_digest: None,
            successor_of_mission_id: None,
            task_id: "t-test".into(),
            acceptance_criteria: "Inputs stay put".into(),
            attempt: AttemptRecord {
                id: "a-test".into(),
                task_id: "t-test".into(),
                base_sha: "abc".into(),
                session_id: "s-test".into(),
                runtime_adapter: "fake".into(),
                runtime_config_json:
                    r#"{"adapter":"fake","credentials":"none","sessionInitialized":true}"#.into(),
                policy_json: r#"{"credentials":"none","maxPlanRevisions":3,"maxRepairs":2,"planningTools":"read-only"}"#.into(),
                policy_hash: "sha256:policy".into(),
                verifier_json: r#"{"executes":false,"frozen":true,"id":"switchyard.verifier.v1"}"#.into(),
                environment_json: r#"{"confinement":"deferred_to_s3","host":"single"}"#.into(),
                repair_count: 0,
                max_repairs: 2,
                plan_revision_count: 0,
                max_plan_revisions: 3,
                successor_of: None,
                frozen_inputs_json: None,
                owner_generation: None,
            },
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn frozen_document_is_sorted_rfc8785_and_drops_the_session_marker() {
        let canonical = frozen_canonical(&mission()).unwrap();
        assert!(!canonical.contains("sessionInitialized"), "{canonical}");
        assert!(
            canonical.starts_with(r#"{"acceptanceCriteria":"Inputs stay put","baseSha":"abc","#)
        );
        let again = frozen_canonical(&mission()).unwrap();
        assert_eq!(canonical, again);
        let hash = payload_hash(&frozen_document(&mission()).unwrap()).unwrap();
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash.len(), "sha256:".len() + 64);
    }

    #[test]
    fn plan_digest_document_binds_the_plan_to_frozen_limits_not_the_plan_text_alone() {
        let frozen = frozen_document(&mission()).unwrap();
        let canonical = canonicalize(&plan_digest_document("do the work", &frozen)).unwrap();
        assert_eq!(
            canonical,
            concat!(
                r#"{"baseSha":"abc","executionLimits":{"maxPlanRevisions":3,"maxRepairs":2},"plan":"do the work","sandboxPolicy":{"#,
                r#""confinement":"deferred_to_s3","credentials":"none","host":"single","planningTools":"read-only"},"verifier":{"#,
                r#""executes":false,"frozen":true,"id":"switchyard.verifier.v1"}}"#
            )
        );
        let digest = plan_digest_hash("do the work", &frozen).unwrap();
        assert_ne!(digest, plan_digest_for("do the work").unwrap());
    }
}
