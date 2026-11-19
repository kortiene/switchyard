//! S2-2: frozen budgets exhaust into needs_attention, and verifier time is
//! not attempt-wall time. No live provider.
use std::time::Duration;

use super::fixtures::{admit, deliver, open_world};
use crate::budget::{
    ATTEMPT_WALL_COUNTS_VERIFIER, ATTEMPT_WALL_SECONDS, DEFAULT_MAX_PLAN_REVISIONS,
    DEFAULT_MAX_REPAIRS,
};
use crate::{CommandBody, CreateMission, Phase, SuccessorChanges};

#[test]
fn attempt_wall_exhausts_without_a_provider_and_does_not_verify() {
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

    host.begin_bridge_for_test();
    host.advance_attempt_clock_for_test(Duration::from_secs(ATTEMPT_WALL_SECONDS + 1));
    let error = host.enforce_attempt_wall(&created.mission_id).unwrap_err();
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

    let again = host.enforce_attempt_wall(&created.mission_id).unwrap_err();
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
fn product_mode_completes_when_idle_with_no_pending_work() {
    assert!(crate::foreground_run_complete(false, true, false, false));
    assert!(!crate::foreground_run_complete(false, true, true, true));
    assert!(!crate::foreground_run_complete(false, false, false, true));
    assert!(crate::foreground_run_complete(true, true, false, true));
    assert!(
        !crate::foreground_run_complete(true, true, false, false),
        "the s3-acceptance replay still waits for its composed effects"
    );
}

#[test]
fn repair_wait_does_not_treat_a_pending_or_superseded_repair_as_complete() {
    assert_eq!(crate::repair_wait_outcome(true, Some("pending")), None);
    assert_eq!(
        crate::repair_wait_outcome(true, Some("superseded")),
        Some("repair_after_settle")
    );
    assert_eq!(
        crate::repair_wait_outcome(true, Some("acknowledged")),
        Some("complete")
    );
    let error = crate::Error::RepairAfterSettle;
    assert_eq!(error.category(), "repair_after_settle");
    assert!(error.to_string().contains("mission recover"), "{error}");
}

#[test]
fn ledger_frozen_at_1a12f25_reopens_without_drift_and_admits_pause_and_verification() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let written = plant_pre_budget_freeze(&mut host, &world);
    let conn = rusqlite::Connection::open(&world.db).unwrap();
    conn.execute(
        "UPDATE attempts SET frozen_inputs_json = ?1 WHERE id = ?2",
        rusqlite::params![crate::freeze::FREEZE_AT_1A12F25, written.attempt_id],
    )
    .unwrap();
    drop(conn);
    drop(host);

    let mut host = crate::Host::open(&world.db).unwrap();
    assert_eq!(
        host.canonical_attempt_freeze(&written.mission_id).unwrap(),
        crate::freeze::FREEZE_AT_1A12F25
    );
    admit(&mut host, &written.mission_id, CommandBody::Pause);
    let paused = host.snapshot(&written.mission_id).unwrap();
    assert_eq!(paused.phase, Phase::Pausing);
    assert!(paused.attention_reason.is_none());
    assert_eq!(
        paused.frozen_inputs_json.as_deref(),
        Some(crate::freeze::FREEZE_AT_1A12F25)
    );

    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = plant_pre_budget_freeze(&mut host, &world);
    let digest = {
        use super::fixtures::{notify, plan_note, stored_plan_digest};
        assert_eq!(
            notify(&mut host, &created.mission_id, plan_note("do the work")),
            Phase::AwaitingPlanApproval
        );
        let digest = stored_plan_digest(&host, &created.mission_id);
        admit(
            &mut host,
            &created.mission_id,
            CommandBody::ApprovePlan {
                plan_digest: digest.clone(),
            },
        );
        digest
    };
    let _ = digest;
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::RequestVerification { quiescent: true },
    );
    let verifying = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(verifying.phase, Phase::Verifying);
    assert!(verifying.attention_reason.is_none());
    assert_eq!(
        verifying.frozen_inputs_json.as_deref(),
        Some(crate::freeze::FREEZE_AT_1A12F25)
    );
}

fn plant_pre_budget_freeze(
    host: &mut crate::Host,
    world: &super::fixtures::World,
) -> crate::MissionSnapshot {
    let created = super::fixtures::create(host, &world.repo, 2);
    let conn = rusqlite::Connection::open(&world.db).unwrap();
    conn.execute(
        "UPDATE missions SET objective = ?1, source_issue_json = ?2 WHERE id = ?3",
        rusqlite::params![
            "Ship the freeze",
            r#"{"number":172,"provider":"github","snapshotAt":10,"title":"S4-A"}"#,
            created.mission_id,
        ],
    )
    .unwrap();
    conn.execute(
        "UPDATE tasks SET acceptance_criteria = ?1 WHERE mission_id = ?2 AND root = 1",
        rusqlite::params!["Inputs stay put", created.mission_id],
    )
    .unwrap();
    conn.execute(
        "UPDATE attempts SET base_sha = 'abc', max_repairs = 2, max_plan_revisions = 3, policy_json = ?1 WHERE mission_id = ?2 AND active = 1",
        rusqlite::params![
            r#"{"credentials":"none","maxPlanRevisions":3,"maxRepairs":2,"planningTools":"read-only"}"#,
            created.mission_id,
        ],
    )
    .unwrap();
    drop(conn);
    admit(host, &created.mission_id, CommandBody::StartPlanning);
    let written = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(
        written.frozen_inputs_json.as_deref(),
        Some(crate::freeze::FREEZE_AT_1A12F25)
    );
    written
}
