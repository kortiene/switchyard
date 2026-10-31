use super::fixtures::{
    admit, admit_expect, command_payload, create, deliver, fail_note, notify, open_world, plan_note,
};
use crate::domain::AttentionReason;
use crate::execution::{FakeRuntime, RuntimeNotification, RuntimeRequest};
use crate::policy::plan_digest_for;
use crate::{CommandBody, EffectProof, Error, Host, Phase};

fn recover_to(host: &mut Host, mission: &str, target: Phase) -> Result<(), Error> {
    admit_expect(
        host,
        mission,
        CommandBody::ExplicitRecovery {
            expected_target: target,
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    )
}

fn uncertain(host: &mut Host, mission: &str, reason: AttentionReason) {
    notify(
        host,
        mission,
        RuntimeNotification::Uncertainty {
            reason,
            detail: "reconcile the current activity".into(),
        },
    );
}

fn continuation(host: &Host, mission: &str) -> crate::CommandView {
    let pending = host.snapshot(mission).unwrap().pending_commands;
    assert_eq!(pending.len(), 1, "exactly one continuation must be queued");
    assert_eq!(pending[0].kind, "continue");
    pending[0].clone()
}

#[test]
fn failed_pause_recovery_waits_for_abort_across_restart() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let mission = &created.mission_id;
    admit(&mut host, mission, CommandBody::StartPlanning);
    deliver(&mut host, mission);
    let pause = admit(&mut host, mission, CommandBody::Pause);
    uncertain(&mut host, mission, AttentionReason::FailedStop);
    let before = host.snapshot(mission).unwrap();
    assert_eq!(
        recover_to(&mut host, mission, Phase::Paused)
            .unwrap_err()
            .category(),
        "guard_failed"
    );
    let after = host.snapshot(mission).unwrap();
    assert_eq!(after.phase, Phase::NeedsAttention);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.commands.len(), before.commands.len());
    drop(host);

    let mut host = Host::open(&world.db).unwrap();
    assert!(recover_to(&mut host, mission, Phase::Paused).is_err());
    deliver(&mut host, mission);
    assert!(
        host.runtime_mut()
            .unwrap()
            .stored_receipt(&pause.request_id)
            .is_some()
    );
    recover_to(&mut host, mission, Phase::Paused).unwrap();
    admit(&mut host, mission, CommandBody::Resume { authorized: true });
    let continued = continuation(&host, mission);
    deliver(&mut host, mission);
    assert_eq!(
        host.runtime_mut().unwrap().admission_order(),
        &[pause.request_id, continued.request_id.clone()]
    );
    assert_eq!(host.runtime_mut().unwrap().submission_count(), 3);
    drop(host);

    let mut host = Host::open(&world.db).unwrap();
    host.recover(mission).unwrap();
    assert_eq!(host.runtime_mut().unwrap().submission_count(), 3);
    assert!(host.snapshot(mission).unwrap().pending_commands.is_empty());
}

#[test]
fn revised_plan_resume_ignores_prior_planning_acknowledgment() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let mission = &created.mission_id;
    admit(&mut host, mission, CommandBody::StartPlanning);
    deliver(&mut host, mission);
    notify(&mut host, mission, plan_note("first plan"));
    let revised = admit(
        &mut host,
        mission,
        CommandBody::RequestRevisedPlan {
            feedback: "revise the plan".into(),
        },
    );
    admit(&mut host, mission, CommandBody::Pause);
    deliver(&mut host, mission);
    admit(&mut host, mission, CommandBody::ConfirmStopped);
    admit(&mut host, mission, CommandBody::Resume { authorized: true });
    let continued = continuation(&host, mission);
    let snap = host.snapshot(mission).unwrap();
    assert_eq!(snap.phase, Phase::Planning);
    assert_eq!(snap.plan_revision_count, 1);
    assert!(snap.plan_digest.is_none());
    assert!(
        snap.commands
            .iter()
            .any(|command| command.id == revised.command_id && command.state == "superseded")
    );
    deliver(&mut host, mission);
    assert!(
        host.runtime_mut()
            .unwrap()
            .stored_receipt(&revised.request_id)
            .is_none()
    );
    assert!(
        host.runtime_mut()
            .unwrap()
            .stored_receipt(&continued.request_id)
            .is_some()
    );
    assert_eq!(host.runtime_mut().unwrap().submission_count(), 3);
}

#[test]
fn second_repair_resume_ignores_prior_repair_acknowledgment() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let mission = &created.mission_id;
    admit(&mut host, mission, CommandBody::StartPlanning);
    deliver(&mut host, mission);
    notify(&mut host, mission, plan_note("plan"));
    admit(
        &mut host,
        mission,
        CommandBody::ApprovePlan {
            plan_digest: plan_digest_for("plan").unwrap(),
        },
    );
    deliver(&mut host, mission);
    for round in 1..=2 {
        notify(
            &mut host,
            mission,
            RuntimeNotification::ImplementationSettled { quiescent: true },
        );
        notify(&mut host, mission, fail_note(&created.content_digest));
        admit(
            &mut host,
            mission,
            CommandBody::AuthorizeBoundedRepair {
                rationale: format!("repair {round}"),
            },
        );
        if round == 1 {
            deliver(&mut host, mission);
        }
    }
    admit(&mut host, mission, CommandBody::Pause);
    deliver(&mut host, mission);
    admit(&mut host, mission, CommandBody::ConfirmStopped);
    admit(&mut host, mission, CommandBody::Resume { authorized: true });
    let continued = continuation(&host, mission);
    assert_eq!(host.snapshot(mission).unwrap().repair_count, 2);
    assert!(command_payload(&world.db, &continued.request_id).contains("repair"));
    deliver(&mut host, mission);
    assert!(
        host.runtime_mut()
            .unwrap()
            .stored_receipt(&continued.request_id)
            .is_some()
    );
    assert_eq!(host.runtime_mut().unwrap().submission_count(), 5);
}

#[test]
fn explicit_recovery_reuses_pending_request_with_or_without_lost_ack() {
    for lost_ack in [false, true] {
        let world = open_world();
        let mut host = Host::open(&world.db).unwrap();
        let created = create(&mut host, &world.repo, 2);
        let mission = &created.mission_id;
        let start = admit(&mut host, mission, CommandBody::StartPlanning);
        let generation = host.generation();
        drop(host);
        let receipt = if lost_ack {
            let mut runtime = FakeRuntime::open(&crate::journal_path(&world.db)).unwrap();
            Some(
                runtime
                    .submit(&RuntimeRequest {
                        binding: crate::RuntimeBinding {
                            mission_id: mission.clone(),
                            attempt_id: created.attempt_id.clone(),
                            session_id: created.session_id.clone(),
                            generation,
                        },
                        request_id: start.request_id.clone(),
                        payload_hash: start.payload_hash.clone(),
                        method: "submit",
                    })
                    .unwrap(),
            )
        } else {
            None
        };
        let mut host = Host::open(&world.db).unwrap();
        for _ in 0..2 {
            uncertain(&mut host, mission, AttentionReason::ExecutionUncertain);
            recover_to(&mut host, mission, Phase::Planning).unwrap();
            let snap = host.snapshot(mission).unwrap();
            assert_eq!(snap.pending_commands.len(), 1);
            assert_eq!(snap.pending_commands[0].request_id, start.request_id);
            assert!(
                snap.commands
                    .iter()
                    .all(|command| command.kind != "continue")
            );
        }
        deliver(&mut host, mission);
        assert_eq!(host.runtime_mut().unwrap().submission_count(), 1);
        if let Some(receipt) = receipt {
            assert_eq!(
                host.snapshot(mission).unwrap().commands[0]
                    .receipt_id
                    .as_deref(),
                Some(receipt.receipt_id.as_str())
            );
        }
        drop(host);
        let mut host = Host::open(&world.db).unwrap();
        host.recover(mission).unwrap();
        assert_eq!(host.runtime_mut().unwrap().submission_count(), 1);
    }
}

#[test]
fn recovery_reuses_pending_and_acknowledged_continuation() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let mission = &created.mission_id;
    admit(&mut host, mission, CommandBody::StartPlanning);
    admit(&mut host, mission, CommandBody::Pause);
    deliver(&mut host, mission);
    admit(&mut host, mission, CommandBody::ConfirmStopped);
    admit(&mut host, mission, CommandBody::Resume { authorized: true });
    let continued = continuation(&host, mission);
    uncertain(&mut host, mission, AttentionReason::ExecutionUncertain);
    recover_to(&mut host, mission, Phase::Planning).unwrap();
    assert_eq!(
        continuation(&host, mission).request_id,
        continued.request_id
    );
    deliver(&mut host, mission);
    uncertain(&mut host, mission, AttentionReason::ExecutionUncertain);
    recover_to(&mut host, mission, Phase::Planning).unwrap();
    assert!(host.snapshot(mission).unwrap().pending_commands.is_empty());
    assert_eq!(host.runtime_mut().unwrap().submission_count(), 2);
}

#[test]
fn reconciling_lost_start_ack_keeps_its_pending_abort() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let mission = &created.mission_id;
    let start = admit(&mut host, mission, CommandBody::StartPlanning);
    let pause = admit(&mut host, mission, CommandBody::Pause);
    uncertain(&mut host, mission, AttentionReason::FailedStop);
    let generation = host.generation();
    drop(host);
    let mut runtime = FakeRuntime::open(&crate::journal_path(&world.db)).unwrap();
    runtime
        .submit(&RuntimeRequest {
            binding: crate::RuntimeBinding {
                mission_id: mission.clone(),
                attempt_id: created.attempt_id.clone(),
                session_id: created.session_id.clone(),
                generation,
            },
            request_id: start.request_id,
            payload_hash: start.payload_hash,
            method: "submit",
        })
        .unwrap();
    drop(runtime);
    let mut host = Host::open(&world.db).unwrap();
    deliver(&mut host, mission);
    assert!(
        host.runtime_mut()
            .unwrap()
            .stored_receipt(&pause.request_id)
            .is_some()
    );
    recover_to(&mut host, mission, Phase::Paused).unwrap();
}
