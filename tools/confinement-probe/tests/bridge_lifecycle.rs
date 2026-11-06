//! S3-B4 public lifecycle API against the real source-built Pi bridge.
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use switchyard_core::{ConfinedPiConfig, ConfinedPiSession, PiEffectOutcome, RuntimeBinding};

#[path = "bridge_lifecycle/retention.rs"]
mod retention;

fn private_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    root
}

fn path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} is required; no skip"))
}

fn config(root: &Path) -> ConfinedPiConfig {
    ConfinedPiConfig {
        root: root.into(),
        cgroup_root: path("SWITCHYARD_CGROUP_ROOT"),
        node: path("SWITCHYARD_PI_NODE"),
        bubblewrap: path("SWITCHYARD_BWRAP"),
        launcher: path("SWITCHYARD_BRIDGE_LAUNCHER"),
        policy: path("SWITCHYARD_BRIDGE_POLICY"),
        model_endpoint: None,
    }
}

fn binding(generation: u64) -> RuntimeBinding {
    RuntimeBinding {
        mission_id: "lifecycle-m".into(),
        attempt_id: "lifecycle-a".into(),
        session_id: "lifecycle-s".into(),
        generation,
    }
}

fn call(session: &mut ConfinedPiSession, method: &str, id: Option<&str>, params: Value) -> Value {
    session
        .call(&session.binding().clone(), method, id, params, &mut |_| {
            panic!("credential-free lifecycle fixture must not execute workspace effects")
        })
        .unwrap_or_else(|e| panic!("{method}: {e}; {}", session.stderr_tail()))
}

fn submit(session: &mut ConfinedPiSession) -> Value {
    call(
        session,
        "runtime.submit",
        Some("original"),
        json!({"type":"input","content":"hello"}),
    )
}

fn idle(session: &mut ConfinedPiSession) {
    let until = Instant::now() + Duration::from_secs(5);
    while call(
        session,
        "runtime.inspect",
        None,
        json!({"commandIds":["original"]}),
    )["runtimeIdle"]
        != true
    {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn leaf(record: &Value) -> PathBuf {
    PathBuf::from(record["cgroup"]["path"].as_str().unwrap())
}

fn retain(name: &str, value: &Value) {
    fs::write(
        path("SWITCHYARD_CONFINEMENT_EVIDENCE").join(name),
        serde_json::to_vec_pretty(value).unwrap(),
    )
    .unwrap();
}

#[test]
fn confined_lifecycle_replays_receipt_with_limits_and_generation_fences() {
    let tmp = private_root();
    let config = config(tmp.path());
    let mut first = ConfinedPiSession::open(&config, binding(1), "answer").unwrap();
    let record = first.lifecycle();
    let leaf = leaf(&record);
    for (file, expected) in [
        ("memory.max", "268435456"),
        ("memory.swap.max", "0"),
        ("pids.max", "64"),
        ("cpu.max", "50000 100000"),
    ] {
        assert_eq!(
            fs::read_to_string(leaf.join(file)).unwrap().trim(),
            expected
        );
    }
    assert!(ConfinedPiSession::open(&config, binding(2), "answer").is_err());
    assert!(
        first
            .call(
                &binding(2),
                "runtime.submit",
                Some("stale"),
                json!({}),
                &mut |_| PiEffectOutcome::Rejected
            )
            .is_err()
    );
    let receipt = submit(&mut first);
    idle(&mut first);
    let stopped = first.stop().unwrap();
    assert_eq!(stopped["proven"], true);
    drop(first);
    assert!(ConfinedPiSession::open(&config, binding(1), "answer").is_err());
    let mut second = ConfinedPiSession::open(&config, binding(2), "recover").unwrap();
    let snapshot = call(
        &mut second,
        "runtime.inspect",
        None,
        json!({"commandIds":["original"]}),
    );
    assert_eq!(snapshot["scheduling"], "paused");
    assert_eq!(submit(&mut second), receipt);
    assert_ne!(
        second.lifecycle()["cgroup"]["path"],
        record["cgroup"]["path"]
    );
    assert_eq!(second.lifecycle()["recovered"]["proven"], true);
    retain(
        "bridge-lifecycle-replay.json",
        &json!({"initial":record,"stopped":stopped,"reopened":second.lifecycle(),"receipt":receipt}),
    );
}

#[test]
fn confined_lifecycle_kills_hung_bridge_before_inert_reopen() {
    let tmp = private_root();
    let config = config(tmp.path());
    let mut first = ConfinedPiSession::open(&config, binding(1), "ignore-abort").unwrap();
    submit(&mut first);
    let until = Instant::now() + Duration::from_secs(5);
    while !first.stderr_tail().contains("waiting tool entered") {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    let result = first.call(
        &binding(1),
        "runtime.abort",
        Some("stop"),
        json!({}),
        &mut |_| PiEffectOutcome::Rejected,
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("cancellation_timeout")
    );
    assert_eq!(first.lifecycle()["termination"]["proven"], true);
    drop(first);
    let mut recovered = ConfinedPiSession::open(&config, binding(2), "ignore-abort").unwrap();
    let before = call(
        &mut recovered,
        "runtime.inspect",
        None,
        json!({"commandIds":["stop"]}),
    );
    assert_eq!(before["scheduling"], "paused");
    let stop = call(&mut recovered, "runtime.abort", Some("stop"), json!({}));
    assert_eq!(stop["runtimeSettled"], true);
    assert!(!recovered.stderr_tail().contains("waiting tool entered"));
    retain(
        "bridge-lifecycle-hung.json",
        &json!({"before":before,"stop":stop,"lifecycle":recovered.lifecycle()}),
    );
}

#[test]
fn confined_lifecycle_host_loss_recovers_original_session() {
    let tmp = private_root();
    let mut worker = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "bridge_lifecycle_crash_worker", "--nocapture"])
        .env("SWITCHYARD_LIFECYCLE_WORKER", tmp.path())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = worker.stdout.take().unwrap();
    let (send, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let line = line.unwrap();
            if let Some(value) = line.strip_prefix("LIFECYCLE-READY ") {
                let _ = send.send(serde_json::from_str::<Value>(value).unwrap());
                break;
            }
        }
    });
    let evidence = match wait.recv_timeout(Duration::from_secs(15)) {
        Ok(value) => value,
        Err(error) => {
            let _ = worker.kill();
            let _ = worker.wait();
            panic!("{error}");
        }
    };
    worker.kill().unwrap();
    worker.wait().unwrap();
    let mut recovered =
        ConfinedPiSession::open(&config(tmp.path()), binding(2), "recover").unwrap();
    assert_eq!(submit(&mut recovered), evidence["receipt"]);
    // Recovery now durably retires the obsolete leaf; its fresh empty proof
    // and exact identity remain in the host-only retirement receipt.
    retention::assert_retired(tmp.path(), &evidence["record"]);
    assert_eq!(recovered.lifecycle()["recovered"]["proven"], true);
    retain(
        "bridge-lifecycle-host-loss.json",
        &json!({"before":evidence,"after":recovered.lifecycle()}),
    );
}

#[test]
fn bridge_lifecycle_crash_worker() {
    let Some(root) = std::env::var_os("SWITCHYARD_LIFECYCLE_WORKER") else {
        return;
    };
    let mut session =
        ConfinedPiSession::open(&config(Path::new(&root)), binding(1), "answer").unwrap();
    let receipt = submit(&mut session);
    idle(&mut session);
    println!(
        "LIFECYCLE-READY {}",
        json!({"receipt":receipt,"record":session.lifecycle()})
    );
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn confined_lifecycle_rejects_missing_storage_and_replaced_cgroup() {
    let tmp = private_root();
    let config = config(tmp.path());
    let mut session = ConfinedPiSession::open(&config, binding(1), "answer").unwrap();
    session.stop().unwrap();
    drop(session);
    let dir = fs::read_dir(tmp.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let path = dir.join("current.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    // Simulate another owner dying during preparation of a recovered session.
    record["phase"] = json!("prepared");
    assert_eq!(record["requiresExisting"], true);
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let database = dir.join("state/session.sqlite");
    fs::rename(&database, dir.join("retained.sqlite")).unwrap();
    assert!(ConfinedPiSession::open(&config, binding(2), "recover").is_err());
    fs::rename(dir.join("retained.sqlite"), &database).unwrap();
    record["cgroup"]["ino"] = json!(record["cgroup"]["ino"].as_u64().unwrap() + 1);
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(ConfinedPiSession::open(&config, binding(2), "recover").is_err());
}

#[test]
fn confined_launcher_cannot_self_join_or_launch_without_release() {
    for (release, parent) in [
        (false, std::process::id()),
        (true, std::process::id()),
        (false, 0),
    ] {
        let tmp = private_root();
        fs::write(tmp.path().join("cgroup.procs"), "").unwrap();
        let marker = tmp.path().join("must-not-exist");
        let mut child = Command::new(path("SWITCHYARD_BRIDGE_LAUNCHER"))
            .arg(parent.to_string())
            .arg(tmp.path())
            .args(["--", "/usr/bin/touch"])
            .arg(&marker)
            .stderr(Stdio::piped())
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        if release {
            child.stdin.as_mut().unwrap().write_all(b"GO\n").unwrap();
        }
        drop(child.stdin.take());
        let result = child.wait_with_output().unwrap();
        assert!(!result.status.success());
        if parent == 0 {
            assert!(String::from_utf8_lossy(&result.stderr).contains("owner died"));
        }
        assert!(!marker.exists());
        assert_eq!(
            fs::read_to_string(tmp.path().join("cgroup.procs")).unwrap(),
            ""
        );
    }
}

#[test]
fn confined_lifecycle_wall_deadline_kills_idle_bridge() {
    let tmp = private_root();
    let mut session = ConfinedPiSession::open(&config(tmp.path()), binding(1), "answer").unwrap();
    let leaf = leaf(&session.lifecycle());
    assert!(
        fs::read_to_string(leaf.join("cgroup.events"))
            .unwrap()
            .contains("populated 1")
    );
    let until = Instant::now() + Duration::from_secs(65);
    while !fs::read_to_string(leaf.join("cgroup.events"))
        .unwrap()
        .contains("populated 0")
    {
        assert!(
            Instant::now() < until,
            "wall deadline did not stop the bridge"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(session.stop().unwrap()["proven"], true);
}

#[test]
fn confined_lifecycle_invalid_policy_stops_before_sqlite() {
    let tmp = private_root();
    let install = tempfile::tempdir().unwrap();
    let mut config = config(tmp.path());
    config.policy = install.path().join("invalid.so");
    fs::write(&config.policy, "not a shared library").unwrap();
    let error = ConfinedPiSession::open(&config, binding(1), "answer")
        .err()
        .expect("an invalid policy must not start Pi");
    assert!(
        error
            .to_string()
            .contains("bridge lockdown guard rejected startup"),
        "{error}"
    );
    let directory = fs::read_dir(tmp.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(!directory.join("state/session.sqlite").exists());
    let record: Value =
        serde_json::from_slice(&fs::read(directory.join("current.json")).unwrap()).unwrap();
    assert_eq!(record["phase"], "stopped");
    assert_eq!(record["requiresExisting"], false);
    assert_eq!(record["termination"]["proven"], true);
    config.policy = path("SWITCHYARD_BRIDGE_POLICY");
    let planted = directory.join("state/session.sqlite");
    fs::write(&planted, "partial").unwrap();
    fs::set_permissions(&planted, fs::Permissions::from_mode(0o600)).unwrap();
    let refused = ConfinedPiSession::open(&config, binding(2), "answer")
        .err()
        .expect("a different runtime must not adopt bridge state");
    assert!(
        refused
            .to_string()
            .contains("incompatible or stale confined bridge generation"),
        "{refused}"
    );
    fs::remove_file(&planted).unwrap();
    let mut reopened =
        ConfinedPiSession::open(&config, binding(2), "answer").unwrap_or_else(|error| {
            panic!("proven stop before open must allow a new generation: {error}")
        });
    assert_eq!(reopened.lifecycle()["phase"], "opened");
    assert!(directory.join("state/session.sqlite").exists());
    let recovered: Value =
        serde_json::from_slice(&fs::read(directory.join("recovered-1.json")).unwrap()).unwrap();
    assert_eq!(recovered["replacedConfiguration"], record["configuration"]);
    assert_ne!(
        reopened.lifecycle()["configuration"],
        record["configuration"]
    );
    let receipt = submit(&mut reopened);
    idle(&mut reopened);
    retain(
        "bridge-lifecycle-unopened-stop.json",
        &json!({"rejected": record, "reopened": reopened.lifecycle(), "receipt": receipt}),
    );
    assert_eq!(reopened.stop().unwrap()["proven"], true);
}

#[test]
fn confined_lifecycle_opened_stop_still_requires_original_database() {
    let tmp = private_root();
    let config = config(tmp.path());
    let mut session = ConfinedPiSession::open(&config, binding(1), "answer").unwrap();
    let stopped = session.stop().unwrap();
    assert_eq!(stopped["proven"], true);
    assert_eq!(session.stop().unwrap(), stopped);
    drop(session);
    let directory = fs::read_dir(tmp.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let record: Value =
        serde_json::from_slice(&fs::read(directory.join("current.json")).unwrap()).unwrap();
    assert_eq!(record["phase"], "stopped");
    assert_eq!(record["requiresExisting"], true);
    assert_eq!(record["termination"], stopped);
    let database = directory.join("state/session.sqlite");
    fs::rename(&database, directory.join("state/retained.sqlite")).unwrap();
    let error = ConfinedPiSession::open(&config, binding(2), "recover")
        .err()
        .expect("an opened session must keep its original database");
    assert!(
        error
            .to_string()
            .contains("original Pi database is missing"),
        "{error}"
    );
    fs::rename(directory.join("state/retained.sqlite"), &database).unwrap();
    let reopened = ConfinedPiSession::open(&config, binding(2), "recover").unwrap();
    assert_eq!(reopened.lifecycle()["phase"], "opened");
    assert_eq!(reopened.lifecycle()["requiresExisting"], true);
}

#[test]
fn confined_lifecycle_open_on_finished_thread_still_submits() {
    let tmp = private_root();
    let config = config(tmp.path());
    let binding = binding(1);
    // The spawning thread exits before submit. Parent-death SIGKILL follows
    // that thread, so this fails if open forks the launcher on the caller.
    let mut session =
        std::thread::spawn(move || ConfinedPiSession::open(&config, binding, "answer"))
            .join()
            .expect("open thread panicked")
            .unwrap_or_else(|error| panic!("open on a finished thread: {error}"));
    // Delivery of the parent-death signal follows thread exit. Wait so a
    // caller-thread fork is already dead before submit, instead of racing it.
    std::thread::sleep(Duration::from_millis(200));
    submit(&mut session);
    idle(&mut session);
    assert_eq!(session.stop().unwrap()["proven"], true);
}
