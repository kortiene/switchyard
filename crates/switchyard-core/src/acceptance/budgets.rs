//! S2-2: frozen budgets exhaust into needs_attention, and verifier time is
//! not attempt-wall time. No live provider.
use std::time::Duration;

use super::fixtures::{admit, deliver, open_world};
use crate::budget::{
    ATTEMPT_WALL_COUNTS_VERIFIER, ATTEMPT_WALL_SECONDS, DEFAULT_MAX_PLAN_REVISIONS,
    DEFAULT_MAX_REPAIRS,
};
use crate::{CommandBody, CommandKind, CreateMission, Phase, SuccessorChanges};

#[test]
fn request_cap_exhausts_without_a_provider_and_does_not_verify() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = host
        .create_mission(CreateMission {
            repo: world.repo.clone(),
            issue_number: 218,
            issue_title: "budgets".into(),
            objective: "Freeze the runtime budgets".into(),
            acceptance: "Caps exhaust without a provider".into(),
            actor: "operator".into(),
            max_repairs: DEFAULT_MAX_REPAIRS,
            max_plan_revisions: DEFAULT_MAX_PLAN_REVISIONS,
        })
        .unwrap();
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let frozen = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(frozen.phase, Phase::Planning);
    let document: serde_json::Value =
        serde_json::from_str(frozen.frozen_inputs_json.as_deref().unwrap()).unwrap();
    let limits = &document["executionLimits"];
    assert_eq!(limits["maxPlanRevisions"], 2);
    assert_eq!(limits["maxRepairs"], 1);
    assert_eq!(limits["bridgeWallSeconds"], 1200);
    assert_eq!(limits["attemptWallSeconds"], 2700);
    assert_eq!(limits["maxModelRequests"], 40);
    assert_eq!(limits["maxRequestBytes"], 256 * 1024);
    assert_eq!(limits["maxResponseBytes"], 1024 * 1024);
    assert_eq!(limits["bridgeCpuSeconds"], 120);
    let before = frozen.frozen_inputs_json.clone();
    let objective = frozen.objective.clone();
    let digest = frozen.content_digest.clone();
    let revision = frozen.revision;

    let error = host
        .enforce_request_budget(&created.mission_id, 256 * 1024 + 1)
        .unwrap_err();
    assert_eq!(error.category(), "runtime_budget_exhausted");
    let attention = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(attention.phase, Phase::NeedsAttention);
    assert_eq!(
        attention.attention_reason.as_deref(),
        Some("runtime_budget_exhausted")
    );
    assert_eq!(attention.frozen_inputs_json, before);
    assert_eq!(attention.objective, objective);
    assert_eq!(attention.content_digest, digest);
    assert_eq!(attention.plan_digest, frozen.plan_digest);
    assert_eq!(attention.repair_count, frozen.repair_count);
    assert_eq!(attention.revision, revision + 1);
    assert_ne!(
        attention.permitted_recovery_target,
        Some(Phase::Verifying),
        "budget exhaustion must not take the verification edge"
    );
    assert!(
        attention
            .commands
            .iter()
            .all(|command| command.kind != "request_verification")
    );
    assert!(
        attention
            .commands
            .iter()
            .any(|command| command.kind == "detect_uncertainty")
    );

    let again = host
        .enforce_request_budget(&created.mission_id, 256 * 1024 + 1)
        .unwrap_err();
    assert_eq!(again.category(), "runtime_budget_exhausted");
    let still = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(still.revision, attention.revision);
    assert_eq!(still.frozen_inputs_json, before);

    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    deliver(&mut host, &created.mission_id);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Cancelled
    );
    let successor = host
        .create_successor_with(
            &created.mission_id,
            "operator",
            &SuccessorChanges {
                max_repairs: Some(4),
                ..SuccessorChanges::default()
            },
        )
        .unwrap();
    assert_eq!(successor.max_repairs, 4);
    assert_eq!(successor.phase, Phase::Draft);
    assert!(successor.frozen_inputs_json.is_none());
    let source = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(source.phase, Phase::Cancelled);
    assert_eq!(source.frozen_inputs_json, before);
}

#[test]
fn attempt_wall_does_not_count_two_verifier_deadlines() {
    const {
        assert!(!ATTEMPT_WALL_COUNTS_VERIFIER);
    }
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = host
        .create_mission(CreateMission {
            repo: world.repo.clone(),
            issue_number: 218,
            issue_title: "budgets".into(),
            objective: "Freeze the runtime budgets".into(),
            acceptance: "Caps exhaust without a provider".into(),
            actor: "operator".into(),
            max_repairs: DEFAULT_MAX_REPAIRS,
            max_plan_revisions: DEFAULT_MAX_PLAN_REVISIONS,
        })
        .unwrap();
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    host.begin_bridge_for_test();
    host.advance_attempt_clock_for_test(Duration::from_secs(20 * 60));
    host.begin_verify_for_test();
    // Original verify plus the one repair verify. Each deadline is 1800s.
    host.advance_attempt_clock_for_test(Duration::from_secs(1800 * 2));
    host.end_verify_for_test();
    host.enforce_attempt_wall(&created.mission_id).unwrap();
    let running = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(running.phase, Phase::Planning);
    assert!(running.attention_reason.is_none());

    host.advance_attempt_clock_for_test(Duration::from_secs(ATTEMPT_WALL_SECONDS - 20 * 60 + 1));
    let error = host.enforce_attempt_wall(&created.mission_id).unwrap_err();
    assert_eq!(error.category(), "runtime_budget_exhausted");
    let attention = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(attention.phase, Phase::NeedsAttention);
    assert_eq!(
        attention.attention_reason.as_deref(),
        Some("runtime_budget_exhausted")
    );
    assert!(
        attention
            .commands
            .iter()
            .all(|command| command.kind != "request_verification")
    );
}

#[test]
fn repair_after_settle_stays_pending_and_is_not_superseded() {
    assert!(crate::service::is_post_settle_repair(
        CommandKind::AuthorizeBoundedRepair
    ));
    assert!(crate::service::is_post_settle_repair(
        CommandKind::RequestChanges
    ));
    assert!(!crate::service::is_post_settle_repair(
        CommandKind::ApprovePlan
    ));
    assert!(!crate::service::is_post_settle_repair(
        CommandKind::StartPlanning
    ));
    assert_eq!(
        crate::repair_wait_outcome(true, Some("pending")),
        None,
        "a pending repair keeps the foreground wait running"
    );
    assert_eq!(
        crate::repair_wait_outcome(true, Some("superseded")),
        Some("repair_after_settle")
    );
    assert_eq!(
        crate::repair_wait_outcome(true, Some("acknowledged")),
        Some("complete")
    );
    assert_eq!(
        crate::repair_wait_outcome(false, Some("acknowledged")),
        None
    );
    let error = crate::Error::RepairAfterSettle;
    assert_eq!(error.category(), "repair_after_settle");
    assert!(error.to_string().contains("not superseded"), "{error}");
}
