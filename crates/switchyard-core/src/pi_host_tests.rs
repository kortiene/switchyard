//! Real Host ledger + pinned Pi child. No simulated outbox or provider credentials.
use super::*;
use crate::execution::PiRuntime;
use rusqlite::{Connection, params};
use std::process::Command;

struct World {
    _tmp: tempfile::TempDir,
    db: PathBuf,
    repo: PathBuf,
}
fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "--initial-branch=main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
        vec!["commit", "--allow-empty", "-m", "init"],
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
    World {
        db: tmp.path().join("mission.sqlite"),
        repo,
        _tmp: tmp,
    }
}
fn host(world: &World, fixture: &'static str) -> Host {
    let node = std::env::var_os("SWITCHYARD_PI_NODE")
        .expect("pi-contract requires pinned SWITCHYARD_PI_NODE; do not skip");
    let mut host = Host::open_pi_probe(&world.db, Path::new(&node)).unwrap();
    pi(&mut host).fixture = fixture;
    host
}
fn pi(host: &mut Host) -> &mut PiRuntime {
    match &mut host.runtime {
        RuntimeAdapter::Pi(runtime) => runtime,
        _ => panic!("Pi host required"),
    }
}
fn create(host: &mut Host, world: &World) -> MissionSnapshot {
    host.create_mission(CreateMission {
        repo: world.repo.clone(),
        issue_number: 119,
        issue_title: "outbox".into(),
        objective: "Recover one submission".into(),
        acceptance: "No duplicate".into(),
        actor: "operator".into(),
        max_repairs: 1,
        max_plan_revisions: 1,
    })
    .unwrap()
}
fn admit(host: &mut Host, mission: &str, body: CommandBody) -> Result<AdmitResult> {
    host.admit(AdmitRequest {
        command_id: None,
        mission_id: mission.into(),
        attempt_id: None,
        expected_revision: host.snapshot(mission)?.revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body,
    })
}
fn request(
    host: &Host,
    mission: &str,
    command: &AdmitResult,
    method: &'static str,
) -> RuntimeRequest {
    RuntimeRequest {
        binding: host.binding(&host.store.load_mission(mission).unwrap()),
        request_id: command.request_id.clone(),
        payload_hash: command.payload_hash.clone(),
        method,
    }
}
fn first_receipt(host: &mut Host, req: &RuntimeRequest) -> Value {
    pi(host).inspect(req).unwrap()["commands"][0].clone()
}
fn counts(world: &World) -> (i64, i64, i64) {
    let db = Connection::open(&world.db).unwrap();
    (
        db.query_row("SELECT count(*) FROM missions", [], |r| r.get(0))
            .unwrap(),
        db.query_row("SELECT count(*) FROM attempts", [], |r| r.get(0))
            .unwrap(),
        db.query_row("SELECT count(*) FROM commands", [], |r| r.get(0))
            .unwrap(),
    )
}

#[test]
fn lost_ack_recovers_original_receipt_and_identity_across_repeated_restarts() {
    let w = world();
    let mut h = host(&w, "crash-after-admission");
    let m = create(&mut h, &w);
    let c = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
    assert!(h.dispatch_pending(&m.mission_id).is_err());
    assert_eq!(h.snapshot(&m.mission_id).unwrap().pending_commands.len(), 1);
    drop(h);
    let mut original = None;
    for _ in 0..3 {
        let mut h = host(&w, "recover");
        // Explicit initialization/inspection is inert, and the outbox still
        // holds the original request until recover records its durable receipt.
        let req = request(&h, &m.mission_id, &c, "submit");
        pi(&mut h).open(&req.binding, true).unwrap();
        let before = first_receipt(&mut h, &req);
        let id = before["receipt"]["receipt"].as_str().unwrap().to_string();
        if let Some(original) = &original {
            assert_eq!(&id, original);
        } else {
            original = Some(id.clone());
        }
        let after = h.recover(&m.mission_id).unwrap();
        assert_eq!(
            (after.mission_id, after.attempt_id, after.session_id),
            (
                m.mission_id.clone(),
                m.attempt_id.clone(),
                m.session_id.clone()
            )
        );
        assert!(after.pending_commands.is_empty());
        assert_eq!(after.commands[0].request_id, c.request_id);
        assert_eq!(after.commands[0].receipt_id.as_deref(), Some(id.as_str()));
        assert_eq!(after.commands[0].payload_hash, c.payload_hash);
        assert_eq!(after.phase, Phase::Planning);
        assert_eq!(after.runtime_adapter, "pi-probe-v1");
        assert_eq!(first_receipt(&mut h, &req)["receipt"], before["receipt"]);
        assert_eq!(counts(&w), (1, 1, 1));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let state = first_receipt(&mut h, &req);
            if state["status"] == "done" {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "recovered submission never resumed: {state}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[test]
fn crash_before_admission_has_no_receipt_and_dispatches_once_after_restart() {
    let w = world();
    let mut h = host(&w, "crash-before-admission");
    let m = create(&mut h, &w);
    let c = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
    assert!(h.dispatch_pending(&m.mission_id).is_err());
    drop(h);
    let mut h = host(&w, "recover");
    let req = request(&h, &m.mission_id, &c, "submit");
    pi(&mut h).open(&req.binding, true).unwrap();
    assert!(first_receipt(&mut h, &req)["receipt"].is_null());
    h.recover(&m.mission_id).unwrap();
    let first = first_receipt(&mut h, &req);
    assert!(first["receipt"]["receipt"].is_string());
    assert_eq!(h.dispatch_pending(&m.mission_id).unwrap(), 0);
    assert_eq!(first_receipt(&mut h, &req)["receipt"], first["receipt"]);
    assert_eq!(counts(&w), (1, 1, 1));
}

#[test]
fn pending_pause_and_cancel_reconcile_lost_work_ack_before_abort_without_continuation() {
    for stop in [CommandBody::Pause, CommandBody::Cancel] {
        let w = world();
        let mut h = host(&w, "crash-after-admission");
        let m = create(&mut h, &w);
        let c = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
        assert!(h.dispatch_pending(&m.mission_id).is_err());
        let continuation = admit(
            &mut h,
            &m.mission_id,
            CommandBody::Continue {
                activity: "planning".into(),
            },
        )
        .unwrap();
        let stop = admit(&mut h, &m.mission_id, stop).unwrap();
        drop(h);
        let mut h = host(&w, "recover");
        let req = request(&h, &m.mission_id, &c, "submit");
        pi(&mut h).open(&req.binding, true).unwrap();
        let before = first_receipt(&mut h, &req);
        // Crash barrier was before harness.resume: no answer could be produced.
        assert_ne!(before["status"], "done");
        let recovered = h.recover(&m.mission_id).unwrap();
        assert_eq!(recovered.phase, stop.phase);
        assert!(recovered.pending_commands.is_empty());
        assert_eq!(recovered.commands.len(), 3);
        assert_eq!(recovered.commands[1].id, continuation.command_id);
        assert_eq!(recovered.commands[1].state, "superseded");
        assert_eq!(
            recovered.commands[0].receipt_id.as_deref(),
            before["receipt"]["receipt"].as_str()
        );
        let after = first_receipt(&mut h, &req);
        assert_eq!(after["reason"], "aborted");
        assert!(admit(&mut h, &m.mission_id, CommandBody::ConfirmStopped).is_err());
        assert!(admit(&mut h, &m.mission_id, CommandBody::ConfirmCancelled).is_err());
        drop(h);
        let mut h = host(&w, "recover");
        h.recover(&m.mission_id).unwrap();
        assert_eq!(counts(&w), (1, 1, 3));
        assert_eq!(h.snapshot(&m.mission_id).unwrap().phase, stop.phase);
    }
}

#[test]
fn changed_canonical_content_under_same_request_conflicts_without_overwriting_receipt() {
    let w = world();
    let mut h = host(&w, "crash-after-admission");
    let m = create(&mut h, &w);
    let c = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
    assert!(h.dispatch_pending(&m.mission_id).is_err());
    drop(h);
    // Simulate conflicting persisted input, including its correct new hash.
    // This tests conflict against Pi, beyond Host.admit's same-ID guard.
    let changed = json!({"missionId":m.mission_id,"attemptId":m.attempt_id,"kind":"start_planning","body":{"changed":"content"}});
    Connection::open(&w.db)
        .unwrap()
        .execute(
            "UPDATE commands SET payload_canonical=?1,payload_hash=?2 WHERE id=?3",
            params![
                canonicalize(&changed).unwrap(),
                payload_hash(&changed).unwrap(),
                c.command_id
            ],
        )
        .unwrap();
    let mut h = host(&w, "recover");
    let req = request(&h, &m.mission_id, &c, "submit");
    pi(&mut h).open(&req.binding, true).unwrap();
    let before = first_receipt(&mut h, &req);
    assert_eq!(
        h.recover(&m.mission_id).unwrap_err().category(),
        "command_conflict"
    );
    assert_eq!(first_receipt(&mut h, &req), before);
    assert_eq!(h.snapshot(&m.mission_id).unwrap().pending_commands.len(), 1);
}

#[test]
fn missing_empty_and_corrupt_original_storage_block_without_replacement() {
    for fault in ["missing", "empty", "corrupt"] {
        let w = world();
        let mut h = host(&w, "crash-after-admission");
        let m = create(&mut h, &w);
        let c = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
        assert!(h.dispatch_pending(&m.mission_id).is_err());
        let req = request(&h, &m.mission_id, &c, "submit");
        let db = pi(&mut h).database(&req.binding);
        drop(h);
        // Remove sidecars only in this disposable fixture to model storage loss.
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
        }
        match fault {
            "missing" => std::fs::remove_file(&db).unwrap(),
            "empty" => std::fs::write(&db, []).unwrap(),
            _ => std::fs::write(&db, b"corrupt original storage").unwrap(),
        }
        let mut h = host(&w, "recover");
        assert!(h.recover(&m.mission_id).is_err(), "{fault}");
        assert_eq!(counts(&w), (1, 1, 1));
        assert_eq!(
            h.snapshot(&m.mission_id).unwrap().pending_commands[0].request_id,
            c.request_id
        );
        if fault == "missing" {
            assert!(!db.exists());
        }
        if fault == "corrupt" {
            assert_eq!(std::fs::read(&db).unwrap(), b"corrupt original storage");
        }
    }
}

#[test]
fn incompatible_configuration_and_default_fake_reopen_are_rejected() {
    let w = world();
    let mut h = host(&w, "answer");
    create(&mut h, &w);
    drop(h);
    let fake = Host::open(&w.db)
        .err()
        .expect("fake reopen of a Pi ledger")
        .to_string();
    assert!(fake.contains("adapter:"), "{fake}");
    assert!(fake.contains("pi-probe-v1"), "{fake}");
    Connection::open(&w.db).unwrap().execute("UPDATE attempts SET runtime_config_json=json_set(runtime_config_json,'$.fingerprint','wrong')",[]).unwrap();
    let node = std::env::var_os("SWITCHYARD_PI_NODE").unwrap();
    let drifted = Host::open_pi_probe(&w.db, Path::new(&node))
        .err()
        .expect("drifted fingerprint")
        .to_string();
    assert!(drifted.contains("fingerprint:"), "{drifted}");
    assert!(drifted.contains("found \"wrong\""), "{drifted}");
    assert!(drifted.contains("no adapter replacement"), "{drifted}");
}

#[test]
fn stale_owner_generation_cannot_open_or_submit_to_pi() {
    let w = world();
    let mut h = host(&w, "answer");
    let m = create(&mut h, &w);
    admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
    Connection::open(&w.db)
        .unwrap()
        .execute("UPDATE host_owner SET generation=generation+1", [])
        .unwrap();
    assert_eq!(
        h.dispatch_pending(&m.mission_id).unwrap_err().category(),
        "stale_generation"
    );
    assert_eq!(h.snapshot(&m.mission_id).unwrap().pending_commands.len(), 1);
}

#[test]
fn external_observations_and_stop_claims_do_not_gain_authority() {
    let w = world();
    let mut h = host(&w, "answer");
    let m = create(&mut h, &w);
    admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
    for note in [
        json!({"kind":"plan_completed","plan":"bad","digest":"bad","quiescent":true}),
        json!({"kind":"implementation_settled","quiescent":true}),
        json!({"kind":"verification_pass","contentDigest":"bad","evidence":"bad","requirements":[],"completeness":"complete"}),
        json!({"kind":"publication_observed","contentDigest":"bad","commitSha":"bad","treeSha":"bad","prId":"bad"}),
    ] {
        assert!(h.apply_notification_json(&m.mission_id, &note).is_err());
    }
    assert_eq!(h.snapshot(&m.mission_id).unwrap().phase, Phase::Planning);
    assert_eq!(counts(&w), (1, 1, 1));
}

#[test]
fn protocol_bounds_reject_oversized_input_before_ledger_admission_and_unsafe_numbers_before_pi() {
    let w = world();
    let mut h = host(&w, "answer");
    let m = create(&mut h, &w);
    let c = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
    assert!(
        admit(
            &mut h,
            &m.mission_id,
            CommandBody::Steer {
                content: "x".repeat(70_000)
            }
        )
        .is_err()
    );
    assert_eq!(counts(&w), (1, 1, 1));
    let mut req = request(&h, &m.mission_id, &c, "submit");
    let mut payload = CommandBody::StartPlanning.payload(&m.mission_id, &m.attempt_id);
    payload["body"]["integer"] = json!(9_007_199_254_740_992_u64);
    req.payload_hash = payload_hash(&payload).unwrap();
    assert!(
        PiRuntime::validate_command(&req, &payload)
            .unwrap_err()
            .to_string()
            .contains("safe integer")
    );
    payload["body"]["integer"] = json!(9_007_199_254_740_991_u64);
    req.payload_hash = payload_hash(&payload).unwrap();
    PiRuntime::validate_command(&req, &payload).unwrap();
}

#[test]
fn uncertain_effects_prevent_both_new_dispatch_and_recovered_scheduling() {
    for crash in [false, true] {
        let w = world();
        let mut h = host(
            &w,
            if crash {
                "crash-after-admission"
            } else {
                "answer"
            },
        );
        let m = create(&mut h, &w);
        let c = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
        if crash {
            assert!(h.dispatch_pending(&m.mission_id).is_err());
        }
        h.record_operation(
            &m.mission_id,
            "uncertain-fixture",
            &json!({"operation":"unknown"}),
            OperationState::Uncertain,
        )
        .unwrap();
        drop(h);
        let mut h = host(&w, "recover");
        assert!(
            h.recover(&m.mission_id)
                .unwrap_err()
                .to_string()
                .contains("uncertain effects")
        );
        let req = request(&h, &m.mission_id, &c, "submit");
        let snapshot = pi(&mut h).inspect(&req).unwrap();
        assert_eq!(snapshot["scheduling"], "paused");
        assert_eq!(counts(&w), (1, 1, 1));
        if crash {
            assert!(snapshot["commands"][0]["receipt"].is_object());
            assert_ne!(snapshot["commands"][0]["status"], "done");
        } else {
            assert!(snapshot["commands"][0]["receipt"].is_null());
        }
    }
}

#[test]
fn acknowledged_command_missing_in_pi_blocks_instead_of_resubmitting() {
    let w = world();
    let mut h = host(&w, "answer");
    let m = create(&mut h, &w);
    let c = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
    let req = request(&h, &m.mission_id, &c, "submit");
    pi(&mut h).open(&req.binding, false).unwrap();
    h.store.mark_runtime_initialized(&m.attempt_id).unwrap();
    // Model a contradictory Rust receipt using the real Host store; Pi remains
    // a valid, empty session. Recovery must not repair this by creating work.
    h.store
        .acknowledge(
            &c.command_id,
            &json!({"receiptId":"missing","requestId":c.request_id,
        "payloadHash":c.payload_hash,"adapter":"pi-probe-v1","credentials":"none"})
            .to_string(),
        )
        .unwrap();
    admit(
        &mut h,
        &m.mission_id,
        CommandBody::Continue {
            activity: "planning".into(),
        },
    )
    .unwrap();
    drop(h);
    let mut h = host(&w, "recover");
    assert!(
        h.recover(&m.mission_id)
            .unwrap_err()
            .to_string()
            .contains("acknowledged Pi submission is missing")
    );
    let req = request(&h, &m.mission_id, &c, "submit");
    assert!(first_receipt(&mut h, &req)["receipt"].is_null());
    assert_eq!(counts(&w), (1, 1, 2));
    assert_eq!(h.snapshot(&m.mission_id).unwrap().pending_commands.len(), 1);
}

#[test]
fn runtime_open_before_initialization_marker_is_recovered_without_replacing_session() {
    let w = world();
    let mut h = host(&w, "answer");
    let m = create(&mut h, &w);
    let c = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
    let req = request(&h, &m.mission_id, &c, "submit");
    pi(&mut h).open(&req.binding, false).unwrap();
    assert!(!h.store.runtime_initialized(&m.attempt_id).unwrap());
    drop(h); // lost initialization ack, before any command could be sent
    let mut h = host(&w, "recover");
    let after = h.recover(&m.mission_id).unwrap();
    assert_eq!(after.session_id, m.session_id);
    assert_eq!(after.commands[0].request_id, c.request_id);
    assert!(h.store.runtime_initialized(&m.attempt_id).unwrap());
    assert_eq!(counts(&w), (1, 1, 1));
}

#[test]
fn one_host_dispatches_a_successor_attempt_on_its_own_bridge() {
    let w = world();
    let mut h = host(&w, "answer");
    let first = create(&mut h, &w);
    let start_a = admit(&mut h, &first.mission_id, CommandBody::StartPlanning).unwrap();
    assert_eq!(h.dispatch_pending(&first.mission_id).unwrap(), 1);
    // The ledger still allows only one active mission. A later attempt on this
    // same Host is a new session binding and must not be rejected as a rebind.
    Connection::open(&w.db)
        .unwrap()
        .execute(
            "UPDATE missions SET phase='cancelled' WHERE id=?1",
            [&first.mission_id],
        )
        .unwrap();
    let second = h.create_successor(&first.mission_id, "operator").unwrap();
    let start_b = admit(&mut h, &second.mission_id, CommandBody::StartPlanning).unwrap();
    assert_eq!(h.dispatch_pending(&second.mission_id).unwrap(), 1);
    assert_eq!(h.dispatch_pending(&first.mission_id).unwrap(), 0);
    let snap_a = h.snapshot(&first.mission_id).unwrap();
    let snap_b = h.snapshot(&second.mission_id).unwrap();
    assert_ne!(snap_a.session_id, snap_b.session_id);
    assert_ne!(snap_a.attempt_id, snap_b.attempt_id);
    assert!(snap_b.pending_commands.is_empty());
    assert_eq!(snap_a.commands[0].request_id, start_a.request_id);
    assert_eq!(snap_b.commands[0].request_id, start_b.request_id);
    assert_ne!(snap_a.commands[0].request_id, snap_b.commands[0].request_id);
    // Pi receipt ids are per session, so the same text can appear in both files.
    assert!(snap_a.commands[0].receipt_id.is_some());
    assert!(snap_b.commands[0].receipt_id.is_some());
    let binding_a = h.binding(&h.store.load_mission(&first.mission_id).unwrap());
    let binding_b = h.binding(&h.store.load_mission(&second.mission_id).unwrap());
    let req = request(&h, &first.mission_id, &start_a, "submit");
    assert!(first_receipt(&mut h, &req)["receipt"]["receipt"].is_string());
    let db_a = pi(&mut h).database(&binding_a);
    let db_b = pi(&mut h).database(&binding_b);
    assert!(db_a.is_file());
    assert!(db_b.is_file());
    assert_ne!(db_a, db_b);
    assert_eq!(counts(&w), (2, 2, 2));
}

#[test]
fn open_session_rejects_a_conflicting_binding_without_replacing_the_child() {
    let w = world();
    let mut h = host(&w, "answer");
    let mission = create(&mut h, &w);
    let command = admit(&mut h, &mission.mission_id, CommandBody::StartPlanning).unwrap();
    let req = request(&h, &mission.mission_id, &command, "submit");
    pi(&mut h).open(&req.binding, false).unwrap();
    let mut conflicting = req.binding.clone();
    conflicting.mission_id = "other-mission".into();
    let error = pi(&mut h).open(&conflicting, false).unwrap_err();
    assert_eq!(error.category(), "guard_failed");
    assert!(
        error
            .to_string()
            .contains("already open with a different mission, attempt, or generation"),
        "{error}"
    );
    assert_eq!(h.dispatch_pending(&mission.mission_id).unwrap(), 1);
    assert!(
        h.snapshot(&mission.mission_id)
            .unwrap()
            .pending_commands
            .is_empty()
    );
}

#[test]
fn pi_probe_runtime_mut_is_absent() {
    let w = world();
    let mut h = host(&w, "answer");
    assert!(h.runtime_mut().is_none());
}

#[test]
fn crash_after_admission_replay_keeps_the_fixture_and_does_not_sigkill() {
    let w = world();
    let mut h = host(&w, "crash-after-admission");
    let mission = create(&mut h, &w);
    let command = admit(&mut h, &mission.mission_id, CommandBody::StartPlanning).unwrap();
    assert!(h.dispatch_pending(&mission.mission_id).is_err());
    assert_eq!(
        h.snapshot(&mission.mission_id)
            .unwrap()
            .pending_commands
            .len(),
        1
    );
    drop(h);
    let mut h = host(&w, "crash-after-admission");
    let after = h.recover(&mission.mission_id).unwrap();
    assert!(after.pending_commands.is_empty());
    assert_eq!(after.commands[0].request_id, command.request_id);
    assert!(after.commands[0].receipt_id.is_some());
    let req = request(&h, &mission.mission_id, &command, "submit");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let state = first_receipt(&mut h, &req);
        if state["status"] == "done" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "replay with the crash fixture still armed never resumed: {state}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(counts(&w), (1, 1, 1));
}
