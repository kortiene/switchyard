use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use tempfile::TempDir;

use crate::delivery::{publication_operation_id, publication_payload, publication_payload_hash};
use crate::domain::AttentionReason;
use crate::execution::{FakeRuntime, RuntimeNotification, RuntimeRequest, operation_id};
use crate::policy::plan_digest_for;
use crate::{
    AdmitRequest, CommandBody, CommandOrigin, CommandState, EffectProof, Error, Host,
    OperationState, Phase, PublicationProof,
};

mod recovery_regressions;

struct World {
    _tmp: TempDir,
    db: PathBuf,
    repo: PathBuf,
}

fn open_world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    World {
        db: tmp.path().join("mission.sqlite"),
        repo,
        _tmp: tmp,
    }
}

fn init_repo(repo: &Path) {
    git(repo, &["init", "--initial-branch=main"]);
    git(repo, &["config", "user.email", "switchyard@example.com"]);
    git(repo, &["config", "user.name", "Switchyard"]);
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    git(repo, &["add", "README.md"]);
    git(repo, &["commit", "-m", "init"]);
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_out(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn fingerprint(repo: &Path) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        git_out(repo, &["rev-parse", "HEAD"]),
        git_out(repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
        git_out(repo, &["status", "--porcelain=v1"]),
        git_out(repo, &["diff", "--cached", "--name-only"])
    )
}

fn create(host: &mut Host, repo: &Path, max_repairs: u32) -> crate::MissionSnapshot {
    host.create_mission(crate::CreateMission {
        repo: repo.to_path_buf(),
        issue_number: 95,
        issue_title: "S1".into(),
        objective: "Establish the ledger".into(),
        acceptance: "Restart restores the same attempt".into(),
        actor: "operator".into(),
        max_repairs,
        max_plan_revisions: 3,
    })
    .unwrap()
}

fn deliver(host: &mut Host, mission: &str) {
    host.dispatch_pending(mission).unwrap();
}

fn admit(host: &mut Host, mission: &str, body: CommandBody) -> crate::AdmitResult {
    let snap = host.snapshot(mission).unwrap();
    host.admit(AdmitRequest {
        command_id: None,
        mission_id: mission.to_string(),
        attempt_id: None,
        expected_revision: snap.revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body,
    })
    .unwrap()
}

fn admit_expect(host: &mut Host, mission: &str, body: CommandBody) -> Result<(), Error> {
    let snap = host.snapshot(mission).unwrap();
    host.admit(AdmitRequest {
        command_id: None,
        mission_id: mission.to_string(),
        attempt_id: None,
        expected_revision: snap.revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body,
    })
    .map(|_| ())
}

fn notify(host: &mut Host, mission: &str, notification: RuntimeNotification) -> Phase {
    host.runtime_mut().push_notification(notification);
    let outcome = host.poll_runtime(mission).unwrap().pop().unwrap();
    outcome.phase()
}

fn plan_note(plan: &str) -> RuntimeNotification {
    RuntimeNotification::PlanCompleted {
        plan: plan.into(),
        digest: plan_digest_for(plan).unwrap(),
        quiescent: true,
    }
}

fn pass_note(digest: &str) -> RuntimeNotification {
    RuntimeNotification::VerificationPass {
        content_digest: digest.into(),
        evidence: "host observed a passing check".into(),
        requirements: vec!["Restart restores the same attempt".into()],
        completeness: "complete".into(),
    }
}

fn fail_note(digest: &str) -> RuntimeNotification {
    RuntimeNotification::VerificationFail {
        content_digest: digest.into(),
        evidence: "assertion failed".into(),
        requirements: vec!["Restart restores the same attempt".into()],
        completeness: "complete".into(),
    }
}

fn proof_for(digest: &str, pr_id: &str) -> PublicationProof {
    PublicationProof {
        content_digest: digest.into(),
        commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        tree_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        pr_id: pr_id.into(),
    }
}

fn drive_to_publishing(host: &mut Host, mission: &str) -> String {
    admit(host, mission, CommandBody::StartPlanning);
    host.dispatch_pending(mission).unwrap();
    let plan = "implement the ledger";
    assert_eq!(
        notify(host, mission, plan_note(plan)),
        Phase::AwaitingPlanApproval
    );
    admit(
        host,
        mission,
        CommandBody::ApprovePlan {
            plan_digest: plan_digest_for(plan).unwrap(),
        },
    );
    assert_eq!(
        notify(
            host,
            mission,
            RuntimeNotification::ImplementationSettled { quiescent: true }
        ),
        Phase::Verifying
    );
    let digest = host.snapshot(mission).unwrap().content_digest;
    assert_eq!(
        notify(host, mission, pass_note(&digest)),
        Phase::AwaitingReview
    );
    admit(
        host,
        mission,
        CommandBody::ApproveReview {
            digest: digest.clone(),
            rationale: "matches the frozen acceptance".into(),
        },
    );
    admit(
        host,
        mission,
        CommandBody::Publish {
            target_branch: "switchyard/s1".into(),
            base_branch: "main".into(),
            authorized: true,
        },
    );
    assert_eq!(host.snapshot(mission).unwrap().phase, Phase::Publishing);
    digest
}

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
fn second_active_owner_is_rejected_and_inspection_does_not_take_the_lock() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let generation = host.generation();
    let revision = created.revision;
    match Host::open(&world.db) {
        Err(error) => assert_eq!(error.category(), "second_active_owner"),
        Ok(_) => panic!("a second active owner was admitted"),
    }
    let inspected = Host::inspect(&world.db, Some(&created.mission_id)).unwrap();
    assert_eq!(inspected[0].revision, revision);
    assert_eq!(inspected[0].attempt_id, created.attempt_id);
    assert_eq!(inspected[0].recorded_owner_generation, generation);
    assert_eq!(host.generation(), generation);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().revision,
        revision
    );
    drop(host);
    std::fs::remove_file(crate::execution::lock_path(&world.db)).unwrap();
    let _ = Host::inspect(&world.db, Some(&created.mission_id)).unwrap();
    assert!(!crate::execution::lock_path(&world.db).exists());
}

#[test]
fn fake_runtime_drives_the_state_machine_without_credentials_or_git_writes() {
    let world = open_world();
    let before = fingerprint(&world.repo);
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let attempt = created.attempt_id.clone();
    let digest = drive_to_publishing(&mut host, &created.mission_id);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().attempt_id,
        attempt
    );
    assert_eq!(fingerprint(&world.repo), before);
    let published = notify(
        &mut host,
        &created.mission_id,
        RuntimeNotification::PublicationObserved {
            content_digest: digest.clone(),
            commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            tree_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            pr_id: "101".into(),
        },
    );
    assert_eq!(published, Phase::Published);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().attempt_id,
        attempt
    );
    assert_eq!(fingerprint(&world.repo), before);
    let terminal = admit_expect(&mut host, &created.mission_id, CommandBody::Cancel).unwrap_err();
    assert_eq!(terminal.category(), "terminal_immutable");
    let source_revision = host.snapshot(&created.mission_id).unwrap().revision;
    let successor = host
        .create_successor(&created.mission_id, "operator")
        .unwrap();
    assert_ne!(successor.mission_id, created.mission_id);
    assert_ne!(successor.attempt_id, attempt);
    assert_eq!(successor.phase, Phase::Draft);
    assert_eq!(
        successor.successor_of_mission_id.as_deref(),
        Some(created.mission_id.as_str())
    );
    let source = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(source.phase, Phase::Published);
    assert_eq!(source.revision, source_revision);
    let text = ledger_text(&world.db);
    assert!(
        text.contains("\"credentials\":\"none\"") || text.contains("\"credentials\": \"none\""),
        "runtime config must record that the fake adapter has no credentials"
    );
    assert!(!text.contains("ghp_"));
    assert!(!text.contains("sk-"));
}

fn ledger_text(db: &Path) -> String {
    let mut bytes = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{suffix}", db.display()));
        if let Ok(chunk) = std::fs::read(&path) {
            bytes.extend(chunk);
        }
    }
    let journal = crate::journal_path(db);
    if let Ok(chunk) = std::fs::read(journal) {
        bytes.extend(chunk);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

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
fn cancellation_during_publication_can_finish_or_discover_the_pr() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let digest = drive_to_publishing(&mut host, &created.mission_id);
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    let cancelling = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(
        cancelling.delivery.as_ref().unwrap().approved_digest,
        digest
    );
    assert_eq!(cancelling.phase, Phase::Cancelling);
    assert_eq!(
        cancelling.delivery.as_ref().unwrap().publication_state,
        "intent_recorded"
    );
    deliver(&mut host, &created.mission_id);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    );
    let cancelled = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(cancelled.phase, Phase::Cancelled);
    assert!(
        cancelled.delivery.is_some(),
        "created branch/commit intent is retained"
    );
    drop(host);

    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let digest = drive_to_publishing(&mut host, &created.mission_id);
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    let delivery_id = host
        .snapshot(&created.mission_id)
        .unwrap()
        .delivery
        .unwrap()
        .id;
    let proof = proof_for(&digest, "101");
    deliver(&mut host, &created.mission_id);
    host.record_operation(
        &created.mission_id,
        &publication_operation_id(&delivery_id).unwrap(),
        &publication_payload(&proof),
        OperationState::Completed,
    )
    .unwrap();
    let blocked = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    )
    .unwrap_err();
    assert_eq!(blocked.category(), "guard_failed");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Cancelling
    );
    assert_eq!(
        notify(
            &mut host,
            &created.mission_id,
            RuntimeNotification::PublicationObserved {
                content_digest: proof.content_digest.clone(),
                commit_sha: proof.commit_sha.clone(),
                tree_sha: proof.tree_sha.clone(),
                pr_id: proof.pr_id.clone(),
            }
        ),
        Phase::Published
    );
    let _ = publication_payload_hash(&proof).unwrap();
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
fn real_git_identity_is_frozen_and_a_non_repo_is_rejected() {
    let world = open_world();
    let before = fingerprint(&world.repo);
    let head = git_out(&world.repo, &["rev-parse", "HEAD"]);
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    assert_eq!(created.head_sha, head.trim());
    assert_eq!(created.base_sha, head.trim());
    assert_eq!(fingerprint(&world.repo), before);
    std::fs::write(world.repo.join("later.txt"), "later\n").unwrap();
    git(&world.repo, &["add", "later.txt"]);
    git(&world.repo, &["commit", "-m", "later"]);
    let snap = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(snap.base_sha, head.trim());
    assert_ne!(
        snap.base_sha,
        git_out(&world.repo, &["rev-parse", "HEAD"]).trim()
    );
    drop(host);

    let mut fresh = Host::open(&world._tmp.path().join("other.sqlite")).unwrap();
    let empty = world._tmp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let error = fresh
        .create_mission(crate::CreateMission {
            repo: empty,
            issue_number: 1,
            issue_title: String::new(),
            objective: "x".into(),
            acceptance: "y".into(),
            actor: "operator".into(),
            max_repairs: 1,
            max_plan_revisions: 1,
        })
        .unwrap_err();
    assert_eq!(error.category(), "git_failed");
    let bare = world._tmp.path().join("nocommit");
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "--initial-branch=main"]);
    let error = fresh
        .create_mission(crate::CreateMission {
            repo: bare,
            issue_number: 1,
            issue_title: String::new(),
            objective: "x".into(),
            acceptance: "y".into(),
            actor: "operator".into(),
            max_repairs: 1,
            max_plan_revisions: 1,
        })
        .unwrap_err();
    assert_eq!(error.category(), "git_failed");
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
fn aliased_database_paths_share_one_owner_and_generation_is_reread() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let alias = world._tmp.path().join("alias.sqlite");
    std::os::unix::fs::symlink(&world.db, &alias).unwrap();
    match Host::open(&alias) {
        Err(error) => assert_eq!(error.category(), "second_active_owner"),
        Ok(_) => panic!("symlink opened a second owner"),
    }
    let alias_lock = crate::execution::lock_path(&alias);
    assert!(
        !alias_lock.exists(),
        "alias must not grow its own owner lock"
    );
    let generation = host.generation();
    let conn = rusqlite::Connection::open(&world.db).unwrap();
    conn.execute("UPDATE host_owner SET generation = generation + 1", [])
        .unwrap();
    let error =
        admit_expect(&mut host, &created.mission_id, CommandBody::StartPlanning).unwrap_err();
    assert_eq!(error.category(), "stale_generation");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Draft
    );
    let binding = crate::RuntimeBinding {
        mission_id: created.mission_id.clone(),
        attempt_id: created.attempt_id.clone(),
        session_id: created.session_id.clone(),
        generation,
    };
    assert_eq!(
        host.accept_runtime_binding(&binding, &created.mission_id)
            .unwrap_err()
            .category(),
        "stale_generation"
    );
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

fn command_payload(db: &Path, request_id: &str) -> String {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT payload_canonical FROM commands WHERE request_id = ?1",
        [request_id],
        |row| row.get(0),
    )
    .unwrap()
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

#[test]
fn ci_pins_the_rust_core_job_without_replacing_npm_verify() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let toolchain = std::fs::read_to_string(root.join("rust-toolchain.toml")).unwrap();
    let workflow = std::fs::read_to_string(root.join(".github/workflows/verify.yml")).unwrap();
    assert!(toolchain.contains("1.99.1"));
    assert!(workflow.contains("name: rust-core"));
    assert!(workflow.contains("runs-on: ubuntu-24.04"));
    assert!(workflow.contains("toolchain: 1.99.1"));
    assert!(workflow.contains("npm run verify"));
    assert!(workflow.contains("working-directory: adw_sdlc"));
    assert!(workflow.contains("20.19.0"));
}

#[test]
fn core_manifest_has_no_model_or_forge_client() {
    let manifest = include_str!("../Cargo.toml");
    for banned in ["reqwest", "octocrab", "git2", "libgit2", "reqwest"] {
        assert!(
            !manifest.contains(banned),
            "{banned} must not be a dependency"
        );
    }
}
