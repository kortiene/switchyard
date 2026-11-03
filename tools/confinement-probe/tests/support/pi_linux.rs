#![allow(dead_code)]
//! Shared setup for real confined Pi -> Rust -> confined Linux effects.
//! Mission setup uses FakeRuntime; this does not enable product Pi dispatch.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use switchyard_confinement_probe::{
    bridge::{BridgeSandbox, prepare},
    probe,
};
use switchyard_core::{
    AdmitRequest, CommandBody, CommandOrigin, CreateMission, Host, LinuxExecutionConfig, PiBridge,
    PiEffectOutcome, RuntimeBinding, parse_notification, payload_hash,
};

pub(super) fn env_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} required; no skip or fallback"))
}
pub(super) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
pub(super) fn config(stage: Option<&str>) -> LinuxExecutionConfig {
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
pub(super) fn binding(host: &Host, mission: &str) -> RuntimeBinding {
    let snap = host.snapshot(mission).unwrap();
    RuntimeBinding {
        mission_id: snap.mission_id,
        attempt_id: snap.attempt_id,
        session_id: snap.session_id,
        generation: host.generation(),
    }
}
pub(super) fn admit(host: &mut Host, mission: &str, body: CommandBody) {
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
pub(super) struct World {
    pub(super) dir: tempfile::TempDir,
    pub(super) mission: String,
    pub(super) workspace: PathBuf,
    head: Vec<u8>,
    index: Vec<u8>,
}
impl World {
    pub(super) fn new() -> (Self, Host, BridgeSandbox) {
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
                issue_title: "Pi Linux effect".into(),
                objective: "one real confined effect".into(),
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
    pub(super) fn db(&self) -> PathBuf {
        self.dir.path().join("mission.sqlite")
    }
    pub(super) fn assert_origin(&self) {
        let repo = self.dir.path().join("repo");
        assert_eq!(git(&repo, &["rev-parse", "HEAD"]), self.head);
        assert_eq!(git(&repo, &["ls-files", "--stage"]), self.index);
        assert_eq!(git(&repo, &["branch", "--show-current"]), b"main\n");
        assert!(git(&repo, &["status", "--porcelain"]).is_empty());
        assert_eq!(fs::read(repo.join("README.md")).unwrap(), b"original");
    }
}
pub(super) fn spawn(
    sandbox: &BridgeSandbox,
    binding: RuntimeBinding,
    mode: &str,
    existing: bool,
) -> PiBridge {
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
pub(super) fn run(bridge: &mut PiBridge, effect: &mut dyn FnMut(&Value) -> PiEffectOutcome) {
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
pub(super) fn stop(bridge: &mut PiBridge, sandbox: &BridgeSandbox) {
    let namespace = probe::namespace_from_status(&sandbox.status_root).unwrap();
    let members = probe::namespace_members(&namespace).unwrap();
    assert!(!members.is_empty());
    bridge.terminate();
    assert!(bridge.exit_status().unwrap().is_some());
    probe::require_quiescent(&namespace, &members).unwrap();
}
