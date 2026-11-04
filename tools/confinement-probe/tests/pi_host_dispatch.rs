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
        Self::configured(fixture, |_, _| {})
    }
    fn configured(
        fixture: &str,
        configure: impl FnOnce(&Path, &mut ConfinedPiConfig),
    ) -> (Self, Host) {
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
        let mut config = bridge(&root);
        configure(dir.path(), &mut config);
        let mut host = Host::open_confined_pi(&db, config, linux(None), fixture).unwrap();
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
    fn receipts(&self) -> Vec<(String, Option<String>)> {
        let conn = rusqlite::Connection::open_with_flags(
            self.db(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut statement = conn.prepare("SELECT operation_id, receipt_json FROM linux_execution_effects ORDER BY operation_id").unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
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
    let receipts = world.receipts();
    assert!(receipts.iter().all(|(_, receipt)| receipt.is_some()));
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
    assert_eq!(world.receipts(), receipts);
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
            && world.workspace.join("process.heartbeat").exists()
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
    assert_eq!(result["linuxExecutionEffects"].as_array().unwrap().len(), 1);
    let receipt: Value = serde_json::from_str(world.receipts()[0].1.as_deref().unwrap()).unwrap();
    assert_eq!(receipt["disposition"], "denied");
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
    for (fixture, stage) in [
        ("file-write", "bridge-live"),
        ("file-write", "pi-before-ack"),
        ("file-write", "prepared"),
        ("file-write", "receipt"),
        ("process-fast", "running"),
        ("process-fast", "result"),
        ("process-fast", "receipt"),
    ] {
        let (world, host) = World::new(fixture);
        drop(host);
        let mut worker = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "host_dispatch_crash_worker", "--nocapture"])
            .env("SW_HOST_ROOT", world.dir.path())
            .env("SW_HOST_MISSION", &world.mission)
            .env("SW_HOST_STAGE", stage)
            .env("SW_HOST_FIXTURE", fixture)
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
        let original_receipts = world.receipts();
        if stage == "receipt" && fixture == "file-write" {
            assert_eq!(
                fs::read_to_string(world.workspace.join("output.txt")).unwrap(),
                "once"
            );
            fs::write(world.workspace.join("output.txt"), "must not repeat").unwrap();
        }
        let mut host = world.reopen();
        let recovered = host.recover(&world.mission);
        if matches!(stage, "receipt" | "prepared") {
            assert!(
                recovered.is_ok(),
                "settled {stage} evidence must reconcile before scheduling: {recovered:?}"
            );
        }
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
        if stage == "receipt" && fixture == "file-write" {
            assert_eq!(
                fs::read_to_string(world.workspace.join("output.txt")).unwrap(),
                "must not repeat"
            );
            let after = host.snapshot(&world.mission).unwrap().to_json();
            assert_eq!(
                original[0]["operationId"],
                after["linuxExecutionEffects"][0]["operationId"]
            );
            assert!(original_receipts[0].1.is_some());
            assert_eq!(original_receipts, world.receipts());
        }
        if fixture == "process-fast" && world.workspace.join("process.marker").exists() {
            assert_eq!(
                fs::read_to_string(world.workspace.join("process.marker")).unwrap(),
                "once\n"
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
        &std::env::var("SW_HOST_FIXTURE").unwrap(),
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

#[test]
fn host_hung_extension_stop_recovers_inert_and_confirms_only_after_proof() {
    let (world, mut host) = World::new("ignore-abort");
    host.dispatch_pending(&world.mission).unwrap();
    until(&mut host, &world.mission, |v| {
        v["piHost"]["observation"]["liveTaskCount"]
            .as_u64()
            .is_some_and(|n| n > 0)
    });
    std::thread::sleep(Duration::from_millis(200));
    admit(&mut host, &world.mission, CommandBody::Cancel);
    let result = host.dispatch_stops(&world.mission);
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("cancellation_timeout")
    );
    let before = host.snapshot(&world.mission).unwrap();
    assert_eq!(before.phase, Phase::Cancelling);
    assert_eq!(
        before.to_json()["piHost"]["lifecycle"]["termination"]["proven"],
        true
    );
    drop(host);
    let mut host = world.reopen();
    host.dispatch_stops(&world.mission).unwrap();
    let after = host.snapshot(&world.mission).unwrap();
    assert_eq!(after.phase, Phase::Cancelled);
    assert_eq!(
        after.to_json()["piHost"]["lifecycle"]["recovered"]["proven"],
        true
    );
}

#[test]
fn host_rejects_stale_and_foreign_effect_bindings_without_mutation() {
    use switchyard_core::{PiEffectOutcome, RuntimeBinding};
    let (world, host) = World::new("file-write");
    let snapshot = host.snapshot(&world.mission).unwrap();
    let old = RuntimeBinding {
        mission_id: world.mission.clone(),
        attempt_id: snapshot.attempt_id.clone(),
        session_id: snapshot.session_id.clone(),
        generation: host.generation(),
    };
    drop(host);
    let mut host = world.reopen();
    assert!(host.accept_runtime_binding(&old, &world.mission).is_err());
    let mut foreign = old.clone();
    foreign.generation = host.generation();
    foreign.session_id.push_str("-foreign");
    for binding in [old, foreign] {
        assert_eq!(
            host.execute_pi_linux_write(&binding, &json!({}), 0),
            PiEffectOutcome::Rejected
        );
        assert_eq!(
            host.execute_pi_linux_process(&binding, &json!({}), 0),
            PiEffectOutcome::Rejected
        );
    }
    assert!(
        host.snapshot(&world.mission)
            .unwrap()
            .linux_execution_effects
            .is_empty()
    );
    assert!(!world.workspace.join("output.txt").exists());
    world.origin();
}

#[test]
fn host_driven_bridge_denies_files_sockets_and_child_processes() {
    use std::net::{TcpListener, TcpStream};
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let _positive = TcpStream::connect(tcp.local_addr().unwrap()).unwrap();
    let mut unix_positive = None;
    let (world, mut host) = World::configured("answer", |root, config| {
        unix_positive =
            Some(std::os::unix::net::UnixListener::bind(root.join("host.sock")).unwrap());
        let _control = std::os::unix::net::UnixStream::connect(root.join("host.sock")).unwrap();
        // Reuse the B1 negative probes as a trusted test-only entry point in the
        // Host's actual bridge. The wrapper only adds read-only probe/spec mounts;
        // every production namespace, resource limit and seccomp option remains.
        // B4 keeps its owner lock outside /state, so the old B1 lock-mount checks
        // do not apply. Its actual Host lock is instead a protected absent path.
        let probe = include_str!("../bridge/probe.mjs")
            .lines()
            .filter(|line| !line.starts_with("denied('owner lock"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(root.join("probe.mjs"), probe).unwrap();
        fs::write(root.join("case.json"), json!({
            "protected":[root.join("mission.sqlite"), root.join("mission.sqlite.owner.lock"), root.join("repo/README.md")],
            "port":tcp.local_addr().unwrap().port(), "socket":root.join("host.sock"),
        }).to_string()).unwrap();
        let original = config.launcher.clone();
        let wrapper = root.join("probe-launcher");
        // Test-created paths have no shell metacharacters; argv is otherwise
        // forwarded as an array, never joined or evaluated as shell source.
        for path in [root, &original] {
            assert!(!path.to_str().unwrap().contains('\''));
        }
        fs::write(
            &wrapper,
            format!(
                r#"#!/bin/bash
set -euo pipefail
args=()
for arg in "$@"; do
  if [[ "$arg" == --unshare-user ]]; then
    args+=(--ro-bind '{root}/probe.mjs' /probe.mjs --ro-bind '{root}/case.json' /case.json)
  fi
  if [[ "$arg" == /runtime/test/bridge-child.ts ]]; then
    args+=(/probe.mjs)
  else
    args+=("$arg")
  fi
done
exec '{launcher}' "${{args[@]}}"
"#,
                root = root.display(),
                launcher = original.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        config.launcher = wrapper;
    });
    host.dispatch_pending(&world.mission).unwrap();
    until(&mut host, &world.mission, |v| {
        v["piHost"]["observation"]["runtimeIdle"] == true
    });
    let session = fs::read_dir(world.dir.path().join("bridge"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("session-")
        })
        .unwrap();
    let report: Vec<String> =
        serde_json::from_slice(&fs::read(session.join("state/probe.json")).unwrap()).unwrap();
    for required in [
        "existing Node executable cannot spawn",
        "host TCP denied",
        "host Unix socket denied",
        "loopback listener denied",
        "worker works and inherits subprocess denial",
    ] {
        assert!(
            report.iter().any(|line| line == required),
            "missing {required}: {report:?}"
        );
    }
    assert!(
        report
            .iter()
            .filter(|line| line.starts_with("read "))
            .count()
            >= 3
    );
    assert!(
        report
            .iter()
            .filter(|line| line.starts_with("write "))
            .count()
            >= 3
    );
    world.origin();
}

#[test]
fn host_stop_reconciles_lost_work_ack_before_closing_bridge() {
    let (world, mut host) = World::new("file-write");
    host.dispatch_pending(&world.mission).unwrap();
    until(&mut host, &world.mission, |v| {
        v["piHost"]["observation"]["runtimeIdle"] == true
    });
    let conn = rusqlite::Connection::open(world.db()).unwrap();
    let (id, receipt): (String, String) = conn
        .query_row(
            "SELECT id,runtime_receipt_json FROM commands WHERE kind='approve_plan'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    conn.execute(
        "UPDATE commands SET state='pending',runtime_receipt_json=NULL WHERE id=?1",
        [&id],
    )
    .unwrap();
    admit(&mut host, &world.mission, CommandBody::Cancel);
    host.dispatch_stops(&world.mission).unwrap();
    let after: (String, String) = conn
        .query_row(
            "SELECT state,runtime_receipt_json FROM commands WHERE id=?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(after, ("acknowledged".into(), receipt));
    assert_eq!(
        host.snapshot(&world.mission).unwrap().phase,
        Phase::Cancelled
    );
    assert_eq!(
        fs::read_to_string(world.workspace.join("output.txt")).unwrap(),
        "once"
    );
}
