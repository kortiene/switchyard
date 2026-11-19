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
pub(crate) const WORK_BRIEF_SCHEMA: &str = "switchyard.work-brief.v1";

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

/// Body hashed as the plan digest.
///
/// The object is `{schema, attemptId, frozenInputsDigest, plan}`.
/// `frozenInputsDigest` is `payload_hash` of the whole stored frozen-input
/// document, so one field binds all eight inputs. There is no separate
/// sandbox digest. Host admission stores and checks this value. An adapter
/// authority profile does not.
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

/// Plan digest for the stored freeze document.
///
/// `frozen_inputs_json` is the canonical `switchyard.attempt-freeze.v1`
/// document already written on the attempt. Its `payload_hash` is the
/// `frozenInputsDigest` inside the plan-digest document.
pub fn bound_plan_digest(plan: &str, attempt_id: &str, frozen_inputs_json: &str) -> Result<String> {
    let document: Value = serde_json::from_str(frozen_inputs_json)
        .map_err(|_| Error::guard("frozen inputs are not valid JSON"))?;
    let frozen_inputs_digest = payload_hash(&document)?;
    plan_digest_hash(plan, attempt_id, &frozen_inputs_digest)
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

/// Model input for `approve_plan`, `authorize_bounded_repair`, and
/// `request_changes`. RFC 8785 JSON. Built only from the stored freeze and
/// the approved plan. `writeScope` is the frozen sandbox policy.
/// `repairFeedback` is omitted unless the command carries feedback.
pub fn work_brief(
    frozen_inputs_json: &str,
    plan: &str,
    plan_digest: &str,
    repair_feedback: Option<&str>,
) -> Result<String> {
    let frozen: Value = serde_json::from_str(frozen_inputs_json)
        .map_err(|_| Error::guard("frozen inputs are not valid JSON"))?;
    if frozen.get("schema").and_then(Value::as_str) != Some(ATTEMPT_FREEZE_SCHEMA) {
        return Err(Error::guard("frozen inputs are not an attempt freeze"));
    }
    let objective = text_field(&frozen, "objective")?;
    let acceptance = text_field(&frozen, "acceptanceCriteria")?;
    let write_scope = frozen
        .get("sandboxPolicy")
        .cloned()
        .filter(|value| value.is_object())
        .ok_or_else(|| Error::guard("frozen inputs have no write scope"))?;
    let verifier = frozen
        .get("verifier")
        .cloned()
        .filter(|value| value.is_object())
        .ok_or_else(|| Error::guard("frozen inputs have no verifier"))?;
    require_text(plan, "plan")?;
    require_text(plan_digest, "plan digest")?;
    let mut document = json!({
        "acceptanceCriteria": acceptance,
        "objective": objective,
        "plan": plan,
        "planDigest": plan_digest,
        "schema": WORK_BRIEF_SCHEMA,
        "verifier": verifier,
        "writeScope": write_scope,
    });
    if let Some(feedback) = repair_feedback {
        require_text(feedback, "repair feedback")?;
        document
            .as_object_mut()
            .expect("work brief is an object")
            .insert("repairFeedback".into(), json!(feedback));
    }
    canonicalize(&document)
}

/// `Some` when this command's model input is the work brief and the freeze
/// plus the approved plan are both present. `None` leaves admission free to
/// report a missing freeze or an empty repair note.
pub(crate) fn optional_work_brief(
    mission: &MissionRecord,
    payload: &Value,
) -> Result<Option<String>> {
    let kind = payload.get("kind").and_then(Value::as_str);
    let feedback = match kind {
        Some("approve_plan") => None,
        Some("authorize_bounded_repair") => match payload.pointer("/body/rationale") {
            Some(Value::String(text)) if !text.trim().is_empty() => Some(text.as_str()),
            _ => return Ok(None),
        },
        Some("request_changes") => match payload.pointer("/body/feedback") {
            Some(Value::String(text)) if !text.trim().is_empty() => Some(text.as_str()),
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };
    let (Some(frozen), Some(plan)) = (
        mission.attempt.frozen_inputs_json.as_deref(),
        mission.plan_body.as_deref(),
    ) else {
        return Ok(None);
    };
    // Prefer the ledger's approved digest. Recompute only when it was not stored.
    let digest = match mission
        .plan_digest
        .as_deref()
        .filter(|digest| !digest.trim().is_empty())
    {
        Some(digest) => digest.to_string(),
        None => bound_plan_digest(plan, &mission.attempt.id, frozen)?,
    };
    Ok(Some(work_brief(frozen, plan, &digest, feedback)?))
}

#[cfg(any(test, feature = "pi-contract"))]
pub(crate) fn command_uses_work_brief(payload: &Value) -> bool {
    matches!(
        payload.get("kind").and_then(Value::as_str),
        Some("approve_plan" | "authorize_bounded_repair" | "request_changes")
    )
}

/// Wire params for a work-brief submission. `hostCommand` stays the command
/// payload and its hash. `content` is the brief bytes.
#[cfg(any(test, feature = "pi-contract"))]
pub(crate) fn work_brief_model_params(payload: &Value, payload_hash: &str, brief: &str) -> Value {
    json!({
        "type": "input",
        "content": brief,
        "hostCommand": {
            "payload": payload,
            "payloadHash": payload_hash,
        },
    })
}

fn text_field(document: &Value, key: &str) -> Result<String> {
    let text = document
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::guard(format!("frozen inputs are missing {key}")))?;
    require_text(text, key)?;
    Ok(text.to_string())
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
        let text_only = payload_hash(&Value::String("do the work".into())).unwrap();
        assert_ne!(digest, text_only);
        let stored = canonicalize(&frozen).unwrap();
        assert_eq!(
            bound_plan_digest("do the work", "a-test", &stored).unwrap(),
            digest
        );
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

    #[test]
    fn work_brief_replay_sends_identical_bytes_and_keeps_host_command() {
        let frozen = frozen_canonical(&mission()).unwrap();
        let plan = "do the work";
        let digest = bound_plan_digest(plan, "a-test", &frozen).unwrap();
        let first = work_brief(&frozen, plan, &digest, None).unwrap();
        let second = work_brief(&frozen, plan, &digest, None).unwrap();
        assert_eq!(first.as_bytes(), second.as_bytes());
        assert_eq!(
            first,
            format!(
                r#"{{"acceptanceCriteria":"Inputs stay put","objective":"Ship the freeze","plan":"do the work","planDigest":"{digest}","schema":"switchyard.work-brief.v1","verifier":{{"executes":false,"frozen":true,"id":"switchyard.verifier.v1"}},"writeScope":{{"environment":{{"confinement":"deferred_to_s3","host":"single"}},"policy":{{"credentials":"none","planningTools":"read-only"}}}}}}"#
            )
        );
        assert!(!first.contains("baseSha"), "{first}");
        assert!(!first.contains("issueSnapshot"), "{first}");
        assert!(!first.contains("runtimeConfig"), "{first}");
        assert!(
            first.contains(&format!(r#""planDigest":"{digest}""#)),
            "{first}"
        );
        assert!(!first.contains("repairFeedback"), "{first}");
        let repaired = work_brief(&frozen, plan, &digest, Some("try again")).unwrap();
        let repaired_again = work_brief(&frozen, plan, &digest, Some("try again")).unwrap();
        assert_eq!(repaired.as_bytes(), repaired_again.as_bytes());
        assert!(
            repaired.contains(r#""repairFeedback":"try again""#),
            "{repaired}"
        );
        assert_ne!(repaired.as_bytes(), first.as_bytes());

        let payload = json!({
            "kind": "approve_plan",
            "missionId": "m-test",
            "attemptId": "a-test",
            "body": {"planDigest": "sha256:plan"},
        });
        let hash = payload_hash(&payload).unwrap();
        let params = work_brief_model_params(&payload, &hash, &first);
        let replay = work_brief_model_params(&payload, &hash, &second);
        assert_eq!(
            canonicalize(&params).unwrap().as_bytes(),
            canonicalize(&replay).unwrap().as_bytes()
        );
        assert_eq!(params["content"], first);
        assert_eq!(params["hostCommand"]["payload"], payload);
        assert_eq!(params["hostCommand"]["payloadHash"], hash);
        assert_ne!(params["content"], canonicalize(&payload).unwrap());
        assert!(command_uses_work_brief(&payload));
        assert!(!command_uses_work_brief(&json!({"kind": "start_planning"})));
    }

    #[test]
    fn cancel_and_abort_do_not_read_a_damaged_plan() {
        let mut stored = mission();
        let frozen = frozen_canonical(&stored).unwrap();
        stored.attempt.frozen_inputs_json = Some(frozen);
        stored.plan_body = Some("   ".into());
        for kind in ["cancel", "pause"] {
            let payload = json!({"kind": kind, "body": {}});
            assert_eq!(
                optional_work_brief(&stored, &payload).unwrap(),
                None,
                "{kind}"
            );
        }
        let approve = json!({"kind": "approve_plan", "body": {"planDigest": "sha256:x"}});
        assert!(optional_work_brief(&stored, &approve).is_err());
    }
}
