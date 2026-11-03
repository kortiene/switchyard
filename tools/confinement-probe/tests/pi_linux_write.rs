//! Real confined Pi -> public Host mapper -> real confined Linux write.
//! Mission setup uses FakeRuntime; this does not enable product Pi dispatch.
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;
use switchyard_confinement_probe::{bridge::BridgeSandbox, probe};
use switchyard_core::{Host, Phase, PiEffectOutcome};

#[path = "support/pi_linux.rs"]
mod support;
use support::*;

#[test]
fn confined_multi_and_nested_writes_use_distinct_linux_receipts() {
    let (world, mut host, sandbox) = World::new();
    host.enable_linux_execution(config(None)).unwrap();
    let binding = binding(&host, &world.mission);
    let mut bridge = spawn(&sandbox, binding.clone(), "tools", false);
    let mut receipts = Vec::new();
    run(&mut bridge, &mut |params| {
        let outcome = host.execute_pi_linux_write(&binding, params, now() + 20_000);
        let PiEffectOutcome::Completed(receipt) = &outcome else {
            panic!("write failed: {outcome:?}")
        };
        assert_eq!(receipt["simulated"], false);
        receipts.push((params.clone(), receipt.clone()));
        outcome
    });
    assert_eq!(receipts.len(), 3);
    let mut ids: Vec<_> = receipts
        .iter()
        .map(|(p, _)| p["operationId"].as_str().unwrap())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3);
    for (path, contents) in [
        ("pair-1", "first"),
        ("pair-2", "second"),
        ("nested", "child"),
    ] {
        assert_eq!(
            fs::read_to_string(world.workspace.join(path)).unwrap(),
            contents
        );
    }
    assert_eq!(
        host.snapshot(&world.mission)
            .unwrap()
            .linux_execution_effects
            .len(),
        3
    );
    for (params, receipt) in receipts {
        assert_eq!(
            host.execute_pi_linux_write(&binding, &params, 0),
            PiEffectOutcome::Completed(receipt)
        );
    }
    stop(&mut bridge, &sandbox);
    world.assert_origin();
}

#[test]
fn confined_write_cannot_follow_a_symlink_into_host_authority() {
    let (world, mut host, sandbox) = World::new();
    let protected = world.dir.path().join("repo/README.md");
    symlink(&protected, world.workspace.join("output.txt")).unwrap();
    host.enable_linux_execution(config(None)).unwrap();
    let binding = binding(&host, &world.mission);
    let mut bridge = spawn(&sandbox, binding.clone(), "file-write", false);
    let mut outcomes = Vec::new();
    run(&mut bridge, &mut |params| {
        let outcome = host.execute_pi_linux_write(&binding, params, now() + 20_000);
        outcomes.push((params.clone(), outcome.clone()));
        outcome
    });
    assert_eq!(outcomes.len(), 1);
    let (params, outcome) = &outcomes[0];
    let PiEffectOutcome::Completed(receipt) = outcome else {
        panic!("expected a durable denied receipt: {outcome:?}");
    };
    // Completed means a settled outcome, not a successful mutation. The
    // Linux helper definitively refused the non-regular path before writing.
    assert_eq!(receipt["disposition"], "denied");
    assert_eq!(receipt["result"]["helper"]["ok"], false);
    assert_eq!(receipt["result"]["helper"]["reason"], "not-regular");
    assert_eq!(receipt["result"]["quiescenceProven"], true);
    assert!(
        fs::symlink_metadata(world.workspace.join("output.txt"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        host.snapshot(&world.mission).unwrap().phase,
        Phase::Implementing
    );
    assert!(!host.snapshot(&world.mission).unwrap().execution_blocked);
    fs::remove_file(world.workspace.join("output.txt")).unwrap();
    fs::write(
        world.workspace.join("output.txt"),
        "now writable; still no replay",
    )
    .unwrap();
    assert_eq!(
        host.execute_pi_linux_write(&binding, params, now() + 20_000),
        *outcome
    );
    assert_eq!(
        fs::read(world.workspace.join("output.txt")).unwrap(),
        b"now writable; still no replay"
    );
    stop(&mut bridge, &sandbox);
    world.assert_origin();
}

#[test]
fn host_sigkill_at_linux_boundaries_replays_without_reapplying() {
    for stage in ["prepared", "result", "receipt", "completed"] {
        crash_replay(stage);
    }
}

fn crash_replay(stage: &str) {
    let (world, host, sandbox) = World::new();
    drop(host);
    let mut worker = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "pi_linux_crash_worker", "--nocapture"])
        .env("SW_PI_LINUX_ROOT", world.dir.path())
        .env("SW_PI_LINUX_MISSION", &world.mission)
        .env("SW_PI_LINUX_STAGE", stage)
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
            assert!(!world.workspace.join("output.txt").exists());
            PiEffectOutcome::Rejected
        }
        "result" => {
            assert!(receipt.is_none());
            PiEffectOutcome::Uncertain
        }
        _ => PiEffectOutcome::Completed(serde_json::from_str(&receipt.unwrap()).unwrap()),
    };
    if stage != "prepared" {
        assert_eq!(
            fs::read(world.workspace.join("output.txt")).unwrap(),
            b"once"
        );
    }
    // Any blind repeat would overwrite this independent marker. Its inode and
    // timestamp also stay unchanged across both direct and actual Pi replay.
    fs::write(
        world.workspace.join("output.txt"),
        "replay must not overwrite",
    )
    .unwrap();
    let before = fs::metadata(world.workspace.join("output.txt")).unwrap();
    let mut host = Host::open(&world.db()).unwrap();
    host.enable_linux_execution(config(None)).unwrap();
    let current = binding(&host, &world.mission);
    let mut stale = current.clone();
    stale.generation -= 1;
    assert_eq!(
        host.execute_pi_linux_write(&stale, &params, now() + 20_000),
        PiEffectOutcome::Rejected
    );
    let mut bridge = spawn(&sandbox, current.clone(), "recover", true);
    let mut replayed = 0;
    run(&mut bridge, &mut |effect| {
        assert_eq!(
            *effect, params,
            "Pi must preserve the task, slot, and wire payload"
        );
        let outcome = host.execute_pi_linux_write(&current, effect, now() + 20_000);
        assert_eq!(outcome, expected);
        replayed += 1;
        outcome
    });
    assert_eq!(replayed, 1);
    assert_eq!(host.execute_pi_linux_write(&current, &params, 0), expected);
    let after = fs::metadata(world.workspace.join("output.txt")).unwrap();
    assert_eq!(
        fs::read(world.workspace.join("output.txt")).unwrap(),
        b"replay must not overwrite"
    );
    assert_eq!(
        (before.ino(), before.mtime(), before.mtime_nsec()),
        (after.ino(), after.mtime(), after.mtime_nsec())
    );
    let snap = host.snapshot(&world.mission).unwrap();
    assert_eq!(snap.linux_execution_effects.len(), 1);
    assert_eq!(snap.execution_blocked, stage == "result");
    if stage == "result" {
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
    println!("Pi/Linux crash boundary {stage}: original outcome retained; no repeated write");
}

#[test]
fn pi_linux_crash_worker() {
    let Some(root) = std::env::var_os("SW_PI_LINUX_ROOT").map(PathBuf::from) else {
        return;
    };
    let mission = std::env::var("SW_PI_LINUX_MISSION").unwrap();
    let stage = std::env::var("SW_PI_LINUX_STAGE").unwrap();
    let mut host = Host::open(&root.join("mission.sqlite")).unwrap();
    host.enable_linux_execution(config(Some(&stage))).unwrap();
    let binding = binding(&host, &mission);
    let sandbox = BridgeSandbox {
        launcher: root.join("bridge/launch"),
        database: root.join("bridge/state/session.sqlite"),
        status_root: root.join("bridge/supervisor"),
    };
    let mut bridge = spawn(&sandbox, binding.clone(), "file-write", false);
    run(&mut bridge, &mut |params| {
        fs::write(
            root.join("request.json"),
            serde_json::to_vec(params).unwrap(),
        )
        .unwrap();
        host.execute_pi_linux_write(&binding, params, now() + 20_000)
    });
    panic!("crash worker passed its mandatory barrier");
}
