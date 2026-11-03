use super::*;

fn binding() -> RuntimeBinding {
    RuntimeBinding {
        mission_id: "m".into(),
        attempt_id: "a".into(),
        session_id: "s".into(),
        generation: 1,
    }
}
fn frame(payload: Value) -> Value {
    json!({"operationId":operation_id("a", "s", "task", "slot").unwrap(),
        "taskId":"task", "effectKey":"slot", "payloadHash":payload_hash(&payload).unwrap(), "payload":payload})
}
fn start() -> Value {
    json!({"method":"process.start","argv":["/bin/echo","hello"],
        "capability":"workspace-process","deadlinePolicy":"attempt-bound"})
}
#[test]
fn process_wire_mapping_is_strict_and_has_no_host_authority() {
    let b = binding();
    assert!(matches!(
        parse_process(&b, &frame(start()), 42).unwrap().operation,
        LinuxOperation::ExecStart { .. }
    ));
    for (key, value) in [
        ("method", json!("exec")),
        ("capability", json!("host-process")),
        ("deadlinePolicy", json!("unbounded")),
        ("deadlineMs", json!(99)),
        ("cwd", json!("/")),
        ("env", json!({"TOKEN":"secret"})),
        ("binding", json!({"generation":2})),
        ("argv", json!([])),
        ("argv", json!(["/bin/echo", 1])),
        ("argv", json!(["x\u{0}y"])),
        ("argv", json!(vec!["x"; 33])),
        ("argv", json!(["x".repeat(4097)])),
    ] {
        let mut p = start();
        p[key] = value;
        assert!(parse_process(&b, &frame(p), 42).is_err(), "{key}");
    }
    for key in ["operationId", "payloadHash", "taskId", "effectKey", "extra"] {
        let mut p = frame(start());
        p[key] = json!("forged");
        assert!(parse_process(&b, &p, 42).is_err(), "{key}");
    }
    for method in ["process.status", "process.cancel"] {
        let p = json!({"method":method,"startOperationId":operation_id("a","s","start","slot").unwrap(),
            "startPayloadHash":"sha256:original", "capability":"workspace-process","deadlinePolicy":"attempt-bound"});
        let request = parse_process(&b, &frame(p.clone()), 42).unwrap();
        assert_eq!(request.deadline_ms, 42);
        let mut bad = p;
        bad["argv"] = json!(["extra"]);
        assert!(parse_process(&b, &frame(bad), 42).is_err());
    }
}

#[test]
fn start_handle_replay_freezes_identity_without_treating_orphans_as_running() {
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
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let db = dir.path().join("mission.sqlite");
    let mut h = Host::open(&db).unwrap();
    let m = h
        .create_mission(CreateMission {
            repo,
            issue_number: 97,
            issue_title: "process".into(),
            objective: "process".into(),
            acceptance: "recover".into(),
            actor: "operator".into(),
            max_repairs: 1,
            max_plan_revisions: 1,
        })
        .unwrap();
    let mut b = RuntimeBinding {
        mission_id: m.mission_id,
        attempt_id: m.attempt_id,
        session_id: m.session_id,
        generation: h.generation(),
    };
    let mut p = frame(start());
    p["operationId"] = json!(operation_id(&b.attempt_id, &b.session_id, "task", "slot").unwrap());
    let req = parse_process(&b, &p, 123).unwrap();
    h.store.prepare_linux(&req, None).unwrap();
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE linux_execution_effects SET state='running', released_at=1, entered_at=1",
            [],
        )
        .unwrap();
    // A persisted running row alone is not a live supervisor or a final result.
    assert_eq!(
        h.pi_linux_outcome(&req).unwrap(),
        PiEffectOutcome::Uncertain
    );
    let receipt = json!({"operationId":req.operation_id().unwrap(),"payloadHash":req.payload_hash().unwrap(),
        "backend":crate::linux_execution::BUBBLEWRAP_BACKEND,"simulated":false,"disposition":"exited", "result":{"exitCode":7}});
    h.store
        .complete_linux(&req, &receipt, &json!({"proven":true}))
        .unwrap();
    let original = h.execute_pi_linux_process(&b, &p, 0);
    let PiEffectOutcome::Completed(handle) = &original else {
        panic!("{original:?}")
    };
    assert_eq!(handle["kind"], "process-handle");
    assert_eq!(handle["disposition"], "accepted");
    assert!(
        handle.get("result").is_none(),
        "a handle must not claim exit status"
    );
    drop(h);
    let mut h = Host::open(&db).unwrap();
    assert_eq!(
        h.execute_pi_linux_process(&b, &p, 0),
        PiEffectOutcome::Rejected
    );
    b.generation = h.generation();
    assert_eq!(h.execute_pi_linux_process(&b, &p, i64::MAX), original);
    p["payload"]["argv"] = json!(["/bin/false"]);
    p["payloadHash"] = json!(payload_hash(&p["payload"]).unwrap());
    assert_eq!(
        h.execute_pi_linux_process(&b, &p, 0),
        PiEffectOutcome::Conflict
    );
}
