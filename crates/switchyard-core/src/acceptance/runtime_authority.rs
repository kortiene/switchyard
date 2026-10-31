use super::*;

#[test]
fn runtime_notifications_never_set_mission_status_directly() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let error = host
        .apply_notification_json(
            &created.mission_id,
            &json!({"kind": "set_status", "status": "published"}),
        )
        .unwrap_err();
    assert_eq!(error.category(), "unknown_notification");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Draft
    );
    let illegal = host
        .apply_notification_json(
            &created.mission_id,
            &json!({
                "kind": "plan_completed",
                "plan": "x",
                "digest": plan_digest_for("x").unwrap(),
                "quiescent": true,
                "status": "published"
            }),
        )
        .unwrap_err();
    assert_eq!(illegal.category(), "illegal_transition");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Draft
    );

    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let outcome = host
        .apply_notification_json(
            &created.mission_id,
            &json!({
                "kind": "plan_completed",
                "plan": "x",
                "digest": plan_digest_for("x").unwrap(),
                "quiescent": true,
                "status": "published"
            }),
        )
        .unwrap();
    assert_eq!(outcome.phase(), Phase::AwaitingPlanApproval);
    let revision = outcome.revision();
    let again = host
        .apply_notification_json(
            &created.mission_id,
            &json!({
                "kind": "plan_completed",
                "plan": "x",
                "digest": plan_digest_for("x").unwrap(),
                "quiescent": true
            }),
        )
        .unwrap();
    assert_eq!(again.phase(), Phase::AwaitingPlanApproval);
    assert_eq!(again.revision(), revision);
}

#[test]
fn incomplete_notification_frames_do_not_advance_and_errors_keep_the_queue() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
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
    for body in [
        json!({"kind": "verification_pass", "contentDigest": digest, "evidence": "x", "requirements": ["a"]}),
        json!({"kind": "verification_pass", "contentDigest": digest, "evidence": "x", "requirements": ["a"], "completeness": null}),
        json!({"kind": "verification_fail", "contentDigest": digest, "evidence": "x", "requirements": ["a"], "completeness": 1}),
    ] {
        let error = host
            .apply_notification_json(&created.mission_id, &body)
            .unwrap_err();
        assert_eq!(error.category(), "guard_failed", "{body}");
        assert_eq!(
            host.snapshot(&created.mission_id).unwrap().phase,
            Phase::Verifying
        );
    }
    let error =
        admit_expect(&mut host, &created.mission_id, CommandBody::CompletePass).unwrap_err();
    assert_eq!(error.category(), "guard_failed");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Verifying
    );

    host.runtime_mut().push_notification(pass_note(&digest));
    host.runtime_mut().push_notification(plan_note("later"));
    host.runtime_mut()
        .push_notification(RuntimeNotification::Idle);
    host.poll_runtime(&created.mission_id).unwrap();
    let snap = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(snap.phase, Phase::AwaitingReview);
    assert_eq!(host.runtime_mut().queued_notifications(), 0);
    assert_eq!(snap.quarantined.len(), 1);
    assert_eq!(snap.quarantined[0].1, "illegal_transition");
    assert!(snap.quarantined[0].3.contains("plan_completed"));
    host.poll_runtime(&created.mission_id).unwrap();
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::AwaitingReview
    );
}

#[test]
fn operators_cannot_certify_observations_and_origin_urls_drop_userinfo() {
    let world = open_world();
    git(
        &world.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://user:supersecret@github.com/kortiene/switchyard.git",
        ],
    );
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let stored = ledger_text(&world.db);
    assert!(!stored.contains("supersecret"));
    assert!(stored.contains("github.com/kortiene/switchyard.git"));
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let error = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::PlanCompleted {
            plan: "plan".into(),
            digest: plan_digest_for("plan").unwrap(),
            quiescent: true,
        },
    )
    .unwrap_err();
    assert_eq!(error.category(), "guard_failed");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Planning
    );
    notify(&mut host, &created.mission_id, plan_note("plan"));
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::AwaitingPlanApproval
    );
}
