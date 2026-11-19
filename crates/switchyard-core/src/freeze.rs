//! Host admission for the `draft → planning` freeze.
//!
//! This is not an adapter authority check. `AuthorityProfile.certifies_mission_facts`
//! does not decide whether these inputs are complete, canonical, or unchanged.
use serde_json::{Value, json};

use crate::canonical::{canonicalize, payload_hash};
use crate::domain::{CommandKind, MissionRecord, Phase};
use crate::error::{Error, Result};

pub(crate) const ATTEMPT_FREEZE_SCHEMA: &str = "switchyard.attempt-freeze.v1";

/// Canonical freeze `1a12f25` wrote for the unit-test mission. Budget keys are
/// absent, and `planSource` is absent, which is `runtime`. A newer binary must
/// rebuild these bytes when the attempt policy has neither.
#[cfg(test)]
pub(crate) const FREEZE_AT_1A12F25: &str = concat!(
    r#"{"acceptanceCriteria":"Inputs stay put","baseSha":"abc","executionLimits":{"maxPlanRevisions":3,"maxRepairs":2},"#,
    r#""issueSnapshot":{"number":172,"provider":"github","snapshotAt":10,"title":"S4-A"},"objective":"Ship the freeze","#,
    r#""runtimeConfig":{"adapter":"fake","credentials":"none"},"sandboxPolicy":{"environment":{"confinement":"deferred_to_s3","host":"single"},"#,
    r#""policy":{"credentials":"none","planningTools":"read-only"}},"schema":"switchyard.attempt-freeze.v1","#,
    r#""verifier":{"executes":false,"frozen":true,"id":"switchyard.verifier.v1"}}"#
);
pub(crate) const PLAN_DIGEST_SCHEMA: &str = "switchyard.plan-digest.v1";

/// Endpoint missions freeze this object. No-endpoint missions omit it, so
/// `FREEZE_AT_1A12F25` and every other no-endpoint freeze stay byte-identical.
pub(crate) fn model_harness_off() -> Value {
    json!({
        "restartLimit": 0,
        "retryEnabled": false,
        "retryMaxRetries": 0,
        "streamMaxRetries": 0,
    })
}

const BUDGET_KEYS: [&str; 6] = [
    "attemptWallSeconds",
    "bridgeCpuSeconds",
    "bridgeWallSeconds",
    "maxModelRequests",
    "maxRequestBytes",
    "maxResponseBytes",
];

/// Who writes the plan. Frozen on the attempt at `start_planning`.
///
/// Absent on a stored freeze, or absent from attempt policy before that write,
/// means [`PlanSource::Runtime`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanSource {
    /// The runtime writes the plan. This is the default, including Fake `#213`.
    Runtime,
    /// The operator writes the plan. Planning freezes and enqueues no runtime work.
    Operator,
}

impl PlanSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Operator => "operator",
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "runtime" => Ok(Self::Runtime),
            "operator" => Ok(Self::Operator),
            _ => Err(Error::guard("planSource must be operator or runtime")),
        }
    }
}

/// Plan source the host acts on.
///
/// The stored freeze wins after `start_planning`. A missing `planSource` is
/// [`PlanSource::Runtime`]. Before the freeze, the attempt policy is the source,
/// and a missing key is the same default.
pub(crate) fn planning_plan_source(mission: &MissionRecord) -> Result<PlanSource> {
    if let Some(frozen) = &mission.attempt.frozen_inputs_json {
        return plan_source_value(frozen, "frozen inputs");
    }
    plan_source_value(&mission.attempt.policy_json, "sandbox policy")
}

fn plan_source_value(raw: &str, label: &str) -> Result<PlanSource> {
    let value = parse_object(raw, label)?;
    match value.get("planSource") {
        None => Ok(PlanSource::Runtime),
        Some(Value::String(text)) => PlanSource::parse(text),
        Some(_) => Err(Error::guard("planSource must be operator or runtime")),
    }
}

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
    let plan_source = plan_source_value(&mission.attempt.policy_json, "sandbox policy")?;
    let mut document = json!({
        "acceptanceCriteria": mission.acceptance_criteria,
        "baseSha": mission.attempt.base_sha,
        "executionLimits": execution_limits(mission)?,
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
    });
    // `runtime` stays absent so a freeze written before this field still
    // matches byte for byte. Absence reads as `runtime`.
    if plan_source == PlanSource::Operator {
        document["planSource"] = json!("operator");
    }
    Ok(document)
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
        object.remove("planSource");
        object.remove("modelHarness");
        for key in BUDGET_KEYS {
            object.remove(key);
        }
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

/// `maxPlanRevisions` and `maxRepairs` are always present. Each §3.4 budget
/// key is copied from the attempt policy only when that key is already
/// there. Absence is the pre-#218 freeze, not an error.
fn execution_limits(mission: &MissionRecord) -> Result<Value> {
    let policy = parse_object(&mission.attempt.policy_json, "sandbox policy")?;
    let mut limits = serde_json::Map::new();
    limits.insert(
        "maxPlanRevisions".into(),
        json!(mission.attempt.max_plan_revisions),
    );
    limits.insert("maxRepairs".into(), json!(mission.attempt.max_repairs));
    if let Some(object) = policy.as_object() {
        for key in BUDGET_KEYS {
            if let Some(value) = object
                .get(key)
                .filter(|value| value.is_u64() || value.is_i64())
            {
                limits.insert(key.into(), value.clone());
            }
        }
        if let Some(value) = object.get("modelHarness").filter(|value| value.is_object()) {
            limits.insert("modelHarness".into(), value.clone());
        }
    }
    Ok(Value::Object(limits))
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
        assert_eq!(canonical, FREEZE_AT_1A12F25);
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
        assert!(document.get("planSource").is_none());
        assert!(
            document["sandboxPolicy"]["policy"]
                .get("planSource")
                .is_none()
        );
        assert_eq!(
            frozen_canonical(&mission()).unwrap(),
            super::FREEZE_AT_1A12F25
        );
    }

    #[test]
    fn endpoint_freeze_carries_model_harness_and_a_plain_freeze_does_not() {
        let plain = frozen_document(&mission()).unwrap();
        assert!(plain["executionLimits"].get("modelHarness").is_none());
        assert_eq!(frozen_canonical(&mission()).unwrap(), FREEZE_AT_1A12F25);
        let mut record = mission();
        let mut policy = parse_object(&record.attempt.policy_json, "sandbox policy").unwrap();
        policy["modelHarness"] = model_harness_off();
        record.attempt.policy_json = policy.to_string();
        let document = frozen_document(&record).unwrap();
        assert_eq!(
            document["executionLimits"]["modelHarness"],
            model_harness_off()
        );
        assert!(
            document["sandboxPolicy"]["policy"]
                .get("modelHarness")
                .is_none()
        );
        assert_ne!(frozen_canonical(&record).unwrap(), FREEZE_AT_1A12F25);
    }

    #[test]
    fn operator_plan_source_is_frozen_and_kept_out_of_sandbox_policy() {
        let mut record = mission();
        record.attempt.policy_json = r#"{"credentials":"none","maxPlanRevisions":3,"maxRepairs":2,"planSource":"operator","planningTools":"read-only"}"#.into();
        let document = frozen_document(&record).unwrap();
        assert_eq!(document["planSource"], "operator");
        assert!(
            document["sandboxPolicy"]["policy"]
                .get("planSource")
                .is_none()
        );
        assert_eq!(
            document["sandboxPolicy"]["policy"]["planningTools"],
            "read-only"
        );
    }
}
