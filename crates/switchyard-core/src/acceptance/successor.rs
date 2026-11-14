//! S4-A slice 172c: successor attempt for scope or policy changes.
use super::fixtures::{
    admit, admit_expect, create, deliver, git_out, notify, open_world, plan_note,
    stored_plan_digest,
};
use crate::canonical::canonicalize;
use crate::{CommandBody, Host, Phase, SuccessorChanges};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};

fn cancel_to_terminal(host: &mut Host, mission: &str) {
    admit(host, mission, CommandBody::Cancel);
    deliver(host, mission);
    admit(host, mission, CommandBody::ConfirmCancelled);
    assert_eq!(host.snapshot(mission).unwrap().phase, Phase::Cancelled);
}

fn head(repo: &std::path::Path) -> String {
    git_out(repo, &["rev-parse", "HEAD"]).trim().to_string()
}

fn plan_to_approval(host: &mut Host, mission: &str, plan: &str) -> String {
    admit(host, mission, CommandBody::StartPlanning);
    deliver(host, mission);
    assert_eq!(
        notify(host, mission, plan_note(plan)),
        Phase::AwaitingPlanApproval
    );
    stored_plan_digest(host, mission)
}

fn create_decision(host: &Host, mission: &str) -> crate::persistence::StoredDecision {
    host.decisions(mission)
        .unwrap()
        .into_iter()
        .find(|decision| decision.action == "create")
        .expect("create decision")
}

fn attempt_plan(db: &std::path::Path, attempt: &str) -> (Option<String>, Option<String>, i64, i64) {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT plan_body, plan_digest, repair_count, plan_revision_count
             FROM attempts WHERE id = ?1",
            [attempt],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
}

fn mission_plan(db: &std::path::Path, mission: &str) -> (Option<String>, Option<String>) {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT plan_body, plan_digest FROM missions WHERE id = ?1",
            [mission],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

fn successor_mission(db: &std::path::Path, attempt: &str) -> Option<String> {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT mission_id FROM attempts WHERE successor_of = ?1",
            [attempt],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

#[test]
fn non_terminal_source_is_refused() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let refused = host
        .create_successor(&created.mission_id, "operator")
        .unwrap_err();
    assert_eq!(refused.category(), "successor_source_not_terminal");
    assert!(
        refused.to_string().contains("confirm_cancelled"),
        "{refused}"
    );
    assert_ne!(refused.category(), "frozen_inputs_missing");
    let still = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(still.phase, Phase::Draft);
    assert!(still.frozen_inputs_json.is_none());
    assert_eq!(
        rusqlite::Connection::open(&world.db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM missions", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn successor_exists_names_the_existing_mission_on_retry() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    cancel_to_terminal(&mut host, &created.mission_id);
    let successor = host
        .create_successor(&created.mission_id, "operator")
        .unwrap();
    let retry = host
        .create_successor(&created.mission_id, "operator")
        .unwrap_err();
    assert_eq!(retry.category(), "successor_exists");
    assert!(retry.to_string().contains(&successor.mission_id), "{retry}");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Cancelled
    );
    assert_eq!(
        host.snapshot(&successor.mission_id).unwrap().phase,
        Phase::Draft
    );

    cancel_to_terminal(&mut host, &successor.mission_id);
    let after_terminal = host
        .create_successor(&created.mission_id, "operator")
        .unwrap_err();
    assert_eq!(after_terminal.category(), "successor_exists");
    assert!(
        after_terminal.to_string().contains(&successor.mission_id),
        "{after_terminal}"
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Cancelled
    );
    assert_eq!(
        successor_mission(&world.db, &created.attempt_id).as_deref(),
        Some(successor.mission_id.as_str())
    );
}

#[test]
fn active_mission_exists_blocks_a_successor() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let source = create(&mut host, &world.repo, 2);
    cancel_to_terminal(&mut host, &source.mission_id);
    let active = create(&mut host, &world.repo, 2);
    let blocked = host
        .create_successor(&source.mission_id, "operator")
        .unwrap_err();
    assert_eq!(blocked.category(), "active_mission_exists");
    assert!(successor_mission(&world.db, &source.attempt_id).is_none());
    assert_eq!(
        host.snapshot(&active.mission_id).unwrap().phase,
        Phase::Draft
    );
    assert_eq!(
        host.snapshot(&source.mission_id).unwrap().phase,
        Phase::Cancelled
    );

    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let first = create(&mut host, &world.repo, 2);
    cancel_to_terminal(&mut host, &first.mission_id);
    let second = create(&mut host, &world.repo, 2);
    cancel_to_terminal(&mut host, &second.mission_id);
    let pending = host
        .create_successor(&first.mission_id, "operator")
        .unwrap();
    let blocked = host
        .create_successor(&second.mission_id, "operator")
        .unwrap_err();
    assert_eq!(blocked.category(), "active_mission_exists");
    assert!(successor_mission(&world.db, &second.attempt_id).is_none());
    assert_eq!(
        host.snapshot(&pending.mission_id).unwrap().phase,
        Phase::Draft
    );
    assert_eq!(
        successor_mission(&world.db, &first.attempt_id).as_deref(),
        Some(pending.mission_id.as_str())
    );
}

#[test]
fn source_changed_during_create_writes_nothing() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    cancel_to_terminal(&mut host, &created.mission_id);
    let revision = host.snapshot(&created.mission_id).unwrap().revision;
    let decisions_before: i64 = rusqlite::Connection::open(&world.db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM decisions", [], |row| row.get(0))
        .unwrap();
    host.tamper_source_on_next_successor();
    let changed = host
        .create_successor(&created.mission_id, "operator")
        .unwrap_err();
    assert_eq!(changed.category(), "source_changed_during_create");
    assert!(
        changed.to_string().contains("nothing was written"),
        "{changed}"
    );
    assert!(changed.to_string().contains("retry"), "{changed}");
    let source = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(source.phase, Phase::Cancelled);
    assert_eq!(source.revision, revision + 1);
    assert!(successor_mission(&world.db, &created.attempt_id).is_none());
    let conn = rusqlite::Connection::open(&world.db).unwrap();
    let missions: i64 = conn
        .query_row("SELECT COUNT(*) FROM missions", [], |row| row.get(0))
        .unwrap();
    let attempts: i64 = conn
        .query_row("SELECT COUNT(*) FROM attempts", [], |row| row.get(0))
        .unwrap();
    let decisions: i64 = conn
        .query_row("SELECT COUNT(*) FROM decisions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(missions, 1);
    assert_eq!(attempts, 1);
    assert_eq!(decisions, decisions_before);
    let successor = host
        .create_successor(&created.mission_id, "operator")
        .unwrap();
    assert_eq!(successor.phase, Phase::Draft);
    assert_ne!(successor.mission_id, created.mission_id);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Cancelled
    );
    assert_eq!(
        successor_mission(&world.db, &created.attempt_id).as_deref(),
        Some(successor.mission_id.as_str())
    );
}

#[test]
fn null_freeze_source_accepts_a_successor() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    assert!(created.frozen_inputs_json.is_none());
    cancel_to_terminal(&mut host, &created.mission_id);
    let source = host.snapshot(&created.mission_id).unwrap();
    assert!(source.frozen_inputs_json.is_none());
    let successor = host
        .create_successor(&created.mission_id, "operator")
        .unwrap();
    assert_ne!(successor.mission_id, created.mission_id);
    assert_eq!(successor.phase, Phase::Draft);
    assert!(successor.frozen_inputs_json.is_none());
    assert!(successor.plan_digest.is_none());
    let decision = create_decision(&host, &successor.mission_id);
    let payload: Value = serde_json::from_str(&decision.rationale).unwrap();
    assert_eq!(payload["sourceMissionId"], created.mission_id);
    assert_eq!(payload["sourceAttemptId"], created.attempt_id);
    assert_eq!(payload["actor"], "operator");
    assert_eq!(payload["overridden"], json!([]));
    assert_eq!(decision.actor, "operator");
    admit(&mut host, &successor.mission_id, CommandBody::StartPlanning);
    let frozen = host.snapshot(&successor.mission_id).unwrap();
    assert!(frozen.frozen_inputs_json.is_some());
    let source_after = host.snapshot(&created.mission_id).unwrap();
    assert!(source_after.frozen_inputs_json.is_none());
    assert_eq!(source_after.phase, Phase::Cancelled);
    assert_eq!(source_after.revision, source.revision);
}

#[test]
fn successor_resets_counters_and_leaves_plan_columns_null() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let plan = "implement the ledger";
    plan_to_approval(&mut host, &created.mission_id, plan);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::RequestRevisedPlan {
            feedback: "once".into(),
        },
    );
    deliver(&mut host, &created.mission_id);
    assert_eq!(
        notify(&mut host, &created.mission_id, plan_note(plan)),
        Phase::AwaitingPlanApproval
    );
    let planned = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(planned.plan_revision_count, 1);
    assert!(planned.plan_digest.is_some());
    rusqlite::Connection::open(&world.db)
        .unwrap()
        .execute(
            "UPDATE attempts SET repair_count = 2 WHERE id = ?1",
            [&planned.attempt_id],
        )
        .unwrap();
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::Reject {
            rationale: "stop".into(),
        },
    );
    let source = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(source.phase, Phase::Rejected);
    assert_eq!(source.repair_count, 2);
    assert_eq!(source.plan_revision_count, 1);
    let successor = host
        .create_successor(&created.mission_id, "operator")
        .unwrap();
    assert_eq!(successor.phase, Phase::Draft);
    assert_eq!(successor.repair_count, 0);
    assert_eq!(successor.plan_revision_count, 0);
    assert!(successor.plan_body.is_none());
    assert!(successor.plan_digest.is_none());
    assert_eq!(successor.max_repairs, source.max_repairs);
    assert_eq!(successor.max_plan_revisions, source.max_plan_revisions);
    let (body, digest, repairs, revisions) = attempt_plan(&world.db, &successor.attempt_id);
    assert!(body.is_none());
    assert!(digest.is_none());
    assert_eq!(repairs, 0);
    assert_eq!(revisions, 0);
    assert_eq!(mission_plan(&world.db, &successor.mission_id), (None, None));
    let source_plan = attempt_plan(&world.db, &source.attempt_id);
    assert_eq!(source_plan.0.as_deref(), Some(plan));
    assert!(source_plan.1.is_some());
    assert_eq!(source_plan.2, 2);
    assert_eq!(source_plan.3, 1);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Rejected
    );
}

#[test]
fn old_plan_digest_cannot_approve_the_successor() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let plan = "implement the ledger";
    let old_digest = plan_to_approval(&mut host, &created.mission_id, plan);
    let source_freeze = host
        .snapshot(&created.mission_id)
        .unwrap()
        .frozen_inputs_json
        .unwrap();
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::Reject {
            rationale: "new attempt".into(),
        },
    );
    let successor = host
        .create_successor(&created.mission_id, "operator")
        .unwrap();
    assert!(successor.plan_digest.is_none());
    assert_ne!(successor.attempt_id, created.attempt_id);
    let fresh = plan_to_approval(&mut host, &successor.mission_id, plan);
    let stored = host.snapshot(&successor.mission_id).unwrap();
    let successor_freeze = stored.frozen_inputs_json.clone().unwrap();
    assert_eq!(
        fresh,
        crate::bound_plan_digest(plan, &stored.attempt_id, &successor_freeze).unwrap()
    );
    assert_ne!(fresh, old_digest);
    assert_eq!(
        old_digest,
        crate::bound_plan_digest(plan, &created.attempt_id, &source_freeze).unwrap()
    );
    assert_ne!(
        fresh,
        crate::bound_plan_digest(plan, &created.attempt_id, &successor_freeze).unwrap()
    );
    let stale = admit_expect(
        &mut host,
        &successor.mission_id,
        CommandBody::ApprovePlan {
            plan_digest: old_digest,
        },
    )
    .unwrap_err();
    assert_eq!(stale.category(), "stale_plan_digest");
    let waiting = host.snapshot(&successor.mission_id).unwrap();
    assert_eq!(waiting.phase, Phase::AwaitingPlanApproval);
    assert_eq!(waiting.revision, stored.revision);
    assert!(
        waiting
            .commands
            .iter()
            .all(|command| command.kind != "approve_plan")
    );
    admit(
        &mut host,
        &successor.mission_id,
        CommandBody::ApprovePlan { plan_digest: fresh },
    );
    assert_eq!(
        host.snapshot(&successor.mission_id).unwrap().phase,
        Phase::Implementing
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Rejected
    );
}

#[test]
fn overrides_land_in_the_fresh_freeze_and_the_create_decision() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    cancel_to_terminal(&mut host, &created.mission_id);
    let head = head(&world.repo);
    host.tamper_attempt_base_sha(&created.mission_id, "deadbeef")
        .unwrap();
    rusqlite::Connection::open(&world.db)
        .unwrap()
        .execute(
            "UPDATE attempts SET runtime_config_json = ?1 WHERE id = ?2",
            (
                r#"{"adapter":"fake","credentials":"none","stale":true}"#,
                &created.attempt_id,
            ),
        )
        .unwrap();
    let source_revision = host.snapshot(&created.mission_id).unwrap().revision;
    let verifier = r#"{"id":"switchyard.verifier.v1","frozen":true,"executes":false}"#;
    let successor = host
        .create_successor_with(
            &created.mission_id,
            "sam",
            &SuccessorChanges {
                objective: Some("Successor objective".into()),
                acceptance: Some("first criterion\nsecond criterion".into()),
                max_repairs: Some(9),
                max_plan_revisions: Some(11),
                verifier_json: Some(verifier.into()),
            },
        )
        .unwrap();
    assert_eq!(successor.objective, "Successor objective");
    assert_eq!(successor.base_sha, head);
    assert_ne!(successor.base_sha, "deadbeef");
    assert_eq!(successor.max_repairs, 9);
    assert_eq!(successor.max_plan_revisions, 11);
    assert_eq!(successor.repair_count, 0);
    assert_eq!(successor.plan_revision_count, 0);
    assert!(successor.plan_digest.is_none());
    let conn = rusqlite::Connection::open(&world.db).unwrap();
    let (acceptance, runtime, stored_verifier): (String, String, String) = conn
        .query_row(
            "SELECT t.acceptance_criteria, a.runtime_config_json, a.verifier_json
             FROM attempts a JOIN tasks t ON t.id = a.task_id WHERE a.id = ?1",
            [&successor.attempt_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(acceptance, "first criterion\nsecond criterion");
    assert_eq!(runtime, r#"{"adapter":"fake","credentials":"none"}"#);
    assert!(!runtime.contains("stale"), "{runtime}");
    assert_eq!(
        stored_verifier,
        r#"{"executes":false,"frozen":true,"id":"switchyard.verifier.v1"}"#
    );
    let decision = create_decision(&host, &successor.mission_id);
    assert_eq!(decision.actor, "sam");
    assert_eq!(decision.digest.as_deref(), Some(head.as_str()));
    let expected = canonicalize(&json!({
        "actor": "sam",
        "overridden": ["acceptanceCriteria", "maxPlanRevisions", "maxRepairs", "objective", "verifier"],
        "sourceAttemptId": created.attempt_id,
        "sourceMissionId": created.mission_id,
    }))
    .unwrap();
    assert_eq!(decision.rationale, expected);
    admit(&mut host, &successor.mission_id, CommandBody::StartPlanning);
    let frozen = host.snapshot(&successor.mission_id).unwrap();
    let document: Value =
        serde_json::from_str(frozen.frozen_inputs_json.as_deref().unwrap()).unwrap();
    assert_eq!(document["objective"], "Successor objective");
    assert_eq!(
        document["acceptanceCriteria"],
        "first criterion\nsecond criterion"
    );
    assert_eq!(document["baseSha"], head);
    assert_eq!(document["executionLimits"]["maxRepairs"], 9);
    assert_eq!(document["executionLimits"]["maxPlanRevisions"], 11);
    assert_eq!(document["verifier"]["id"], "switchyard.verifier.v1");
    assert_eq!(document["runtimeConfig"]["adapter"], "fake");
    assert!(document["runtimeConfig"].get("stale").is_none());
    assert_eq!(document["issueSnapshot"]["number"], 95);
    assert_eq!(document["issueSnapshot"]["title"], "S1");
    let source = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(source.phase, Phase::Cancelled);
    assert_eq!(source.revision, source_revision);
    assert_eq!(source.base_sha, "deadbeef");
    assert_eq!(source.objective, "Establish the ledger");
}

#[test]
fn drift_then_cancel_then_successor() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let plan = "implement the ledger";
    let old_digest = plan_to_approval(&mut host, &created.mission_id, plan);
    let frozen = host
        .snapshot(&created.mission_id)
        .unwrap()
        .frozen_inputs_json
        .clone();
    host.tamper_attempt_base_sha(&created.mission_id, "deadbeef")
        .unwrap();
    let drifted = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::ApprovePlan {
            plan_digest: old_digest.clone(),
        },
    )
    .unwrap_err();
    assert_eq!(drifted.category(), "guard_failed");
    let attention = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(attention.phase, Phase::NeedsAttention);
    assert_eq!(
        attention.attention_reason.as_deref(),
        Some("frozen_input_drift")
    );
    let too_soon = host
        .create_successor(&created.mission_id, "operator")
        .unwrap_err();
    assert_eq!(too_soon.category(), "successor_source_not_terminal");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::NeedsAttention
    );
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Cancelling
    );
    let early = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    )
    .unwrap_err();
    assert_eq!(early.category(), "guard_failed");
    deliver(&mut host, &created.mission_id);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    );
    let source = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(source.phase, Phase::Cancelled);
    assert_eq!(source.frozen_inputs_json, frozen);
    assert_eq!(source.base_sha, "deadbeef");
    let successor = host
        .create_successor(&created.mission_id, "operator")
        .unwrap();
    assert_eq!(successor.phase, Phase::Draft);
    assert_eq!(successor.base_sha, head(&world.repo));
    assert_ne!(successor.base_sha, "deadbeef");
    assert!(successor.plan_digest.is_none());
    assert_eq!(successor.repair_count, 0);
    assert_eq!(successor.plan_revision_count, 0);
    let source_after = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(source_after.phase, Phase::Cancelled);
    assert_eq!(source_after.revision, source.revision);
    assert_eq!(source_after.frozen_inputs_json, frozen);
    let fresh = plan_to_approval(&mut host, &successor.mission_id, plan);
    assert_ne!(fresh, old_digest);
    let stale = admit_expect(
        &mut host,
        &successor.mission_id,
        CommandBody::ApprovePlan {
            plan_digest: old_digest,
        },
    )
    .unwrap_err();
    assert_eq!(stale.category(), "stale_plan_digest");
    assert_eq!(
        host.snapshot(&successor.mission_id).unwrap().phase,
        Phase::AwaitingPlanApproval
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Cancelled
    );
}

#[test]
fn override_refusals_write_nothing() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    cancel_to_terminal(&mut host, &created.mission_id);
    let decisions_before: i64 = rusqlite::Connection::open(&world.db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM decisions", [], |row| row.get(0))
        .unwrap();
    let cases = [
        (
            SuccessorChanges {
                objective: Some(String::new()),
                ..SuccessorChanges::default()
            },
            "guard_failed",
            "objective",
        ),
        (
            SuccessorChanges {
                objective: Some("   ".into()),
                ..SuccessorChanges::default()
            },
            "guard_failed",
            "objective",
        ),
        (
            SuccessorChanges {
                acceptance: Some(String::new()),
                ..SuccessorChanges::default()
            },
            "guard_failed",
            "acceptance",
        ),
        (
            SuccessorChanges {
                verifier_json: Some("[]".into()),
                ..SuccessorChanges::default()
            },
            "guard_failed",
            "JSON object",
        ),
        (
            SuccessorChanges {
                verifier_json: Some("not-json".into()),
                ..SuccessorChanges::default()
            },
            "guard_failed",
            "JSON",
        ),
        (
            SuccessorChanges {
                verifier_json: Some(
                    r#"{"executes":true,"frozen":true,"id":"switchyard.verifier.v1"}"#.into(),
                ),
                ..SuccessorChanges::default()
            },
            "verifier_not_supported",
            "switchyard.verifier.v1",
        ),
        (
            SuccessorChanges {
                verifier_json: Some(
                    r#"{"executes":false,"frozen":true,"id":"switchyard.verifier.other"}"#.into(),
                ),
                ..SuccessorChanges::default()
            },
            "verifier_not_supported",
            "switchyard.verifier.v1",
        ),
    ];
    for (changes, category, message) in cases {
        let error = host
            .create_successor_with(&created.mission_id, "operator", &changes)
            .unwrap_err();
        assert_eq!(error.category(), category, "{error}");
        assert!(error.to_string().contains(message), "{error}");
        assert!(successor_mission(&world.db, &created.attempt_id).is_none());
        let conn = rusqlite::Connection::open(&world.db).unwrap();
        let missions: i64 = conn
            .query_row("SELECT COUNT(*) FROM missions", [], |row| row.get(0))
            .unwrap();
        let decisions: i64 = conn
            .query_row("SELECT COUNT(*) FROM decisions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(missions, 1, "{error}");
        assert_eq!(decisions, decisions_before, "{error}");
    }
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Cancelled
    );
}
