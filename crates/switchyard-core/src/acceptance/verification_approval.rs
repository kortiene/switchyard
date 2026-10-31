use super::fixtures::{
    World, admit, admit_expect, create, deliver, drive_to_publishing, notify, open_world,
    pass_note, plan_note,
};
use crate::domain::AttentionReason;
use crate::execution::RuntimeNotification;
use crate::policy::plan_digest_for;
use crate::{CommandBody, EffectProof, Host, Phase};
use serde_json::json;
use std::path::Path;

#[test]
fn content_drift_recovers_to_verifying_and_publication_stays_reconcile_only() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let digest = drive_to_publishing(&mut host, &created.mission_id);
    host.note_content_digest(
        &created.mission_id,
        host.snapshot(&created.mission_id).unwrap().revision,
        "drifted-digest",
        "operator",
    )
    .unwrap();
    let attention = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(attention.phase, Phase::NeedsAttention);
    assert_eq!(attention.permitted_recovery_target, Some(Phase::Publishing));
    assert!(attention.reconcile_only);
    let rejected = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Approved,
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    )
    .unwrap_err();
    assert_eq!(rejected.category(), "guard_failed");
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Publishing,
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Publishing
    );
    let republish = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::Publish {
            target_branch: "other".into(),
            base_branch: "main".into(),
            authorized: true,
        },
    )
    .unwrap_err();
    assert_eq!(republish.category(), "illegal_transition");
    assert_eq!(
        notify(
            &mut host,
            &created.mission_id,
            RuntimeNotification::PublicationObserved {
                content_digest: digest,
                commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                tree_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                pr_id: "101".into(),
            }
        ),
        Phase::Published
    );
    drop(host);

    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let digest = drive_to_publishing(&mut host, &created.mission_id);
    assert_eq!(
        notify(
            &mut host,
            &created.mission_id,
            RuntimeNotification::PublicationObserved {
                content_digest: digest.clone(),
                commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                tree_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                pr_id: "101".into(),
            }
        ),
        Phase::Published
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Published
    );
    drop(host);
    let mut host = Host::open(&world.db).unwrap();
    let approved = world_approved_for_drift(&mut host, &world);
    host.note_content_digest(
        &approved.mission_id,
        host.snapshot(&approved.mission_id).unwrap().revision,
        "changed-after-approval",
        "operator",
    )
    .unwrap();
    let snap = host.snapshot(&approved.mission_id).unwrap();
    assert_eq!(snap.phase, Phase::NeedsAttention);
    assert_eq!(snap.permitted_recovery_target, Some(Phase::Verifying));
    admit(
        &mut host,
        &approved.mission_id,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Verifying,
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    );
    assert_eq!(
        host.snapshot(&approved.mission_id).unwrap().phase,
        Phase::Verifying
    );
}

fn world_approved_for_drift(host: &mut Host, world: &World) -> crate::MissionSnapshot {
    let created = create(host, &world.repo, 2);
    admit(host, &created.mission_id, CommandBody::StartPlanning);
    host.dispatch_pending(&created.mission_id).unwrap();
    notify(host, &created.mission_id, plan_note("plan"));
    admit(
        host,
        &created.mission_id,
        CommandBody::ApprovePlan {
            plan_digest: plan_digest_for("plan").unwrap(),
        },
    );
    notify(
        host,
        &created.mission_id,
        RuntimeNotification::ImplementationSettled { quiescent: true },
    );
    let digest = host.snapshot(&created.mission_id).unwrap().content_digest;
    notify(host, &created.mission_id, pass_note(&digest));
    admit(
        host,
        &created.mission_id,
        CommandBody::ApproveReview {
            digest,
            rationale: "approve the current digest".into(),
        },
    );
    host.snapshot(&created.mission_id).unwrap()
}

#[test]
fn terminal_missions_reject_content_writes() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    deliver(&mut host, &created.mission_id);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    );
    let before = host.snapshot(&created.mission_id).unwrap();
    let error = host
        .note_content_digest(
            &created.mission_id,
            before.revision,
            "rewritten",
            "operator",
        )
        .unwrap_err();
    assert_eq!(error.category(), "terminal_immutable");
    let after = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(after.phase, Phase::Cancelled);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.content_digest, before.content_digest);

    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    host.dispatch_pending(&created.mission_id).unwrap();
    notify(&mut host, &created.mission_id, plan_note("plan"));
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::Reject {
            rationale: "stop".into(),
        },
    );
    let before = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(before.phase, Phase::Rejected);
    let error = host
        .note_content_digest(
            &created.mission_id,
            before.revision,
            "rewritten",
            "operator",
        )
        .unwrap_err();
    assert_eq!(error.category(), "terminal_immutable");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().revision,
        before.revision
    );
}

#[test]
fn paused_review_drift_repairs_the_resume_target() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let approved = world_approved_for_drift(&mut host, &world);
    admit(&mut host, &approved.mission_id, CommandBody::Pause);
    deliver(&mut host, &approved.mission_id);
    admit(&mut host, &approved.mission_id, CommandBody::ConfirmStopped);
    assert_eq!(
        host.snapshot(&approved.mission_id).unwrap().resume_target,
        Some(Phase::Approved)
    );
    host.note_content_digest(
        &approved.mission_id,
        host.snapshot(&approved.mission_id).unwrap().revision,
        "drifted-while-paused",
        "operator",
    )
    .unwrap();
    admit(
        &mut host,
        &approved.mission_id,
        CommandBody::DetectUncertainty {
            reason: AttentionReason::Uncertainty,
            content_digest: None,
        },
    );
    assert_eq!(
        host.snapshot(&approved.mission_id)
            .unwrap()
            .permitted_recovery_target,
        Some(Phase::Paused)
    );
    admit(
        &mut host,
        &approved.mission_id,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Paused,
            ownership_proven: true,
            effects: EffectProof::Reconciled,
        },
    );
    let recovered = host.snapshot(&approved.mission_id).unwrap();
    assert_eq!(recovered.phase, Phase::Paused);
    assert_eq!(recovered.resume_target, Some(Phase::Verifying));
    admit(
        &mut host,
        &approved.mission_id,
        CommandBody::Resume { authorized: true },
    );
    assert_eq!(
        host.snapshot(&approved.mission_id).unwrap().phase,
        Phase::Verifying
    );
}

#[test]
fn paused_review_targets_without_pass_evidence_resume_into_verifying() {
    for approve in [false, true] {
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
        notify(&mut host, &created.mission_id, pass_note(&digest));
        if approve {
            admit(
                &mut host,
                &created.mission_id,
                CommandBody::ApproveReview {
                    digest: digest.clone(),
                    rationale: "approve".into(),
                },
            );
            assert_eq!(
                host.snapshot(&created.mission_id).unwrap().phase,
                Phase::Approved
            );
        } else {
            assert_eq!(
                host.snapshot(&created.mission_id).unwrap().phase,
                Phase::AwaitingReview
            );
            assert!(
                host.snapshot(&created.mission_id)
                    .unwrap()
                    .reviewed_digest
                    .is_none()
            );
        }
        admit(&mut host, &created.mission_id, CommandBody::Pause);
        deliver(&mut host, &created.mission_id);
        admit(&mut host, &created.mission_id, CommandBody::ConfirmStopped);
        host.note_content_digest(
            &created.mission_id,
            host.snapshot(&created.mission_id).unwrap().revision,
            "drifted-review-content",
            "operator",
        )
        .unwrap();
        admit(
            &mut host,
            &created.mission_id,
            CommandBody::DetectUncertainty {
                reason: AttentionReason::Uncertainty,
                content_digest: None,
            },
        );
        admit(
            &mut host,
            &created.mission_id,
            CommandBody::ExplicitRecovery {
                expected_target: Phase::Paused,
                ownership_proven: true,
                effects: EffectProof::Reconciled,
            },
        );
        let paused = host.snapshot(&created.mission_id).unwrap();
        assert_eq!(paused.phase, Phase::Paused);
        assert_eq!(paused.resume_target, Some(Phase::Verifying));
        admit(
            &mut host,
            &created.mission_id,
            CommandBody::Resume { authorized: true },
        );
        assert_eq!(
            host.snapshot(&created.mission_id).unwrap().phase,
            Phase::Verifying
        );
    }
}

#[test]
fn terminal_verification_notifications_do_not_write_evidence() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let digest = drive_to_publishing(&mut host, &created.mission_id);
    notify(
        &mut host,
        &created.mission_id,
        RuntimeNotification::PublicationObserved {
            content_digest: digest,
            commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            tree_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            pr_id: "101".into(),
        },
    );
    assert_terminal_notification_is_inert(
        &world.db,
        &mut host,
        &created.mission_id,
        Phase::Published,
    );

    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    deliver(&mut host, &created.mission_id);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    );
    assert_terminal_notification_is_inert(
        &world.db,
        &mut host,
        &created.mission_id,
        Phase::Cancelled,
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
        CommandBody::Reject {
            rationale: "no".into(),
        },
    );
    assert_terminal_notification_is_inert(
        &world.db,
        &mut host,
        &created.mission_id,
        Phase::Rejected,
    );
}

fn assert_terminal_notification_is_inert(db: &Path, host: &mut Host, mission: &str, phase: Phase) {
    let before_evidence = sql_count(db, "evidence");
    let before_verifications = sql_count(db, "verifications");
    let before_latest = latest_outcome(db, mission);
    let before = host.snapshot(mission).unwrap();
    for kind in ["verification_error", "verification_incomplete"] {
        let error = host
            .apply_notification_json(mission, &json!({"kind": kind, "detail": "late"}))
            .unwrap_err();
        assert_eq!(error.category(), "terminal_immutable");
    }
    let after = host.snapshot(mission).unwrap();
    assert_eq!(after.phase, phase);
    assert_eq!(after.revision, before.revision);
    assert_eq!(sql_count(db, "evidence"), before_evidence);
    assert_eq!(sql_count(db, "verifications"), before_verifications);
    assert_eq!(latest_outcome(db, mission), before_latest);
}

fn sql_count(db: &Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn latest_outcome(db: &Path, mission: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT outcome FROM verifications WHERE mission_id = ?1 AND invalidated = 0 ORDER BY seq DESC LIMIT 1",
        [mission],
        |row| row.get(0),
    )
    .ok()
}
