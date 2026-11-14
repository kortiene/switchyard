//! S4-A slice 172b: persist the bound plan digest and admit approval against it.
use super::fixtures::{
    admit, admit_expect, create, deliver, notify, open_world, pass_note, plan_note,
};
use crate::canonical::payload_hash;
use crate::execution::RuntimeNotification;
use crate::{AdmitRequest, CommandBody, CommandOrigin, EffectProof, Phase};
use serde_json::Value;

fn text_hash(plan: &str) -> String {
    payload_hash(&Value::String(plan.into())).unwrap()
}

fn attempt_plan(db: &std::path::Path, attempt: &str) -> (Option<String>, Option<String>) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT plan_body, plan_digest FROM attempts WHERE id = ?1",
        [attempt],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

fn legacy_mission_plan(db: &std::path::Path, mission: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT plan_digest FROM missions WHERE id = ?1",
        [mission],
        |row| row.get(0),
    )
    .unwrap()
}

fn seed_unfrozen(db: &std::path::Path, mission: &str) {
    let legacy = text_hash("old plan");
    rusqlite::Connection::open(db)
        .unwrap()
        .execute(
            "UPDATE missions SET phase = 'awaiting_plan_approval', plan_body = 'old plan',
             plan_digest = ?1 WHERE id = ?2",
            rusqlite::params![legacy, mission],
        )
        .unwrap();
}

fn approve(
    host: &mut crate::Host,
    mission: &str,
    digest: &str,
    actor: &str,
) -> Result<(), crate::Error> {
    let revision = host.snapshot(mission).unwrap().revision;
    host.admit(AdmitRequest {
        command_id: None,
        mission_id: mission.to_string(),
        attempt_id: None,
        expected_revision: revision,
        actor: actor.into(),
        origin: CommandOrigin::Operator,
        body: CommandBody::ApprovePlan {
            plan_digest: digest.into(),
        },
    })
    .map(|_| ())
}

#[test]
fn plan_completed_stores_the_bound_digest_on_the_attempt() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let before = host.snapshot(&created.mission_id).unwrap();
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let frozen = host.snapshot(&created.mission_id).unwrap();
    let plan = "implement the ledger";
    let note = plan_note(plan);
    assert_eq!(
        notify(&mut host, &created.mission_id, note),
        Phase::AwaitingPlanApproval
    );
    let stored = host.snapshot(&created.mission_id).unwrap();
    let digest = stored.plan_digest.clone().unwrap();
    assert_eq!(stored.plan_body.as_deref(), Some(plan));
    assert_ne!(digest, text_hash(plan));
    assert_eq!(
        digest,
        crate::bound_plan_digest(
            plan,
            &stored.attempt_id,
            stored.frozen_inputs_json.as_deref().unwrap(),
        )
        .unwrap()
    );
    assert!(digest.starts_with("sha256:"));
    assert_eq!(digest.len(), "sha256:".len() + 64);
    let (body, attempt_digest) = attempt_plan(&world.db, &stored.attempt_id);
    assert_eq!(body.as_deref(), Some(plan));
    assert_eq!(attempt_digest.as_deref(), Some(digest.as_str()));
    assert_eq!(legacy_mission_plan(&world.db, &created.mission_id), None);
    assert_eq!(stored.frozen_inputs_json, frozen.frozen_inputs_json);
    assert_eq!(stored.max_plan_revisions, 3);
    assert_eq!(stored.revision, before.revision + 2);
    let command = stored
        .commands
        .iter()
        .find(|command| command.kind == "plan_completed")
        .unwrap();
    assert_eq!(command.resulting_revision, Some(stored.revision));
    let decision = host
        .decisions(&created.mission_id)
        .unwrap()
        .into_iter()
        .find(|decision| decision.action == "plan_completed")
        .unwrap();
    assert_eq!(decision.digest.as_deref(), Some(digest.as_str()));
    assert_eq!(decision.command_id.as_deref(), Some(command.id.as_str()));

    let stale = approve(&mut host, &created.mission_id, &text_hash(plan), "operator").unwrap_err();
    assert_eq!(stale.category(), "stale_plan_digest");
    let unchanged = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(unchanged.phase, Phase::AwaitingPlanApproval);
    assert_eq!(unchanged.revision, stored.revision);
    assert!(
        unchanged
            .commands
            .iter()
            .all(|command| command.kind != "approve_plan")
    );

    approve(&mut host, &created.mission_id, &digest, "sam").unwrap();
    let implementing = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(implementing.phase, Phase::Implementing);
    assert_eq!(implementing.revision, stored.revision + 1);
    assert_eq!(implementing.plan_digest.as_deref(), Some(digest.as_str()));
    assert_eq!(implementing.max_plan_revisions, 3);
    assert_eq!(implementing.frozen_inputs_json, stored.frozen_inputs_json);
    let approved = implementing
        .commands
        .iter()
        .find(|command| command.kind == "approve_plan")
        .unwrap();
    assert_eq!(approved.state, "pending");
    assert_eq!(approved.expected_revision, stored.revision);
    assert_eq!(approved.resulting_revision, Some(implementing.revision));
    assert_eq!(approved.origin, "operator");
    let decision = host
        .decisions(&created.mission_id)
        .unwrap()
        .into_iter()
        .find(|decision| decision.action == "approve_plan")
        .unwrap();
    assert_eq!(decision.actor, "sam");
    assert_eq!(decision.digest.as_deref(), Some(digest.as_str()));
    assert_eq!(decision.command_id.as_deref(), Some(approved.id.as_str()));
    assert!(!decision.invalidated);
}

#[test]
fn pre_v11_null_freeze_refuses_approve_and_still_admits_cancel_and_reject() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    seed_unfrozen(&world.db, &created.mission_id);
    let seeded = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(seeded.phase.as_str(), "awaiting_plan_approval");
    assert!(seeded.frozen_inputs_json.is_none());
    assert!(seeded.plan_digest.is_none());
    assert_eq!(
        legacy_mission_plan(&world.db, &created.mission_id).as_deref(),
        Some(text_hash("old plan").as_str())
    );

    let stale = host.admit(AdmitRequest {
        command_id: None,
        mission_id: created.mission_id.clone(),
        attempt_id: None,
        expected_revision: seeded.revision + 9,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body: CommandBody::ApprovePlan {
            plan_digest: text_hash("old plan"),
        },
    });
    assert_eq!(stale.unwrap_err().category(), "stale_revision");
    assert!(
        host.snapshot(&created.mission_id)
            .unwrap()
            .commands
            .is_empty()
    );

    let command_id = "c-missing-freeze".to_string();
    let missing = host.admit(AdmitRequest {
        command_id: Some(command_id.clone()),
        mission_id: created.mission_id.clone(),
        attempt_id: None,
        expected_revision: seeded.revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body: CommandBody::ApprovePlan {
            plan_digest: text_hash("old plan"),
        },
    });
    let missing = missing.unwrap_err();
    assert_eq!(missing.category(), "frozen_inputs_missing");
    assert!(
        missing.to_string().contains("successor attempt"),
        "{missing}"
    );
    let refused = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(refused.phase, Phase::AwaitingPlanApproval);
    assert_eq!(refused.revision, seeded.revision);
    assert!(refused.attention_reason.is_none());
    assert_eq!(refused.commands.len(), 1);
    assert_eq!(refused.commands[0].kind, "approve_plan");
    assert_eq!(refused.commands[0].state, "rejected");
    assert_eq!(
        refused.commands[0].resulting_revision,
        Some(seeded.revision)
    );
    let receipt: String = rusqlite::Connection::open(&world.db)
        .unwrap()
        .query_row(
            "SELECT runtime_receipt_json FROM commands WHERE id = ?1",
            [&command_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(receipt.contains("frozen_inputs_missing"), "{receipt}");
    let again = host.admit(AdmitRequest {
        command_id: Some(command_id),
        mission_id: created.mission_id.clone(),
        attempt_id: None,
        expected_revision: seeded.revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body: CommandBody::ApprovePlan {
            plan_digest: text_hash("old plan"),
        },
    });
    assert_eq!(again.unwrap_err().category(), "frozen_inputs_missing");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().commands.len(),
        1
    );

    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Cancelling
    );

    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    seed_unfrozen(&world.db, &created.mission_id);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::Reject {
            rationale: "no freeze".into(),
        },
    );
    let rejected = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(rejected.phase, Phase::Rejected);
    assert!(
        rejected
            .commands
            .iter()
            .any(|command| command.kind == "reject")
    );
    let decision = host
        .decisions(&created.mission_id)
        .unwrap()
        .into_iter()
        .find(|decision| decision.action == "reject")
        .unwrap();
    assert_eq!(decision.actor, "operator");
}

#[test]
fn revised_plan_invalidates_the_previous_digest() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    deliver(&mut host, &created.mission_id);
    let first = plan_note("first plan");
    notify(&mut host, &created.mission_id, first);
    let old = host
        .snapshot(&created.mission_id)
        .unwrap()
        .plan_digest
        .unwrap();
    let frozen = host
        .snapshot(&created.mission_id)
        .unwrap()
        .frozen_inputs_json
        .unwrap();
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::RequestRevisedPlan {
            feedback: "narrow the first step".into(),
        },
    );
    let revised = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(revised.phase, Phase::Planning);
    assert_eq!(revised.plan_revision_count, 1);
    assert!(revised.plan_digest.is_none());
    assert_eq!(revised.max_plan_revisions, 3);
    assert_eq!(revised.frozen_inputs_json.as_deref(), Some(frozen.as_str()));
    assert!(!frozen.contains("narrow the first step"));
    let decision = host
        .decisions(&created.mission_id)
        .unwrap()
        .into_iter()
        .find(|decision| decision.action == "request_revised_plan")
        .unwrap();
    assert_eq!(decision.digest.as_deref(), Some(old.as_str()));
    assert_eq!(attempt_plan(&world.db, &revised.attempt_id), (None, None));

    let second = plan_note("second plan");
    notify(&mut host, &created.mission_id, second);
    let current = host
        .snapshot(&created.mission_id)
        .unwrap()
        .plan_digest
        .unwrap();
    assert_ne!(current, old);
    let stale = approve(&mut host, &created.mission_id, &old, "operator").unwrap_err();
    assert_eq!(stale.category(), "stale_plan_digest");
    let waiting = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(waiting.phase, Phase::AwaitingPlanApproval);
    assert_eq!(waiting.plan_digest.as_deref(), Some(current.as_str()));
    approve(&mut host, &created.mission_id, &current, "operator").unwrap();
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Implementing
    );
}

#[test]
fn revision_cap_is_exactly_the_frozen_max_and_the_other_exits_remain() {
    for exit in ["approve", "reject", "cancel"] {
        let world = open_world();
        let mut host = crate::Host::open(&world.db).unwrap();
        let created = create(&mut host, &world.repo, 2);
        admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
        deliver(&mut host, &created.mission_id);
        for n in 1..=3 {
            let note = plan_note(&format!("plan {n}"));
            notify(&mut host, &created.mission_id, note);
            admit(
                &mut host,
                &created.mission_id,
                CommandBody::RequestRevisedPlan {
                    feedback: format!("revision {n}"),
                },
            );
            assert_eq!(
                host.snapshot(&created.mission_id)
                    .unwrap()
                    .plan_revision_count,
                n
            );
        }
        let note = plan_note("final plan");
        notify(&mut host, &created.mission_id, note);
        let at_cap = host.snapshot(&created.mission_id).unwrap();
        assert_eq!(at_cap.plan_revision_count, 3);
        assert_eq!(at_cap.max_plan_revisions, 3);
        assert_eq!(at_cap.phase, Phase::AwaitingPlanApproval);
        let blocked = admit_expect(
            &mut host,
            &created.mission_id,
            CommandBody::RequestRevisedPlan {
                feedback: "one more".into(),
            },
        )
        .unwrap_err();
        assert_eq!(blocked.category(), "plan_revision_limit_reached");
        let still = host.snapshot(&created.mission_id).unwrap();
        assert_eq!(still.phase, Phase::AwaitingPlanApproval);
        assert_eq!(still.plan_revision_count, 3);
        assert_eq!(still.revision, at_cap.revision);
        assert!(still.attention_reason.is_none());
        match exit {
            "approve" => {
                approve(
                    &mut host,
                    &created.mission_id,
                    still.plan_digest.as_deref().unwrap(),
                    "operator",
                )
                .unwrap();
                assert_eq!(
                    host.snapshot(&created.mission_id).unwrap().phase,
                    Phase::Implementing
                );
            }
            "reject" => {
                admit(
                    &mut host,
                    &created.mission_id,
                    CommandBody::Reject {
                        rationale: "stop at the cap".into(),
                    },
                );
                assert_eq!(
                    host.snapshot(&created.mission_id).unwrap().phase,
                    Phase::Rejected
                );
            }
            "cancel" => {
                admit(&mut host, &created.mission_id, CommandBody::Cancel);
                assert_eq!(
                    host.snapshot(&created.mission_id).unwrap().phase,
                    Phase::Cancelling
                );
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn revise_replay_after_restart_does_not_double_count() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let mission = created.mission_id.clone();
    admit(&mut host, &mission, CommandBody::StartPlanning);
    deliver(&mut host, &mission);
    let note = plan_note("plan");
    notify(&mut host, &mission, note);
    let revision = host.snapshot(&mission).unwrap().revision;
    let command_id = "c-revise-once".to_string();
    host.admit(AdmitRequest {
        command_id: Some(command_id.clone()),
        mission_id: mission.clone(),
        attempt_id: None,
        expected_revision: revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body: CommandBody::RequestRevisedPlan {
            feedback: "once".into(),
        },
    })
    .unwrap();
    assert_eq!(host.snapshot(&mission).unwrap().plan_revision_count, 1);
    let decisions = host
        .decisions(&mission)
        .unwrap()
        .into_iter()
        .filter(|decision| decision.action == "request_revised_plan")
        .count();
    assert_eq!(decisions, 1);
    drop(host);

    let mut host = crate::Host::open(&world.db).unwrap();
    host.dispatch_pending(&mission).unwrap();
    assert_eq!(host.snapshot(&mission).unwrap().plan_revision_count, 1);
    let replay = host
        .admit(AdmitRequest {
            command_id: Some(command_id),
            mission_id: mission.clone(),
            attempt_id: None,
            expected_revision: host.snapshot(&mission).unwrap().revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::RequestRevisedPlan {
                feedback: "once".into(),
            },
        })
        .unwrap();
    assert!(replay.idempotent);
    assert_eq!(host.snapshot(&mission).unwrap().plan_revision_count, 1);
    assert_eq!(host.snapshot(&mission).unwrap().phase, Phase::Planning);
    let decisions = host
        .decisions(&mission)
        .unwrap()
        .into_iter()
        .filter(|decision| decision.action == "request_revised_plan")
        .count();
    assert_eq!(decisions, 1);
}

#[test]
fn cancel_after_frozen_input_drift_settles_cancelled() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    deliver(&mut host, &created.mission_id);
    let frozen = host
        .snapshot(&created.mission_id)
        .unwrap()
        .frozen_inputs_json;
    host.tamper_attempt_base_sha(&created.mission_id, "deadbeef")
        .unwrap();
    let error = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::Steer {
            content: "keep going".into(),
        },
    )
    .unwrap_err();
    assert_eq!(error.category(), "guard_failed");
    assert_eq!(
        host.snapshot(&created.mission_id)
            .unwrap()
            .attention_reason
            .as_deref(),
        Some("frozen_input_drift")
    );
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    let early = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    )
    .unwrap_err();
    assert_eq!(early.category(), "guard_failed");
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
    let cancelled = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(cancelled.phase, Phase::Cancelled);
    assert_eq!(cancelled.frozen_inputs_json, frozen);
}

#[test]
fn approve_and_recovery_stay_refused_after_frozen_input_drift() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    deliver(&mut host, &created.mission_id);
    let note = plan_note("plan");
    notify(&mut host, &created.mission_id, note);
    let digest = host
        .snapshot(&created.mission_id)
        .unwrap()
        .plan_digest
        .unwrap();
    host.tamper_attempt_base_sha(&created.mission_id, "cafebabe")
        .unwrap();
    let first = approve(&mut host, &created.mission_id, &digest, "operator").unwrap_err();
    assert_eq!(first.category(), "guard_failed");
    assert!(first.to_string().contains("drifted"), "{first}");
    let attention = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(attention.phase, Phase::NeedsAttention);
    assert_eq!(
        attention.attention_reason.as_deref(),
        Some("frozen_input_drift")
    );
    assert!(
        attention
            .commands
            .iter()
            .all(|command| command.kind != "approve_plan")
    );
    let again = approve(&mut host, &created.mission_id, &digest, "operator").unwrap_err();
    assert_eq!(again.category(), "guard_failed");
    let target = attention.permitted_recovery_target.unwrap();
    let recovery = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::ExplicitRecovery {
            expected_target: target,
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    )
    .unwrap_err();
    assert_eq!(recovery.category(), "guard_failed");
    assert!(recovery.to_string().contains("drifted"), "{recovery}");
    let still = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(still.phase, Phase::NeedsAttention);
    assert_eq!(still.revision, attention.revision);
}

#[test]
fn plan_completed_stores_the_host_digest_and_ignores_the_observation_digest() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    host.apply_notification_json(
        &created.mission_id,
        &serde_json::json!({
            "kind": "plan_completed",
            "plan": "do the work",
            "digest": "sha256:not-a-gate",
            "quiescent": true
        }),
    )
    .unwrap();
    let stored = host.snapshot(&created.mission_id).unwrap();
    let digest = crate::bound_plan_digest(
        "do the work",
        &stored.attempt_id,
        stored.frozen_inputs_json.as_deref().unwrap(),
    )
    .unwrap();
    assert_eq!(stored.plan_digest.as_deref(), Some(digest.as_str()));
    assert_ne!(stored.plan_digest.as_deref(), Some("sha256:not-a-gate"));
    assert_eq!(legacy_mission_plan(&world.db, &created.mission_id), None);
}

#[test]
fn tampered_plan_body_routes_to_plan_digest_mismatch_and_cancel_still_works() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    deliver(&mut host, &created.mission_id);
    let note = plan_note("plan");
    notify(&mut host, &created.mission_id, note);
    let stored = host.snapshot(&created.mission_id).unwrap();
    rusqlite::Connection::open(&world.db)
        .unwrap()
        .execute(
            "UPDATE attempts SET plan_body = 'tampered plan' WHERE id = ?1",
            [&stored.attempt_id],
        )
        .unwrap();
    let refused = approve(
        &mut host,
        &created.mission_id,
        stored.plan_digest.as_deref().unwrap(),
        "operator",
    )
    .unwrap_err();
    assert_eq!(refused.category(), "guard_failed");
    assert_ne!(refused.category(), "stale_plan_digest");
    let attention = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(attention.phase, Phase::NeedsAttention);
    assert_eq!(
        attention.attention_reason.as_deref(),
        Some("plan_digest_mismatch")
    );
    assert!(
        attention
            .commands
            .iter()
            .all(|command| command.kind != "approve_plan")
    );
    let recovery = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::ExplicitRecovery {
            expected_target: attention.permitted_recovery_target.unwrap(),
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    )
    .unwrap_err();
    assert_eq!(recovery.category(), "guard_failed");
    assert!(
        recovery
            .to_string()
            .contains("does not match the plan body"),
        "{recovery}"
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::NeedsAttention
    );
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Cancelling
    );
}

#[test]
fn reject_outside_plan_approval_records_no_plan_digest() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    deliver(&mut host, &created.mission_id);
    let note = plan_note("plan");
    notify(&mut host, &created.mission_id, note);
    let plan_digest = host
        .snapshot(&created.mission_id)
        .unwrap()
        .plan_digest
        .unwrap();
    approve(&mut host, &created.mission_id, &plan_digest, "operator").unwrap();
    assert_eq!(
        notify(
            &mut host,
            &created.mission_id,
            RuntimeNotification::ImplementationSettled { quiescent: true }
        ),
        Phase::Verifying
    );
    let during_verification = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::Reject {
            rationale: "not from verification".into(),
        },
    )
    .unwrap_err();
    assert_eq!(during_verification.category(), "illegal_transition");
    assert!(
        host.decisions(&created.mission_id)
            .unwrap()
            .iter()
            .all(|decision| decision.action != "reject")
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Verifying
    );

    let content = host.snapshot(&created.mission_id).unwrap().content_digest;
    assert_eq!(
        notify(&mut host, &created.mission_id, pass_note(&content)),
        Phase::AwaitingReview
    );
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::Reject {
            rationale: "review says no".into(),
        },
    );
    let rejected = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(rejected.phase, Phase::Rejected);
    let decision = host
        .decisions(&created.mission_id)
        .unwrap()
        .into_iter()
        .find(|decision| decision.action == "reject")
        .unwrap();
    assert_eq!(decision.digest, None);
    assert_ne!(decision.digest.as_deref(), Some(plan_digest.as_str()));
    assert_eq!(decision.actor, "operator");
}
