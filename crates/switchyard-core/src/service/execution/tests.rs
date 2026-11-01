use super::*;
use crate::{ExecutionOperation, FAKE_WRITE_SCOPE};
use rusqlite::Connection;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn setup() -> (tempfile::TempDir, Host, ExecutionRequest) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let root = dir.path().join("workspaces");
    std::fs::create_dir(&repo).unwrap();
    std::fs::create_dir(&root).unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.email", "exec@example.com"]);
    git(&repo, &["config", "user.name", "Exec"]);
    std::fs::write(repo.join("README.md"), "original").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "base"]);
    let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
    let m = host
        .create_mission(CreateMission {
            repo,
            issue_number: 123,
            issue_title: "execution".into(),
            objective: "simulated effect".into(),
            acceptance: "no duplicate".into(),
            actor: "operator".into(),
            max_repairs: 1,
            max_plan_revisions: 1,
        })
        .unwrap();
    host.prepare_workspace(&m.mission_id, &m.attempt_id, &root)
        .unwrap();
    command(&mut host, &m.mission_id, CommandBody::StartPlanning);
    host.dispatch_pending(&m.mission_id).unwrap();
    let digest = crate::policy::plan_digest_for("plan").unwrap();
    host.apply_notification(
        &m.mission_id,
        RuntimeNotification::PlanCompleted {
            plan: "plan".into(),
            digest: digest.clone(),
            quiescent: true,
        },
    )
    .unwrap();
    command(
        &mut host,
        &m.mission_id,
        CommandBody::ApprovePlan {
            plan_digest: digest,
        },
    );
    let mission = host.store.load_mission(&m.mission_id).unwrap();
    assert_eq!(mission.phase, Phase::Implementing);
    let request = ExecutionRequest {
        binding: host.binding(&mission),
        tool_task_id: "task-1".into(),
        effect_key: "write-file:v1".into(),
        capability_scope: FAKE_WRITE_SCOPE.into(),
        deadline_ms: crate::domain::now_ms() + 60000,
        operation: ExecutionOperation::WriteFile {
            path: "out.txt".into(),
            content: "simulated".into(),
        },
    };
    (dir, host, request)
}
fn command(host: &mut Host, mission: &str, body: CommandBody) {
    let revision = host.snapshot(mission).unwrap().revision;
    host.admit(AdmitRequest {
        command_id: None,
        mission_id: mission.into(),
        attempt_id: None,
        expected_revision: revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body,
    })
    .unwrap();
}
fn state(db: &Path, id: &str) -> String {
    Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT state FROM execution_effects WHERE operation_id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
}
#[test]
fn completed_replay_retains_receipt_without_touching_files_or_runtime() {
    let (dir, mut host, mut request) = setup();
    let receipt = host.execute_fake_operation(&request).unwrap();
    assert!(receipt.simulated);
    assert_eq!(host.execute_fake_operation(&request).unwrap(), receipt);
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 1);
    let workspace = host
        .snapshot(&request.binding.mission_id)
        .unwrap()
        .workspace
        .unwrap();
    assert!(!Path::new(&workspace.path).join("out.txt").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("repo/README.md")).unwrap(),
        "original"
    );
    command(&mut host, &request.binding.mission_id, CommandBody::Pause);
    drop(host);
    let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
    assert_eq!(
        host.execute_fake_operation(&request)
            .unwrap_err()
            .category(),
        "stale_generation"
    );
    request.binding.generation = host.generation();
    assert_eq!(host.execute_fake_operation(&request).unwrap(), receipt);
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
}
#[test]
fn payload_conflicts_and_distinct_task_effect_identities() {
    let (_dir, mut host, request) = setup();
    host.execute_fake_operation(&request).unwrap();
    for n in 0..4 {
        let mut changed = request.clone();
        match n {
            0 => changed.deadline_ms += 1,
            1 => changed.capability_scope = "other".into(),
            2 => {
                changed.operation = ExecutionOperation::WriteFile {
                    path: "other".into(),
                    content: "simulated".into(),
                }
            }
            _ => {
                changed.operation = ExecutionOperation::WriteFile {
                    path: "out.txt".into(),
                    content: "changed".into(),
                }
            }
        }
        assert_eq!(
            host.execute_fake_operation(&changed)
                .unwrap_err()
                .category(),
            "operation_conflict"
        );
    }
    for (task, key) in [
        ("task-1", "second-write"),
        ("task-1/child", "write-file:v1"),
    ] {
        let mut next = request.clone();
        next.tool_task_id = task.into();
        next.effect_key = key.into();
        assert_ne!(
            next.operation_id().unwrap(),
            request.operation_id().unwrap()
        );
        host.execute_fake_operation(&next).unwrap();
    }
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 3);
}
#[test]
fn invalid_binding_scope_paths_deadline_and_phase_never_apply() {
    let (_dir, mut host, request) = setup();
    for n in 0..3 {
        let mut bad = request.clone();
        match n {
            0 => bad.binding.mission_id = "other".into(),
            1 => bad.binding.attempt_id = "other".into(),
            _ => bad.binding.session_id = "other".into(),
        }
        assert!(host.execute_fake_operation(&bad).is_err());
    }
    for path in [
        "../escape",
        "/absolute",
        ".git/config",
        "a/../b",
        "a/.GIT/x",
        "a\\b",
        "",
    ] {
        let mut bad = request.clone();
        bad.operation = ExecutionOperation::WriteFile {
            path: path.into(),
            content: "x".into(),
        };
        assert!(host.execute_fake_operation(&bad).is_err());
    }
    let mut bad = request.clone();
    bad.deadline_ms = 0;
    assert!(host.execute_fake_operation(&bad).is_err());
    bad = request.clone();
    bad.capability_scope = "host.write".into();
    assert!(host.execute_fake_operation(&bad).is_err());
    command(&mut host, &request.binding.mission_id, CommandBody::Pause);
    assert!(host.execute_fake_operation(&request).is_err());
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
}
#[test]
fn interrupted_intent_and_effect_are_quarantined_on_recovery() {
    for stage in ["prepared", "effect"] {
        let (dir, mut host, mut request) = setup();
        assert!(
            host.execute_fake_at(&request, |at| if at == stage {
                Err(Error::storage("crash"))
            } else {
                Ok(())
            })
            .is_err()
        );
        assert_eq!(
            host.runtime_mut().unwrap().simulated_effect_count(),
            usize::from(stage == "effect")
        );
        let id = request.operation_id().unwrap();
        assert_eq!(state(&dir.path().join("mission.sqlite"), &id), "prepared");
        drop(host);
        let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
        request.binding.generation = host.generation();
        let recovered = host.recover(&request.binding.mission_id).unwrap();
        assert_eq!(recovered.phase, Phase::NeedsAttention);
        assert_eq!(state(&dir.path().join("mission.sqlite"), &id), "uncertain");
        assert_eq!(
            host.execute_fake_operation(&request)
                .unwrap_err()
                .category(),
            "execution_uncertain"
        );
        let mut next = request.clone();
        next.effect_key = "different".into();
        assert!(host.execute_fake_operation(&next).is_err());
        assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
        assert!(
            host.settle_operation(&id, &request.payload_hash().unwrap())
                .is_err()
        );
    }
}
#[test]
fn receipt_before_lost_reply_replays_and_stale_settlement_cannot_write() {
    let (dir, mut host, mut request) = setup();
    assert!(
        host.execute_fake_at(&request, |at| if at == "completed" {
            Err(Error::storage("lost reply"))
        } else {
            Ok(())
        })
        .is_err()
    );
    let id = request.operation_id().unwrap();
    let original = host
        .store
        .execution_effect(&id)
        .unwrap()
        .unwrap()
        .receipt
        .unwrap();
    drop(host);
    let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
    assert!(host.store.complete_execution(&request, &original).is_err());
    request.binding.generation = host.generation();
    assert_eq!(host.execute_fake_operation(&request).unwrap(), original);
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
}
#[test]
fn v3_inspection_and_forward_upgrade_preserve_mission_and_workspace() {
    let (dir, host, request) = setup();
    let before = host.snapshot(&request.binding.mission_id).unwrap();
    drop(host);
    let db = dir.path().join("mission.sqlite");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS linux_effect_identity_unique;
         DROP TRIGGER IF EXISTS simulated_effect_identity_unique;
         DROP TRIGGER IF EXISTS linux_resolution_command_unique;
         DROP TRIGGER IF EXISTS simulated_resolution_command_unique;
         DROP TABLE IF EXISTS linux_execution_resolutions;
         DROP TABLE IF EXISTS linux_execution_effects;
         DROP TABLE execution_resolutions;
         DROP TABLE execution_effects;
         DELETE FROM schema_migrations WHERE version>=4;
         PRAGMA user_version=3;",
    )
    .unwrap();
    drop(conn);
    let inspected = Host::inspect(&db, Some(&request.binding.mission_id)).unwrap();
    assert_eq!(inspected[0].workspace, before.workspace);
    let host = Host::open(&db).unwrap();
    assert_eq!(
        host.snapshot(&request.binding.mission_id)
            .unwrap()
            .workspace,
        before.workspace
    );
    assert_eq!(
        Connection::open(&db)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        7
    );
}

#[cfg(unix)]
#[test]
fn crash_worker() {
    use std::io::Write;
    let Ok(db) = std::env::var("SW_EXEC_TEST_DB") else {
        return;
    };
    let mission_id = std::env::var("SW_EXEC_TEST_MISSION").unwrap();
    let stage = std::env::var("SW_EXEC_TEST_STAGE").unwrap();
    let mut host = Host::open(Path::new(&db)).unwrap();
    let mission = host.store.load_mission(&mission_id).unwrap();
    let request = ExecutionRequest {
        binding: host.binding(&mission),
        tool_task_id: "task-1".into(),
        effect_key: "write-file:v1".into(),
        capability_scope: FAKE_WRITE_SCOPE.into(),
        deadline_ms: std::env::var("SW_EXEC_TEST_DEADLINE")
            .unwrap()
            .parse()
            .unwrap(),
        operation: ExecutionOperation::WriteFile {
            path: "out.txt".into(),
            content: "simulated".into(),
        },
    };
    host.execute_fake_at(&request, |at| {
        if at == stage {
            println!("AT-{at}");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
        Ok(())
    })
    .unwrap();
}

#[cfg(unix)]
#[test]
fn sigkill_at_intent_effect_and_receipt_boundaries() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::time::Duration;
    for stage in ["prepared", "effect", "completed"] {
        let (dir, host, mut request) = setup();
        drop(host);
        let db = dir.path().join("mission.sqlite");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "service::execution::tests::crash_worker",
                "--nocapture",
            ])
            .env("SW_EXEC_TEST_DB", &db)
            .env("SW_EXEC_TEST_MISSION", &request.binding.mission_id)
            .env("SW_EXEC_TEST_STAGE", stage)
            .env("SW_EXEC_TEST_DEADLINE", request.deadline_ms.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if let Ok(line) = line {
                    if line.starts_with("AT-") {
                        let _ = tx.send(line);
                        break;
                    }
                } else {
                    break;
                }
            }
        });
        let reached = rx.recv_timeout(Duration::from_secs(20));
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();
        assert_eq!(reached.unwrap(), format!("AT-{stage}"));
        let mut host = Host::open(&db).unwrap();
        request.binding.generation = host.generation();
        let views = Host::inspect(&db, Some(&request.binding.mission_id)).unwrap();
        assert_eq!(views[0].execution_effects.len(), 1);
        if stage == "completed" {
            assert_eq!(views[0].execution_effects[0].state, "completed");
            assert!(host.execute_fake_operation(&request).unwrap().simulated);
        } else {
            assert_eq!(views[0].execution_effects[0].state, "prepared");
            assert_eq!(
                host.execute_fake_operation(&request)
                    .unwrap_err()
                    .category(),
                "execution_uncertain"
            );
            assert_eq!(
                host.snapshot(&request.binding.mission_id).unwrap().phase,
                Phase::NeedsAttention
            );
        }
        assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
    }
}

#[test]
fn prepared_effect_rejects_late_settlement_from_a_previous_owner() {
    let (dir, mut host, request) = setup();
    host.execution_evidence
        .prepare(&mut host.store, &request)
        .unwrap();
    let receipt = host
        .execution_evidence
        .apply(&mut host.runtime, &request)
        .unwrap();
    drop(host);
    let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
    assert_eq!(
        host.store
            .complete_execution(&request, &receipt)
            .unwrap_err()
            .category(),
        "stale_generation"
    );
    assert_eq!(
        state(
            &dir.path().join("mission.sqlite"),
            &request.operation_id().unwrap()
        ),
        "prepared"
    );
    let mut changed = request.clone();
    changed.binding.generation = host.generation();
    assert!(host.store.complete_execution(&changed, &receipt).is_err());
}

#[test]
fn deadline_between_intent_and_apply_is_definite_and_nonblocking() {
    let (dir, mut host, mut request) = setup();
    let mut clock_calls = 0;
    let error = host
        .execute_fake_with_clock(
            &request,
            |_| Ok(()),
            || {
                clock_calls += 1;
                request.deadline_ms - i64::from(clock_calls == 1)
            },
        )
        .unwrap_err();
    assert_eq!(error.category(), "execution_deadline_exceeded");
    let id = request.operation_id().unwrap();
    assert_eq!(
        state(&dir.path().join("mission.sqlite"), &id),
        "not_applied"
    );
    assert_eq!(
        host.snapshot(&request.binding.mission_id).unwrap().phase,
        Phase::Implementing
    );
    assert!(!host.store.ops_blocked(&request.binding.mission_id).unwrap());
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
    drop(host);
    let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
    request.binding.generation = host.generation();
    assert_eq!(
        host.execute_fake_operation(&request)
            .unwrap_err()
            .category(),
        "execution_deadline_exceeded"
    );
    let mut changed = request.clone();
    changed.deadline_ms += 1;
    assert_eq!(
        host.execute_fake_operation(&changed)
            .unwrap_err()
            .category(),
        "operation_conflict"
    );
    changed = request.clone();
    changed.effect_key = "next-effect".into();
    host.execute_fake_operation(&changed).unwrap();
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 1);
    assert_eq!(
        host.recover(&request.binding.mission_id).unwrap().phase,
        Phase::Implementing
    );
}

#[test]
fn apply_and_receipt_errors_immediately_quarantine_without_hiding_the_cause() {
    for fail_apply in [true, false] {
        let (dir, mut host, request) = setup();
        if fail_apply {
            host.runtime_mut().unwrap().fail_next_simulated_effect();
        } else {
            Connection::open(dir.path().join("mission.sqlite")).unwrap().execute_batch(
                "CREATE TRIGGER fail_completion BEFORE UPDATE OF state ON execution_effects WHEN NEW.state='completed'
                 BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;"
            ).unwrap();
        }
        let error = host.execute_fake_operation(&request).unwrap_err();
        assert_eq!(error.category(), "storage_failed");
        assert!(error.to_string().contains(if fail_apply {
            "injected error after simulated effect"
        } else {
            "injected receipt failure"
        }));
        assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 1);
        let snapshot = host.snapshot(&request.binding.mission_id).unwrap();
        assert_eq!(snapshot.phase, Phase::NeedsAttention);
        assert_eq!(snapshot.execution_effects[0].state, "uncertain");
        let mut fresh = request.clone();
        fresh.effect_key = "fresh-effect".into();
        let blocked = host.execute_fake_operation(&fresh).unwrap_err();
        assert_eq!(blocked.category(), "execution_blocked");
        assert!(blocked.to_string().contains("unresolved execution effects"));
        assert!(
            host.store
                .execution_effect(&fresh.operation_id().unwrap())
                .unwrap()
                .is_none()
        );
        assert_eq!(
            host.execute_fake_operation(&request)
                .unwrap_err()
                .category(),
            "execution_uncertain"
        );
        assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 1);
    }
}

#[test]
fn failed_uncertainty_persistence_stays_blocked_and_reports_both_errors() {
    let (dir, mut host, request) = setup();
    let conn = Connection::open(dir.path().join("mission.sqlite")).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fail_updates BEFORE UPDATE ON execution_effects
        BEGIN SELECT RAISE(ABORT, 'injected ledger unavailable'); END;",
    )
    .unwrap();
    host.runtime_mut().unwrap().fail_next_simulated_effect();
    let error = host.execute_fake_operation(&request).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected error after simulated effect")
    );
    assert!(
        error
            .to_string()
            .contains("could not record execution uncertainty")
    );
    assert!(error.to_string().contains("injected ledger unavailable"));
    assert!(host.store.ops_blocked(&request.binding.mission_id).unwrap());
    assert_eq!(
        state(
            &dir.path().join("mission.sqlite"),
            &request.operation_id().unwrap()
        ),
        "prepared"
    );
    conn.execute_batch("DROP TRIGGER fail_updates;").unwrap();
    assert_eq!(
        host.recover(&request.binding.mission_id).unwrap().phase,
        Phase::NeedsAttention
    );
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 1);
}

#[test]
fn unattempted_request_names_the_existing_blocker_not_its_own_identity() {
    let (_dir, mut host, request) = setup();
    let payload = serde_json::json!({"old":"operation"});
    host.record_operation(
        &request.binding.mission_id,
        "old-s1-op",
        &payload,
        crate::OperationState::Prepared,
    )
    .unwrap();
    let error = host.execute_fake_operation(&request).unwrap_err();
    assert_eq!(error.category(), "execution_blocked");
    assert!(error.to_string().contains("generic S1"));
    assert!(!error.to_string().contains(&request.operation_id().unwrap()));
    assert!(
        host.store
            .execution_effect(&request.operation_id().unwrap())
            .unwrap()
            .is_none()
    );
    host.settle_operation("old-s1-op", &crate::payload_hash(&payload).unwrap())
        .unwrap();
    let mut prior = request.clone();
    prior.effect_key = "pending-effect".into();
    host.store.prepare_execution(&prior).unwrap();
    let error = host.execute_fake_operation(&request).unwrap_err();
    assert_eq!(error.category(), "execution_blocked");
    assert!(error.to_string().contains("unresolved execution effects"));
    assert!(
        host.store
            .execution_effect(&request.operation_id().unwrap())
            .unwrap()
            .is_none()
    );
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
}

#[test]
fn error_reconciliation_cannot_mutate_a_committed_result_or_a_new_owner() {
    let (dir, mut host, request) = setup();
    let receipt = host.execute_fake_operation(&request).unwrap();
    let error = host.execution_failure(&request, Error::storage("lost completion response"));
    assert!(error.to_string().contains("lost completion response"));
    assert_eq!(
        host.snapshot(&request.binding.mission_id).unwrap().phase,
        Phase::Implementing
    );
    assert_eq!(host.execute_fake_operation(&request).unwrap(), receipt);
    let mut pending = request.clone();
    pending.effect_key = "pending".into();
    host.store.prepare_execution(&pending).unwrap();
    drop(host);
    let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
    let error = host.execution_failure(&pending, Error::storage("late failure"));
    assert_eq!(error.category(), "stale_generation");
    assert!(error.to_string().contains("late failure"));
    assert!(!error.is_transient());
    assert_eq!(
        state(
            &dir.path().join("mission.sqlite"),
            &pending.operation_id().unwrap()
        ),
        "prepared"
    );
    assert_eq!(
        host.snapshot(&request.binding.mission_id).unwrap().phase,
        Phase::Implementing
    );
}

#[test]
fn v4_upgrade_preserves_completed_prepared_and_uncertain_effects() {
    let (dir, mut host, request) = setup();
    let receipt = host.execute_fake_operation(&request).unwrap();
    let mut pending = request.clone();
    pending.effect_key = "pending".into();
    host.store.prepare_execution(&pending).unwrap();
    let mut uncertain = request.clone();
    uncertain.effect_key = "uncertain".into();
    host.store.prepare_execution(&uncertain).unwrap();
    host.store
        .quarantine_execution(&uncertain.operation_id().unwrap())
        .unwrap();
    let before = host
        .snapshot(&request.binding.mission_id)
        .unwrap()
        .execution_effects;
    drop(host);
    let db = dir.path().join("mission.sqlite");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS linux_effect_identity_unique;
         DROP TRIGGER IF EXISTS simulated_effect_identity_unique;
         DROP TRIGGER IF EXISTS linux_resolution_command_unique;
         DROP TRIGGER IF EXISTS simulated_resolution_command_unique;
         DROP TABLE IF EXISTS linux_execution_resolutions;
         DROP TABLE IF EXISTS linux_execution_effects;
         BEGIN;
         ALTER TABLE execution_effects RENAME TO saved_effects;
         DROP INDEX execution_effects_mission;",
    )
    .unwrap();
    conn.execute_batch(include_str!("../../migrations/004_execution_effects.sql"))
        .unwrap();
    conn.execute_batch(
        "INSERT INTO execution_effects SELECT * FROM saved_effects; DROP TABLE saved_effects;
        DROP TABLE execution_resolutions; DELETE FROM schema_migrations WHERE version>=5; PRAGMA user_version=4; COMMIT;",
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        Host::inspect(&db, Some(&request.binding.mission_id)).unwrap()[0].execution_effects,
        before
    );
    let host = Host::open(&db).unwrap();
    assert_eq!(
        host.snapshot(&request.binding.mission_id)
            .unwrap()
            .execution_effects,
        before
    );
    assert_eq!(
        host.store
            .execution_effect(&request.operation_id().unwrap())
            .unwrap()
            .unwrap()
            .receipt
            .unwrap(),
        receipt
    );
    let conn = Connection::open(&db).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn guard_failure_keeps_category_when_uncertainty_persistence_fails() {
    let (dir, mut host, request) = setup();
    host.store.prepare_execution(&request).unwrap();
    Connection::open(dir.path().join("mission.sqlite"))
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_updates BEFORE UPDATE ON execution_effects
             BEGIN SELECT RAISE(ABORT, 'injected ledger unavailable'); END;",
        )
        .unwrap();
    let error = host.execution_failure(&request, Error::guard("receipt/intent mismatch"));
    assert_eq!(error.category(), "guard_failed");
    assert!(!error.is_transient());
    assert!(error.to_string().contains("receipt/intent mismatch"));
    assert!(error.to_string().contains("injected ledger unavailable"));
    assert_eq!(
        state(
            &dir.path().join("mission.sqlite"),
            &request.operation_id().unwrap()
        ),
        "prepared"
    );
    assert_eq!(
        host.snapshot(&request.binding.mission_id).unwrap().phase,
        Phase::Implementing
    );
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
}

fn resolution_request(
    host: &Host,
    request: &ExecutionRequest,
    decision: crate::ExecutionResolutionDecision,
) -> crate::ExecutionResolutionRequest {
    crate::ExecutionResolutionRequest {
        command_id: "resolve-1".into(),
        binding: RuntimeBinding {
            generation: host.generation(),
            ..request.binding.clone()
        },
        operation_id: request.operation_id().unwrap(),
        payload_hash: request.payload_hash().unwrap(),
        expected_revision: host.snapshot(&request.binding.mission_id).unwrap().revision,
        actor: "operator".into(),
        rationale: "Inspect interrupted simulated execution".into(),
        decision,
    }
}

fn interrupt_effect(host: &mut Host, request: &ExecutionRequest, checkpoint: &str) {
    assert!(
        host.execute_fake_at(request, |at| {
            if at == checkpoint {
                Err(Error::storage("lost response"))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    host.reconcile_execution(&request.binding.mission_id)
        .unwrap();
}

#[test]
fn resolution_proofs_settle_once_and_replay_after_restart_without_execution() {
    use crate::ExecutionResolutionDecision::{Completed, NotApplied};
    for (at, decision) in [("prepared", NotApplied), ("effect", Completed)] {
        let (dir, mut host, mut request) = setup();
        interrupt_effect(&mut host, &request, at);
        let mut resolve = resolution_request(&host, &request, decision);
        let audit = host.resolve_fake_execution(&resolve).unwrap();
        assert_eq!(host.resolve_fake_execution(&resolve).unwrap(), audit);
        let snapshot = host.snapshot(&request.binding.mission_id).unwrap();
        assert_eq!(snapshot.phase, Phase::NeedsAttention);
        assert_eq!(snapshot.revision, resolve.expected_revision + 1);
        assert_eq!(snapshot.execution_resolutions, vec![audit.clone()]);
        assert!(
            !host
                .store
                .execution_blocked(&request.binding.mission_id)
                .unwrap()
        );
        let replay = host.execute_fake_operation(&request);
        if decision == Completed {
            assert_eq!(replay.unwrap().to_json(), audit["evidence"]["receipt"]);
        } else {
            assert_eq!(replay.unwrap_err().category(), "execution_not_applied");
        }
        let mut changed = resolve.clone();
        changed.rationale = "changed decision".into();
        assert_eq!(
            host.resolve_fake_execution(&changed)
                .unwrap_err()
                .category(),
            "command_conflict"
        );
        drop(host);
        let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
        assert_eq!(
            host.resolve_fake_execution(&resolve)
                .unwrap_err()
                .category(),
            "stale_generation"
        );
        resolve.binding.generation = host.generation();
        request.binding.generation = host.generation();
        assert_eq!(host.resolve_fake_execution(&resolve).unwrap(), audit);
        assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
        command(
            &mut host,
            &request.binding.mission_id,
            CommandBody::ExplicitRecovery {
                expected_target: Phase::Implementing,
                ownership_proven: true,
                effects: crate::EffectProof::Reconciled,
            },
        );
        assert_eq!(
            host.snapshot(&request.binding.mission_id).unwrap().phase,
            Phase::Implementing
        );
        let replay = host.execute_fake_operation(&request);
        assert_eq!(replay.is_ok(), decision == Completed);
        assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
    }
}

#[test]
fn resolution_rejects_missing_mismatched_or_stale_evidence_and_abandonment_stays_blocked() {
    use crate::ExecutionResolutionDecision::{Abandon, Completed, NotApplied};
    let (dir, mut host, mut request) = setup();
    interrupt_effect(&mut host, &request, "effect");
    let mut resolve = resolution_request(&host, &request, NotApplied);
    assert!(host.resolve_fake_execution(&resolve).is_err());
    resolve.decision = Completed;
    let mut bad = resolve.clone();
    bad.expected_revision -= 1;
    assert_eq!(
        host.resolve_fake_execution(&bad).unwrap_err().category(),
        "stale_revision"
    );
    let mut bad = resolve.clone();
    bad.binding.session_id = "wrong-session".into();
    assert_eq!(
        host.resolve_fake_execution(&bad).unwrap_err().category(),
        "binding_mismatch"
    );
    let mut bad = resolve.clone();
    bad.payload_hash = "wrong-payload".into();
    assert_eq!(
        host.resolve_fake_execution(&bad).unwrap_err().category(),
        "operation_conflict"
    );
    let mut bad = resolve.clone();
    bad.rationale.clear();
    assert!(host.resolve_fake_execution(&bad).is_err());
    assert!(
        host.snapshot(&request.binding.mission_id)
            .unwrap()
            .execution_resolutions
            .is_empty()
    );
    drop(host);
    let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
    resolve.binding.generation = host.generation();
    request.binding.generation = host.generation();
    assert!(host.resolve_fake_execution(&resolve).is_err());
    resolve.decision = NotApplied;
    assert!(host.resolve_fake_execution(&resolve).is_err());
    resolve.decision = Abandon;
    let audit = host.resolve_fake_execution(&resolve).unwrap();
    assert_eq!(audit["evidence"]["blockRetained"], true);
    assert!(host.store.ops_blocked(&request.binding.mission_id).unwrap());
    let mut fresh = request.clone();
    fresh.effect_key = "new-id".into();
    assert_eq!(
        host.execute_fake_operation(&fresh).unwrap_err().category(),
        "execution_blocked"
    );
    let revision = host.snapshot(&request.binding.mission_id).unwrap().revision;
    assert!(
        host.admit(AdmitRequest {
            command_id: None,
            mission_id: request.binding.mission_id.clone(),
            attempt_id: None,
            expected_revision: revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::ExplicitRecovery {
                expected_target: Phase::Implementing,
                ownership_proven: true,
                effects: crate::EffectProof::Reconciled
            },
        })
        .is_err()
    );
    let mut another = resolve.clone();
    another.command_id = "second-decision".into();
    another.expected_revision = revision;
    assert!(host.resolve_fake_execution(&another).is_err());
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
    drop(host);
    let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
    resolve.binding.generation = host.generation();
    assert_eq!(host.resolve_fake_execution(&resolve).unwrap(), audit);
    assert!(host.store.ops_blocked(&request.binding.mission_id).unwrap());
}

#[test]
fn resolution_audit_failure_rolls_back_effect_and_mission_revision() {
    let (dir, mut host, request) = setup();
    interrupt_effect(&mut host, &request, "effect");
    let resolve = resolution_request(
        &host,
        &request,
        crate::ExecutionResolutionDecision::Completed,
    );
    let before = host.snapshot(&request.binding.mission_id).unwrap();
    let conn = Connection::open(dir.path().join("mission.sqlite")).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fail_resolution BEFORE INSERT ON execution_resolutions
        BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END;",
    )
    .unwrap();
    assert!(
        host.resolve_fake_execution(&resolve)
            .unwrap_err()
            .to_string()
            .contains("injected audit failure")
    );
    let after = host.snapshot(&request.binding.mission_id).unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.execution_effects, before.execution_effects);
    assert!(after.execution_resolutions.is_empty());
    conn.execute_batch("DROP TRIGGER fail_resolution;").unwrap();
    host.resolve_fake_execution(&resolve).unwrap();
}

#[test]
fn resolved_effect_does_not_clear_other_operations_or_allow_terminal_mutation() {
    let (dir, mut host, request) = setup();
    interrupt_effect(&mut host, &request, "effect");
    host.record_operation(
        &request.binding.mission_id,
        "other-op",
        &serde_json::json!({}),
        crate::OperationState::Prepared,
    )
    .unwrap();
    let resolve = resolution_request(
        &host,
        &request,
        crate::ExecutionResolutionDecision::Completed,
    );
    let audit = host.resolve_fake_execution(&resolve).unwrap();
    assert!(host.store.ops_blocked(&request.binding.mission_id).unwrap());
    let revision = host.snapshot(&request.binding.mission_id).unwrap().revision;
    assert!(
        host.admit(AdmitRequest {
            command_id: None,
            mission_id: request.binding.mission_id.clone(),
            attempt_id: None,
            expected_revision: revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::ExplicitRecovery {
                expected_target: Phase::Implementing,
                ownership_proven: true,
                effects: crate::EffectProof::Reconciled
            },
        })
        .is_err()
    );
    // Terminal policy is independently enforced even for a matching old effect.
    Connection::open(dir.path().join("mission.sqlite"))
        .unwrap()
        .execute(
            "UPDATE missions SET phase='cancelled' WHERE id=?1",
            [&request.binding.mission_id],
        )
        .unwrap();
    assert_eq!(host.resolve_fake_execution(&resolve).unwrap(), audit);
    let mut fresh = resolve.clone();
    fresh.command_id = "new-decision".into();
    fresh.expected_revision = revision;
    assert_eq!(
        host.resolve_fake_execution(&fresh).unwrap_err().category(),
        "terminal_immutable"
    );
    assert_eq!(
        host.snapshot(&request.binding.mission_id).unwrap().phase,
        Phase::Cancelled
    );
}

#[test]
fn v5_read_only_inspection_and_upgrade_preserve_unresolved_effects() {
    let (dir, mut host, request) = setup();
    interrupt_effect(&mut host, &request, "prepared");
    let before = host
        .snapshot(&request.binding.mission_id)
        .unwrap()
        .execution_effects;
    drop(host);
    let db = dir.path().join("mission.sqlite");
    Connection::open(&db).unwrap().execute_batch(
        "DROP TABLE execution_resolutions; DELETE FROM schema_migrations WHERE version=6; PRAGMA user_version=5;",
    ).unwrap();
    let inspected = Host::inspect(&db, Some(&request.binding.mission_id)).unwrap();
    assert_eq!(inspected[0].execution_effects, before);
    assert!(inspected[0].execution_resolutions.is_empty());
    let host = Host::open(&db).unwrap();
    assert_eq!(
        host.snapshot(&request.binding.mission_id)
            .unwrap()
            .execution_effects,
        before
    );
    assert!(
        host.store
            .execution_blocked(&request.binding.mission_id)
            .unwrap()
    );
}

#[test]
fn apply_consumes_stale_absence_proof_even_when_backend_returns_an_error() {
    let (_dir, mut host, request) = setup();
    // Emulate a future retry path reaching apply with an old absence marker.
    host.execution_evidence
        .prepare(&mut host.store, &request)
        .unwrap();
    let id = request.operation_id().unwrap();
    assert!(matches!(host.execution_evidence.get(&id), Some(None)));
    host.runtime_mut().unwrap().fail_next_simulated_effect();
    let error = host
        .execution_evidence
        .apply(&mut host.runtime, &request)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected error after simulated effect")
    );
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 1);
    assert!(host.execution_evidence.get(&id).is_none());
    host.reconcile_execution(&request.binding.mission_id)
        .unwrap();
    let resolve = resolution_request(
        &host,
        &request,
        crate::ExecutionResolutionDecision::NotApplied,
    );
    assert!(
        host.resolve_fake_execution(&resolve)
            .unwrap_err()
            .to_string()
            .contains("evidence is unavailable")
    );
    assert!(
        host.store
            .execution_blocked(&request.binding.mission_id)
            .unwrap()
    );
    assert!(
        host.snapshot(&request.binding.mission_id)
            .unwrap()
            .execution_resolutions
            .is_empty()
    );
}

#[test]
fn abandonment_rejects_known_outcomes_without_discarding_their_proof() {
    use crate::ExecutionResolutionDecision::{Abandon, Completed, NotApplied};
    for (at, outcome) in [("prepared", NotApplied), ("effect", Completed)] {
        let (_dir, mut host, request) = setup();
        interrupt_effect(&mut host, &request, at);
        let mut resolve = resolution_request(&host, &request, Abandon);
        let evidence = host.execution_evidence.get(&resolve.operation_id).cloned();
        assert!(evidence.is_some());
        let before = host.snapshot(&request.binding.mission_id).unwrap();
        assert!(
            host.resolve_fake_execution(&resolve)
                .unwrap_err()
                .to_string()
                .contains("resolve the proven outcome")
        );
        assert_eq!(
            host.execution_evidence.get(&resolve.operation_id).cloned(),
            evidence
        );
        let after = host.snapshot(&request.binding.mission_id).unwrap();
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.execution_effects, before.execution_effects);
        assert!(after.execution_resolutions.is_empty());
        resolve.decision = outcome;
        let audit = host.resolve_fake_execution(&resolve).unwrap();
        assert_eq!(audit["request"]["decision"], outcome.as_str());
        assert!(
            !host
                .store
                .execution_blocked(&request.binding.mission_id)
                .unwrap()
        );
    }
}

#[test]
fn resolution_status_uses_commit_order_not_operator_command_ids() {
    let (dir, mut host, first) = setup();
    let mut second = first.clone();
    second.effect_key = "second-effect".into();
    host.execution_evidence
        .prepare(&mut host.store, &first)
        .unwrap();
    host.execution_evidence
        .prepare(&mut host.store, &second)
        .unwrap();
    host.reconcile_execution(&first.binding.mission_id).unwrap();
    let mut audits = Vec::new();
    for (request, id) in [(&first, "z-first"), (&second, "a-second")] {
        let mut resolve = resolution_request(
            &host,
            request,
            crate::ExecutionResolutionDecision::NotApplied,
        );
        resolve.command_id = id.into();
        audits.push(host.resolve_fake_execution(&resolve).unwrap());
    }
    assert_eq!(
        host.snapshot(&first.binding.mission_id)
            .unwrap()
            .execution_resolutions,
        audits
    );
    drop(host);
    let inspected = Host::inspect(
        &dir.path().join("mission.sqlite"),
        Some(&first.binding.mission_id),
    )
    .unwrap();
    assert_eq!(inspected[0].execution_resolutions, audits);
}

#[test]
fn delivered_stops_preserve_execution_uncertainty_and_completion_guards() {
    for abandon in [false, true] {
        let (_dir, mut host, request) = setup();
        let mission = &request.binding.mission_id;
        interrupt_effect(&mut host, &request, "prepared");
        if abandon {
            host.execution_evidence
                .remove(&request.operation_id().unwrap());
            let resolution =
                resolution_request(&host, &request, crate::ExecutionResolutionDecision::Abandon);
            host.resolve_fake_execution(&resolution).unwrap();
        }
        command(&mut host, mission, CommandBody::Cancel);
        let before = host.snapshot(mission).unwrap().to_json();
        assert_eq!(host.dispatch_stops(mission).unwrap(), 1);
        let snapshot = host.snapshot(mission).unwrap();
        assert_eq!(snapshot.phase, Phase::Cancelling);
        assert!(snapshot.execution_blocked);
        let after = snapshot.to_json();
        assert_eq!(before["executionEffects"], after["executionEffects"]);
        assert_eq!(
            before["executionResolutions"],
            after["executionResolutions"]
        );
        assert_eq!(before["workspace"], after["workspace"]);
        assert!(
            after["nextSteps"]
                .to_string()
                .contains("Unresolved execution effects")
        );
        assert!(
            after["nextSteps"]
                .to_string()
                .contains("detect_uncertainty")
        );
        let result = host.admit(AdmitRequest {
            command_id: None,
            mission_id: mission.clone(),
            attempt_id: None,
            expected_revision: snapshot.revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::ConfirmCancelled,
        });
        assert!(result.unwrap_err().to_string().contains("not quiescent"));
        if !abandon {
            // #125 still requires an explicit attention/recovery cycle.
            command(
                &mut host,
                mission,
                CommandBody::DetectUncertainty {
                    reason: AttentionReason::ExecutionUncertain,
                    content_digest: None,
                },
            );
            let attention = host.snapshot(mission).unwrap();
            assert_eq!(attention.phase, Phase::NeedsAttention);
            assert_eq!(attention.prior_phase, Some(Phase::Cancelling));
            assert_eq!(attention.commands.last().unwrap().origin, "operator");
            let resolution = resolution_request(
                &host,
                &request,
                crate::ExecutionResolutionDecision::NotApplied,
            );
            host.resolve_fake_execution(&resolution).unwrap();
            command(
                &mut host,
                mission,
                CommandBody::ExplicitRecovery {
                    expected_target: Phase::Cancelling,
                    ownership_proven: true,
                    effects: crate::domain::EffectProof::Reconciled,
                },
            );
            command(&mut host, mission, CommandBody::ConfirmCancelled);
            assert_eq!(host.snapshot(mission).unwrap().phase, Phase::Cancelled);
        }
    }
}

#[test]
fn pause_abort_is_delivered_even_when_reconciliation_discovers_a_prepared_effect() {
    let (_dir, mut host, request) = setup();
    let mission = &request.binding.mission_id;
    assert!(
        host.execute_fake_at(&request, |at| {
            if at == "prepared" {
                Err(Error::storage("crash before apply"))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    command(&mut host, mission, CommandBody::Pause);
    assert_eq!(host.dispatch_stops(mission).unwrap(), 1);
    let snapshot = host.snapshot(mission).unwrap();
    assert_eq!(snapshot.phase, Phase::Pausing);
    assert_eq!(snapshot.execution_effects[0].state, "uncertain");
    assert!(
        host.admit(AdmitRequest {
            command_id: None,
            mission_id: mission.clone(),
            attempt_id: None,
            expected_revision: snapshot.revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::ConfirmStopped,
        })
        .unwrap_err()
        .to_string()
        .contains("not quiescent")
    );

    command(
        &mut host,
        mission,
        CommandBody::DetectUncertainty {
            reason: AttentionReason::ExecutionUncertain,
            content_digest: None,
        },
    );
    let attention = host.snapshot(mission).unwrap();
    assert_eq!(attention.phase, Phase::NeedsAttention);
    assert_eq!(attention.prior_phase, Some(Phase::Pausing));
    assert_eq!(attention.commands.last().unwrap().origin, "operator");
    let resolution = resolution_request(
        &host,
        &request,
        crate::ExecutionResolutionDecision::NotApplied,
    );
    host.resolve_fake_execution(&resolution).unwrap();
    command(
        &mut host,
        mission,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Paused,
            ownership_proven: true,
            effects: crate::domain::EffectProof::Reconciled,
        },
    );
    assert_eq!(host.snapshot(mission).unwrap().phase, Phase::Paused);
}

#[test]
fn simulated_and_linux_journals_cannot_share_operation_or_resolution_identity() {
    let (dir, mut host, request) = setup();
    let id = request.operation_id().unwrap();
    let db = dir.path().join("mission.sqlite");
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, created_at
             ) VALUES (?1,?2,?3,?4,'sha256:linux','{}',1,'prepared',1)",
            rusqlite::params![
                id,
                request.binding.mission_id,
                request.binding.attempt_id,
                request.binding.session_id
            ],
        )
        .unwrap();
    assert_eq!(
        host.store
            .prepare_execution(&request)
            .unwrap_err()
            .category(),
        "operation_conflict"
    );
    assert_eq!(
        host.execute_fake_operation(&request)
            .unwrap_err()
            .category(),
        "execution_blocked"
    );
    let err = Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, created_at
             ) VALUES (?1,?2,?3,?4,'sha256:sim','{}',1,'prepared',1)",
            rusqlite::params![
                id,
                request.binding.mission_id,
                request.binding.attempt_id,
                request.binding.session_id
            ],
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("operation identity already used"),
        "{err}"
    );
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_resolutions(
                command_id, operation_id, mission_id, request_hash, audit_json
             ) VALUES ('shared-resolution', ?1, ?2, 'sha256:r', '{}')",
            rusqlite::params![id, request.binding.mission_id],
        )
        .unwrap();
    host.store.prepare_execution(&request).ok();
    let mut resolve =
        resolution_request(&host, &request, crate::ExecutionResolutionDecision::Abandon);
    resolve.command_id = "shared-resolution".into();
    assert_eq!(
        host.resolve_fake_execution(&resolve)
            .unwrap_err()
            .category(),
        "command_conflict"
    );
    Connection::open(&db)
        .unwrap()
        .execute_batch(
            "DELETE FROM linux_execution_resolutions; DELETE FROM linux_execution_effects;",
        )
        .unwrap();
    host.execute_fake_operation(&request).unwrap();
    let reverse = Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, created_at
             ) VALUES (?1,?2,?3,?4,'sha256:linux','{}',1,'prepared',1)",
            rusqlite::params![
                id,
                request.binding.mission_id,
                request.binding.attempt_id,
                request.binding.session_id
            ],
        )
        .unwrap_err();
    assert!(
        reverse
            .to_string()
            .contains("operation identity already used"),
        "{reverse}"
    );
    let mut other = request.clone();
    other.effect_key = "other-write".into();
    interrupt_effect(&mut host, &other, "prepared");
    let mut simulated = resolution_request(
        &host,
        &other,
        crate::ExecutionResolutionDecision::NotApplied,
    );
    simulated.command_id = "sim-only-resolution".into();
    host.resolve_fake_execution(&simulated).unwrap();
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, created_at
             ) VALUES ('op-v1:other',?1,?2,?3,'sha256:other','{}',1,'uncertain',1)",
            rusqlite::params![
                request.binding.mission_id,
                request.binding.attempt_id,
                request.binding.session_id
            ],
        )
        .unwrap();
    let command = Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_resolutions(
                command_id, operation_id, mission_id, request_hash, audit_json
             ) VALUES ('sim-only-resolution', 'op-v1:other', ?1, 'sha256:other', '{}')",
            rusqlite::params![request.binding.mission_id],
        )
        .unwrap_err();
    assert!(
        command
            .to_string()
            .contains("resolution command identity already used"),
        "{command}"
    );
}

#[test]
fn default_host_surfaces_linux_rows_without_settling_them() {
    let (dir, host, request) = setup();
    let mission = request.binding.mission_id.clone();
    let db = dir.path().join("mission.sqlite");
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, cgroup_path, cgroup_dev, cgroup_ino, boot_id,
                entered_at, created_at
             ) VALUES (
                'op-v1:linuxrow', ?1, ?2, ?3, 'sha256:row',
                '{\"scope\":\"linux.file.write.v1\",\"operation\":{\"method\":\"write_file\"}}',
                1, 'entered', '/sys/fs/cgroup/switchyard/sy-row', 1, 2, 'boot', 10, 10
             )",
            rusqlite::params![
                mission,
                request.binding.attempt_id,
                request.binding.session_id
            ],
        )
        .unwrap();
    drop(host);
    let mut host = Host::open(&db).unwrap();
    let snapshot = host.snapshot(&mission).unwrap();
    assert!(snapshot.execution_blocked);
    assert_eq!(snapshot.linux_execution_effects[0].state, "entered");
    assert!(snapshot.linux_execution_effects[0].entered);
    let steps = snapshot.to_json()["nextSteps"].to_string();
    assert!(steps.contains("Linux execution evidence"), "{steps}");
    assert!(steps.contains("populated 0"), "{steps}");
    assert!(steps.contains("not_applied"), "{steps}");
    host.reconcile_execution(&mission).unwrap();
    let after = host.snapshot(&mission).unwrap();
    assert_eq!(after.phase, Phase::NeedsAttention);
    assert_eq!(after.linux_execution_effects[0].state, "entered");
    assert!(after.execution_blocked);
}

#[test]
fn v6_upgrade_preserves_effects_and_resolutions() {
    let (dir, mut host, request) = setup();
    interrupt_effect(&mut host, &request, "prepared");
    let resolve = resolution_request(
        &host,
        &request,
        crate::ExecutionResolutionDecision::NotApplied,
    );
    let audit = host.resolve_fake_execution(&resolve).unwrap();
    let before = host.snapshot(&request.binding.mission_id).unwrap();
    drop(host);
    let db = dir.path().join("mission.sqlite");
    Connection::open(&db)
        .unwrap()
        .execute_batch(
            "DROP TRIGGER IF EXISTS simulated_effect_identity_unique;
             DROP TRIGGER IF EXISTS linux_effect_identity_unique;
             DROP TRIGGER IF EXISTS simulated_resolution_command_unique;
             DROP TRIGGER IF EXISTS linux_resolution_command_unique;
             DROP TABLE IF EXISTS linux_execution_resolutions;
             DROP TABLE IF EXISTS linux_execution_effects;
             DELETE FROM schema_migrations WHERE version>=7;
             PRAGMA user_version=6;",
        )
        .unwrap();
    let inspected = Host::inspect(&db, Some(&request.binding.mission_id)).unwrap();
    assert_eq!(inspected[0].execution_effects, before.execution_effects);
    assert_eq!(inspected[0].execution_resolutions, vec![audit.clone()]);
    assert!(inspected[0].linux_execution_effects.is_empty());
    let host = Host::open(&db).unwrap();
    let after = host.snapshot(&request.binding.mission_id).unwrap();
    assert_eq!(after.execution_effects, before.execution_effects);
    assert_eq!(after.execution_resolutions, vec![audit]);
    assert!(after.linux_execution_effects.is_empty());
    let version: i64 = Connection::open(&db)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 7);
}
