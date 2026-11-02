//! Real confined Pi -> public Host mapper -> real confined Linux write.
//! Mission setup uses FakeRuntime; this does not enable product Pi dispatch.
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use switchyard_confinement_probe::{
    bridge::{BridgeSandbox, prepare},
    probe,
};
use switchyard_core::{
    AdmitRequest, CommandBody, CommandOrigin, CreateMission, Host, LinuxExecutionConfig, Phase,
    PiBridge, PiEffectOutcome, RuntimeBinding, parse_notification, payload_hash,
};

fn env_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} required; no skip or fallback"))
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
fn config(stage: Option<&str>) -> LinuxExecutionConfig {
    let helper = env_path("SWITCHYARD_EXECUTION_HELPER")
        .canonicalize()
        .unwrap();
    assert!(helper.is_file());
    LinuxExecutionConfig {
        cgroup_root: env_path("SWITCHYARD_CGROUP_ROOT"),
        bubblewrap: env_path("SWITCHYARD_BWRAP"),
        helper,
        crash_at: stage.map(str::to_owned),
        probe_request: None,
        fail_launch: false,
    }
}
fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
fn binding(host: &Host, mission: &str) -> RuntimeBinding {
    let snap = host.snapshot(mission).unwrap();
    RuntimeBinding {
        mission_id: snap.mission_id,
        attempt_id: snap.attempt_id,
        session_id: snap.session_id,
        generation: host.generation(),
    }
}
fn admit(host: &mut Host, mission: &str, body: CommandBody) {
    host.admit(AdmitRequest {
        command_id: None,
        mission_id: mission.into(),
        attempt_id: None,
        expected_revision: host.snapshot(mission).unwrap().revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body,
    })
    .unwrap();
}
struct World {
    dir: tempfile::TempDir,
    mission: String,
    workspace: PathBuf,
    head: Vec<u8>,
    index: Vec<u8>,
}
impl World {
    fn new() -> (Self, Host, BridgeSandbox) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        fs::write(repo.join("README.md"), "original").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "base"]);
        let mut host = Host::open(&dir.path().join("mission.sqlite")).unwrap();
        let mission = host
            .create_mission(CreateMission {
                repo: repo.clone(),
                issue_number: 97,
                issue_title: "Pi Linux write".into(),
                objective: "one real write".into(),
                acceptance: "recover without reapplying".into(),
                actor: "operator".into(),
                max_repairs: 1,
                max_plan_revisions: 1,
            })
            .unwrap();
        let mission_id = mission.mission_id;
        let root = dir.path().join("workspaces");
        fs::create_dir(&root).unwrap();
        host.prepare_workspace(&mission_id, &mission.attempt_id, &root)
            .unwrap();
        admit(&mut host, &mission_id, CommandBody::StartPlanning);
        host.dispatch_pending(&mission_id).unwrap();
        let digest = payload_hash(&json!("plan")).unwrap();
        host.apply_notification(
            &mission_id,
            parse_notification(&json!({"kind":"plan_completed",
            "plan":"plan","digest":digest,"quiescent":true}))
            .unwrap(),
        )
        .unwrap();
        admit(
            &mut host,
            &mission_id,
            CommandBody::ApprovePlan {
                plan_digest: digest,
            },
        );
        host.dispatch_pending(&mission_id).unwrap();
        let workspace = PathBuf::from(host.snapshot(&mission_id).unwrap().workspace.unwrap().path);
        let bridge_root = dir.path().join("bridge");
        fs::create_dir(&bridge_root).unwrap();
        let sandbox = prepare(&bridge_root, &env_path("SWITCHYARD_BRIDGE_POLICY"), false).unwrap();
        let world = Self {
            head: git(&repo, &["rev-parse", "HEAD"]),
            index: git(&repo, &["ls-files", "--stage"]),
            dir,
            mission: mission_id,
            workspace,
        };
        (world, host, sandbox)
    }
    fn db(&self) -> PathBuf {
        self.dir.path().join("mission.sqlite")
    }
    fn assert_origin(&self) {
        let repo = self.dir.path().join("repo");
        assert_eq!(git(&repo, &["rev-parse", "HEAD"]), self.head);
        assert_eq!(git(&repo, &["ls-files", "--stage"]), self.index);
        assert_eq!(git(&repo, &["branch", "--show-current"]), b"main\n");
        assert!(git(&repo, &["status", "--porcelain"]).is_empty());
        assert_eq!(fs::read(repo.join("README.md")).unwrap(), b"original");
    }
}
fn spawn(sandbox: &BridgeSandbox, binding: RuntimeBinding, mode: &str, existing: bool) -> PiBridge {
    let mut bridge = PiBridge::spawn(
        &sandbox.launcher,
        Path::new("bridge-fixture"),
        &sandbox.database,
        binding,
        mode,
    )
    .unwrap();
    bridge
        .hello()
        .unwrap_or_else(|error| panic!("hello: {error}; {}", bridge.stderr_tail()));
    let opened = bridge
        .call(
            "runtime.open",
            None,
            json!({"requireExisting":existing}),
            &mut |_| panic!("open must not schedule effects"),
        )
        .unwrap();
    assert_eq!(opened["scheduling"], "paused");
    bridge
}
fn run(bridge: &mut PiBridge, effect: &mut dyn FnMut(&Value) -> PiEffectOutcome) {
    bridge
        .call_with_effects(
            "runtime.submit",
            Some("write"),
            json!({"type":"input","content":"write"}),
            effect,
        )
        .unwrap_or_else(|error| panic!("submit: {error}; {}", bridge.stderr_tail()));
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = bridge
            .call_with_effects(
                "runtime.inspect",
                None,
                json!({"commandIds":["write"]}),
                effect,
            )
            .unwrap_or_else(|error| panic!("inspect: {error}; {}", bridge.stderr_tail()));
        if snapshot["runtimeIdle"] == true {
            assert_eq!(snapshot["commands"][0]["status"], "done");
            break;
        }
        assert!(Instant::now() < until, "Pi did not settle");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn stop(bridge: &mut PiBridge, sandbox: &BridgeSandbox) {
    let namespace = probe::namespace_from_status(&sandbox.status_root).unwrap();
    let members = probe::namespace_members(&namespace).unwrap();
    assert!(!members.is_empty());
    bridge.terminate();
    assert!(bridge.exit_status().unwrap().is_some());
    probe::require_quiescent(&namespace, &members).unwrap();
}

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
