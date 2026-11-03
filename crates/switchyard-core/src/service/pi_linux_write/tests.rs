use super::*;
use std::path::Path;
use std::process::Command;

fn setup() -> (tempfile::TempDir, Host, RuntimeBinding, Value) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "--initial-branch=main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
        vec!["commit", "--allow-empty", "-m", "base"],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
    let mission = host
        .create_mission(CreateMission {
            repo,
            issue_number: 97,
            issue_title: "Pi Linux write".into(),
            objective: "one write".into(),
            acceptance: "recover".into(),
            actor: "operator".into(),
            max_repairs: 1,
            max_plan_revisions: 1,
        })
        .unwrap();
    let binding = RuntimeBinding {
        mission_id: mission.mission_id,
        attempt_id: mission.attempt_id,
        session_id: mission.session_id,
        generation: host.generation(),
    };
    let params = frame(&binding);
    (dir, host, binding, params)
}

fn frame(binding: &RuntimeBinding) -> Value {
    let payload = json!({"method":"file.write","path":"output.txt","content":"once",
        "capability":"workspace-write","deadlinePolicy":"attempt-bound"});
    json!({"operationId":crate::operation_id(&binding.attempt_id, &binding.session_id, "task", "write-file:v1").unwrap(),
        "taskId":"task","effectKey":"write-file:v1","payloadHash":payload_hash(&payload).unwrap(),"payload":payload})
}

fn changed(params: &Value, key: &str, value: Value) -> Value {
    let mut params = params.clone();
    params["payload"][key] = value;
    params["payloadHash"] = json!(payload_hash(&params["payload"]).unwrap());
    params
}

fn ledger(dir: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.join("mission.sqlite")).unwrap()
}

#[test]
fn allowlist_identity_and_paths_are_checked_before_admission() {
    let (_dir, mut host, binding, params) = setup();
    assert!(parse_write(&binding, &params, 123).is_ok());
    for (key, value) in [
        ("method", json!("exec_start")),
        ("capability", json!("host-write")),
        ("deadlinePolicy", json!("unbounded")),
        ("extra", json!(true)),
        ("content", json!(1)),
        ("path", json!("../escape")),
        ("path", json!(".git/config")),
        ("path", json!("/mission.sqlite")),
        ("path", json!("a/../escape")),
    ] {
        assert_eq!(
            host.execute_pi_linux_write(&binding, &changed(&params, key, value), 123),
            PiEffectOutcome::Rejected
        );
    }
    for key in ["operationId", "payloadHash", "taskId", "effectKey"] {
        let mut invalid = params.clone();
        invalid[key] = json!("wrong");
        assert_eq!(
            host.execute_pi_linux_write(&binding, &invalid, 123),
            PiEffectOutcome::Rejected
        );
    }
    // Well-formed writes cannot enable a backend implicitly.
    assert_eq!(
        host.execute_pi_linux_write(&binding, &params, 123),
        PiEffectOutcome::Rejected
    );
    assert!(
        host.snapshot(&binding.mission_id)
            .unwrap()
            .linux_execution_effects
            .is_empty()
    );
}

#[test]
fn completed_linux_receipt_and_frozen_deadline_survive_reopen() {
    let (dir, mut host, mut binding, params) = setup();
    let request = parse_write(&binding, &params, 123).unwrap();
    host.store.prepare_linux(&request, None).unwrap();
    let receipt = json!({"operationId":request.operation_id().unwrap(), "payloadHash":request.payload_hash().unwrap(),
        "backend":BUBBLEWRAP_BACKEND,"simulated":false,"result":{"fixture":"classifier only"}});
    host.store
        .complete_linux(&request, &receipt, &json!({"proven":true}))
        .unwrap();
    drop(host);
    let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
    assert_eq!(
        host.execute_pi_linux_write(&binding, &params, i64::MAX),
        PiEffectOutcome::Rejected
    );
    binding.generation = host.generation();
    for deadline in [0, 1, i64::MAX] {
        assert_eq!(
            host.execute_pi_linux_write(&binding, &params, deadline),
            PiEffectOutcome::Completed(receipt.clone())
        );
    }
    assert_eq!(
        host.execute_pi_linux_write(
            &binding,
            &changed(&params, "content", json!("different")),
            1
        ),
        PiEffectOutcome::Conflict
    );
    assert_eq!(
        host.execute_pi_linux_write(&binding, &changed(&params, "path", json!("different")), 1),
        PiEffectOutcome::Conflict
    );
    let stored: String = ledger(dir.path())
        .query_row(
            "SELECT request_json FROM linux_execution_effects",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&stored).unwrap()["deadlineMs"],
        123
    );
}

#[test]
fn classifier_never_treats_absence_or_failed_evidence_as_rejection() {
    let (dir, mut host, binding, params) = setup();
    let request = parse_write(&binding, &params, 123).unwrap();
    assert_eq!(
        host.pi_linux_outcome(&request).unwrap(),
        PiEffectOutcome::Uncertain
    );
    host.store.prepare_linux(&request, None).unwrap();
    assert_eq!(
        host.pi_linux_outcome(&request).unwrap(),
        PiEffectOutcome::Uncertain
    );
    host.store
        .quarantine_linux(&request.operation_id().unwrap())
        .unwrap();
    assert_eq!(
        host.pi_linux_outcome(&request).unwrap(),
        PiEffectOutcome::Uncertain
    );
    // A corrupted stored request is not permission to create a replacement.
    ledger(dir.path())
        .execute("UPDATE linux_execution_effects SET request_json='{}'", [])
        .unwrap();
    assert_eq!(
        host.execute_pi_linux_write(&binding, &params, 123),
        PiEffectOutcome::Uncertain
    );
    ledger(dir.path())
        .execute(
            "ALTER TABLE linux_execution_effects RENAME TO unavailable",
            [],
        )
        .unwrap();
    assert_eq!(
        host.execute_pi_linux_write(&binding, &params, 123),
        PiEffectOutcome::Uncertain
    );
}

#[test]
fn not_applied_and_changed_payloads_remain_distinct() {
    let (_dir, mut host, binding, params) = setup();
    let request = parse_write(&binding, &params, 123).unwrap();
    host.store.prepare_linux(&request, None).unwrap();
    host.store
        .linux_not_applied(&request, &json!({"proven":true}))
        .unwrap();
    assert_eq!(
        host.execute_pi_linux_write(&binding, &params, 123),
        PiEffectOutcome::Rejected
    );
    assert_eq!(
        host.execute_pi_linux_write(
            &binding,
            &changed(&params, "content", json!("different")),
            123
        ),
        PiEffectOutcome::Conflict
    );
}

#[test]
fn simulated_identity_and_foreign_binding_cannot_reach_linux() {
    let (_dir, mut host, binding, params) = setup();
    let simulated = crate::ExecutionRequest {
        binding: binding.clone(),
        tool_task_id: "task".into(),
        effect_key: "write-file:v1".into(),
        capability_scope: crate::FAKE_WRITE_SCOPE.into(),
        deadline_ms: 123,
        operation: crate::ExecutionOperation::WriteFile {
            path: "output.txt".into(),
            content: "once".into(),
        },
    };
    host.store.prepare_execution(&simulated).unwrap();
    assert_eq!(
        host.execute_pi_linux_write(&binding, &params, 123),
        PiEffectOutcome::Conflict
    );
    let mut foreign = binding.clone();
    foreign.session_id = "other-session".into();
    assert_eq!(
        host.execute_pi_linux_write(&foreign, &frame(&foreign), 123),
        PiEffectOutcome::Rejected
    );
    assert!(
        host.snapshot(&binding.mission_id)
            .unwrap()
            .linux_execution_effects
            .is_empty()
    );
}

#[test]
fn malformed_completed_receipts_are_not_reported_as_success() {
    let (dir, mut host, binding, params) = setup();
    let request = parse_write(&binding, &params, 123).unwrap();
    host.store.prepare_linux(&request, None).unwrap();
    ledger(dir.path()).execute("UPDATE linux_execution_effects SET state='completed', receipt_json='{}', quiescence_json='{}'", []).unwrap();
    assert_eq!(
        host.execute_pi_linux_write(&binding, &params, 123),
        PiEffectOutcome::Uncertain
    );
}
