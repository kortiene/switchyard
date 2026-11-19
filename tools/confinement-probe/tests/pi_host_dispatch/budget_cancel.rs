//! SIGINT and SIGKILL on the confined path. Both settle through the existing
//! `stop` proof (`rekill_and_prove`, then reconcile and `ops_blocked`).
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;
use switchyard_core::{Host, Phase};

use super::{World, bridge, linux};

static OPERATOR_STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop(_: libc::c_int) {
    OPERATOR_STOP.store(true, Ordering::SeqCst);
}

fn arm_stop() {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_stop as *const () as usize;
        action.sa_flags = 0;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
    }
}

fn worker_host(root: &std::path::Path, fixture: &str) -> Host {
    Host::open_confined_pi(
        &root.join("mission.sqlite"),
        bridge(&root.join("bridge")),
        linux(None),
        fixture,
    )
    .unwrap()
}

fn wait_line(child: &mut std::process::Child, needle: &str) {
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) if line.contains(needle) => return,
            Ok(_) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("missing {needle}: {error}");
            }
        }
    }
}

fn confirm_receipt(db: &std::path::Path, mission: &str) -> Value {
    let export = Host::export_document(db, mission, "s2-2").unwrap();
    export["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["kind"] == "confirm_cancelled")
        .unwrap()["receipt"]
        .clone()
}

#[test]
fn pre_budget_ledger_cancels_with_a_cgroup_proof() {
    let (world, mut host) = World::new("answer");
    let conn = rusqlite::Connection::open(world.db()).unwrap();
    let policy: String = conn
        .query_row(
            "SELECT policy_json FROM attempts WHERE active = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut policy: Value = serde_json::from_str(&policy).unwrap();
    for key in [
        "attemptWallSeconds",
        "bridgeCpuSeconds",
        "bridgeWallSeconds",
        "maxModelRequests",
        "maxRequestBytes",
        "maxResponseBytes",
    ] {
        policy.as_object_mut().unwrap().remove(key);
    }
    conn.execute(
        "UPDATE attempts SET policy_json = ?1 WHERE active = 1",
        [policy.to_string()],
    )
    .unwrap();
    drop(conn);
    let frozen = host.canonical_attempt_freeze(&world.mission).unwrap();
    assert!(
        !frozen.contains("maxModelRequests"),
        "an old ledger freeze has no budget keys: {frozen}"
    );
    rusqlite::Connection::open(world.db())
        .unwrap()
        .execute(
            "UPDATE attempts SET frozen_inputs_json = ?1 WHERE active = 1",
            [frozen],
        )
        .unwrap();
    let settled = host.interrupt_operator(&world.mission).unwrap();
    assert_eq!(settled.phase, Phase::Cancelled);
    let receipt = confirm_receipt(&world.db(), &world.mission);
    assert_eq!(receipt["proven"], true, "{receipt}");
    assert!(receipt["cgroup"].is_object(), "{receipt}");
}

/// A quarantined writer whose own leaf does not prove empty blocks
/// `confirm_cancelled`. The mission bridge may still stop; that is not
/// evidence this cgroup is empty.
#[test]
fn unproven_writer_leaf_blocks_confirm_cancelled() {
    let (world, mut host) = World::new("answer");
    let snap = host.snapshot(&world.mission).unwrap();
    let root = std::path::PathBuf::from(std::env::var("SWITCHYARD_CGROUP_ROOT").unwrap());
    let leaf = root.join(format!("unproven-writer-{}", std::process::id()));
    std::fs::create_dir_all(&leaf).unwrap();
    let meta = std::fs::metadata(&leaf).unwrap();
    use std::os::unix::fs::MetadataExt;
    let request = serde_json::json!({
        "operation": {"method": "exec_start", "argv": ["/bin/true"]},
        "scope": "linux.exec.start.v1",
        "deadlineMs": 1,
        "toolTaskId": "unproven",
        "effectKey": "unproven-start"
    });
    rusqlite::Connection::open(world.db())
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, cgroup_path, cgroup_dev, cgroup_ino, boot_id,
                entered_at, created_at
             ) VALUES ('unproven-writer',?1,?2,?3,'sha256:unproven',?4,?5,'uncertain',?6,?7,?8,'not-this-boot',1,?9)",
            rusqlite::params![
                snap.mission_id,
                snap.attempt_id,
                snap.session_id,
                request.to_string(),
                host.generation(),
                leaf.to_str().unwrap(),
                i64::try_from(meta.dev()).unwrap(),
                i64::try_from(meta.ino()).unwrap(),
                1_i64,
            ],
        )
        .unwrap();
    let settled = host.interrupt_operator(&world.mission).unwrap();
    assert_eq!(settled.phase, Phase::Cancelling);
    let value = settled.to_json();
    let row = value["linuxExecutionEffects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["operationId"] == "unproven-writer")
        .expect("quarantined writer");
    assert_eq!(row["state"], "uncertain", "{row}");
    assert_eq!(row["receiptRetained"], false, "{row}");
    assert!(
        value["commands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|command| command["kind"] != "confirm_cancelled"),
        "{value}"
    );
    let _ = std::fs::remove_dir_all(&leaf);
}

#[test]
fn sigint_worker() {
    let Ok(root) = std::env::var("SW_BUDGET_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let mission = std::env::var("SW_BUDGET_MISSION").unwrap();
    let mut host = worker_host(&root, "process-hold");
    host.dispatch_pending(&mission).unwrap();
    println!("AT-waiting");
    let _ = std::io::stdout().flush();
    arm_stop();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !OPERATOR_STOP.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "SIGINT did not arrive");
        std::thread::sleep(Duration::from_millis(20));
    }
    let settled = host.interrupt_operator(&mission).unwrap();
    assert_eq!(settled.phase, Phase::Cancelled);
}

#[test]
fn sigint_during_implementation_reaches_cancelled_with_cgroup_proof() {
    let (world, host) = World::new("process-hold");
    drop(host);
    let mut worker = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "budget_cancel::sigint_worker", "--nocapture"])
        .env("SW_BUDGET_ROOT", world.dir.path())
        .env("SW_BUDGET_MISSION", &world.mission)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    wait_line(&mut worker, "AT-waiting");
    nix_signal(worker.id(), "INT");
    let status = worker.wait().unwrap();
    assert!(status.success(), "{status}");
    let inspected = Host::inspect(&world.db(), Some(&world.mission)).unwrap();
    assert_eq!(inspected[0].phase, Phase::Cancelled);
    let receipt = confirm_receipt(&world.db(), &world.mission);
    assert_eq!(receipt["proven"], true, "{receipt}");
    assert!(receipt["cgroup"].is_object(), "{receipt}");
    assert!(
        receipt["events"]
            .as_str()
            .is_some_and(|events| events.contains("populated 0")),
        "{receipt}"
    );
}

#[test]
fn sigkill_worker() {
    let Ok(root) = std::env::var("SW_BUDGET_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let mission = std::env::var("SW_BUDGET_MISSION").unwrap();
    let mut host = worker_host(&root, "process-hold");
    host.dispatch_pending(&mission).unwrap();
    println!("AT-bridge");
    let _ = std::io::stdout().flush();
    loop {
        std::thread::sleep(Duration::from_secs(30));
    }
}

#[test]
fn sigkill_next_open_proves_the_cgroup_then_cancel_settles() {
    let (world, host) = World::new("process-hold");
    drop(host);
    let mut worker = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "budget_cancel::sigkill_worker", "--nocapture"])
        .env("SW_BUDGET_ROOT", world.dir.path())
        .env("SW_BUDGET_MISSION", &world.mission)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    wait_line(&mut worker, "AT-bridge");
    nix_signal(worker.id(), "KILL");
    let status = worker.wait().unwrap();
    assert!(!status.success());
    let mut host = world.reopen();
    // Open proves the previous cgroup empty before launch. Poll is that open.
    let _ = host.poll_runtime(&world.mission);
    let opened = host.snapshot(&world.mission).unwrap().to_json();
    assert_eq!(
        opened["piHost"]["lifecycle"]["recovered"]["proven"], true,
        "next open must prove the old cgroup before anything else: {opened}"
    );
    assert_ne!(opened["phase"], "cancelled");
    let settled = host.interrupt_operator(&world.mission).unwrap();
    assert_eq!(settled.phase, Phase::Cancelled);
    let receipt = confirm_receipt(&world.db(), &world.mission);
    assert_eq!(receipt["proven"], true, "{receipt}");
    assert!(receipt["cgroup"].is_object(), "{receipt}");
}

fn nix_signal(pid: u32, name: &str) {
    let status = Command::new("kill")
        .args([&format!("-{name}"), &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}
