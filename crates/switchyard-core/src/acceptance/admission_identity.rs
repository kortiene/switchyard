use super::*;

#[test]
fn redelivery_with_a_different_payload_conflicts_and_keeps_the_command() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let admitted = admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    drop(host);
    let mut external = FakeRuntime::open(&crate::journal_path(&world.db)).unwrap();
    external
        .submit(&RuntimeRequest {
            binding: crate::RuntimeBinding {
                mission_id: created.mission_id.clone(),
                attempt_id: created.attempt_id.clone(),
                session_id: created.session_id.clone(),
                generation: 1,
            },
            request_id: admitted.request_id.clone(),
            payload_hash: "sha256:different".into(),
            method: "submit",
        })
        .unwrap();
    drop(external);
    let mut host = Host::open(&world.db).unwrap();
    let error = host.dispatch_pending(&created.mission_id).unwrap_err();
    assert_eq!(error.category(), "command_conflict");
    let snap = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(snap.pending_commands[0].id, admitted.command_id);
    assert_eq!(snap.attempt_id, created.attempt_id);
    assert_eq!(snap.phase, Phase::Planning);
}

#[test]
fn stale_revision_mismatched_ids_and_illegal_transitions_do_not_move() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let stale = host
        .admit(AdmitRequest {
            command_id: None,
            mission_id: created.mission_id.clone(),
            attempt_id: None,
            expected_revision: 99,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::StartPlanning,
        })
        .unwrap_err();
    assert_eq!(stale.category(), "stale_revision");
    let mismatch = host
        .admit(AdmitRequest {
            command_id: None,
            mission_id: created.mission_id.clone(),
            attempt_id: Some("a-other".into()),
            expected_revision: created.revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::StartPlanning,
        })
        .unwrap_err();
    assert_eq!(mismatch.category(), "binding_mismatch");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Draft
    );

    let binding = crate::RuntimeBinding {
        mission_id: "m-other".into(),
        attempt_id: created.attempt_id.clone(),
        session_id: created.session_id.clone(),
        generation: host.generation(),
    };
    assert_eq!(
        host.accept_runtime_binding(&binding, &created.mission_id)
            .unwrap_err()
            .category(),
        "binding_mismatch"
    );
    let stale_gen = crate::RuntimeBinding {
        generation: 0,
        mission_id: created.mission_id.clone(),
        ..binding
    };
    assert_eq!(
        host.accept_runtime_binding(&stale_gen, &created.mission_id)
            .unwrap_err()
            .category(),
        "stale_generation"
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().revision,
        created.revision
    );

    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let first = host.snapshot(&created.mission_id).unwrap();
    let retry = host
        .admit(AdmitRequest {
            command_id: Some(first.commands[0].id.clone()),
            mission_id: created.mission_id.clone(),
            attempt_id: Some(created.attempt_id.clone()),
            expected_revision: 1,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::StartPlanning,
        })
        .unwrap();
    assert!(retry.idempotent);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().revision,
        first.revision
    );
    let conflict = host
        .admit(AdmitRequest {
            command_id: Some(first.commands[0].id.clone()),
            mission_id: created.mission_id.clone(),
            attempt_id: None,
            expected_revision: first.revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::Pause,
        })
        .unwrap_err();
    assert_eq!(conflict.category(), "command_conflict");
}

#[test]
fn operation_conflict_does_not_change_phase_and_duplicate_id_replays() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let op = operation_id(
        &created.attempt_id,
        &created.session_id,
        "task-9",
        "write-file:v1",
    )
    .unwrap();
    host.record_operation(
        &created.mission_id,
        &op,
        &json!({"path": "a"}),
        OperationState::Completed,
    )
    .unwrap();
    let error = host
        .record_operation(
            &created.mission_id,
            &op,
            &json!({"path": "b"}),
            OperationState::Completed,
        )
        .unwrap_err();
    assert_eq!(error.category(), "operation_conflict");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Draft
    );
    host.record_operation(
        &created.mission_id,
        &op,
        &json!({"path": "a"}),
        OperationState::Completed,
    )
    .unwrap();
}

#[test]
fn steering_keeps_phase_and_a_second_mission_is_rejected() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let before = host.snapshot(&created.mission_id).unwrap();
    let steered = admit(
        &mut host,
        &created.mission_id,
        CommandBody::Steer {
            content: "keep the helper private".into(),
        },
    );
    assert_eq!(steered.phase, Phase::Planning);
    assert_eq!(steered.revision, before.revision);
    assert_eq!(steered.state, CommandState::Pending);
    let error = host
        .create_mission(crate::CreateMission {
            repo: world.repo.clone(),
            issue_number: 96,
            issue_title: String::new(),
            objective: "another".into(),
            acceptance: "no".into(),
            actor: "operator".into(),
            max_repairs: 1,
            max_plan_revisions: 1,
        })
        .unwrap_err();
    assert_eq!(error.category(), "active_mission_exists");
}
