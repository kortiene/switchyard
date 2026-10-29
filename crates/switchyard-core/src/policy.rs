use crate::canonical::payload_hash;
use crate::domain::{
    AttemptRecord, AttentionReason, CommandBody, DeliveryRecord, MissionRecord, Phase,
    PublicationProof, VerificationFact, VerificationOutcome, content_drift,
    validated_recovery_target,
};
use crate::error::{Error, Result};
use crate::transitions::TargetSpec;

/// Facts Rust already stored. A runtime notification cannot replace these.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub ops_blocked: bool,
    pub verification: Option<VerificationFact>,
    pub delivery: Option<DeliveryRecord>,
    pub publication_receipt: Option<PublicationProof>,
    pub pending_pause: bool,
    pub pending_cancel: bool,
    pub acknowledged_pause: bool,
    pub acknowledged_cancel: bool,
    pub admitted_runtime_work: bool,
}

#[derive(Clone, Debug)]
pub struct TransitionPlan {
    pub next: Phase,
    pub resume_target: Option<Phase>,
    pub prior_phase: Option<Phase>,
    pub attention_reason: Option<AttentionReason>,
    pub permitted_recovery_target: Option<Phase>,
    pub reconcile_only: bool,
    pub content_digest: String,
    pub plan_digest: Option<String>,
    pub plan_body: Option<String>,
    pub reviewed_digest: Option<String>,
    pub repair_count: u32,
    pub plan_revision_count: u32,
    pub invalidate_evidence: bool,
    pub invalidate_reviews: bool,
    pub invalidate_plan_approval: bool,
    pub decision_digest: Option<String>,
    pub delivery_insert: Option<NewDelivery>,
    pub delivery_publish: Option<PublicationProof>,
}

#[derive(Clone, Debug)]
pub struct NewDelivery {
    pub target_branch: String,
    pub base_branch: String,
    pub approved_digest: String,
}

pub fn authorize(mission: &MissionRecord, body: &CommandBody, facts: &Facts) -> Result<()> {
    let blocked = || Error::guard("runtime or writers are not quiescent");
    match body {
        CommandBody::StartPlanning => Ok(()),
        CommandBody::PlanCompleted {
            plan,
            digest,
            quiescent,
        } => {
            if !quiescent || facts.ops_blocked {
                return Err(blocked());
            }
            require_plan_digest(plan, digest)
        }
        CommandBody::RequestRevisedPlan { feedback } => {
            require_nonempty(feedback, "feedback")?;
            if mission.attempt.plan_revision_count >= mission.attempt.max_plan_revisions {
                return Err(Error::UsageLimitExhausted {
                    count: mission.attempt.plan_revision_count,
                    max: mission.attempt.max_plan_revisions,
                });
            }
            Ok(())
        }
        CommandBody::ApprovePlan { plan_digest } => match &mission.plan_digest {
            Some(current) if current == plan_digest => Ok(()),
            _ => Err(Error::guard("plan digest does not match the frozen plan")),
        },
        CommandBody::RequestVerification { quiescent } => {
            if !quiescent || facts.ops_blocked {
                return Err(blocked());
            }
            Ok(())
        }
        CommandBody::CompletePass => require_pass(mission, facts),
        CommandBody::AuthorizeBoundedRepair { rationale } => {
            require_nonempty(rationale, "rationale")?;
            require_repairable_fail(mission, facts)?;
            require_repair_allowance(&mission.attempt)?;
            if facts.ops_blocked {
                return Err(blocked());
            }
            Ok(())
        }
        CommandBody::RequestChanges {
            feedback,
            within_frozen,
        } => {
            require_nonempty(feedback, "feedback")?;
            if !within_frozen {
                return Err(Error::guard("request changes expands the frozen scope"));
            }
            require_repair_allowance(&mission.attempt)?;
            if facts.ops_blocked {
                return Err(blocked());
            }
            Ok(())
        }
        CommandBody::ApproveReview { digest, rationale } => {
            require_nonempty(rationale, "rationale")?;
            if digest != &mission.content_digest {
                return Err(Error::guard("review digest does not match current content"));
            }
            if content_drift(mission) {
                return Err(Error::guard("content drift blocks approval"));
            }
            require_pass(mission, facts)
        }
        CommandBody::Publish {
            target_branch,
            base_branch,
            authorized,
        } => {
            if !authorized {
                return Err(Error::guard("publication requires explicit authorization"));
            }
            require_nonempty(target_branch, "target branch")?;
            require_nonempty(base_branch, "base branch")?;
            if mission.reconcile_only {
                return Err(Error::guard(
                    "publishing is reconcile-only until the existing delivery is reconciled",
                ));
            }
            if facts.delivery.is_some() {
                return Err(Error::guard("a delivery intent is already recorded"));
            }
            match &mission.reviewed_digest {
                Some(reviewed) if reviewed == &mission.content_digest => Ok(()),
                _ => Err(Error::guard(
                    "publication content does not match the reviewed digest",
                )),
            }
        }
        CommandBody::ReconcileDeliveryComplete {
            content_digest,
            commit_sha,
            tree_sha,
            pr_id,
        }
        | CommandBody::DiscoverPublicationCompleted {
            content_digest,
            commit_sha,
            tree_sha,
            pr_id,
        } => {
            let proof = PublicationProof {
                content_digest: content_digest.clone(),
                commit_sha: commit_sha.clone(),
                tree_sha: tree_sha.clone(),
                pr_id: pr_id.clone(),
            };
            require_publication_proof(facts, &proof)
        }
        CommandBody::Reject { rationale } => {
            require_nonempty(rationale, "rationale")?;
            if facts.ops_blocked {
                return Err(blocked());
            }
            if facts
                .delivery
                .as_ref()
                .is_some_and(|delivery| delivery.publication_state != "published")
            {
                return Err(Error::guard(
                    "rejection is not allowed while delivery is in flight",
                ));
            }
            Ok(())
        }
        CommandBody::Pause | CommandBody::Cancel => Ok(()),
        CommandBody::ConfirmStopped => {
            if facts.ops_blocked {
                return Err(blocked());
            }
            if facts.pending_pause {
                return Err(Error::guard(
                    "pause abort is still pending and has not been delivered",
                ));
            }
            if facts.admitted_runtime_work && !facts.acknowledged_pause {
                return Err(Error::guard(
                    "runtime work is outstanding until the pause abort is delivered",
                ));
            }
            Ok(())
        }
        CommandBody::Resume { authorized } => {
            if !authorized {
                return Err(Error::guard("resume requires explicit authorization"));
            }
            if facts.ops_blocked {
                return Err(blocked());
            }
            let target = mission
                .resume_target
                .ok_or_else(|| Error::guard("paused mission has no resume target"))?;
            if !target.is_pause_source() {
                return Err(Error::guard("resume target is not a recorded pause source"));
            }
            revalidate_resume(mission, facts, target)
        }
        CommandBody::ConfirmCancelled => {
            if facts.ops_blocked {
                return Err(blocked());
            }
            if facts.pending_cancel {
                return Err(Error::guard(
                    "cancel abort is still pending and has not been delivered",
                ));
            }
            if facts.admitted_runtime_work && !facts.acknowledged_cancel {
                return Err(Error::guard(
                    "runtime work is outstanding until the cancel abort is delivered",
                ));
            }
            if let Some(proof) = &facts.publication_receipt
                && let Some(delivery) = &facts.delivery
                && proof.matches_digest(&delivery.approved_digest)
            {
                return Err(Error::guard(
                    "a matching published pull request exists; cancellation cannot undo it",
                ));
            }
            Ok(())
        }
        CommandBody::DetectUncertainty { .. } => Ok(()),
        CommandBody::ExplicitRecovery {
            expected_target,
            ownership_proven,
            effects,
        } => {
            if !ownership_proven {
                return Err(Error::guard("recovery requires explicit ownership proof"));
            }
            if facts.ops_blocked || matches!(effects, crate::domain::EffectProof::Unresolved) {
                return Err(Error::guard("unresolved effects block recovery"));
            }
            let validated = validated_recovery_target(mission)?;
            if *expected_target != validated {
                return Err(Error::guard(format!(
                    "recovery target {expected_target} does not match validated target {validated}"
                )));
            }
            if validated == Phase::Paused && facts.ops_blocked {
                return Err(blocked());
            }
            Ok(())
        }
        CommandBody::Continue { activity } => {
            require_nonempty(activity, "activity")?;
            if !matches!(mission.phase, Phase::Planning | Phase::Implementing) {
                return Err(Error::guard(
                    "runtime continuation is only admitted in planning or implementing",
                ));
            }
            Ok(())
        }
        CommandBody::Steer { content } => {
            require_nonempty(content, "steer content")?;
            if !matches!(
                mission.phase,
                Phase::Planning | Phase::AwaitingPlanApproval | Phase::Implementing
            ) {
                return Err(Error::guard("steering admission is closed in this phase"));
            }
            Ok(())
        }
    }
}

fn revalidate_resume(mission: &MissionRecord, facts: &Facts, target: Phase) -> Result<()> {
    if matches!(target, Phase::Approved | Phase::AwaitingReview) {
        if content_drift(mission) {
            return Err(Error::guard(
                "content drift blocks resume to a review phase; recover through needs_attention",
            ));
        }
        require_pass(mission, facts)?;
    }
    if target == Phase::AwaitingPlanApproval && mission.plan_digest.is_none() {
        return Err(Error::guard(
            "resume to plan approval requires the frozen plan",
        ));
    }
    if target == Phase::Approved
        && mission.reviewed_digest.as_ref() != Some(&mission.content_digest)
    {
        return Err(Error::guard("resume cannot reuse a stale approval"));
    }
    Ok(())
}

fn require_pass(mission: &MissionRecord, facts: &Facts) -> Result<()> {
    match &facts.verification {
        Some(verification)
            if verification.outcome == VerificationOutcome::Pass
                && verification.complete
                && verification.content_digest == mission.content_digest =>
        {
            Ok(())
        }
        _ => Err(Error::guard(
            "complete PASS evidence for the current content is required",
        )),
    }
}

fn require_repairable_fail(mission: &MissionRecord, facts: &Facts) -> Result<()> {
    match &facts.verification {
        Some(verification)
            if verification.outcome == VerificationOutcome::Fail
                && verification.content_digest == mission.content_digest =>
        {
            Ok(())
        }
        _ => Err(Error::guard(
            "bounded repair requires a FAIL for the current content",
        )),
    }
}

fn require_repair_allowance(attempt: &AttemptRecord) -> Result<()> {
    if attempt.repair_count >= attempt.max_repairs {
        Err(Error::RepairLimitExhausted {
            count: attempt.repair_count,
            max: attempt.max_repairs,
        })
    } else {
        Ok(())
    }
}

fn require_plan_digest(plan: &str, digest: &str) -> Result<()> {
    let actual = plan_digest_for(plan)?;
    if actual != digest {
        return Err(Error::guard("plan digest does not match the plan body"));
    }
    Ok(())
}

fn require_nonempty(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(Error::guard(format!("{label} is required")))
    } else {
        Ok(())
    }
}

fn require_publication_proof(facts: &Facts, proof: &PublicationProof) -> Result<()> {
    let delivery = facts
        .delivery
        .as_ref()
        .ok_or_else(|| Error::guard("no delivery intent is recorded"))?;
    if !proof.matches_digest(&delivery.approved_digest) {
        return Err(Error::guard(
            "publication proof does not match the authorized delivery",
        ));
    }
    let Some(existing) = &facts.publication_receipt else {
        return Err(Error::guard(
            "publication proof has not been observed; an operator command cannot certify it",
        ));
    };
    if existing != proof {
        return Err(Error::guard(
            "publication proof conflicts with the recorded receipt",
        ));
    }
    Ok(())
}

pub fn project(
    mission: &MissionRecord,
    spec: TargetSpec,
    body: &CommandBody,
    facts: &Facts,
) -> Result<TransitionPlan> {
    let next = match spec {
        TargetSpec::Fixed(phase) => phase,
        TargetSpec::SavedResume => mission
            .resume_target
            .ok_or_else(|| Error::guard("paused mission has no resume target"))?,
        TargetSpec::ValidatedRecovery => validated_recovery_target(mission)?,
    };
    let mut plan = TransitionPlan {
        next,
        resume_target: mission.resume_target,
        prior_phase: mission.prior_phase,
        attention_reason: mission.attention_reason,
        permitted_recovery_target: mission.permitted_recovery_target,
        reconcile_only: mission.reconcile_only,
        content_digest: mission.content_digest.clone(),
        plan_digest: mission.plan_digest.clone(),
        plan_body: mission.plan_body.clone(),
        reviewed_digest: mission.reviewed_digest.clone(),
        repair_count: mission.attempt.repair_count,
        plan_revision_count: mission.attempt.plan_revision_count,
        invalidate_evidence: false,
        invalidate_reviews: false,
        invalidate_plan_approval: false,
        decision_digest: None,
        delivery_insert: None,
        delivery_publish: None,
    };
    match body {
        CommandBody::PlanCompleted {
            plan: text, digest, ..
        } => {
            plan.plan_body = Some(text.clone());
            plan.plan_digest = Some(digest.clone());
            plan.decision_digest = Some(digest.clone());
        }
        CommandBody::RequestRevisedPlan { .. } => {
            plan.plan_digest = None;
            plan.plan_body = None;
            plan.plan_revision_count = mission.attempt.plan_revision_count.saturating_add(1);
            plan.invalidate_plan_approval = true;
        }
        CommandBody::ApprovePlan { plan_digest } => {
            plan.decision_digest = Some(plan_digest.clone());
        }
        CommandBody::AuthorizeBoundedRepair { .. } | CommandBody::RequestChanges { .. } => {
            plan.repair_count = mission.attempt.repair_count.saturating_add(1);
            plan.reviewed_digest = None;
            plan.invalidate_evidence = true;
            plan.invalidate_reviews = true;
        }
        CommandBody::ApproveReview { digest, .. } => {
            plan.reviewed_digest = Some(digest.clone());
            plan.decision_digest = Some(digest.clone());
        }
        CommandBody::Publish {
            target_branch,
            base_branch,
            ..
        } => {
            plan.delivery_insert = Some(NewDelivery {
                target_branch: target_branch.clone(),
                base_branch: base_branch.clone(),
                approved_digest: mission.content_digest.clone(),
            });
            plan.decision_digest = Some(mission.content_digest.clone());
        }
        CommandBody::ReconcileDeliveryComplete {
            content_digest,
            commit_sha,
            tree_sha,
            pr_id,
        }
        | CommandBody::DiscoverPublicationCompleted {
            content_digest,
            commit_sha,
            tree_sha,
            pr_id,
        } => {
            plan.delivery_publish = Some(PublicationProof {
                content_digest: content_digest.clone(),
                commit_sha: commit_sha.clone(),
                tree_sha: tree_sha.clone(),
                pr_id: pr_id.clone(),
            });
            plan.reconcile_only = false;
            plan.decision_digest = Some(content_digest.clone());
        }
        CommandBody::Pause => {
            plan.resume_target = Some(mission.phase);
        }
        CommandBody::Cancel => {
            plan.prior_phase = Some(mission.phase);
        }
        CommandBody::DetectUncertainty {
            reason,
            content_digest,
        } => {
            let delivery_in_flight = facts
                .delivery
                .as_ref()
                .is_some_and(|delivery| delivery.publication_state != "published");
            let permitted = crate::domain::permitted_recovery_target(
                mission.phase,
                *reason,
                delivery_in_flight,
            );
            plan.prior_phase = Some(mission.phase);
            plan.attention_reason = Some(*reason);
            plan.permitted_recovery_target = Some(permitted);
            plan.reconcile_only = permitted == Phase::Publishing;
            if let Some(digest) = content_digest {
                plan.content_digest = digest.clone();
            }
            if matches!(
                reason,
                AttentionReason::InvalidApproval
                    | AttentionReason::VerificationError
                    | AttentionReason::VerificationIncomplete
            ) {
                plan.invalidate_evidence = true;
                plan.invalidate_reviews = true;
            }
        }
        CommandBody::ExplicitRecovery { .. } => {
            plan.attention_reason = None;
            plan.permitted_recovery_target = None;
            if next == Phase::Publishing {
                plan.reconcile_only = true;
            }
            if next == Phase::Paused {
                let pass_for_current = crate::domain::has_complete_pass_for_digest(
                    facts.verification.as_ref(),
                    &mission.content_digest,
                );
                plan.resume_target =
                    crate::domain::repaired_resume_target(mission, pass_for_current);
                if plan.resume_target == Some(Phase::Verifying) {
                    plan.invalidate_evidence = true;
                    plan.invalidate_reviews = true;
                    plan.reviewed_digest = None;
                }
            }
            if next == Phase::Verifying {
                plan.invalidate_evidence = true;
                plan.invalidate_reviews = true;
                plan.reviewed_digest = None;
            }
        }
        _ => {}
    }
    Ok(plan)
}

pub fn plan_digest_for(plan: &str) -> Result<String> {
    payload_hash(&serde_json::Value::String(plan.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AttemptRecord, EffectProof, RepoIdentity, SourceIssue, permitted_recovery_target,
    };

    fn mission(phase: Phase) -> MissionRecord {
        MissionRecord {
            id: "m-test".into(),
            revision: 3,
            phase,
            objective: "objective".into(),
            repo: RepoIdentity {
                toplevel: "/repo".into(),
                head_sha: "abc".into(),
                branch: "main".into(),
                porcelain_hash: "sha256:00".into(),
                origin_url: None,
            },
            source_issue: SourceIssue {
                provider: "github".into(),
                number: 95,
                title: "title".into(),
                snapshot_at: 0,
            },
            resume_target: None,
            prior_phase: None,
            attention_reason: None,
            permitted_recovery_target: None,
            reconcile_only: false,
            content_digest: "digest-1".into(),
            plan_digest: Some("plan-digest".into()),
            plan_body: Some("plan".into()),
            reviewed_digest: None,
            successor_of_mission_id: None,
            task_id: "t-test".into(),
            acceptance_criteria: "acceptance".into(),
            attempt: AttemptRecord {
                id: "a-test".into(),
                task_id: "t-test".into(),
                base_sha: "abc".into(),
                session_id: "s-test".into(),
                runtime_adapter: "fake".into(),
                policy_json: "{}".into(),
                policy_hash: "sha256:policy".into(),
                verifier_json: "{}".into(),
                repair_count: 0,
                max_repairs: 2,
                plan_revision_count: 0,
                max_plan_revisions: 3,
                successor_of: None,
            },
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn repair_projection_increments_once_and_stops_at_the_limit() {
        let mut mission = mission(Phase::Verifying);
        let facts = Facts {
            verification: Some(VerificationFact {
                outcome: VerificationOutcome::Fail,
                content_digest: "digest-1".into(),
                complete: true,
            }),
            ..Facts::default()
        };
        let body = CommandBody::AuthorizeBoundedRepair {
            rationale: "fix the assertion".into(),
        };
        authorize(&mission, &body, &facts).unwrap();
        let plan = project(
            &mission,
            TargetSpec::Fixed(Phase::Implementing),
            &body,
            &facts,
        )
        .unwrap();
        assert_eq!(plan.repair_count, 1);
        assert!(plan.invalidate_evidence && plan.invalidate_reviews);
        mission.attempt.repair_count = 2;
        let error = authorize(&mission, &body, &facts).unwrap_err();
        assert_eq!(error.category(), "repair_limit_exhausted");
    }

    #[test]
    fn recovery_targets_follow_the_s1_rules() {
        assert_eq!(
            permitted_recovery_target(Phase::Cancelling, AttentionReason::FailedStop, false),
            Phase::Cancelling
        );
        assert_eq!(
            permitted_recovery_target(Phase::Pausing, AttentionReason::FailedStop, false),
            Phase::Paused
        );
        assert_eq!(
            permitted_recovery_target(Phase::Publishing, AttentionReason::ExecutionUncertain, true),
            Phase::Publishing
        );
        assert_eq!(
            permitted_recovery_target(Phase::Approved, AttentionReason::InvalidApproval, false),
            Phase::Verifying
        );
        assert_eq!(
            permitted_recovery_target(Phase::Implementing, AttentionReason::Uncertainty, false),
            Phase::Implementing
        );
    }

    #[test]
    fn content_drift_on_resume_does_not_silently_restore_approval() {
        let mut mission = mission(Phase::Paused);
        mission.resume_target = Some(Phase::Approved);
        mission.reviewed_digest = Some("digest-old".into());
        mission.content_digest = "digest-new".into();
        let facts = Facts {
            verification: Some(VerificationFact {
                outcome: VerificationOutcome::Pass,
                content_digest: "digest-new".into(),
                complete: true,
            }),
            ..Facts::default()
        };
        let error =
            authorize(&mission, &CommandBody::Resume { authorized: true }, &facts).unwrap_err();
        assert_eq!(error.category(), "guard_failed");
    }

    #[test]
    fn explicit_recovery_rejects_a_caller_chosen_approved_target_after_drift() {
        let mut mission = mission(Phase::NeedsAttention);
        mission.permitted_recovery_target = Some(Phase::Approved);
        mission.reviewed_digest = Some("old".into());
        mission.content_digest = "new".into();
        mission.prior_phase = Some(Phase::Approved);
        let facts = Facts::default();
        let error = authorize(
            &mission,
            &CommandBody::ExplicitRecovery {
                expected_target: Phase::Approved,
                ownership_proven: true,
                effects: EffectProof::Reconciled,
            },
            &facts,
        )
        .unwrap_err();
        assert!(error.to_string().contains("verifying"), "{error}");
        authorize(
            &mission,
            &CommandBody::ExplicitRecovery {
                expected_target: Phase::Verifying,
                ownership_proven: true,
                effects: EffectProof::Reconciled,
            },
            &facts,
        )
        .unwrap();
    }
}
