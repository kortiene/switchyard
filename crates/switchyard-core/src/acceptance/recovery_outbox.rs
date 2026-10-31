use super::*;

#[test]
fn restart_after_admission_keeps_mission_attempt_and_pending_command() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let (mode, synchronous) = host.durability().unwrap();
    assert!(mode.eq_ignore_ascii_case("wal"));
    assert_eq!(
        synchronous, 2,
        "synchronous=FULL is required for process-crash durability"
    );
    let created = create(&mut host, &world.repo, 2);
    let admitted = host
        .admit(AdmitRequest {
            command_id: Some("c-start".into()),
            mission_id: created.mission_id.clone(),
            attempt_id: Some(created.attempt_id.clone()),
            expected_revision: created.revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::StartPlanning,
        })
        .unwrap();
    assert_eq!(admitted.state, CommandState::Pending);
    assert_eq!(admitted.phase, Phase::Planning);
    let generation = host.generation();
    drop(host);

    let host = Host::open(&world.db).unwrap();
    assert!(host.generation() > generation);
    let snap = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(snap.attempt_id, created.attempt_id);
    assert_eq!(snap.session_id, created.session_id);
    assert_eq!(snap.phase, Phase::Planning);
    assert_eq!(snap.pending_commands.len(), 1);
    assert_eq!(snap.pending_commands[0].id, "c-start");
    assert_eq!(
        snap.pending_commands[0].request_id,
        format!("{}:c-start", created.attempt_id)
    );
    assert_eq!(snap.runtime_adapter, "fake");
    assert!(snap.verifier_json.contains("\"executes\":false"));

    let journal = crate::journal_path(&world.db);
    let mut external = FakeRuntime::open(&journal).unwrap();
    let receipt = external
        .submit(&RuntimeRequest {
            binding: crate::RuntimeBinding {
                mission_id: created.mission_id.clone(),
                attempt_id: created.attempt_id.clone(),
                session_id: created.session_id.clone(),
                generation: 1,
            },
            request_id: snap.pending_commands[0].request_id.clone(),
            payload_hash: snap.pending_commands[0].payload_hash.clone(),
            method: "submit",
        })
        .unwrap();
    drop(external);
    drop(host);

    let mut host = Host::open(&world.db).unwrap();
    assert_eq!(host.dispatch_pending(&created.mission_id).unwrap(), 1);
    assert_eq!(host.runtime_mut().submission_count(), 1);
    assert_eq!(host.dispatch_pending(&created.mission_id).unwrap(), 0);
    let snap = host.snapshot(&created.mission_id).unwrap();
    assert!(snap.pending_commands.is_empty());
    assert_eq!(
        snap.commands[0].receipt_id.as_deref(),
        Some(receipt.receipt_id.as_str())
    );
    assert_eq!(snap.attempt_id, created.attempt_id);
    assert_eq!(snap.phase, Phase::Planning);
}

#[test]
fn needs_attention_recovery_repair_limits_and_terminal_identity() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 1);
    let attempt = created.attempt_id.clone();
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    host.dispatch_pending(&created.mission_id).unwrap();
    notify(&mut host, &created.mission_id, plan_note("plan"));
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ApprovePlan {
            plan_digest: plan_digest_for("plan").unwrap(),
        },
    );
    notify(
        &mut host,
        &created.mission_id,
        RuntimeNotification::ImplementationSettled { quiescent: true },
    );
    let digest = host.snapshot(&created.mission_id).unwrap().content_digest;
    assert_eq!(
        notify(&mut host, &created.mission_id, fail_note(&digest)),
        Phase::Verifying
    );
    assert_eq!(host.snapshot(&created.mission_id).unwrap().repair_count, 0);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::AuthorizeBoundedRepair {
            rationale: "repair the check".into(),
        },
    );
    assert_eq!(host.snapshot(&created.mission_id).unwrap().repair_count, 1);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Implementing
    );
    drop(host);
    let mut host = Host::open(&world.db).unwrap();
    assert_eq!(host.snapshot(&created.mission_id).unwrap().repair_count, 1);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().attempt_id,
        attempt
    );
    notify(
        &mut host,
        &created.mission_id,
        RuntimeNotification::ImplementationSettled { quiescent: true },
    );
    notify(&mut host, &created.mission_id, fail_note(&digest));
    let limited = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::AuthorizeBoundedRepair {
            rationale: "another repair".into(),
        },
    )
    .unwrap_err();
    assert_eq!(limited.category(), "repair_limit_exhausted");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Verifying
    );
    assert_eq!(host.snapshot(&created.mission_id).unwrap().repair_count, 1);

    assert_eq!(
        notify(
            &mut host,
            &created.mission_id,
            RuntimeNotification::Uncertainty {
                reason: AttentionReason::ExhaustedLimits,
                detail: "repair allowance is spent".into(),
            }
        ),
        Phase::NeedsAttention
    );
    let wrong = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Approved,
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    )
    .unwrap_err();
    assert_eq!(wrong.category(), "guard_failed");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::NeedsAttention
    );
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Verifying,
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Verifying
    );
    assert_ne!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Approved
    );
}

#[test]
fn failed_journal_write_is_not_acknowledged_on_retry_or_restart() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    host.runtime_mut().fail_next_persist();
    let error = host.dispatch_pending(&created.mission_id).unwrap_err();
    assert_eq!(error.category(), "storage_failed");
    assert_eq!(host.runtime_mut().submission_count(), 0);
    let pending = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(pending.pending_commands.len(), 1);
    assert_eq!(host.dispatch_pending(&created.mission_id).unwrap(), 1);
    assert_eq!(host.runtime_mut().submission_count(), 1);
    let request_id = host.snapshot(&created.mission_id).unwrap().commands[0]
        .request_id
        .clone();
    drop(host);

    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    host.runtime_mut().fail_next_persist();
    assert!(host.dispatch_pending(&created.mission_id).is_err());
    drop(host);
    let journal = std::fs::read_to_string(crate::journal_path(&world.db)).unwrap_or_default();
    assert!(!journal.contains("start_planning") && !journal.contains("payloadHash"));
    let mut host = Host::open(&world.db).unwrap();
    assert_eq!(host.runtime_mut().submission_count(), 0);
    assert_eq!(
        host.snapshot(&created.mission_id)
            .unwrap()
            .pending_commands
            .len(),
        1
    );
    assert_eq!(host.dispatch_pending(&created.mission_id).unwrap(), 1);
    assert_eq!(host.runtime_mut().submission_count(), 1);
    let _ = request_id;
}

#[test]
fn cancelled_mission_does_not_dispatch_pending_work_and_outbox_keeps_admission_order() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    host.admit(AdmitRequest {
        command_id: Some("z-start".into()),
        mission_id: created.mission_id.clone(),
        attempt_id: None,
        expected_revision: created.revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body: CommandBody::StartPlanning,
    })
    .unwrap();
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    deliver(&mut host, &created.mission_id);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    );
    drop(host);
    let mut host = Host::open(&world.db).unwrap();
    host.recover(&created.mission_id).unwrap();
    let snap = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(snap.phase, Phase::Cancelled);
    assert_eq!(snap.attempt_id, created.attempt_id);
    assert_eq!(snap.commands[0].id, "z-start");
    assert_eq!(snap.commands[0].state, "superseded");
    assert!(
        host.runtime_mut()
            .stored_receipt(&format!("{}:z-start", created.attempt_id))
            .is_none()
    );

    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    host.dispatch_pending(&created.mission_id).unwrap();
    host.admit(AdmitRequest {
        command_id: Some("z-steer".into()),
        mission_id: created.mission_id.clone(),
        attempt_id: None,
        expected_revision: host.snapshot(&created.mission_id).unwrap().revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body: CommandBody::Steer {
            content: "first".into(),
        },
    })
    .unwrap();
    host.admit(AdmitRequest {
        command_id: Some("a-steer".into()),
        mission_id: created.mission_id.clone(),
        attempt_id: None,
        expected_revision: host.snapshot(&created.mission_id).unwrap().revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body: CommandBody::Steer {
            content: "second".into(),
        },
    })
    .unwrap();
    let conn = rusqlite::Connection::open(&world.db).unwrap();
    conn.execute("UPDATE commands SET created_at = 1", [])
        .unwrap();
    let snap = host.snapshot(&created.mission_id).unwrap();
    let ids: Vec<&str> = snap
        .commands
        .iter()
        .map(|command| command.id.as_str())
        .collect();
    let first = ids.iter().position(|id| *id == "z-steer").unwrap();
    let second = ids.iter().position(|id| *id == "a-steer").unwrap();
    assert!(first < second, "admission order lost: {ids:?}");
    host.dispatch_pending(&created.mission_id).unwrap();
    let order = host.runtime_mut().admission_order().to_vec();
    assert_eq!(
        order.last().map(String::as_str),
        Some(snap.commands[second].request_id.as_str())
    );
    assert!(order.iter().any(|id| id.ends_with(":z-steer")));
    assert!(
        order
            .windows(2)
            .any(|pair| pair[0].ends_with(":z-steer") && pair[1].ends_with(":a-steer"))
    );
}

#[test]
fn resume_enqueues_a_runtime_continuation_for_withheld_work() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    admit(&mut host, &created.mission_id, CommandBody::Pause);
    deliver(&mut host, &created.mission_id);
    let mid = host.snapshot(&created.mission_id).unwrap();
    assert!(
        mid.commands
            .iter()
            .any(|command| { command.kind == "start_planning" && command.state == "superseded" })
    );
    admit(&mut host, &created.mission_id, CommandBody::ConfirmStopped);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::Resume { authorized: true },
    );
    let resumed = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(resumed.phase, Phase::Planning);
    let continuation = resumed
        .commands
        .iter()
        .find(|command| command.kind == "continue" && command.state == "pending")
        .expect("resume enqueues a planning continuation");
    deliver(&mut host, &created.mission_id);
    assert!(
        host.runtime_mut()
            .stored_receipt(&continuation.request_id)
            .is_some()
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Planning
    );

    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    deliver(&mut host, &created.mission_id);
    notify(&mut host, &created.mission_id, plan_note("plan"));
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ApprovePlan {
            plan_digest: plan_digest_for("plan").unwrap(),
        },
    );
    admit(&mut host, &created.mission_id, CommandBody::Pause);
    deliver(&mut host, &created.mission_id);
    admit(&mut host, &created.mission_id, CommandBody::ConfirmStopped);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::Resume { authorized: true },
    );
    let resumed = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(resumed.phase, Phase::Implementing);
    assert!(
        resumed
            .commands
            .iter()
            .any(|command| command.kind == "continue" && command.state == "pending")
    );
    deliver(&mut host, &created.mission_id);
    assert!(host.runtime_mut().submission_count() >= 2);

    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    deliver(&mut host, &created.mission_id);
    notify(&mut host, &created.mission_id, plan_note("plan"));
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ApprovePlan {
            plan_digest: plan_digest_for("plan").unwrap(),
        },
    );
    notify(
        &mut host,
        &created.mission_id,
        RuntimeNotification::ImplementationSettled { quiescent: true },
    );
    let digest = host.snapshot(&created.mission_id).unwrap().content_digest;
    notify(&mut host, &created.mission_id, fail_note(&digest));
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::AuthorizeBoundedRepair {
            rationale: "repair the check".into(),
        },
    );
    admit(&mut host, &created.mission_id, CommandBody::Pause);
    deliver(&mut host, &created.mission_id);
    let paused_work = host.snapshot(&created.mission_id).unwrap();
    assert!(paused_work.commands.iter().any(|command| {
        command.kind == "authorize_bounded_repair" && command.state == "superseded"
    }));
    admit(&mut host, &created.mission_id, CommandBody::ConfirmStopped);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::Resume { authorized: true },
    );
    let resumed = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(resumed.phase, Phase::Implementing);
    assert_eq!(resumed.repair_count, 1);
    let continuation = resumed
        .commands
        .iter()
        .find(|command| command.kind == "continue" && command.state == "pending")
        .expect("resume enqueues a repair continuation");
    let payload = command_payload(&world.db, &continuation.request_id);
    assert!(payload.contains("repair"), "{payload}");
    deliver(&mut host, &created.mission_id);
    assert!(
        host.runtime_mut()
            .stored_receipt(&continuation.request_id)
            .is_some()
    );
}
