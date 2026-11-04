//! Host-driven B5 conformance. The approved-plan database fixture deliberately
//! bypasses S4 planning, but neither setup nor execution uses FakeRuntime or
//! calls PiBridge/ConfinedPiSession directly.
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use switchyard_core::{
    AdmitRequest, CommandBody, CommandOrigin, ConfinedPiConfig, CreateMission, Host,
    LinuxExecutionConfig, Phase, payload_hash,
};

fn env_path(key: &str) -> PathBuf {
    std::env::var_os(key)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{key} required; no fallback"))
}
fn bridge(root: &Path) -> ConfinedPiConfig {
    ConfinedPiConfig {
        root: root.into(),
        cgroup_root: env_path("SWITCHYARD_CGROUP_ROOT"),
        node: env_path("SWITCHYARD_PI_NODE"),
        bubblewrap: env_path("SWITCHYARD_BWRAP"),
        launcher: env_path("SWITCHYARD_BRIDGE_LAUNCHER"),
        policy: env_path("SWITCHYARD_BRIDGE_POLICY"),
    }
}
fn linux(stage: Option<&str>) -> LinuxExecutionConfig {
    LinuxExecutionConfig {
        cgroup_root: env_path("SWITCHYARD_CGROUP_ROOT"),
        bubblewrap: env_path("SWITCHYARD_BWRAP"),
        helper: env_path("SWITCHYARD_EXECUTION_HELPER"),
        crash_at: stage.map(str::to_owned),
        probe_request: None,
        fail_launch: false,
    }
}
fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
    let result = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
struct World {
    dir: tempfile::TempDir,
    mission: String,
    workspace: PathBuf,
    fixture: String,
}
impl World {
    fn new(fixture: &str) -> (Self, Host) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        fs::write(repo.join("README.md"), "original").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "base"]);
        let root = dir.path().join("bridge");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let db = dir.path().join("mission.sqlite");
        let mut host = Host::open_confined_pi(&db, bridge(&root), linux(None), fixture).unwrap();
        let m = host
            .create_mission(CreateMission {
                repo,
                issue_number: 148,
                issue_title: "Host dispatch".into(),
                objective: "one confined effect".into(),
                acceptance: "original receipts".into(),
                actor: "operator".into(),
                max_repairs: 1,
                max_plan_revisions: 1,
            })
            .unwrap();
        let workspaces = dir.path().join("workspaces");
        fs::create_dir(&workspaces).unwrap();
        host.prepare_workspace(&m.mission_id, &m.attempt_id, &workspaces)
            .unwrap();
        // S4 is excluded from B5: import an awaiting-approval fixture, then use
        // normal operator admission and the real Host outbox for all execution.
        let digest = payload_hash(&json!("fixture plan")).unwrap();
        rusqlite::Connection::open(&db).unwrap().execute("UPDATE missions SET phase='awaiting_plan_approval',plan_body='fixture plan',plan_digest=?1 WHERE id=?2", [&digest, &m.mission_id]).unwrap();
        admit(
            &mut host,
            &m.mission_id,
            CommandBody::ApprovePlan {
                plan_digest: digest,
            },
        );
        let workspace = PathBuf::from(
            host.snapshot(&m.mission_id)
                .unwrap()
                .workspace
                .unwrap()
                .path,
        );
        (
            Self {
                dir,
                mission: m.mission_id,
                workspace,
                fixture: fixture.into(),
            },
            host,
        )
    }
    fn db(&self) -> PathBuf {
        self.dir.path().join("mission.sqlite")
    }
    fn reopen(&self) -> Host {
        Host::open_confined_pi(
            &self.db(),
            bridge(&self.dir.path().join("bridge")),
            linux(None),
            &self.fixture,
        )
        .unwrap()
    }
    fn origin(&self) {
        let repo = self.dir.path().join("repo");
        assert_eq!(fs::read(repo.join("README.md")).unwrap(), b"original");
        assert!(git(&repo, &["status", "--porcelain"]).is_empty());
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
fn until(host: &mut Host, mission: &str, predicate: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        host.poll_runtime(mission).unwrap();
        let value = host.snapshot(mission).unwrap().to_json();
        if predicate(&value) {
            return value;
        }
        assert!(Instant::now() < deadline, "Host did not settle: {value}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
fn host_dispatch_writes_and_reopens_original_receipts() {
    let (world, mut host) = World::new("tools");
    assert_eq!(host.dispatch_pending(&world.mission).unwrap(), 1);
    let before = until(&mut host, &world.mission, |v| {
        v["piHost"]["observation"]["runtimeIdle"] == true
    });
    assert_eq!(before["linuxExecutionEffects"].as_array().unwrap().len(), 3);
    assert_eq!(
        fs::read_to_string(world.workspace.join("pair-1")).unwrap(),
        "first"
    );
    assert_eq!(
        fs::read_to_string(world.workspace.join("nested")).unwrap(),
        "child"
    );
    fs::write(world.workspace.join("pair-1"), "do not replay").unwrap();
    drop(host);
    let mut host = world.reopen();
    host.recover(&world.mission).unwrap();
    let after = until(&mut host, &world.mission, |v| {
        v["piHost"]["observation"]["runtimeIdle"] == true
    });
    assert_eq!(
        before["linuxExecutionEffects"],
        after["linuxExecutionEffects"]
    );
    assert_eq!(
        fs::read_to_string(world.workspace.join("pair-1")).unwrap(),
        "do not replay"
    );
    assert_eq!(after["piHost"]["lifecycle"]["recovered"]["proven"], true);
    assert!(Host::inspect(&world.db(), Some(&world.mission)).is_ok());
    world.origin();
}
#[test]
fn host_dispatch_process_status_and_cancel_are_durable() {
    let (world, mut host) = World::new("process-cancel");
    host.dispatch_pending(&world.mission).unwrap();
    let result = until(&mut host, &world.mission, |v| {
        v["piHost"]["observation"]["runtimeIdle"] == true
    });
    assert_eq!(result["linuxExecutionEffects"].as_array().unwrap().len(), 3);
    assert_eq!(result["executionBlocked"], false);
    assert_eq!(
        fs::read_to_string(world.workspace.join("process.marker")).unwrap(),
        "once\n"
    );
    world.origin();
}
#[test]
fn host_operator_cancel_proves_both_sandboxes_empty() {
    let (world, mut host) = World::new("process-hold");
    host.dispatch_pending(&world.mission).unwrap();
    until(&mut host, &world.mission, |v| {
        v["linuxExecutionEffects"]
            .as_array()
            .is_some_and(|v| v.iter().any(|e| e["state"] == "running"))
    });
    admit(&mut host, &world.mission, CommandBody::Cancel);
    host.dispatch_stops(&world.mission).unwrap();
    let snap = host.snapshot(&world.mission).unwrap();
    assert_eq!(snap.phase, Phase::Cancelled);
    let value = snap.to_json();
    assert_eq!(value["piHost"]["lifecycle"]["termination"]["proven"], true);
    assert_eq!(value["executionBlocked"], false);
    let heartbeat = fs::read(world.workspace.join("process.heartbeat")).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        fs::read(world.workspace.join("process.heartbeat")).unwrap(),
        heartbeat
    );
}
#[test]
fn host_forwarded_write_denies_escape_and_preserves_original() {
    let (world, mut host) = World::new("file-write");
    symlink(
        world.dir.path().join("repo/README.md"),
        world.workspace.join("output.txt"),
    )
    .unwrap();
    host.dispatch_pending(&world.mission).unwrap();
    let result = until(&mut host, &world.mission, |v| {
        v["piHost"]["observation"]["runtimeIdle"] == true
    });
    assert!(
        result["linuxExecutionEffects"]
            .to_string()
            .contains("denied")
    );
    world.origin();
}
#[test]
fn host_confined_refuses_forged_authority_and_adapter_switch() {
    let (world, mut host) = World::new("answer");
    for body in [
        CommandBody::ConfirmStopped,
        CommandBody::ConfirmCancelled,
        CommandBody::RequestVerification { quiescent: true },
    ] {
        assert!(
            host.admit(AdmitRequest {
                command_id: None,
                mission_id: world.mission.clone(),
                attempt_id: None,
                expected_revision: host.snapshot(&world.mission).unwrap().revision,
                actor: "forged".into(),
                origin: CommandOrigin::RuntimeObservation,
                body
            })
            .is_err()
        );
    }
    drop(host);
    assert!(Host::open(&world.db()).is_err());
    assert!(Host::open_pi_probe(&world.db(), &env_path("SWITCHYARD_PI_NODE")).is_err());
}

#[test]
fn host_sigkill_reconciles_bridge_outbox_and_original_effects() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc;
    for stage in ["bridge-live", "pi-before-ack", "prepared", "receipt"] {
        let (world, host) = World::new("file-write");
        drop(host);
        let mut worker = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "host_dispatch_crash_worker", "--nocapture"])
            .env("SW_HOST_ROOT", world.dir.path())
            .env("SW_HOST_MISSION", &world.mission)
            .env("SW_HOST_STAGE", stage)
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
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(line) if line.contains(&format!("AT-{stage}")) => break,
                Ok(_) => {}
                Err(error) => {
                    let _ = worker.kill();
                    let _ = worker.wait();
                    panic!("missing Host barrier {stage}: {error}");
                }
            }
        }
        worker.kill().unwrap();
        worker.wait().unwrap();
        let before = Host::inspect(&world.db(), Some(&world.mission)).unwrap()[0].to_json();
        let original = before["linuxExecutionEffects"].clone();
        if stage == "receipt" {
            assert_eq!(
                fs::read_to_string(world.workspace.join("output.txt")).unwrap(),
                "once"
            );
            fs::write(world.workspace.join("output.txt"), "must not repeat").unwrap();
        }
        let mut host = world.reopen();
        let recovered = host.recover(&world.mission);
        let snapshot = host.snapshot(&world.mission).unwrap().to_json();
        assert_eq!(snapshot["piHost"]["lifecycle"]["recovered"]["proven"], true);
        if recovered.is_ok() {
            until(&mut host, &world.mission, |v| {
                v["piHost"]["observation"]["runtimeIdle"] == true
            });
        } else {
            assert_eq!(
                host.snapshot(&world.mission).unwrap().phase,
                Phase::NeedsAttention
            );
        }
        if stage == "receipt" {
            assert_eq!(
                fs::read_to_string(world.workspace.join("output.txt")).unwrap(),
                "must not repeat"
            );
            let after = host.snapshot(&world.mission).unwrap().to_json();
            assert_eq!(
                original[0]["operationId"],
                after["linuxExecutionEffects"][0]["operationId"]
            );
            assert_eq!(
                original[0]["receipt"],
                after["linuxExecutionEffects"][0]["receipt"]
            );
        }
        world.origin();
    }
}

#[test]
fn host_dispatch_crash_worker() {
    let Some(root) = std::env::var_os("SW_HOST_ROOT").map(PathBuf::from) else {
        return;
    };
    let stage = std::env::var("SW_HOST_STAGE").unwrap();
    let mission = std::env::var("SW_HOST_MISSION").unwrap();
    let mut host = Host::open_confined_pi(
        &root.join("mission.sqlite"),
        bridge(&root.join("bridge")),
        linux(Some(&stage)),
        "file-write",
    )
    .unwrap();
    host.dispatch_pending(&mission).unwrap();
    until(&mut host, &mission, |_| false);
    panic!("Host passed mandatory crash barrier");
}

#[test]
fn host_pause_bypasses_unverified_workspace_and_stops_descendants() {
    let (world, mut host) = World::new("process-hold");
    host.dispatch_pending(&world.mission).unwrap();
    until(&mut host, &world.mission, |v| {
        v["linuxExecutionEffects"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["state"] == "running"))
    });
    // Stops are delivered without verifying/reusing this intentionally blocked workspace.
    rusqlite::Connection::open(world.db())
        .unwrap()
        .execute("UPDATE workspaces SET verified_generation=NULL", [])
        .unwrap();
    admit(&mut host, &world.mission, CommandBody::Pause);
    host.dispatch_stops(&world.mission).unwrap();
    assert_eq!(host.snapshot(&world.mission).unwrap().phase, Phase::Paused);
    assert_eq!(
        host.snapshot(&world.mission).unwrap().to_json()["piHost"]["lifecycle"]["termination"]["proven"],
        true
    );
}
