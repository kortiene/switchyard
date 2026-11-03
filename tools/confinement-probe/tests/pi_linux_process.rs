//! Both confinement boundaries and durable process handles; mission setup is
//! FakeRuntime. The harness drives Pi explicitly, not a product runtime driver.
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use switchyard_confinement_probe::{bridge::BridgeSandbox, probe};
use switchyard_core::{
    CommandBody, Host, Phase, PiEffectOutcome, RuntimeBinding, operation_id, payload_hash,
};
#[path = "support/pi_linux.rs"]
mod support;
use support::*;

fn completed(outcome: PiEffectOutcome) -> Value {
    let PiEffectOutcome::Completed(receipt) = outcome else {
        panic!("expected receipt: {outcome:?}")
    };
    receipt
}
fn observation(b: &RuntimeBinding, key: &str, handle: &Value, method: &str) -> Value {
    let payload = json!({"method":method,"startOperationId":handle["startOperationId"],
        "startPayloadHash":handle["startPayloadHash"], "capability":"workspace-process", "deadlinePolicy":"attempt-bound"});
    json!({"operationId":operation_id(&b.attempt_id,&b.session_id,"observer",key).unwrap(),
        "taskId":"observer","effectKey":key,"payloadHash":payload_hash(&payload).unwrap(),"payload":payload})
}
fn wait_file(path: &std::path::Path) {
    let until = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < until, "missing {}", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn confined_process_start_status_cancel_retain_original_receipts() {
    let (world, mut host, sandbox) = World::new();
    host.enable_linux_execution(config(None)).unwrap();
    let b = binding(&host, &world.mission);
    let mut bridge = spawn(&sandbox, b.clone(), "process-cancel", false);
    let mut effects = Vec::new();
    run(&mut bridge, &mut |params| {
        let outcome = host.execute_pi_linux_process(&b, params, now() + 20_000);
        assert!(
            matches!(outcome, PiEffectOutcome::Completed(_)),
            "{outcome:?}: {params}"
        );
        effects.push((params.clone(), outcome.clone()));
        outcome
    });
    assert_eq!(effects.len(), 3);
    let (params, handle) = effects
        .iter()
        .find(|(p, _)| p["payload"]["method"] == "process.start")
        .unwrap();
    let handle = completed(handle.clone());
    assert_eq!(handle["kind"], "process-handle");
    assert_eq!(handle["disposition"], "accepted");
    assert_eq!(
        host.snapshot(&world.mission)
            .unwrap()
            .linux_execution_effects
            .len(),
        3
    );
    let heartbeat = world.workspace.join("process.heartbeat");
    wait_file(&heartbeat);
    let frozen = fs::read(&heartbeat).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(fs::read(&heartbeat).unwrap(), frozen);
    for (p, outcome) in &effects {
        assert_eq!(
            host.execute_pi_linux_process(&b, p, 0),
            *outcome,
            "historical receipt changed"
        );
    }
    let status = observation(&b, "after-cancel", &handle, "process.status");
    let receipt = completed(host.execute_pi_linux_process(&b, &status, now() + 20_000));
    assert_eq!(receipt["startState"], "completed");
    assert_eq!(receipt["quiescence"]["proven"], true);
    assert_eq!(receipt["startResult"]["disposition"], "cancelled");
    assert_eq!(
        fs::read(world.workspace.join("process.marker")).unwrap(),
        b"once\n"
    );
    // Same identity with a different command cannot start another process.
    let mut conflict = params.clone();
    conflict["payload"]["argv"] = json!(["/bin/true"]);
    conflict["payloadHash"] = json!(payload_hash(&conflict["payload"]).unwrap());
    assert_eq!(
        host.execute_pi_linux_process(&b, &conflict, 0),
        PiEffectOutcome::Conflict
    );
    stop(&mut bridge, &sandbox);
    world.assert_origin();
}

#[test]
fn fresh_status_after_host_restart_marks_retained_handle_uncertain() {
    let (world, mut host, sandbox) = World::new();
    host.enable_linux_execution(config(None)).unwrap();
    let b = binding(&host, &world.mission);
    let mut bridge = spawn(&sandbox, b.clone(), "process-wait", false);
    let mut handle = Value::Null;
    run(&mut bridge, &mut |params| {
        let out = host.execute_pi_linux_process(&b, params, now() + 20_000);
        handle = completed(out.clone());
        out
    });
    assert_eq!(handle["kind"], "process-handle");
    let before = observation(&b, "before-restart", &handle, "process.status");
    let original = completed(host.execute_pi_linux_process(&b, &before, now() + 20_000));
    assert_eq!(original["startState"], "running");
    stop(&mut bridge, &sandbox);
    drop(host);

    let mut host = Host::open(&world.db()).unwrap();
    host.enable_linux_execution(config(None)).unwrap();
    let b = binding(&host, &world.mission);
    // The caller retained its handle. Do not call recover or replay start.
    assert_eq!(
        host.execute_pi_linux_process(&b, &before, 0),
        PiEffectOutcome::Completed(original.clone())
    );
    for key in ["after-restart", "later"] {
        let status = observation(&b, key, &handle, "process.status");
        let receipt = completed(host.execute_pi_linux_process(&b, &status, now() + 20_000));
        assert_eq!(receipt["startState"], "uncertain");
        assert_eq!(receipt["startReceiptRetained"], false);
        assert_eq!(receipt["startResult"], Value::Null);
        assert_eq!(receipt["quiescence"]["proven"], true);
    }
    let snapshot = host.snapshot(&world.mission).unwrap();
    assert_eq!(snapshot.phase, Phase::NeedsAttention);
    assert!(snapshot.execution_blocked);
    assert_eq!(
        host.execute_pi_linux_process(&b, &before, 0),
        PiEffectOutcome::Completed(original)
    );
    assert_eq!(
        fs::read(world.workspace.join("process.marker")).unwrap(),
        b"once\n"
    );
    world.assert_origin();
}

#[test]
fn fresh_status_after_host_restart_leaves_live_process_in_cgroup() {
    let (world, host, sandbox) = World::new();
    drop(host);
    let worker = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "pi_linux_process_live_orphan_worker",
            "--nocapture",
        ])
        .env("SW_PI_PROCESS_ROOT", world.dir.path())
        .env("SW_PI_PROCESS_MISSION", &world.mission)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    // Drop kills the cgroup and the stopped supervisor even when an assertion fails.
    let mut live = LiveOrphan {
        worker,
        supervisor: None,
        cgroup: None,
    };
    let stdout = live.worker.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let until = Instant::now() + Duration::from_secs(25);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) if line.contains("AT-live") => break,
            Ok(_) => {}
            Err(error) => panic!("missing live barrier: {error}"),
        }
    }
    let info: Value =
        serde_json::from_slice(&fs::read(world.dir.path().join("live.json")).unwrap()).unwrap();
    let supervisor = u32::try_from(info["supervisor"].as_u64().unwrap()).unwrap();
    let cgroup = PathBuf::from(info["cgroup"].as_str().unwrap());
    let handle = info["handle"].clone();
    let before = info["before"].clone();
    let original = info["receipt"].clone();
    live.supervisor = Some(supervisor);
    live.cgroup = Some(cgroup.clone());
    let heartbeat = world.workspace.join("process.heartbeat");
    wait_file(&heartbeat);
    assert_heartbeat_advances(&heartbeat);
    assert_cgroup_populated(&cgroup);
    // Host death closes the supervisor's control pipe, and that supervisor
    // would then write cgroup.kill. Stop it first so the workload stays in
    // the leaf across the restart. Status must not finish that kill.
    signal(supervisor, "-STOP");
    assert_heartbeat_advances(&heartbeat);
    let namespace = probe::namespace_from_status(&sandbox.status_root).unwrap();
    let members = probe::namespace_members(&namespace).unwrap();
    assert!(!members.is_empty());
    live.worker.kill().unwrap();
    live.worker.wait().unwrap();
    probe::require_quiescent(&namespace, &members).unwrap();
    assert_cgroup_populated(&cgroup);
    assert_heartbeat_advances(&heartbeat);

    let mut host = Host::open(&world.db()).unwrap();
    host.enable_linux_execution(config(None)).unwrap();
    let b = binding(&host, &world.mission);
    assert_eq!(
        host.execute_pi_linux_process(&b, &before, 0),
        PiEffectOutcome::Completed(original.clone())
    );
    assert_eq!(effect_state(&host, &world.mission, &handle), "running");
    assert_heartbeat_advances(&heartbeat);
    for key in ["after-restart", "later"] {
        let status = observation(&b, key, &handle, "process.status");
        let receipt = completed(host.execute_pi_linux_process(&b, &status, now() + 20_000));
        assert_eq!(receipt["startState"], "uncertain");
        assert_eq!(receipt["startReceiptRetained"], false);
        assert_eq!(receipt["startResult"], Value::Null);
        assert_eq!(receipt["quiescence"]["proven"], false);
        assert_cgroup_populated(&cgroup);
        assert_heartbeat_advances(&heartbeat);
    }
    let snapshot = host.snapshot(&world.mission).unwrap();
    assert_eq!(snapshot.phase, Phase::NeedsAttention);
    assert!(snapshot.execution_blocked);
    assert_eq!(
        host.execute_pi_linux_process(&b, &before, 0),
        PiEffectOutcome::Completed(original)
    );
    assert_eq!(effect_state(&host, &world.mission, &handle), "uncertain");
    assert_eq!(
        fs::read(world.workspace.join("process.marker")).unwrap(),
        b"once\n"
    );
    world.assert_origin();
}

struct LiveOrphan {
    worker: std::process::Child,
    supervisor: Option<u32>,
    cgroup: Option<PathBuf>,
}

impl Drop for LiveOrphan {
    fn drop(&mut self) {
        if let Some(path) = &self.cgroup {
            let _ = fs::write(path.join("cgroup.kill"), b"1\n");
            let until = Instant::now() + Duration::from_secs(2);
            while Instant::now() < until {
                let events = fs::read_to_string(path.join("cgroup.events")).unwrap_or_default();
                if events.lines().any(|line| line == "populated 0") {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        if let Some(pid) = self.supervisor {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
        let _ = self.worker.kill();
        let _ = self.worker.wait();
    }
}

fn signal(pid: u32, flag: &str) {
    let status = Command::new("kill")
        .args([flag, &pid.to_string()])
        .status()
        .unwrap_or_else(|error| panic!("spawn kill {flag} {pid}: {error}"));
    assert!(status.success(), "kill {flag} {pid} failed: {status}");
}

fn assert_heartbeat_advances(path: &std::path::Path) {
    let before = fs::metadata(path).unwrap().len();
    let until = Instant::now() + Duration::from_secs(2);
    while fs::metadata(path).unwrap().len() <= before {
        assert!(
            Instant::now() < until,
            "process heartbeat stopped at {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn assert_cgroup_populated(path: &std::path::Path) {
    let events = fs::read_to_string(path.join("cgroup.events"))
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert!(
        events.lines().any(|line| line == "populated 1"),
        "cgroup {} is not populated:\n{events}",
        path.display()
    );
}

fn effect_state(host: &Host, mission: &str, handle: &Value) -> String {
    let id = handle["startOperationId"].as_str().unwrap();
    host.snapshot(mission)
        .unwrap()
        .linux_execution_effects
        .into_iter()
        .find(|effect| effect.operation_id == id)
        .unwrap()
        .state
}

#[test]
fn process_output_and_nonzero_exit_are_durable_status_evidence() {
    let (world, mut host, sandbox) = World::new();
    host.enable_linux_execution(config(None)).unwrap();
    let b = binding(&host, &world.mission);
    let mut bridge = spawn(&sandbox, b.clone(), "process-fast", false);
    let mut handle = Value::Null;
    run(&mut bridge, &mut |params| {
        let out = host.execute_pi_linux_process(&b, params, now() + 20_000);
        handle = completed(out.clone());
        out
    });
    let until = Instant::now() + Duration::from_secs(10);
    let mut sequence = 0;
    let (params, receipt) = loop {
        sequence += 1;
        let params = observation(&b, &format!("status-{sequence}"), &handle, "process.status");
        let receipt = completed(host.execute_pi_linux_process(&b, &params, now() + 20_000));
        if receipt["startState"] == "completed" {
            break (params, receipt);
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(receipt["startResult"]["exitCode"], 7);
    assert_eq!(receipt["startResult"]["stdout"], "process-output");
    assert_eq!(receipt["quiescence"]["proven"], true);
    assert_eq!(
        host.execute_pi_linux_process(&b, &params, 0),
        PiEffectOutcome::Completed(receipt)
    );
    stop(&mut bridge, &sandbox);
    world.assert_origin();
}

#[test]
fn admitted_pause_cancel_stop_forwarded_descendants_past_workspace_blocks() {
    for cancel in [false, true] {
        let (world, mut host, sandbox) = World::new();
        host.enable_linux_execution(config(None)).unwrap();
        let b = binding(&host, &world.mission);
        let mut bridge = spawn(&sandbox, b.clone(), "process-hold", false);
        let mut request = None;
        let mut effect = |params: &Value| {
            let outcome = host.execute_pi_linux_process(&b, params, now() + 20_000);
            assert_eq!(completed(outcome.clone())["kind"], "process-handle");
            request = Some(params.clone());
            outcome
        };
        bridge
            .call_with_effects(
                "runtime.submit",
                Some("process"),
                json!({"type":"input","content":"run"}),
                &mut effect,
            )
            .unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        // Draining inspect processes the queued start request. The Pi tool stays
        // pending cooperatively after receiving the handle until runtime.abort.
        loop {
            bridge
                .call_with_effects(
                    "runtime.inspect",
                    None,
                    json!({"commandIds":["process"]}),
                    &mut effect,
                )
                .unwrap();
            if world.workspace.join("process.heartbeat").exists() {
                break;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(20));
        }
        let params = request.unwrap();
        let before = host.snapshot(&world.mission).unwrap().to_json()["workspace"].clone();
        admit(
            &mut host,
            &world.mission,
            if cancel {
                CommandBody::Cancel
            } else {
                CommandBody::Pause
            },
        );
        // A live Linux writer itself blocks workspace readiness. Dispatch must
        // still deliver the admitted stop and kill all descendant writers.
        assert_eq!(
            host.dispatch_pending(&world.mission)
                .unwrap_err()
                .category(),
            "execution_blocked"
        );
        let snapshot = host.snapshot(&world.mission).unwrap();
        assert_eq!(snapshot.to_json()["workspace"], before);
        assert!(snapshot.execution_blocked);
        assert_eq!(
            snapshot.phase,
            if cancel {
                Phase::Cancelling
            } else {
                Phase::Pausing
            }
        );
        assert!(
            snapshot
                .linux_execution_effects
                .iter()
                .all(|e| e.quiescence == "populated 0")
        );
        let stop_command = snapshot
            .commands
            .iter()
            .find(|c| c.kind == if cancel { "cancel" } else { "pause" })
            .unwrap();
        assert_eq!(stop_command.state, "acknowledged");
        // The explicit bridge driver composes the same operator identity with
        // real Pi abort; product Host/Pi/Linux driving remains separate work.
        let aborted = bridge
            .call_with_effects(
                "runtime.abort",
                Some(&stop_command.request_id),
                json!({}),
                &mut |p| host.execute_pi_linux_process(&b, p, now() + 20_000),
            )
            .unwrap();
        assert_eq!(aborted["runtimeSettled"], true);
        let frozen = fs::read(world.workspace.join("process.heartbeat")).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            fs::read(world.workspace.join("process.heartbeat")).unwrap(),
            frozen
        );
        assert_eq!(
            host.execute_pi_linux_process(&b, &params, 0),
            PiEffectOutcome::Uncertain
        );
        assert_eq!(
            fs::read(world.workspace.join("process.marker")).unwrap(),
            b"once\n"
        );
        stop(&mut bridge, &sandbox);
        world.assert_origin();
    }
}

#[test]
fn host_sigkill_preserves_process_uncertainty_without_relaunch() {
    for stage in ["prepared", "running", "result", "receipt", "completed"] {
        crash_replay(stage);
    }
}

fn crash_replay(stage: &str) {
    let (world, host, sandbox) = World::new();
    drop(host);
    let mut worker = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "pi_linux_process_crash_worker", "--nocapture"])
        .env("SW_PI_PROCESS_ROOT", world.dir.path())
        .env("SW_PI_PROCESS_MISSION", &world.mission)
        .env("SW_PI_PROCESS_STAGE", stage)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = worker.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let until = Instant::now() + Duration::from_secs(25);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) if line.contains(&format!("AT-{stage}")) => break,
            Ok(_) => {}
            Err(error) => {
                let _ = worker.kill();
                let _ = worker.wait();
                panic!("missing {stage} barrier: {error}");
            }
        }
    }
    let namespace = probe::namespace_from_status(&sandbox.status_root).unwrap();
    let members = probe::namespace_members(&namespace).unwrap();
    assert!(!members.is_empty());
    worker.kill().unwrap();
    worker.wait().unwrap();
    probe::require_quiescent(&namespace, &members).unwrap();
    let params: Value =
        serde_json::from_slice(&fs::read(world.dir.path().join("request.json")).unwrap()).unwrap();
    let conn = rusqlite::Connection::open_with_flags(
        world.db(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let (receipt, frozen): (Option<String>, String) = conn
        .query_row(
            "SELECT receipt_json, request_json FROM linux_execution_effects WHERE operation_id=?1",
            [params["operationId"].as_str().unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    drop(conn);
    let expected = match stage {
        "prepared" => {
            assert!(receipt.is_none());
            assert!(!world.workspace.join("process.marker").exists());
            PiEffectOutcome::Rejected
        }
        "running" | "result" => {
            assert!(receipt.is_none());
            PiEffectOutcome::Uncertain
        }
        _ => {
            let r: Value = serde_json::from_str(&receipt.unwrap()).unwrap();
            PiEffectOutcome::Completed(
                json!({"kind":"process-handle", "operationId":r["operationId"],
                "payloadHash":r["payloadHash"],"backend":r["backend"],"simulated":false,"disposition":"accepted",
                "startOperationId":r["operationId"],"startPayloadHash":r["payloadHash"]}),
            )
        }
    };
    if stage != "prepared" {
        assert_eq!(
            fs::read(world.workspace.join("process.marker")).unwrap(),
            b"once\n"
        );
    }
    let mut verifier = Host::open(&world.db()).unwrap();
    verifier.enable_linux_execution(config(None)).unwrap();
    let _ = verifier.recover(&world.mission);
    assert!(
        verifier
            .snapshot(&world.mission)
            .unwrap()
            .linux_execution_effects
            .iter()
            .all(|e| e.quiescence == "populated 0")
    );
    drop(verifier);
    if stage == "running" {
        wait_file(&world.workspace.join("process.heartbeat"));
        let stopped = fs::read(world.workspace.join("process.heartbeat")).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            fs::read(world.workspace.join("process.heartbeat")).unwrap(),
            stopped
        );
    }
    // Any blind repeat would append to this independent marker. Its inode and
    // timestamp also stay unchanged across both direct and actual Pi replay.
    fs::write(
        world.workspace.join("process.marker"),
        "replay must not overwrite",
    )
    .unwrap();
    let before = fs::metadata(world.workspace.join("process.marker")).unwrap();
    let mut host = Host::open(&world.db()).unwrap();
    host.enable_linux_execution(config(None)).unwrap();
    let current = binding(&host, &world.mission);
    let mut stale = current.clone();
    stale.generation -= 1;
    assert_eq!(
        host.execute_pi_linux_process(&stale, &params, now() + 20_000),
        PiEffectOutcome::Rejected
    );
    let mut bridge = spawn(&sandbox, current.clone(), "recover", true);
    let mut replayed = 0;
    run(&mut bridge, &mut |effect| {
        assert_eq!(
            *effect, params,
            "Pi must preserve the task, slot, and wire payload"
        );
        let outcome = host.execute_pi_linux_process(&current, effect, now() + 20_000);
        assert_eq!(outcome, expected);
        replayed += 1;
        outcome
    });
    assert_eq!(replayed, 1);
    assert_eq!(
        host.execute_pi_linux_process(&current, &params, 0),
        expected
    );
    let after = fs::metadata(world.workspace.join("process.marker")).unwrap();
    assert_eq!(
        fs::read(world.workspace.join("process.marker")).unwrap(),
        b"replay must not overwrite"
    );
    assert_eq!(
        (before.ino(), before.mtime(), before.mtime_nsec()),
        (after.ino(), after.mtime(), after.mtime_nsec())
    );
    let snap = host.snapshot(&world.mission).unwrap();
    assert_eq!(snap.linux_execution_effects.len(), 1);
    assert_eq!(
        snap.execution_blocked,
        matches!(stage, "running" | "result")
    );
    if matches!(stage, "running" | "result") {
        assert_eq!(snap.phase, Phase::NeedsAttention);
    }
    let conn = rusqlite::Connection::open_with_flags(
        world.db(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let after: String = conn
        .query_row(
            "SELECT request_json FROM linux_execution_effects",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        after, frozen,
        "replay must not extend the deadline or change intent"
    );
    stop(&mut bridge, &sandbox);
    world.assert_origin();
    println!(
        "Pi/Linux crash boundary {stage}: original process outcome retained; no repeated command"
    );
}

#[test]
fn pi_linux_process_crash_worker() {
    let Some(root) = std::env::var_os("SW_PI_PROCESS_ROOT").map(PathBuf::from) else {
        return;
    };
    let mission = std::env::var("SW_PI_PROCESS_MISSION").unwrap();
    let stage = std::env::var("SW_PI_PROCESS_STAGE").unwrap();
    let mut host = Host::open(&root.join("mission.sqlite")).unwrap();
    host.enable_linux_execution(config(Some(&stage))).unwrap();
    let binding = binding(&host, &mission);
    let sandbox = BridgeSandbox {
        launcher: root.join("bridge/launch"),
        database: root.join("bridge/state/session.sqlite"),
        status_root: root.join("bridge/supervisor"),
    };
    let mut bridge = spawn(
        &sandbox,
        binding.clone(),
        if stage == "running" {
            "process-wait"
        } else {
            "process-fast"
        },
        false,
    );
    run(&mut bridge, &mut |params| {
        fs::write(
            root.join("request.json"),
            serde_json::to_vec(params).unwrap(),
        )
        .unwrap();
        host.execute_pi_linux_process(&binding, params, now() + 20_000)
    });
    panic!("crash worker passed its mandatory barrier");
}

#[test]
fn pi_linux_process_live_orphan_worker() {
    let Some(root) = std::env::var_os("SW_PI_PROCESS_ROOT").map(PathBuf::from) else {
        return;
    };
    let mission = std::env::var("SW_PI_PROCESS_MISSION").unwrap();
    let mut host = Host::open(&root.join("mission.sqlite")).unwrap();
    host.enable_linux_execution(config(None)).unwrap();
    let binding = binding(&host, &mission);
    let sandbox = BridgeSandbox {
        launcher: root.join("bridge/launch"),
        database: root.join("bridge/state/session.sqlite"),
        status_root: root.join("bridge/supervisor"),
    };
    let mut bridge = spawn(&sandbox, binding.clone(), "process-wait", false);
    let mut handle = Value::Null;
    run(&mut bridge, &mut |params| {
        let out = host.execute_pi_linux_process(&binding, params, now() + 20_000);
        handle = completed(out.clone());
        out
    });
    assert_eq!(handle["kind"], "process-handle");
    let before = observation(&binding, "before-restart", &handle, "process.status");
    let original = completed(host.execute_pi_linux_process(&binding, &before, now() + 20_000));
    assert_eq!(original["startState"], "running");
    let start_id = handle["startOperationId"].as_str().unwrap();
    let conn = rusqlite::Connection::open_with_flags(
        root.join("mission.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let (pid, cgroup): (i64, String) = conn
        .query_row(
            "SELECT launcher_pid, cgroup_path FROM linux_execution_effects WHERE operation_id=?1",
            [start_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    drop(conn);
    fs::write(
        root.join("live.json"),
        serde_json::to_vec(&json!({
            "supervisor": pid,
            "cgroup": cgroup,
            "handle": handle,
            "before": before,
            "receipt": original,
        }))
        .unwrap(),
    )
    .unwrap();
    println!("AT-live");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    // Stay alive so Drop does not kill the cgroup. The parent SIGKILLs this process.
    loop {
        std::thread::sleep(Duration::from_secs(30));
    }
}
