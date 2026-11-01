use super::*;
use crate::{ExecutionOperation, FAKE_WRITE_SCOPE};
use rusqlite::Connection;
use std::path::Path;
use std::process::Command;

#[cfg(feature = "pi-contract")]
mod pi_effects;

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
        "DROP TRIGGER IF EXISTS supersession_command_unique;
         DROP TRIGGER IF EXISTS execution_supersessions_no_update;
         DROP TRIGGER IF EXISTS execution_supersessions_no_delete;
         DROP TRIGGER IF EXISTS execution_resolutions_no_update;
         DROP TRIGGER IF EXISTS execution_resolutions_no_delete;
         DROP TRIGGER IF EXISTS linux_resolutions_no_update;
         DROP TRIGGER IF EXISTS linux_resolutions_no_delete;
         DROP TABLE IF EXISTS execution_supersessions;
         DROP TRIGGER IF EXISTS linux_effect_identity_unique;
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
        9
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
    #[cfg(feature = "pi-contract")]
    assert_eq!(
        host.execute_fake_operation_outcome(&request),
        crate::PiEffectOutcome::Rejected
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
        "DROP TRIGGER IF EXISTS supersession_command_unique;
         DROP TRIGGER IF EXISTS execution_supersessions_no_update;
         DROP TRIGGER IF EXISTS execution_supersessions_no_delete;
         DROP TRIGGER IF EXISTS execution_resolutions_no_update;
         DROP TRIGGER IF EXISTS execution_resolutions_no_delete;
         DROP TRIGGER IF EXISTS linux_resolutions_no_update;
         DROP TRIGGER IF EXISTS linux_resolutions_no_delete;
         DROP TABLE IF EXISTS execution_supersessions;
         DROP TRIGGER IF EXISTS linux_effect_identity_unique;
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
        9
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
        #[cfg(feature = "pi-contract")]
        assert_eq!(
            host.execute_fake_operation_outcome(&request),
            if decision == Completed {
                crate::PiEffectOutcome::Completed(audit["evidence"]["receipt"].clone())
            } else {
                crate::PiEffectOutcome::Rejected
            }
        );
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
        "DROP TRIGGER IF EXISTS supersession_command_unique;
         DROP TRIGGER IF EXISTS execution_supersessions_no_update;
         DROP TRIGGER IF EXISTS execution_supersessions_no_delete;
         DROP TRIGGER IF EXISTS execution_resolutions_no_update;
         DROP TRIGGER IF EXISTS execution_resolutions_no_delete;
         DROP TABLE IF EXISTS execution_supersessions;
         DROP TABLE execution_resolutions; DELETE FROM schema_migrations WHERE version=6; PRAGMA user_version=5;",
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
            "DROP TRIGGER IF EXISTS linux_resolutions_no_delete;
             DELETE FROM linux_execution_resolutions;
             DELETE FROM linux_execution_effects;",
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
            "DROP TRIGGER IF EXISTS supersession_command_unique;
             DROP TRIGGER IF EXISTS execution_supersessions_no_update;
             DROP TRIGGER IF EXISTS execution_supersessions_no_delete;
             DROP TRIGGER IF EXISTS execution_resolutions_no_update;
             DROP TRIGGER IF EXISTS execution_resolutions_no_delete;
             DROP TRIGGER IF EXISTS linux_resolutions_no_update;
             DROP TRIGGER IF EXISTS linux_resolutions_no_delete;
             DROP TABLE IF EXISTS execution_supersessions;
             DROP TRIGGER IF EXISTS simulated_effect_identity_unique;
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
    assert_eq!(version, 9);
}

#[test]
fn released_linux_intent_cannot_be_settled_as_unreleased_after_restart() {
    let (dir, mut host, fake) = setup();
    let mut request = crate::LinuxExecutionRequest {
        binding: fake.binding,
        tool_task_id: "release-test".into(),
        effect_key: "write".into(),
        capability_scope: crate::LINUX_WRITE_SCOPE.into(),
        deadline_ms: fake.deadline_ms,
        operation: crate::LinuxOperation::WriteFile {
            path: "out.txt".into(),
            content: "once".into(),
        },
    };
    host.store.prepare_linux(&request, None).unwrap();
    host.store
        .mark_linux_released(&request, 123, "start")
        .unwrap();
    let id = request.operation_id().unwrap();
    let released = host.store.linux_effect(&id).unwrap().unwrap().released_at;
    assert!(released.is_some());
    assert!(
        host.store
            .mark_linux_released(&request, 123, "start")
            .is_err()
    );
    drop(host);
    let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
    request.binding.generation = host.generation();
    let proof = crate::persistence::linux_execution::quiescence_document(
        true,
        "populated 0",
        Some(0),
        "",
        None,
    );
    assert!(host.store.linux_not_applied(&request, &proof).is_err());
    let row = host.store.linux_effect(&id).unwrap().unwrap();
    assert_eq!(row.released_at, released);
    assert!(row.entered_at.is_none());
    assert!(row.receipt_json.is_none());
    assert!(
        host.snapshot(&request.binding.mission_id)
            .unwrap()
            .execution_blocked
    );
}

#[test]
fn v7_upgrade_preserves_receipts_and_treats_unresolved_release_as_unknown() {
    let (dir, mut host, fake) = setup();
    let receipt = host.execute_fake_operation(&fake).unwrap();
    let mission = fake.binding.mission_id.clone();
    let db = dir.path().join("mission.sqlite");
    drop(host);
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "DROP TRIGGER simulated_effect_identity_unique;
         DROP TRIGGER simulated_resolution_command_unique;
         DROP TABLE linux_execution_resolutions;
         DROP TABLE linux_execution_effects;
         DELETE FROM schema_migrations WHERE version>=7;
         PRAGMA user_version=7;",
    )
    .unwrap();
    // Exercise the earlier v7 shape that lacked the entered CHECK as well.
    let v7 = include_str!("../../migrations/007_linux_execution.sql").replace(
        "    CHECK (state != 'entered' OR entered_at IS NOT NULL),\n",
        "",
    );
    conn.execute_batch(&v7).unwrap();
    conn.execute(
        "INSERT INTO schema_migrations(version, applied_at) VALUES (7, 1)",
        [],
    )
    .unwrap();
    for (id, state, receipt, quiescence) in [
        ("linux-unknown", "prepared", None, None),
        (
            "linux-completed",
            "completed",
            Some("{\"original\":true}"),
            Some("{\"proven\":true}"),
        ),
    ] {
        conn.execute(
            "INSERT INTO linux_execution_effects(operation_id,mission_id,attempt_id,session_id,
             payload_hash,request_json,started_generation,state,cgroup_path,cgroup_dev,cgroup_ino,
             boot_id,receipt_json,quiescence_json,created_at)
             VALUES (?1,?2,?3,?4,'hash','{}',1,?5,'/old/leaf',1,2,'boot',?6,?7,10)",
            rusqlite::params![
                id,
                mission,
                fake.binding.attempt_id,
                fake.binding.session_id,
                state,
                receipt,
                quiescence
            ],
        )
        .unwrap();
    }
    let inspected = Host::inspect(&db, Some(&mission)).unwrap();
    assert!(
        inspected[0]
            .linux_execution_effects
            .iter()
            .all(|row| row.released.is_none())
    );
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
    let mut host = Host::open(&db).unwrap();
    let unknown = host.store.linux_effect("linux-unknown").unwrap().unwrap();
    assert!(unknown.released_at.is_some());
    assert!(unknown.entered_at.is_none());
    let completed = host.store.linux_effect("linux-completed").unwrap().unwrap();
    assert_eq!(completed.state, "completed");
    assert_eq!(
        completed.receipt_json.as_deref(),
        Some("{\"original\":true}")
    );
    assert!(
        conn.execute(
            "UPDATE linux_execution_effects SET state='entered' WHERE operation_id='linux-unknown'",
            []
        )
        .is_err()
    );
    let mut fake = fake;
    fake.binding.generation = host.generation();
    assert_eq!(host.execute_fake_operation(&fake).unwrap(), receipt);
    assert!(host.snapshot(&mission).unwrap().execution_blocked);
    drop(host);
    let host = Host::open(&db).unwrap();
    assert_eq!(
        host.store
            .linux_effect("linux-unknown")
            .unwrap()
            .unwrap()
            .released_at,
        unknown.released_at
    );
}

fn effect_blob(db: &Path, id: &str) -> String {
    Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT operation_id||'|'||mission_id||'|'||attempt_id||'|'||session_id||'|'||payload_hash||'|'||request_json||'|'||started_generation||'|'||state||'|'||ifnull(receipt_json,'')||'|'||created_at FROM execution_effects WHERE operation_id=?1",
            [id],
            |row| row.get(0),
        )
        .unwrap()
}

fn resolution_blob(db: &Path, command_id: &str) -> String {
    Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT command_id||'|'||operation_id||'|'||mission_id||'|'||request_hash||'|'||audit_json FROM execution_resolutions WHERE command_id=?1",
            [command_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn count_rows(db: &Path, table: &str) -> i64 {
    Connection::open(db)
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn staged(path: &Path) -> Vec<u8> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["ls-files", "--stage"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn head_sha(path: &Path) -> Vec<u8> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn worktree_admin(tree: &Path) -> std::path::PathBuf {
    let dot = std::fs::read_to_string(tree.join(".git")).unwrap();
    std::path::PathBuf::from(dot.trim().strip_prefix("gitdir: ").unwrap())
}

/// `fake_effect_outcome` maps a non-transient error with no simulated row to
/// Rejected. That is sound only because the simulator commits intent before
/// apply. Storage, git, and a busy workspace must stay transient; otherwise an
/// error returned before a simulated row is visible would become a definite
/// rejection. The match is exhaustive so a new error cannot skip this decision.
/// `execute_fake_operation` returns the storage, git, workspace, guard, binding,
/// deadline, conflict, and settlement variants below, directly or wrapped in
/// `WithContext`.
fn simulated_no_row_error_is_transient(error: &Error) -> bool {
    match error {
        Error::WithContext { source, .. } => simulated_no_row_error_is_transient(source),
        Error::Storage { .. } | Error::GitFailed { .. } | Error::WorkspaceBusy { .. } => true,
        Error::IllegalTransition { .. }
        | Error::TerminalImmutable { .. }
        | Error::StaleRevision { .. }
        | Error::BindingMismatch { .. }
        | Error::SecondActiveOwner
        | Error::WorkspaceUncertain { .. }
        | Error::CommandConflict { .. }
        | Error::ExecutionDeadlineExceeded { .. }
        | Error::ExecutionBlocked { .. }
        | Error::ExecutionNotApplied { .. }
        | Error::ExecutionUncertain { .. }
        | Error::OperationConflict { .. }
        | Error::StaleGeneration { .. }
        | Error::RepairLimitExhausted { .. }
        | Error::UsageLimitExhausted { .. }
        | Error::AdmissionRejected { .. }
        | Error::GuardFailed { .. }
        | Error::UnknownNotification { .. }
        | Error::NotFound
        | Error::ActiveMissionExists
        | Error::Canonical { .. } => false,
    }
}

#[test]
fn execute_fake_operation_pins_is_transient_for_every_error_it_can_return() {
    let errors = [
        Error::storage("disk"),
        Error::git("inspect", None),
        Error::git("status", Some(1)),
        Error::WorkspaceBusy {
            workspace_id: "ws".into(),
        },
        Error::storage("disk").with_context("could not record execution uncertainty"),
        Error::guard("simulated execution requires FakeRuntime"),
        Error::guard("invalid or oversized simulated write")
            .with_context("could not record execution uncertainty"),
        Error::BindingMismatch {
            detail: "effect binding mismatch".into(),
        },
        Error::StaleGeneration {
            expected: 2,
            actual: 1,
        },
        Error::ExecutionDeadlineExceeded {
            operation_id: "op".into(),
        },
        Error::ExecutionBlocked {
            mission_id: "m".into(),
            detail: "unresolved execution effects".into(),
        },
        Error::ExecutionNotApplied {
            operation_id: "op".into(),
        },
        Error::ExecutionUncertain {
            operation_id: "op".into(),
        },
        Error::OperationConflict {
            operation_id: "op".into(),
        },
        Error::NotFound,
        Error::Canonical {
            detail: "rejected".into(),
        },
        Error::WorkspaceUncertain {
            workspace_id: "ws".into(),
            detail: "verify".into(),
        },
        Error::SecondActiveOwner,
        Error::IllegalTransition {
            from: "implementing".into(),
            command: "detect_uncertainty".into(),
        },
        Error::TerminalImmutable {
            phase: "cancelled".into(),
        },
        Error::StaleRevision {
            expected: 1,
            actual: 2,
        },
        Error::CommandConflict {
            command_id: "c".into(),
        },
        Error::ActiveMissionExists,
        Error::RepairLimitExhausted { count: 1, max: 1 },
        Error::UsageLimitExhausted { count: 1, max: 1 },
        Error::AdmissionRejected {
            category: "busy".into(),
        },
        Error::UnknownNotification {
            kind: "nope".into(),
        },
    ];
    for error in &errors {
        assert_eq!(
            error.is_transient(),
            simulated_no_row_error_is_transient(error),
            "{error:?}"
        );
    }

    let (_dir, mut host, request) = setup();
    let mut stale = request.clone();
    stale.binding.generation = 0;
    let mut escaped = request.clone();
    escaped.operation = ExecutionOperation::WriteFile {
        path: "../escape".into(),
        content: "no".into(),
    };
    let mut other_attempt = request.clone();
    other_attempt.binding.attempt_id = "other-attempt".into();
    let mut expired = request.clone();
    expired.deadline_ms = 0;
    let returned = [
        host.execute_fake_operation(&stale).unwrap_err(),
        host.execute_fake_operation(&escaped).unwrap_err(),
        host.execute_fake_operation(&other_attempt).unwrap_err(),
        host.execute_fake_operation(&expired).unwrap_err(),
    ];
    for error in &returned {
        assert!(
            !error.is_transient(),
            "semantic refusal became transient: {error:?}"
        );
        assert_eq!(
            error.is_transient(),
            simulated_no_row_error_is_transient(error),
            "{error:?}"
        );
    }
    let completed = host.execute_fake_operation(&request).unwrap();
    let mut changed = request.clone();
    changed.operation = ExecutionOperation::WriteFile {
        path: "out.txt".into(),
        content: "different".into(),
    };
    let conflict = host.execute_fake_operation(&changed).unwrap_err();
    assert_eq!(conflict.category(), "operation_conflict");
    assert!(!conflict.is_transient());
    assert_eq!(host.execute_fake_operation(&request).unwrap(), completed);
}

#[cfg(feature = "pi-contract")]
#[test]
fn typed_effect_outcomes_use_ledger_evidence_and_preserve_conflicts() {
    use crate::PiEffectOutcome;
    let (_dir, mut host, request) = setup();
    let mut invalid = request.clone();
    invalid.operation = ExecutionOperation::WriteFile {
        path: "../escape".into(),
        content: "no".into(),
    };
    assert_eq!(
        host.execute_fake_operation_outcome(&invalid),
        PiEffectOutcome::Rejected
    );
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
    assert!(
        host.snapshot(&request.binding.mission_id)
            .unwrap()
            .execution_effects
            .is_empty()
    );

    let first = host.execute_fake_operation_outcome(&request);
    assert!(matches!(first, PiEffectOutcome::Completed(_)));
    assert_eq!(host.execute_fake_operation_outcome(&request), first);
    let before = host
        .snapshot(&request.binding.mission_id)
        .unwrap()
        .to_json();
    let mut changed = request.clone();
    changed.operation = ExecutionOperation::WriteFile {
        path: "out.txt".into(),
        content: "different".into(),
    };
    assert_eq!(
        host.execute_fake_operation_outcome(&changed),
        PiEffectOutcome::Conflict
    );
    assert_eq!(host.execute_fake_operation_outcome(&request), first);
    let mut stale = request.clone();
    stale.binding.generation = 0;
    assert_eq!(
        host.execute_fake_operation_outcome(&stale),
        PiEffectOutcome::Rejected
    );
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 1);
    assert_eq!(
        host.snapshot(&request.binding.mission_id)
            .unwrap()
            .to_json(),
        before
    );
}

#[cfg(feature = "pi-contract")]
#[test]
fn typed_effect_outcomes_keep_post_effect_errors_uncertain_until_durable_receipt() {
    use crate::PiEffectOutcome;
    for at in ["prepared", "effect", "completed"] {
        for storage_error in [false, true] {
            let (dir, mut host, mut request) = setup();
            let result = host.execute_fake_at(&request, |checkpoint| {
                if checkpoint != at {
                    return Ok(());
                }
                Err(if storage_error {
                    Error::storage("injected loss")
                } else {
                    Error::guard("injected loss")
                })
            });
            let observed = host.fake_effect_outcome(&request, result).unwrap();
            if at == "completed" {
                assert!(matches!(observed, PiEffectOutcome::Completed(_)));
            } else {
                assert_eq!(observed, PiEffectOutcome::Uncertain);
            }
            assert_eq!(
                host.runtime_mut().unwrap().simulated_effect_count(),
                usize::from(at != "prepared")
            );
            drop(host);
            let mut recovered = Host::open(&dir.path().join("mission.sqlite")).unwrap();
            request.binding.generation = recovered.generation();
            assert_eq!(recovered.execute_fake_operation_outcome(&request), observed);
            assert_eq!(recovered.runtime_mut().unwrap().simulated_effect_count(), 0);
            assert_eq!(
                recovered
                    .snapshot(&request.binding.mission_id)
                    .unwrap()
                    .execution_blocked,
                at != "completed"
            );
        }
    }
}

#[cfg(feature = "pi-contract")]
#[test]
fn typed_effect_outcomes_preserve_abandonment_and_unknown_storage_failures() {
    use crate::PiEffectOutcome;
    let (dir, mut host, mut request, _abandon) = abandoned_effect();
    assert_eq!(
        host.execute_fake_operation_outcome(&request),
        PiEffectOutcome::Uncertain
    );
    assert!(
        host.snapshot(&request.binding.mission_id)
            .unwrap()
            .execution_blocked
    );
    let mut fresh = request.clone();
    fresh.effect_key = "new-effect".into();
    assert_eq!(
        host.execute_fake_operation_outcome(&fresh),
        PiEffectOutcome::Rejected
    );
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 1);
    assert_eq!(
        host.fake_effect_outcome(&fresh, Err(Error::storage("unknown storage state")))
            .unwrap(),
        PiEffectOutcome::Uncertain
    );
    drop(host);
    let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
    request.binding.generation = host.generation();
    assert_eq!(
        host.execute_fake_operation_outcome(&request),
        PiEffectOutcome::Uncertain
    );
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
    assert!(
        host.snapshot(&request.binding.mission_id)
            .unwrap()
            .execution_blocked
    );
}

#[cfg(feature = "pi-contract")]
#[test]
fn completed_row_with_a_supersession_replays_as_uncertain() {
    use crate::PiEffectOutcome;
    let (dir, mut host, request) = setup();
    let completed = host.execute_fake_operation_outcome(&request);
    assert!(matches!(completed, PiEffectOutcome::Completed(_)));
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 1);
    // The supersede API refuses a completed row. Insert the pairing the replay
    // rule still has to classify: supersession wins, and the receipt is not reissued.
    let inserted = Connection::open(dir.path().join("mission.sqlite"))
        .unwrap()
        .execute(
            "INSERT INTO execution_supersessions(
                command_id, operation_id, journal, abandonment_command_id, request_hash, audit_json
             ) VALUES ('completed-supersede', ?1, 'simulated', 'completed-abandon', 'sha256:forced', '{}')",
            [request.operation_id().unwrap()],
        )
        .unwrap();
    assert_eq!(inserted, 1);
    assert_eq!(
        host.execute_fake_operation(&request)
            .unwrap_err()
            .category(),
        "execution_uncertain"
    );
    assert_eq!(
        host.execute_fake_operation_outcome(&request),
        PiEffectOutcome::Uncertain
    );
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 1);
}

#[cfg(feature = "pi-contract")]
#[test]
fn operation_owned_by_another_binding_is_rejected_not_conflict() {
    use crate::PiEffectOutcome;
    let (dir, mut host, request) = setup();
    let completed = host.execute_fake_operation_outcome(&request);
    assert!(matches!(completed, PiEffectOutcome::Completed(_)));
    let db = dir.path().join("mission.sqlite");
    let id = request.operation_id().unwrap();
    let owners = [
        (
            "mission_id",
            request.binding.mission_id.as_str(),
            "other-mission",
        ),
        (
            "attempt_id",
            request.binding.attempt_id.as_str(),
            "other-attempt",
        ),
        (
            "session_id",
            request.binding.session_id.as_str(),
            "other-session",
        ),
    ];
    for (column, original, foreign) in owners {
        retarget_effect_owner(&db, &id, column, foreign);
        assert_eq!(
            host.execute_fake_operation(&request)
                .unwrap_err()
                .category(),
            "binding_mismatch"
        );
        assert_eq!(
            host.execute_fake_operation_outcome(&request),
            PiEffectOutcome::Rejected
        );
        assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 1);
        retarget_effect_owner(&db, &id, column, original);
    }
    assert_eq!(host.execute_fake_operation_outcome(&request), completed);
}

#[cfg(feature = "pi-contract")]
fn retarget_effect_owner(db: &Path, id: &str, column: &str, value: &str) {
    let sql = match column {
        "mission_id" => "UPDATE execution_effects SET mission_id=?1 WHERE operation_id=?2",
        "attempt_id" => "UPDATE execution_effects SET attempt_id=?1 WHERE operation_id=?2",
        "session_id" => "UPDATE execution_effects SET session_id=?1 WHERE operation_id=?2",
        _ => panic!("unexpected effect owner column"),
    };
    // The stored owner is not a second live mission. Foreign keys stay on for
    // the Host connection; this test connection only rewrites the row the
    // classifier reads back.
    let connection = Connection::open(db).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    let changed = connection.execute(sql, [value, id]).unwrap();
    assert_eq!(changed, 1);
}

fn abandoned_effect() -> (tempfile::TempDir, Host, ExecutionRequest, serde_json::Value) {
    let (dir, mut host, request) = setup();
    interrupt_effect(&mut host, &request, "effect");
    host.execution_evidence
        .remove(&request.operation_id().unwrap());
    let resolve = resolution_request(&host, &request, crate::ExecutionResolutionDecision::Abandon);
    let audit = host.resolve_fake_execution(&resolve).unwrap();
    (dir, host, request, audit)
}

fn supersede_of(
    host: &Host,
    request: &ExecutionRequest,
    abandon: &serde_json::Value,
    command_id: &str,
) -> crate::ExecutionSupersessionRequest {
    crate::ExecutionSupersessionRequest {
        command_id: command_id.into(),
        binding: RuntimeBinding {
            generation: host.generation(),
            ..request.binding.clone()
        },
        operation_id: request.operation_id().unwrap(),
        payload_hash: request.payload_hash().unwrap(),
        abandonment_command_id: abandon["commandId"].as_str().unwrap().into(),
        abandonment_request_hash: abandon["requestHash"].as_str().unwrap().into(),
        expected_revision: host.snapshot(&request.binding.mission_id).unwrap().revision,
        actor: "operator".into(),
        rationale: "Fresh quiescence and workspace evidence; outcome stays unknown".into(),
    }
}

#[test]
fn worktree_digest_distinguishes_raw_line_endings() {
    let (_dir, host, request) = setup();
    let workspace = host
        .snapshot(&request.binding.mission_id)
        .unwrap()
        .workspace
        .unwrap();
    let tree = Path::new(&workspace.path);
    std::fs::write(tree.join(".gitattributes"), "data.txt text\n").unwrap();
    std::fs::write(tree.join("data.txt"), b"hello\n").unwrap();
    let lf = crate::execution::workspace::content_digest(tree).unwrap();
    std::fs::write(tree.join("data.txt"), b"hello\r\n").unwrap();
    let crlf = crate::execution::workspace::content_digest(tree).unwrap();
    assert_ne!(
        lf, crlf,
        "different on-disk bytes must have different evidence digests"
    );
}

#[test]
fn worktree_digest_bypasses_ident_and_encoding_attributes() {
    let (_dir, host, request) = setup();
    let workspace = host
        .snapshot(&request.binding.mission_id)
        .unwrap()
        .workspace
        .unwrap();
    let tree = Path::new(&workspace.path);
    std::fs::write(
        tree.join(".gitattributes"),
        "data.txt ident\nraw.bin working-tree-encoding=UTF-16LE\n",
    )
    .unwrap();
    // Invalid UTF-16 is still valid raw content evidence; Git add would reject it.
    std::fs::write(tree.join("raw.bin"), [0xff, 0x00, 0x80]).unwrap();
    std::fs::write(tree.join("data.txt"), "$Id: first $\n").unwrap();
    let first = crate::execution::workspace::content_digest(tree).unwrap();
    std::fs::write(tree.join("data.txt"), "$Id: second $\n").unwrap();
    let second = crate::execution::workspace::content_digest(tree).unwrap();
    assert_ne!(first, second, "ident expansion must not be collapsed");
}

#[cfg(unix)]
#[test]
fn worktree_digest_matches_git_for_raw_files_modes_and_symlinks() {
    use std::os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
    };
    let (dir, host, request) = setup();
    let workspace = host
        .snapshot(&request.binding.mission_id)
        .unwrap()
        .workspace
        .unwrap();
    let tree = Path::new(&workspace.path);
    let repo = dir.path().join("repo");
    let outside = dir.path().join("outside");
    std::fs::write(&outside, "not workspace content").unwrap();
    // More than two hash batches, including link targets that must not be read.
    for i in 0..70 {
        if i % 3 == 0 {
            symlink(&outside, tree.join(format!("link-{i}"))).unwrap();
        } else {
            std::fs::write(tree.join(format!("file-{i}")), [0, i, 0xff]).unwrap();
        }
    }
    let odd = std::ffi::OsString::from_vec(b"odd\tname\n-\xff".to_vec());
    std::fs::write(tree.join(odd), b"raw\0bytes").unwrap();
    std::fs::write(tree.join("run"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(tree.join("run"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_file(tree.join("README.md")).unwrap();
    git(tree, &["add", "--all", "--force"]);
    let expected = Command::new("git")
        .arg("-C")
        .arg(tree)
        .arg("write-tree")
        .output()
        .unwrap();
    assert!(expected.status.success());
    let expected = String::from_utf8(expected.stdout).unwrap();
    let index_before = staged(tree);
    let objects_before = loose_object_count(&repo);
    let digest = crate::execution::workspace::content_digest(tree).unwrap();
    assert_eq!(digest, expected.trim());
    std::fs::write(&outside, "changed outside the workspace").unwrap();
    assert_eq!(
        crate::execution::workspace::content_digest(tree).unwrap(),
        digest
    );
    std::fs::set_permissions(tree.join("run"), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_ne!(
        crate::execution::workspace::content_digest(tree).unwrap(),
        digest
    );
    assert_eq!(staged(tree), index_before);
    assert_eq!(loose_object_count(&repo), objects_before);
}

#[cfg(unix)]
#[test]
fn worktree_digest_refuses_unrepresentable_content_and_cleans_scratch() {
    for nested in [false, true] {
        let (_dir, host, request) = setup();
        let workspace = host
            .snapshot(&request.binding.mission_id)
            .unwrap()
            .workspace
            .unwrap();
        let tree = Path::new(&workspace.path);
        if nested {
            std::fs::create_dir(tree.join("nested")).unwrap();
            git(&tree.join("nested"), &["init"]);
        } else {
            assert!(
                Command::new("mkfifo")
                    .arg(tree.join("fifo"))
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let error = crate::execution::workspace::content_digest(tree).unwrap_err();
        assert!(
            error.to_string().contains(if nested {
                "nested Git"
            } else {
                "special files"
            }),
            "{error}"
        );
        assert!(
            !std::fs::read_dir(tree.parent().unwrap())
                .unwrap()
                .any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with("switchyard-digest-")
                })
        );
    }
}

#[test]
fn supersede_refuses_missing_owner_marker() {
    let (_dir, mut host, request, abandon) = abandoned_effect();
    let workspace = host
        .store
        .load_workspace(&request.binding.attempt_id)
        .unwrap()
        .unwrap();
    std::fs::remove_file(workspace.slot().join("owner.json")).unwrap();
    assert!(crate::execution::workspace::acquire(&workspace).is_err());
    let sup = supersede_of(&host, &request, &abandon, "review-missing-marker");
    let result = host.supersede_execution(&sup);
    let after = host.snapshot(&request.binding.mission_id).unwrap();
    assert!(
        result.is_err() && after.execution_blocked && after.execution_supersessions.is_empty(),
        "missing marker: accepted={}, blocked={}, supersessions={}",
        result.is_ok(),
        after.execution_blocked,
        after.execution_supersessions.len()
    );
}

#[test]
fn supersede_refuses_changed_owner_marker() {
    let (_dir, mut host, request, abandon) = abandoned_effect();
    let workspace = host
        .store
        .load_workspace(&request.binding.attempt_id)
        .unwrap()
        .unwrap();
    std::fs::write(workspace.slot().join("owner.json"), "{}").unwrap();
    assert!(crate::execution::workspace::acquire(&workspace).is_err());
    let sup = supersede_of(&host, &request, &abandon, "review-changed-marker");
    let result = host.supersede_execution(&sup);
    let after = host.snapshot(&request.binding.mission_id).unwrap();
    assert!(
        result.is_err() && after.execution_blocked && after.execution_supersessions.is_empty(),
        "changed marker: accepted={}, blocked={}, supersessions={}",
        result.is_ok(),
        after.execution_blocked,
        after.execution_supersessions.len()
    );
}

#[test]
fn supersede_refuses_held_workspace_lease() {
    let (_dir, mut host, request, abandon) = abandoned_effect();
    let workspace = host
        .store
        .load_workspace(&request.binding.attempt_id)
        .unwrap()
        .unwrap();
    let lease = crate::execution::workspace::acquire(&workspace)
        .unwrap()
        .unwrap();
    assert!(
        crate::execution::workspace::acquire(&workspace)
            .unwrap()
            .is_none()
    );
    let sup = supersede_of(&host, &request, &abandon, "review-held-lease");
    let result = host.supersede_execution(&sup);
    assert_eq!(result.as_ref().unwrap_err().category(), "workspace_busy");
    let after = host.snapshot(&request.binding.mission_id).unwrap();
    assert!(
        result.is_err() && after.execution_blocked && after.execution_supersessions.is_empty(),
        "held lease: accepted={}, blocked={}, supersessions={}",
        result.is_ok(),
        after.execution_blocked,
        after.execution_supersessions.len()
    );
    assert_eq!(after.revision, sup.expected_revision);
    drop(lease);
    host.supersede_execution(&sup).unwrap();
}

#[test]
fn supersede_refuses_nonempty_quiescence_document() {
    use crate::persistence::linux_execution::fresh_quiescence_is_empty;
    let empty = serde_json::json!({"proven": true, "populated": 0, "reason": "populated 0"});
    assert!(fresh_quiescence_is_empty(&empty));
    assert!(fresh_quiescence_is_empty(
        &serde_json::json!({"proven": true, "populated": 0, "reason": "no_workload"})
    ));
    assert!(!fresh_quiescence_is_empty(
        &serde_json::json!({"proven": false, "populated": 1, "reason": "nonempty"})
    ));
    assert!(!fresh_quiescence_is_empty(
        &serde_json::json!({"proven": true, "populated": 1, "reason": "populated 0"})
    ));
    for reason in ["missing", "mismatched", "outside_root", "unproven"] {
        assert!(
            !fresh_quiescence_is_empty(&serde_json::json!({"proven": false, "reason": reason})),
            "{reason}"
        );
    }
}

#[test]
fn supersede_abandoned_simulated_effect_keeps_uncertainty_and_allows_recovery() {
    let (dir, mut host, mut request, abandon) = abandoned_effect();
    let mission = request.binding.mission_id.clone();
    let db = dir.path().join("mission.sqlite");
    let id = request.operation_id().unwrap();
    let effect_before = effect_blob(&db, &id);
    let resolution_before = resolution_blob(&db, abandon["commandId"].as_str().unwrap());
    let workspace = host.snapshot(&mission).unwrap().workspace.unwrap();
    let tree = Path::new(&workspace.path).to_path_buf();
    let repo = dir.path().join("repo");
    std::fs::write(tree.join(".sy-tmp-left"), "leftover-v1").unwrap();
    std::fs::write(tree.join(".gitignore"), "*.hide\n").unwrap();
    std::fs::write(tree.join("note.hide"), "hidden-v1").unwrap();
    let source_stage = staged(&repo);
    let source_head = head_sha(&repo);
    let tree_stage = staged(&tree);
    let tree_head = head_sha(&tree);
    let admin = worktree_admin(&tree);
    let verifications = count_rows(&db, "verifications");
    let deliveries = count_rows(&db, "deliveries");
    let commands = count_rows(&db, "commands");
    let files_before = host.runtime_mut().unwrap().simulated_file_map();
    let applied_before = host.runtime_mut().unwrap().simulated_effect_count();
    let mut sup = supersede_of(&host, &request, &abandon, "supersede-1");
    let audit = host.supersede_execution(&sup).unwrap();
    assert_eq!(audit["evidence"]["outcomeKnown"], false);
    assert_eq!(audit["evidence"]["effectBlockLifted"], true);
    assert_eq!(audit["evidence"]["digestIncludesIgnored"], true);
    assert_eq!(audit["evidence"]["journal"], "simulated");
    let digest = audit["evidence"]["workspaceDigest"].as_str().unwrap();
    assert_eq!(
        digest,
        crate::execution::workspace::content_digest(&tree).unwrap()
    );
    std::fs::write(tree.join("note.hide"), "hidden-v2").unwrap();
    assert_ne!(
        crate::execution::workspace::content_digest(&tree).unwrap(),
        digest,
        "ignored files are part of the temporary-index digest"
    );
    std::fs::write(tree.join("note.hide"), "hidden-v1").unwrap();
    std::fs::write(tree.join(".sy-tmp-left"), "leftover-v2").unwrap();
    assert_ne!(
        crate::execution::workspace::content_digest(&tree).unwrap(),
        digest,
        ".sy-tmp leftovers are part of the digest"
    );
    assert_eq!(effect_blob(&db, &id), effect_before);
    assert_eq!(
        resolution_blob(&db, abandon["commandId"].as_str().unwrap()),
        resolution_before
    );
    assert_eq!(staged(&repo), source_stage);
    assert_eq!(head_sha(&repo), source_head);
    assert_eq!(staged(&tree), tree_stage);
    assert_eq!(head_sha(&tree), tree_head);
    assert!(!admin.join("index.lock").exists());
    let snapshot = host.snapshot(&mission).unwrap();
    assert_eq!(snapshot.phase, Phase::NeedsAttention);
    assert_eq!(snapshot.execution_effects[0].state, "uncertain");
    assert!(snapshot.execution_effects[0].superseded);
    assert!(!snapshot.execution_blocked);
    assert_eq!(snapshot.execution_supersessions, vec![audit.clone()]);
    let steps = snapshot.to_json()["nextSteps"].to_string();
    assert!(steps.contains("unknown"), "{steps}");
    assert!(
        steps.contains("workspace digest") || steps.contains("digest"),
        "{steps}"
    );
    assert_eq!(count_rows(&db, "verifications"), verifications);
    assert_eq!(count_rows(&db, "deliveries"), deliveries);
    assert_eq!(count_rows(&db, "commands"), commands);
    request.binding.generation = host.generation();
    assert_eq!(
        host.execute_fake_operation(&request)
            .unwrap_err()
            .category(),
        "execution_uncertain"
    );
    assert_eq!(
        host.runtime_mut().unwrap().simulated_effect_count(),
        applied_before
    );
    assert_eq!(
        host.runtime_mut().unwrap().simulated_file_map(),
        files_before
    );
    command(
        &mut host,
        &mission,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Implementing,
            ownership_proven: true,
            effects: crate::EffectProof::Reconciled,
        },
    );
    assert_eq!(host.snapshot(&mission).unwrap().phase, Phase::Implementing);
    assert_eq!(host.supersede_execution(&sup).unwrap(), audit);
    let mut changed = sup.clone();
    changed.rationale = "different payload".into();
    changed.expected_revision = host.snapshot(&mission).unwrap().revision;
    assert_eq!(
        host.supersede_execution(&changed).unwrap_err().category(),
        "command_conflict"
    );
    let mut second = supersede_of(&host, &request, &abandon, "supersede-2");
    second.expected_revision = host.snapshot(&mission).unwrap().revision;
    assert!(host.supersede_execution(&second).is_err());
    assert_eq!(
        host.snapshot(&mission)
            .unwrap()
            .execution_supersessions
            .len(),
        1
    );
    drop(host);
    let mut host = Host::open(&db).unwrap();
    assert_eq!(
        host.supersede_execution(&sup).unwrap_err().category(),
        "stale_generation"
    );
    sup.rationale = "Fresh quiescence and workspace evidence; outcome stays unknown".into();
    sup.binding.generation = host.generation();
    assert_eq!(host.supersede_execution(&sup).unwrap(), audit);
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 0);
}

#[test]
fn supersede_refuses_unresolved_settled_mismatched_stale_and_terminal_requests() {
    let (dir, mut host, request) = setup();
    interrupt_effect(&mut host, &request, "effect");
    let mission = request.binding.mission_id.clone();
    let _db = dir.path().join("mission.sqlite");
    let mut unresolved = supersede_of(
        &host,
        &request,
        &serde_json::json!({"commandId": "missing", "requestHash": "sha256:missing"}),
        "supersede-missing",
    );
    unresolved.abandonment_command_id = "missing".into();
    unresolved.abandonment_request_hash = "sha256:missing".into();
    assert!(
        host.supersede_execution(&unresolved)
            .unwrap_err()
            .to_string()
            .contains("committed abandonment")
    );
    assert!(host.snapshot(&mission).unwrap().execution_blocked);
    assert!(
        host.snapshot(&mission)
            .unwrap()
            .execution_supersessions
            .is_empty()
    );
    let completed = resolution_request(
        &host,
        &request,
        crate::ExecutionResolutionDecision::Completed,
    );
    let completed_audit = host.resolve_fake_execution(&completed).unwrap();
    let mut settled = supersede_of(&host, &request, &completed_audit, "supersede-completed");
    assert!(host.supersede_execution(&settled).is_err());
    assert!(
        host.snapshot(&mission)
            .unwrap()
            .execution_supersessions
            .is_empty()
    );
    assert_eq!(
        host.snapshot(&mission).unwrap().execution_effects[0].state,
        "completed"
    );
    drop(host);

    let (dir, mut host, request) = setup();
    interrupt_effect(&mut host, &request, "prepared");
    let not_applied = resolution_request(
        &host,
        &request,
        crate::ExecutionResolutionDecision::NotApplied,
    );
    let audit = host.resolve_fake_execution(&not_applied).unwrap();
    settled = supersede_of(&host, &request, &audit, "supersede-not-applied");
    assert!(host.supersede_execution(&settled).is_err());
    assert_eq!(
        host.snapshot(&request.binding.mission_id)
            .unwrap()
            .execution_effects[0]
            .state,
        "not_applied"
    );
    drop((dir, host));

    let (dir, mut host, request, abandon) = abandoned_effect();
    let mission = request.binding.mission_id.clone();
    let db = dir.path().join("mission.sqlite");
    let revision = host.snapshot(&mission).unwrap().revision;
    let mut bad = supersede_of(&host, &request, &abandon, "supersede-bad");
    bad.expected_revision -= 1;
    assert_eq!(
        host.supersede_execution(&bad).unwrap_err().category(),
        "stale_revision"
    );
    bad.expected_revision = revision;
    bad.binding.session_id = "wrong-session".into();
    assert_eq!(
        host.supersede_execution(&bad).unwrap_err().category(),
        "binding_mismatch"
    );
    bad.binding.session_id = request.binding.session_id.clone();
    bad.payload_hash = "sha256:wrong".into();
    assert_eq!(
        host.supersede_execution(&bad).unwrap_err().category(),
        "operation_conflict"
    );
    bad.payload_hash = request.payload_hash().unwrap();
    bad.abandonment_command_id = "wrong-abandonment".into();
    assert!(
        host.supersede_execution(&bad)
            .unwrap_err()
            .to_string()
            .contains("committed abandonment")
    );
    bad.abandonment_command_id = abandon["commandId"].as_str().unwrap().into();
    bad.abandonment_request_hash = "sha256:wrong".into();
    assert!(
        host.supersede_execution(&bad)
            .unwrap_err()
            .to_string()
            .contains("does not match")
    );
    bad.abandonment_request_hash = abandon["requestHash"].as_str().unwrap().into();
    bad.rationale.clear();
    assert!(host.supersede_execution(&bad).is_err());
    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE missions SET phase='implementing' WHERE id=?1",
            [&mission],
        )
        .unwrap();
    bad.rationale = "Fresh quiescence and workspace evidence; outcome stays unknown".into();
    assert!(
        host.supersede_execution(&bad)
            .unwrap_err()
            .to_string()
            .contains("needs_attention")
    );
    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE missions SET phase='cancelled' WHERE id=?1",
            [&mission],
        )
        .unwrap();
    assert_eq!(
        host.supersede_execution(&bad).unwrap_err().category(),
        "terminal_immutable"
    );
    assert!(host.snapshot(&mission).unwrap().execution_blocked);
    assert!(
        host.snapshot(&mission)
            .unwrap()
            .execution_supersessions
            .is_empty()
    );
    assert_eq!(host.snapshot(&mission).unwrap().revision, revision);
    drop(host);
    let mut host = Host::open(&db).unwrap();
    // Terminal missions stay terminal; reopen still refuses the old generation
    // only after the phase is put back. Restore needs_attention for that check.
    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE missions SET phase='needs_attention' WHERE id=?1",
            [&mission],
        )
        .unwrap();
    bad.binding.generation = host.generation().saturating_sub(1);
    bad.expected_revision = host.snapshot(&mission).unwrap().revision;
    assert_eq!(
        host.supersede_execution(&bad).unwrap_err().category(),
        "stale_generation"
    );
    assert!(
        host.snapshot(&mission)
            .unwrap()
            .execution_supersessions
            .is_empty()
    );
}

#[test]
fn supersede_workspace_failures_keep_the_block_and_leave_git_unchanged() {
    for case in ["unready", "pointer", "lock", "head"] {
        let (dir, mut host, request, abandon) = abandoned_effect();
        let mission = request.binding.mission_id.clone();
        let db = dir.path().join("mission.sqlite");
        let repo = dir.path().join("repo");
        let tree =
            Path::new(&host.snapshot(&mission).unwrap().workspace.unwrap().path).to_path_buf();
        let admin = worktree_admin(&tree);
        let revision = host.snapshot(&mission).unwrap().revision;
        match case {
            "unready" => {
                Connection::open(&db)
                    .unwrap()
                    .execute(
                        "UPDATE workspaces SET state='uncertain' WHERE attempt_id=?1",
                        [&request.binding.attempt_id],
                    )
                    .unwrap();
            }
            "pointer" => {
                std::fs::write(tree.join(".git"), "gitdir: /tmp/not-the-worktree\n").unwrap();
            }
            "lock" => {
                std::fs::write(admin.join("index.lock"), b"held").unwrap();
            }
            "head" => {
                let tree_id = String::from_utf8(head_sha(&repo)).unwrap();
                let tree_id = tree_id.trim().to_string();
                let object = Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(["rev-parse", &format!("{tree_id}^{{tree}}")])
                    .output()
                    .unwrap();
                assert!(object.status.success());
                let object = String::from_utf8(object.stdout).unwrap();
                let orphan = Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(["commit-tree", object.trim(), "-m", "unrelated"])
                    .output()
                    .unwrap();
                assert!(
                    orphan.status.success(),
                    "{}",
                    String::from_utf8_lossy(&orphan.stderr)
                );
                let orphan = String::from_utf8(orphan.stdout).unwrap();
                std::fs::write(admin.join("HEAD"), format!("{}\n", orphan.trim())).unwrap();
            }
            _ => unreachable!(),
        }
        let source_stage = staged(&repo);
        let source_head = head_sha(&repo);
        let branch = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["branch", "--show-current"])
            .output()
            .unwrap()
            .stdout;
        let sup = supersede_of(&host, &request, &abandon, "supersede-workspace");
        let error = host.supersede_execution(&sup).unwrap_err();
        assert!(
            !error.to_string().contains("quiescence was proved"),
            "workspace refusal must happen before any kill: {error}"
        );
        match case {
            "unready" => assert!(error.to_string().contains("ready workspace"), "{error}"),
            "pointer" => assert!(
                error.to_string().contains("gitdir")
                    || error.to_string().contains("backlink")
                    || error.to_string().contains("Git")
                    || error.to_string().contains("workspace evidence is missing"),
                "{error}"
            ),
            "lock" => assert!(error.to_string().contains("index.lock"), "{error}"),
            "head" => assert!(error.to_string().contains("does not descend"), "{error}"),
            _ => unreachable!(),
        }
        assert!(host.snapshot(&mission).unwrap().execution_blocked);
        assert!(
            host.snapshot(&mission)
                .unwrap()
                .execution_supersessions
                .is_empty()
        );
        assert_eq!(host.snapshot(&mission).unwrap().revision, revision);
        assert_eq!(staged(&repo), source_stage);
        assert_eq!(head_sha(&repo), source_head);
        let branch_after = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["branch", "--show-current"])
            .output()
            .unwrap()
            .stdout;
        assert_eq!(branch_after, branch);
        if case != "lock" {
            assert!(!admin.join("index.lock").exists());
        }
    }
}

#[test]
fn other_unresolved_effects_block_pause_and_cancel_until_superseded() {
    let (dir, mut host, request, abandon) = abandoned_effect();
    let mission = request.binding.mission_id.clone();
    let mut second = request.clone();
    second.effect_key = "second-effect".into();
    second.binding.generation = host.generation();
    host.store.prepare_execution(&second).unwrap();
    let mut abandon_second =
        resolution_request(&host, &second, crate::ExecutionResolutionDecision::Abandon);
    abandon_second.command_id = "resolve-2".into();
    let second_audit = host.resolve_fake_execution(&abandon_second).unwrap();
    let first = supersede_of(&host, &request, &abandon, "supersede-first");
    host.supersede_execution(&first).unwrap();
    assert!(host.snapshot(&mission).unwrap().execution_blocked);
    command(&mut host, &mission, CommandBody::Cancel);
    assert_eq!(host.dispatch_stops(&mission).unwrap(), 1);
    let snapshot = host.snapshot(&mission).unwrap();
    assert_eq!(snapshot.phase, Phase::Cancelling);
    assert!(
        host.admit(AdmitRequest {
            command_id: None,
            mission_id: mission.clone(),
            attempt_id: None,
            expected_revision: snapshot.revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::ConfirmCancelled,
        })
        .unwrap_err()
        .to_string()
        .contains("not quiescent")
    );
    let mut during_stop = supersede_of(&host, &second, &second_audit, "supersede-second");
    assert!(
        host.supersede_execution(&during_stop)
            .unwrap_err()
            .to_string()
            .contains("needs_attention")
    );
    command(
        &mut host,
        &mission,
        CommandBody::DetectUncertainty {
            reason: AttentionReason::ExecutionUncertain,
            content_digest: None,
        },
    );
    assert_eq!(
        host.snapshot(&mission).unwrap().phase,
        Phase::NeedsAttention
    );
    during_stop.expected_revision = host.snapshot(&mission).unwrap().revision;
    during_stop.binding.generation = host.generation();
    host.supersede_execution(&during_stop).unwrap();
    assert!(!host.snapshot(&mission).unwrap().execution_blocked);
    command(
        &mut host,
        &mission,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Cancelling,
            ownership_proven: true,
            effects: crate::EffectProof::Reconciled,
        },
    );
    command(&mut host, &mission, CommandBody::ConfirmCancelled);
    assert_eq!(host.snapshot(&mission).unwrap().phase, Phase::Cancelled);
    let _ = dir;
}

#[test]
fn supersede_audit_failure_rolls_back_and_resolution_rows_are_append_only() {
    let (dir, mut host, request, abandon) = abandoned_effect();
    let mission = request.binding.mission_id.clone();
    let db = dir.path().join("mission.sqlite");
    let before = host.snapshot(&mission).unwrap();
    let effect_before = effect_blob(&db, &request.operation_id().unwrap());
    Connection::open(&db)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_supersession BEFORE INSERT ON execution_supersessions
             BEGIN SELECT RAISE(ABORT, 'injected supersession failure'); END;",
        )
        .unwrap();
    let sup = supersede_of(&host, &request, &abandon, "supersede-rollback");
    let error = host.supersede_execution(&sup).unwrap_err();
    let error_text = error.to_string();
    assert!(
        error_text.contains("injected supersession failure"),
        "{error_text}"
    );
    assert!(error_text.contains("quiescence was proved"), "{error_text}");
    assert!(
        error_text.contains("synchronous_in_process_no_workers"),
        "{error_text}"
    );
    let after = host.snapshot(&mission).unwrap();
    assert_eq!(after.revision, before.revision);
    assert!(after.execution_blocked);
    assert!(after.execution_supersessions.is_empty());
    assert_eq!(
        effect_blob(&db, &request.operation_id().unwrap()),
        effect_before
    );
    Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TRIGGER fail_supersession;")
        .unwrap();
    let audit = host.supersede_execution(&sup).unwrap();
    let conn = Connection::open(&db).unwrap();
    for statement in [
        "UPDATE execution_supersessions SET request_hash='x'",
        "DELETE FROM execution_supersessions",
        "UPDATE execution_resolutions SET request_hash='x'",
        "DELETE FROM execution_resolutions",
    ] {
        let error = conn.execute(statement, []).unwrap_err();
        assert!(
            error.to_string().contains("append-only"),
            "{statement}: {error}"
        );
    }
    let error = conn
        .execute(
            "INSERT INTO execution_resolutions(command_id, operation_id, mission_id, request_hash, audit_json)
             VALUES (?1, 'op-v1:unused', ?2, 'sha256:x', '{}')",
            rusqlite::params![sup.command_id, mission],
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("resolution command identity already used"),
        "{error}"
    );
    let error = conn
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, created_at
             ) VALUES ('op-v1:linux-append', ?1, ?2, ?3, 'sha256:linux', '{}', 1, 'uncertain', 1)",
            rusqlite::params![
                mission,
                request.binding.attempt_id,
                request.binding.session_id
            ],
        )
        .unwrap();
    assert_eq!(error, 1);
    let error = conn
        .execute(
            "INSERT INTO linux_execution_resolutions(
                command_id, operation_id, mission_id, request_hash, audit_json
             ) VALUES ('linux-append-resolution', 'op-v1:linux-append', ?1, 'sha256:linux', '{}')",
            [&mission],
        )
        .unwrap();
    assert_eq!(error, 1);
    let error = conn
        .execute(
            "UPDATE linux_execution_resolutions SET request_hash='x'",
            [],
        )
        .unwrap_err();
    assert!(error.to_string().contains("append-only"), "{error}");
    let error = conn
        .execute("DELETE FROM linux_execution_resolutions", [])
        .unwrap_err();
    assert!(error.to_string().contains("append-only"), "{error}");
    let error = conn
        .execute(
            "INSERT INTO execution_supersessions(
                command_id, operation_id, journal, abandonment_command_id, request_hash, audit_json
             ) VALUES ('linux-append-resolution', 'op-v1:linux-append', 'linux', 'other-abandon', 'sha256:x', '{}')",
            [],
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("supersession command identity already used"),
        "{error}"
    );
    drop(conn);
    assert_eq!(host.supersede_execution(&sup).unwrap(), audit);
    let mut again = supersede_of(&host, &request, &abandon, "supersede-again");
    again.expected_revision = host.snapshot(&mission).unwrap().revision;
    assert!(
        host.supersede_execution(&again)
            .unwrap_err()
            .to_string()
            .contains("immutable supersession")
    );
}

#[test]
fn default_build_refuses_linux_supersession_with_guidance() {
    let (dir, host, request) = setup();
    let mission = request.binding.mission_id.clone();
    let db = dir.path().join("mission.sqlite");
    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE missions SET phase='needs_attention' WHERE id=?1",
            [&mission],
        )
        .unwrap();
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, cgroup_path, cgroup_dev, cgroup_ino, boot_id,
                entered_at, created_at
             ) VALUES (
                'op-v1:linuxrow', ?1, ?2, ?3, 'sha256:row', '{}', 1, 'uncertain',
                '/sys/fs/cgroup/switchyard/sy-row', 1, 2, 'boot', 10, 10
             )",
            rusqlite::params![
                mission,
                request.binding.attempt_id,
                request.binding.session_id
            ],
        )
        .unwrap();
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_resolutions(
                command_id, operation_id, mission_id, request_hash, audit_json
             ) VALUES (
                'abandon-linux-row', 'op-v1:linuxrow', ?1, 'sha256:abandon',
                '{\"request\":{\"decision\":\"abandoned\"}}'
             )",
            [&mission],
        )
        .unwrap();
    drop(host);
    let mut host = Host::open(&db).unwrap();
    let before = host.snapshot(&mission).unwrap();
    assert!(before.execution_blocked);
    let mut sup = crate::ExecutionSupersessionRequest {
        command_id: "supersede-linux".into(),
        binding: RuntimeBinding {
            generation: host.generation(),
            ..request.binding.clone()
        },
        operation_id: "op-v1:linuxrow".into(),
        payload_hash: "sha256:row".into(),
        abandonment_command_id: "abandon-linux-row".into(),
        abandonment_request_hash: "sha256:abandon".into(),
        expected_revision: before.revision,
        actor: "operator".into(),
        rationale: "cannot prove a cgroup from the default build".into(),
    };
    let error = host.supersede_execution(&sup).unwrap_err();
    assert!(
        error.to_string().contains("delegated cgroup backend"),
        "{error}"
    );
    assert!(error.to_string().contains("stays uncertain"), "{error}");
    let after = host.snapshot(&mission).unwrap();
    assert!(after.execution_blocked);
    assert!(after.execution_supersessions.is_empty());
    assert_eq!(after.linux_execution_effects[0].state, "uncertain");
    assert!(!after.linux_execution_effects[0].superseded);
    assert_eq!(after.revision, before.revision);
    sup.command_id = "supersede-linux-2".into();
    assert!(host.supersede_execution(&sup).is_err());
}

#[test]
fn v8_upgrade_preserves_abandoned_simulated_and_linux_rows() {
    let (dir, host, request, abandon) = abandoned_effect();
    let mission = request.binding.mission_id.clone();
    let db = dir.path().join("mission.sqlite");
    let effect_before = effect_blob(&db, &request.operation_id().unwrap());
    let resolution_before = resolution_blob(&db, abandon["commandId"].as_str().unwrap());
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, cgroup_path, cgroup_dev, cgroup_ino, boot_id,
                entered_at, receipt_json, quiescence_json, created_at
             ) VALUES (
                'op-v1:v8linux', ?1, ?2, ?3, 'sha256:v8', '{\"intent\":true}', 1, 'uncertain',
                '/old/leaf', 4, 5, 'boot-v8', 9, NULL, '{\"proven\":true,\"populated\":0}', 11
             )",
            rusqlite::params![
                mission,
                request.binding.attempt_id,
                request.binding.session_id
            ],
        )
        .unwrap();
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_resolutions(
                command_id, operation_id, mission_id, request_hash, audit_json
             ) VALUES (
                'abandon-v8-linux', 'op-v1:v8linux', ?1, 'sha256:v8-abandon',
                '{\"request\":{\"decision\":\"abandoned\"},\"kept\":true}'
             )",
            [&mission],
        )
        .unwrap();
    let linux_effect: String = Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT operation_id||'|'||state||'|'||request_json||'|'||ifnull(receipt_json,'')||'|'||ifnull(quiescence_json,'')||'|'||ifnull(released_at,-1) FROM linux_execution_effects WHERE operation_id='op-v1:v8linux'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let linux_resolution: String = Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT command_id||'|'||request_hash||'|'||audit_json FROM linux_execution_resolutions WHERE command_id='abandon-v8-linux'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(host);
    Connection::open(&db)
        .unwrap()
        .execute_batch(
            "DROP TABLE IF EXISTS execution_supersessions;
             DELETE FROM schema_migrations WHERE version>=9;
             PRAGMA user_version=8;",
        )
        .unwrap();
    let inspected = Host::inspect(&db, Some(&mission)).unwrap();
    assert!(inspected[0].execution_supersessions.is_empty());
    assert!(
        inspected[0]
            .execution_effects
            .iter()
            .all(|effect| !effect.superseded)
    );
    assert!(
        inspected[0]
            .linux_execution_effects
            .iter()
            .all(|effect| !effect.superseded)
    );
    let version: i64 = Connection::open(&db)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 8);
    let host = Host::open(&db).unwrap();
    assert_eq!(
        effect_blob(&db, &request.operation_id().unwrap()),
        effect_before
    );
    assert_eq!(
        resolution_blob(&db, abandon["commandId"].as_str().unwrap()),
        resolution_before
    );
    let linux_after: String = Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT operation_id||'|'||state||'|'||request_json||'|'||ifnull(receipt_json,'')||'|'||ifnull(quiescence_json,'')||'|'||ifnull(released_at,-1) FROM linux_execution_effects WHERE operation_id='op-v1:v8linux'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let resolution_after: String = Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT command_id||'|'||request_hash||'|'||audit_json FROM linux_execution_resolutions WHERE command_id='abandon-v8-linux'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(linux_after, linux_effect);
    assert_eq!(resolution_after, linux_resolution);
    let version: i64 = Connection::open(&db)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 9);
    let snapshot = host.snapshot(&mission).unwrap();
    assert!(snapshot.execution_blocked);
    assert!(snapshot.execution_supersessions.is_empty());
    assert_eq!(snapshot.execution_effects[0].state, "uncertain");
    assert!(!snapshot.execution_effects[0].superseded);
}

fn loose_object_count(repo: &Path) -> usize {
    let objects = repo.join(".git/objects");
    let mut count = 0usize;
    for entry in std::fs::read_dir(&objects).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.len() == 2 && name.chars().all(|c| c.is_ascii_hexdigit()) {
            count += std::fs::read_dir(entry.path()).unwrap().count();
        }
    }
    count
}

#[test]
fn supersede_sibling_rekill_cannot_race_a_second_host() {
    let (dir, host, _request, _abandon) = abandoned_effect();
    let db = dir.path().join("mission.sqlite");
    let error = match Host::open(&db) {
        Err(error) => error,
        Ok(_) => panic!("a second host opened the locked ledger"),
    };
    assert_eq!(error.category(), "second_active_owner");
    assert!(
        error
            .to_string()
            .contains("second active owner already holds the host lock")
    );
    drop(host);
    let _reopened = Host::open(&db).unwrap();
}

#[test]
fn worktree_digest_keeps_ignored_blobs_out_of_the_repository() {
    let (dir, host, request) = setup();
    let repo = dir.path().join("repo");
    let tree = Path::new(
        &host
            .snapshot(&request.binding.mission_id)
            .unwrap()
            .workspace
            .unwrap()
            .path,
    )
    .to_path_buf();
    let before = loose_object_count(&repo);
    std::fs::write(tree.join(".gitignore"), "secret.env\n").unwrap();
    std::fs::write(tree.join("secret.env"), "TOKEN=should-not-enter-the-odb\n").unwrap();
    std::fs::write(tree.join(".sy-tmp-left"), "leftover").unwrap();
    let digest = crate::execution::workspace::content_digest(&tree).unwrap();
    assert_eq!(digest.len(), 40);
    assert_eq!(loose_object_count(&repo), before);
    std::fs::write(tree.join("secret.env"), "TOKEN=changed\n").unwrap();
    let again = crate::execution::workspace::content_digest(&tree).unwrap();
    assert_ne!(again, digest);
    assert_eq!(loose_object_count(&repo), before);
    let slot = tree.parent().unwrap();
    let stale = slot.join("switchyard-digest-stale");
    std::fs::create_dir(&stale).unwrap();
    std::fs::write(stale.join("secret.env"), "TOKEN=left-by-a-killed-host\n").unwrap();
    let _ = crate::execution::workspace::content_digest(&tree).unwrap();
    assert!(!stale.exists(), "stale digest scratch must be removed");
    let leftovers = std::fs::read_dir(slot).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("switchyard-digest-")
    });
    assert!(!leftovers, "digest scratch must not outlive the call");
    let _ = host;
}

#[test]
fn supersede_refuses_a_copied_ledger_before_any_kill() {
    let (dir, host, request, abandon) = abandoned_effect();
    let mission = request.binding.mission_id.clone();
    let db = dir.path().join("mission.sqlite");
    drop(host);
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
    }
    let copy = dir.path().join("copy.sqlite");
    std::fs::copy(&db, &copy).unwrap();
    let mut original = Host::open(&db).unwrap();
    let mut copied = Host::open(&copy).unwrap();
    let revision = copied.snapshot(&mission).unwrap().revision;
    let sup = supersede_of(&copied, &request, &abandon, "supersede-copied-ledger");
    let error = copied.supersede_execution(&sup).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("conflicts with its reservation"), "{text}");
    assert!(
        !text.contains("quiescence was proved"),
        "copied ledger must be refused before any kill: {text}"
    );
    assert_eq!(copied.snapshot(&mission).unwrap().revision, revision);
    assert!(
        copied
            .snapshot(&mission)
            .unwrap()
            .execution_supersessions
            .is_empty()
    );
    drop(copied);
    let real = supersede_of(&original, &request, &abandon, "supersede-original-ledger");
    original.supersede_execution(&real).unwrap();
}

#[test]
fn simulated_supersede_names_the_blocking_linux_sibling() {
    let (dir, mut host, request, abandon) = abandoned_effect();
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
                'op-v1:blocking-sibling', ?1, ?2, ?3, 'sha256:sibling', '{}', 1, 'entered',
                '/sys/fs/cgroup/switchyard/sy-sibling', 1, 2, 'boot', 10, 10
             )",
            rusqlite::params![
                mission,
                request.binding.attempt_id,
                request.binding.session_id
            ],
        )
        .unwrap();
    let revision = host.snapshot(&mission).unwrap().revision;
    let sup = supersede_of(&host, &request, &abandon, "supersede-sibling-name");
    let error = host.supersede_execution(&sup).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("op-v1:blocking-sibling"), "{text}");
    assert!(text.contains("simulated supersession refused"), "{text}");
    assert!(
        !text.contains("linux supersession requires the delegated cgroup backend"),
        "{text}"
    );
    assert!(host.snapshot(&mission).unwrap().execution_blocked);
    assert!(
        host.snapshot(&mission)
            .unwrap()
            .execution_supersessions
            .is_empty()
    );
    assert_eq!(host.snapshot(&mission).unwrap().revision, revision);
}

#[test]
fn supersede_unique_races_are_guard_errors() {
    let (dir, mut host, request, abandon) = abandoned_effect();
    let mission = request.binding.mission_id.clone();
    let db = dir.path().join("mission.sqlite");
    let abandonment = abandon["commandId"].as_str().unwrap();
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO execution_supersessions(
                command_id, operation_id, journal, abandonment_command_id, request_hash, audit_json
             ) VALUES ('decoy-supersede', 'op-v1:decoy', 'simulated', ?1, 'sha256:decoy', '{}')",
            [abandonment],
        )
        .unwrap();
    let revision = host.snapshot(&mission).unwrap().revision;
    let sup = supersede_of(&host, &request, &abandon, "supersede-abandon-unique");
    let error = host.supersede_execution(&sup).unwrap_err();
    let text = error.to_string();
    assert_eq!(error.category(), "guard_failed");
    assert!(
        text.contains("abandonment already has an immutable supersession"),
        "{text}"
    );
    assert!(!text.contains("storage failed"), "{text}");
    assert!(text.contains("quiescence was proved"), "{text}");
    assert_eq!(host.snapshot(&mission).unwrap().revision, revision);
    assert!(
        host.snapshot(&mission)
            .unwrap()
            .execution_supersessions
            .is_empty()
    );

    let (dir, mut host, request, abandon) = abandoned_effect();
    let mission = request.binding.mission_id.clone();
    let db = dir.path().join("mission.sqlite");
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO execution_supersessions(
                command_id, operation_id, journal, abandonment_command_id, request_hash, audit_json
             ) VALUES ('decoy-operation', ?1, 'simulated', 'other-abandonment', 'sha256:decoy', '{}')",
            [request.operation_id().unwrap()],
        )
        .unwrap();
    let revision = host.snapshot(&mission).unwrap().revision;
    let raced = supersede_of(&host, &request, &abandon, "supersede-operation-unique");
    let error = host
        .store
        .commit_supersession(&raced, "simulated", &serde_json::json!({}))
        .unwrap_err();
    let text = error.to_string();
    assert_eq!(error.category(), "guard_failed");
    assert!(
        text.contains("effect already has an immutable supersession"),
        "{text}"
    );
    assert!(!text.contains("storage failed"), "{text}");
    assert_eq!(host.snapshot(&mission).unwrap().revision, revision);
    let rows = host.snapshot(&mission).unwrap().execution_supersessions;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].get("resultingRevision").is_none(), "{rows:?}");
}
