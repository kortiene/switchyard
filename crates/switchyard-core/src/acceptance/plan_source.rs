//! Frozen `planSource`. Default `runtime` keeps Fake `#213`. `operator` freezes
//! without enqueueing runtime work, and a confined host admits `mission plan --file`.
use super::fixtures::{admit, create, open_world};
use crate::freeze::PlanSource;
use crate::{CommandBody, Host, Phase};
use std::fs;

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
#[derive(PartialEq, Debug)]
struct Fingerprint {
    phase: Phase,
    revision: u64,
    plan_digest: Option<String>,
    plan_body: Option<String>,
    commands: usize,
    decisions: usize,
}

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
fn fingerprint(host: &Host, mission: &str) -> Fingerprint {
    let snap = host.snapshot(mission).unwrap();
    Fingerprint {
        phase: snap.phase,
        revision: snap.revision,
        plan_digest: snap.plan_digest,
        plan_body: snap.plan_body,
        commands: snap.commands.len(),
        decisions: host.decisions(mission).unwrap().len(),
    }
}

#[test]
fn default_freeze_records_plan_source_runtime_and_still_enqueues() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let frozen = host.snapshot(&created.mission_id).unwrap();
    let document: serde_json::Value =
        serde_json::from_str(frozen.frozen_inputs_json.as_deref().unwrap()).unwrap();
    assert!(document.get("planSource").is_none());
    assert!(
        document["sandboxPolicy"]["policy"]
            .get("planSource")
            .is_none()
    );
    let command = frozen
        .commands
        .iter()
        .find(|command| command.kind == "start_planning")
        .unwrap();
    assert_eq!(command.state, "pending");
    assert_eq!(frozen.phase, Phase::Planning);
}

#[test]
fn ledger_frozen_at_1a12f25_reopens_without_drift_and_admits_pause() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
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
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let conn = rusqlite::Connection::open(&world.db).unwrap();
    conn.execute(
        "UPDATE attempts SET frozen_inputs_json = ?1 WHERE id = ?2",
        rusqlite::params![crate::freeze::FREEZE_AT_1A12F25, created.attempt_id],
    )
    .unwrap();
    drop(conn);
    drop(host);

    let mut host = Host::open(&world.db).unwrap();
    let reopened = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(reopened.phase, Phase::Planning);
    assert!(reopened.attention_reason.is_none());
    assert_eq!(
        reopened.frozen_inputs_json.as_deref(),
        Some(crate::freeze::FREEZE_AT_1A12F25)
    );
    assert_eq!(
        host.canonical_attempt_freeze(&created.mission_id).unwrap(),
        crate::freeze::FREEZE_AT_1A12F25
    );
    admit(&mut host, &created.mission_id, CommandBody::Pause);
    let paused = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(paused.phase, Phase::Pausing);
    assert!(paused.attention_reason.is_none());
    assert_eq!(
        paused.frozen_inputs_json.as_deref(),
        Some(crate::freeze::FREEZE_AT_1A12F25)
    );
}

#[test]
fn operator_plan_source_is_written_in_the_create_insert() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_plan_source(
            crate::CreateMission {
                repo: world.repo.clone(),
                issue_number: 219,
                issue_title: "plan source".into(),
                objective: "Write the plan".into(),
                acceptance: "The policy insert carries it".into(),
                actor: "operator".into(),
                max_repairs: 2,
                max_plan_revisions: 3,
            },
            None,
            PlanSource::Operator,
        )
        .unwrap();
    let policy: String = rusqlite::Connection::open(&world.db)
        .unwrap()
        .query_row(
            "SELECT policy_json FROM attempts WHERE id = ?1",
            [&created.attempt_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(policy.contains(r#""planSource":"operator""#), "{policy}");
}

#[test]
fn operator_start_planning_freezes_and_sends_nothing() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    host.set_plan_source(&created.mission_id, PlanSource::Operator)
        .unwrap();
    let before = host.runtime_mut().unwrap().submission_count();
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let dispatched = host.dispatch_pending(&created.mission_id).unwrap();
    assert_eq!(dispatched, 0);
    assert_eq!(host.runtime_mut().unwrap().submission_count(), before);
    assert!(host.runtime_mut().unwrap().admission_order().is_empty());
    let frozen = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(frozen.phase, Phase::Planning);
    assert!(frozen.inputs_frozen);
    let document: serde_json::Value =
        serde_json::from_str(frozen.frozen_inputs_json.as_deref().unwrap()).unwrap();
    assert_eq!(document["planSource"], "operator");
    assert!(
        document["sandboxPolicy"]["policy"]
            .get("planSource")
            .is_none()
    );
    let command = frozen
        .commands
        .iter()
        .find(|command| command.kind == "start_planning")
        .unwrap();
    assert_eq!(command.state, "reconciled");
    assert!(command.receipt_id.is_none());
    assert_eq!(command.origin, "operator");
}

#[test]
fn fake_plan_file_stays_a_runtime_observation_when_plan_source_is_operator() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    host.set_plan_source(&created.mission_id, PlanSource::Operator)
        .unwrap();
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let text = "implement the ledger\n";
    let path = world._tmp.path().join("plan.txt");
    fs::write(&path, text).unwrap();
    host.apply_plan_file(&created.mission_id, &path, true)
        .unwrap();
    let stored = host.snapshot(&created.mission_id).unwrap();
    let digest = crate::bound_plan_digest(
        text,
        &stored.attempt_id,
        stored.frozen_inputs_json.as_deref().unwrap(),
    )
    .unwrap();
    assert_eq!(stored.plan_digest.as_deref(), Some(digest.as_str()));
    assert_eq!(stored.plan_body.as_deref(), Some(text));
    assert_eq!(stored.phase, Phase::AwaitingPlanApproval);
    let command = stored
        .commands
        .iter()
        .find(|command| command.kind == "plan_completed")
        .unwrap();
    assert_eq!(command.origin, "runtime_observation");
    let actor = host
        .decisions(&created.mission_id)
        .unwrap()
        .into_iter()
        .find(|decision| decision.action == "plan_completed")
        .unwrap()
        .actor;
    assert_eq!(actor, "operator-plan-file");
}

#[test]
fn successor_can_freeze_plan_source_operator() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    host.set_plan_source(&created.mission_id, PlanSource::Operator)
        .unwrap();
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    host.dispatch_pending(&created.mission_id).unwrap();
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    );
    let copied = host
        .create_successor(&created.mission_id, "operator")
        .unwrap();
    admit(&mut host, &copied.mission_id, CommandBody::StartPlanning);
    let copied_doc: serde_json::Value = serde_json::from_str(
        host.snapshot(&copied.mission_id)
            .unwrap()
            .frozen_inputs_json
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(copied_doc["planSource"], "operator");

    admit(&mut host, &copied.mission_id, CommandBody::Cancel);
    host.dispatch_pending(&copied.mission_id).unwrap();
    admit(&mut host, &copied.mission_id, CommandBody::ConfirmCancelled);
    let forced = host
        .create_successor_with(
            &copied.mission_id,
            "operator",
            &crate::SuccessorChanges {
                plan_source: Some(PlanSource::Runtime),
                ..crate::SuccessorChanges::default()
            },
        )
        .unwrap();
    admit(&mut host, &forced.mission_id, CommandBody::StartPlanning);
    let forced_doc: serde_json::Value = serde_json::from_str(
        host.snapshot(&forced.mission_id)
            .unwrap()
            .frozen_inputs_json
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert!(forced_doc.get("planSource").is_none());
    let rationale = host
        .decisions(&forced.mission_id)
        .unwrap()
        .into_iter()
        .find(|decision| decision.action == "create")
        .unwrap()
        .rationale;
    assert!(rationale.contains("planSource"), "{rationale}");
}

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
#[test]
fn confined_operator_plan_file_digest_equals_fake_path_for_the_same_bytes() {
    let text = "implement the ledger\n";
    let world = open_world();
    let mut fake = Host::open(&world.db).unwrap();
    let created = create(&mut fake, &world.repo, 2);
    fake.set_plan_source(&created.mission_id, PlanSource::Operator)
        .unwrap();

    let confined_dir = world._tmp.path().join("confined");
    fs::create_dir(&confined_dir).unwrap();
    let confined_db = confined_dir.join("mission.sqlite");
    let mut confined = Host::open_confined_admission(&confined_db).unwrap();
    let confined_created = create(&mut confined, &world.repo, 2);
    align_freeze_inputs(
        &confined_db,
        &confined_created.mission_id,
        &world.db,
        &created.mission_id,
        &created.attempt_id,
    );

    admit(&mut fake, &created.mission_id, CommandBody::StartPlanning);
    admit(
        &mut confined,
        &confined_created.mission_id,
        CommandBody::StartPlanning,
    );
    let fake_frozen = fake.snapshot(&created.mission_id).unwrap();
    let confined_frozen = confined.snapshot(&confined_created.mission_id).unwrap();
    assert_eq!(
        fake_frozen.frozen_inputs_json, confined_frozen.frozen_inputs_json,
        "same bytes bind the same freeze"
    );
    assert_eq!(fake_frozen.attempt_id, confined_frozen.attempt_id);
    let confined_start = confined_frozen
        .commands
        .iter()
        .find(|command| command.kind == "start_planning")
        .unwrap();
    assert_eq!(confined_start.state, "reconciled");
    assert!(confined_start.receipt_id.is_none());

    let fake_file = world._tmp.path().join("fake-plan.txt");
    let confined_file = confined_dir.join("plan.txt");
    fs::write(&fake_file, text).unwrap();
    fs::write(&confined_file, text).unwrap();
    fake.apply_plan_file(&created.mission_id, &fake_file, true)
        .unwrap();
    confined
        .apply_plan_file(&confined_created.mission_id, &confined_file, true)
        .unwrap();
    let filed = fake.snapshot(&created.mission_id).unwrap();
    let admitted = confined.snapshot(&confined_created.mission_id).unwrap();
    assert_eq!(filed.plan_digest, admitted.plan_digest);
    assert_eq!(admitted.plan_body.as_deref(), Some(text));
    assert_eq!(
        admitted.plan_digest.as_deref(),
        Some(
            crate::bound_plan_digest(
                text,
                &admitted.attempt_id,
                admitted.frozen_inputs_json.as_deref().unwrap(),
            )
            .unwrap()
            .as_str()
        )
    );
    let fake_origin = filed
        .commands
        .iter()
        .find(|command| command.kind == "plan_completed")
        .unwrap()
        .origin
        .as_str();
    let confined_command = admitted
        .commands
        .iter()
        .find(|command| command.kind == "plan_completed")
        .unwrap();
    assert_eq!(fake_origin, "runtime_observation");
    assert_eq!(confined_command.origin, "operator");
    let actor = confined
        .decisions(&confined_created.mission_id)
        .unwrap()
        .into_iter()
        .find(|decision| decision.action == "plan_completed")
        .unwrap()
        .actor;
    assert_eq!(actor, "operator-plan-file");
}

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
#[test]
fn confined_runtime_or_unset_plan_file_is_refused_and_state_is_unchanged() {
    let world = open_world();
    let db = world._tmp.path().join("confined.sqlite");
    let mut host = Host::open_confined_admission(&db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let missing = world._tmp.path().join("missing-plan.txt");
    let before = fingerprint(&host, &created.mission_id);
    assert_eq!(before.phase, Phase::Planning);
    let error = host
        .apply_plan_file(&created.mission_id, &missing, true)
        .unwrap_err();
    assert_eq!(error.category(), "plan_source_not_operator");
    let message = error.to_string();
    assert!(
        message.contains("start a successor with --plan-source operator"),
        "{message}"
    );
    assert_eq!(fingerprint(&host, &created.mission_id), before);
    assert!(!missing.exists());

    let stored = host
        .snapshot(&created.mission_id)
        .unwrap()
        .frozen_inputs_json
        .unwrap();
    let mut document: serde_json::Value = serde_json::from_str(&stored).unwrap();
    document.as_object_mut().unwrap().remove("planSource");
    let unset = crate::canonicalize(&document).unwrap();
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE attempts SET frozen_inputs_json = ?1 WHERE id = ?2",
            rusqlite::params![unset, created.attempt_id],
        )
        .unwrap();
    let unset_before = fingerprint(&host, &created.mission_id);
    let unset_error = host
        .apply_plan_file(&created.mission_id, &missing, true)
        .unwrap_err();
    assert_eq!(unset_error.category(), "plan_source_not_operator");
    assert_eq!(fingerprint(&host, &created.mission_id), unset_before);
    assert_ne!(unset_before.phase, Phase::NeedsAttention);
}

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
fn align_freeze_inputs(
    db: &std::path::Path,
    mission: &str,
    from_db: &std::path::Path,
    from_mission: &str,
    attempt_id: &str,
) {
    let source = rusqlite::Connection::open(from_db).unwrap();
    let (
        objective,
        issue,
        acceptance,
        base,
        runtime,
        policy,
        policy_hash,
        verifier,
        environment,
        repairs,
        revisions,
    ): (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        i64,
    ) = source
        .query_row(
            "SELECT m.objective, m.source_issue_json, t.acceptance_criteria, a.base_sha,
                    a.runtime_config_json, a.policy_json, a.policy_hash, a.verifier_json,
                    a.environment_json, a.max_repairs, a.max_plan_revisions
             FROM missions m
             JOIN tasks t ON t.mission_id = m.id AND t.root = 1
             JOIN attempts a ON a.mission_id = m.id AND a.active = 1
             WHERE m.id = ?1",
            [from_mission],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                ))
            },
        )
        .unwrap();
    let dest = rusqlite::Connection::open(db).unwrap();
    dest.execute("PRAGMA foreign_keys=OFF", []).unwrap();
    dest.execute(
        "UPDATE attempts SET id = ?1, base_sha = ?2, runtime_config_json = ?3, policy_json = ?4,
                policy_hash = ?5, verifier_json = ?6, environment_json = ?7, max_repairs = ?8,
                max_plan_revisions = ?9
         WHERE mission_id = ?10 AND active = 1",
        rusqlite::params![
            attempt_id,
            base,
            runtime,
            policy,
            policy_hash,
            verifier,
            environment,
            repairs,
            revisions,
            mission
        ],
    )
    .unwrap();
    dest.execute(
        "UPDATE decisions SET attempt_id = ?1 WHERE mission_id = ?2",
        rusqlite::params![attempt_id, mission],
    )
    .unwrap();
    dest.execute(
        "UPDATE missions SET objective = ?1, source_issue_json = ?2 WHERE id = ?3",
        rusqlite::params![objective, issue, mission],
    )
    .unwrap();
    dest.execute(
        "UPDATE tasks SET acceptance_criteria = ?1 WHERE mission_id = ?2 AND root = 1",
        rusqlite::params![acceptance, mission],
    )
    .unwrap();
}
