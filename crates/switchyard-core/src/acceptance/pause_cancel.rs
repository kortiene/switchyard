use super::*;

#[test]
fn pause_resume_and_pause_during_publishing() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let attempt = created.attempt_id.clone();
    admit(&mut host, &created.mission_id, CommandBody::Pause);
    let pausing = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(pausing.phase, Phase::Pausing);
    assert_eq!(pausing.resume_target, Some(Phase::Draft));
    deliver(&mut host, &created.mission_id);
    admit(&mut host, &created.mission_id, CommandBody::ConfirmStopped);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Paused
    );
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::Resume { authorized: true },
    );
    let resumed = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(resumed.phase, Phase::Draft);
    assert_eq!(resumed.attempt_id, attempt);

    let digest = drive_to_publishing(&mut host, &created.mission_id);
    let error = admit_expect(&mut host, &created.mission_id, CommandBody::Pause).unwrap_err();
    assert_eq!(error.category(), "illegal_transition");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Publishing
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().content_digest,
        digest
    );
}

#[test]
fn pause_stop_and_cancel_must_settle_before_work_resumes() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    admit(&mut host, &created.mission_id, CommandBody::Pause);
    assert_eq!(
        notify(
            &mut host,
            &created.mission_id,
            RuntimeNotification::Uncertainty {
                reason: AttentionReason::FailedStop,
                detail: "stop did not finish".into(),
            }
        ),
        Phase::NeedsAttention
    );
    assert_eq!(
        host.snapshot(&created.mission_id)
            .unwrap()
            .permitted_recovery_target,
        Some(Phase::Paused)
    );
    let op = operation_id(
        &created.attempt_id,
        &created.session_id,
        "task-1",
        "exec:start:v1",
    )
    .unwrap();
    host.record_operation(
        &created.mission_id,
        &op,
        &json!({"argv": ["sleep", "1"]}),
        OperationState::Uncertain,
    )
    .unwrap();
    let blocked = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Paused,
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    )
    .unwrap_err();
    assert_eq!(blocked.category(), "guard_failed");
    host.settle_operation(
        &op,
        &crate::payload_hash(&json!({"argv": ["sleep", "1"]})).unwrap(),
    )
    .unwrap();
    deliver(&mut host, &created.mission_id);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Paused,
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Paused
    );

    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    assert_eq!(
        notify(
            &mut host,
            &created.mission_id,
            RuntimeNotification::Uncertainty {
                reason: AttentionReason::FailedStop,
                detail: "cancel did not finish".into(),
            }
        ),
        Phase::NeedsAttention
    );
    assert_eq!(
        host.snapshot(&created.mission_id)
            .unwrap()
            .permitted_recovery_target,
        Some(Phase::Cancelling)
    );
    let skipped = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Planning,
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    )
    .unwrap_err();
    assert_eq!(skipped.category(), "guard_failed");
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Cancelling,
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    );
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
}

#[test]
fn acknowledged_work_requires_a_delivered_abort_before_stop_or_cancel() {
    for cancel in [false, true] {
        let world = open_world();
        let mut host = Host::open(&world.db).unwrap();
        let created = create(&mut host, &world.repo, 2);
        admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
        deliver(&mut host, &created.mission_id);
        assert_eq!(host.runtime_mut().submission_count(), 1);
        if cancel {
            admit(&mut host, &created.mission_id, CommandBody::Cancel);
            let error = admit_expect(
                &mut host,
                &created.mission_id,
                CommandBody::ConfirmCancelled,
            )
            .unwrap_err();
            assert_eq!(error.category(), "guard_failed");
            assert_eq!(
                host.snapshot(&created.mission_id).unwrap().phase,
                Phase::Cancelling
            );
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
        } else {
            admit(&mut host, &created.mission_id, CommandBody::Pause);
            let error = admit_expect(&mut host, &created.mission_id, CommandBody::ConfirmStopped)
                .unwrap_err();
            assert_eq!(error.category(), "guard_failed");
            assert_eq!(
                host.snapshot(&created.mission_id).unwrap().phase,
                Phase::Pausing
            );
            deliver(&mut host, &created.mission_id);
            admit(&mut host, &created.mission_id, CommandBody::ConfirmStopped);
            assert_eq!(
                host.snapshot(&created.mission_id).unwrap().phase,
                Phase::Paused
            );
        }
        let snap = host.snapshot(&created.mission_id).unwrap();
        let start = snap
            .commands
            .iter()
            .find(|command| command.kind == "start_planning")
            .expect("start planning");
        let abort_kind = if cancel { "cancel" } else { "pause" };
        let abort = snap
            .commands
            .iter()
            .find(|command| command.kind == abort_kind)
            .expect("abort");
        assert_eq!(start.state, "acknowledged");
        assert_eq!(abort.state, "acknowledged");
        assert_eq!(
            host.runtime_mut().admission_order(),
            &[start.request_id.clone(), abort.request_id.clone()]
        );
        let journal: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(crate::journal_path(&world.db)).unwrap())
                .unwrap();
        assert_eq!(
            journal["submissions"][&start.request_id]["method"],
            "submit"
        );
        assert_eq!(journal["submissions"][&abort.request_id]["method"], "abort");
    }
}
