//! Host admission for the `draft → planning` freeze.
//!
//! This is not an adapter authority check. `AuthorityProfile.certifies_mission_facts`
//! does not decide whether these inputs are complete, canonical, or unchanged.
use serde_json::{Value, json};

use crate::canonical::{canonicalize, payload_hash};
use crate::domain::{CommandKind, MissionRecord, Phase};
use crate::error::{Error, Result};

pub(crate) const ATTEMPT_FREEZE_SCHEMA: &str = "switchyard.attempt-freeze.v1";
pub(crate) const PLAN_DIGEST_SCHEMA: &str = "switchyard.plan-digest.v1";

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
        "schema": ATTEMPT_FREEZE_SCHEMA,
        "verifier": parse_object(&mission.attempt.verifier_json, "verifier definition")?,
    }))
}

/// Commands that may still be admitted after the frozen inputs drift.
///
/// The normative table keeps cancel available from every nonterminal phase
/// except `cancelling`, and reject from `awaiting_plan_approval`,
/// `awaiting_review`, and `approved`. `confirm_cancelled` stays open so that
/// cancellation can settle. `detect_uncertainty` is the edge into
/// `needs_attention`. Nothing here rewrites the freeze.
pub(crate) fn drift_exit(phase: Phase, kind: CommandKind) -> bool {
    match kind {
        CommandKind::Cancel => !phase.is_terminal() && phase != Phase::Cancelling,
        CommandKind::ConfirmCancelled => phase == Phase::Cancelling,
        CommandKind::Reject => matches!(
            phase,
            Phase::AwaitingPlanApproval | Phase::AwaitingReview | Phase::Approved
        ),
        CommandKind::DetectUncertainty => !phase.is_terminal() && phase != Phase::NeedsAttention,
        _ => false,
    }
}

pub(crate) fn frozen_canonical(mission: &MissionRecord) -> Result<String> {
    canonicalize(&frozen_document(mission)?)
}

/// Body the following slice will hash as the plan digest.
///
/// Not stored or checked by plan approval in this slice. The object is
/// `{schema, attemptId, frozenInputsDigest, plan}`. `frozenInputsDigest` is
/// `payload_hash` of the whole frozen-input document, so one field binds all
/// eight inputs. There is no separate sandbox digest.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_digest_document(
    plan: &str,
    attempt_id: &str,
    frozen_inputs_digest: &str,
) -> Result<Value> {
    require_text(plan, "plan")?;
    require_text(attempt_id, "attempt id")?;
    require_text(frozen_inputs_digest, "frozen inputs digest")?;
    Ok(json!({
        "attemptId": attempt_id,
        "frozenInputsDigest": frozen_inputs_digest,
        "plan": plan,
        "schema": PLAN_DIGEST_SCHEMA,
    }))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_digest_hash(
    plan: &str,
    attempt_id: &str,
    frozen_inputs_digest: &str,
) -> Result<String> {
    payload_hash(&plan_digest_document(
        plan,
        attempt_id,
        frozen_inputs_digest,
    )?)
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
    let mut policy = parse_object(policy_json, "sandbox policy")?;
    if let Some(object) = policy.as_object_mut() {
        object.remove("maxRepairs");
        object.remove("maxPlanRevisions");
    }
    Ok(json!({
        "environment": environment,
        "policy": policy,
    }))
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
        Err(Error::guard(format!("{label} is required")))
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
    fn plan_digest_document_binds_the_plan_to_the_frozen_input_digest() {
        let frozen = frozen_document(&mission()).unwrap();
        let inputs = payload_hash(&frozen).unwrap();
        let canonical =
            canonicalize(&plan_digest_document("do the work", "a-test", &inputs).unwrap()).unwrap();
        assert_eq!(
            canonical,
            format!(
                r#"{{"attemptId":"a-test","frozenInputsDigest":"{inputs}","plan":"do the work","schema":"{PLAN_DIGEST_SCHEMA}"}}"#
            )
        );
        let digest = plan_digest_hash("do the work", "a-test", &inputs).unwrap();
        assert_ne!(digest, plan_digest_for("do the work").unwrap());
        assert!(plan_digest_document("  ", "a-test", &inputs).is_err());
        assert!(plan_digest_document("do the work", "", &inputs).is_err());
        assert!(plan_digest_document("do the work", "a-test", "").is_err());
    }

    #[test]
    fn sandbox_policy_keeps_environment_and_policy_apart() {
        let document = frozen_document(&mission()).unwrap();
        assert_eq!(document["schema"], ATTEMPT_FREEZE_SCHEMA);
        assert_eq!(document["sandboxPolicy"]["environment"]["host"], "single");
        assert_eq!(
            document["sandboxPolicy"]["policy"]["planningTools"],
            "read-only"
        );
        assert!(
            document["sandboxPolicy"]["policy"]
                .get("maxRepairs")
                .is_none()
        );
        assert!(
            document["sandboxPolicy"]["policy"]
                .get("maxPlanRevisions")
                .is_none()
        );
        assert!(document["sandboxPolicy"].get("host").is_none());
    }
}
