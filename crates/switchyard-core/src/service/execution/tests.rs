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
    conn.execute_batch("DROP TABLE execution_effects; DELETE FROM schema_migrations WHERE version=4; PRAGMA user_version=3;").unwrap();
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
        4
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
    host.store.prepare_execution(&request).unwrap();
    let receipt = host.runtime_mut().unwrap().apply(&request).unwrap();
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
