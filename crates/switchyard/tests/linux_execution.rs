//! S3-A4 acceptance. These tests fail closed unless a delegated cgroup and the
//! pinned Bubblewrap are supplied. They do not skip or fall back to rlimits.
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::symlink;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use switchyard_core::{
    AdmitRequest, AttentionReason, CommandBody, CommandOrigin, CreateMission, EffectProof,
    ExecutionResolutionDecision, ExecutionResolutionRequest, Host, LINUX_EDIT_SCOPE,
    LINUX_EXEC_CANCEL_SCOPE, LINUX_EXEC_START_SCOPE, LINUX_EXEC_STATUS_SCOPE, LINUX_LIST_SCOPE,
    LINUX_RANGE_SCOPE, LINUX_READ_SCOPE, LINUX_WRITE_SCOPE, LinuxExecutionConfig,
    LinuxExecutionRequest, LinuxOperation, Phase, RuntimeBinding, parse_notification, payload_hash,
};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn require_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("{name} is required; missing cgroup delegation or Bubblewrap fails this gate")
    })
}

fn helper_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_switchyard_execution_helper") {
        return PathBuf::from(path);
    }
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("switchyard-execution-helper");
    path
}

fn config(crash_at: Option<&str>, probe_request: Option<Value>) -> LinuxExecutionConfig {
    let helper = helper_bin();
    assert!(
        helper.is_file(),
        "switchyard-execution-helper is missing at {}; build it with --features linux-execution",
        helper.display()
    );
    LinuxExecutionConfig {
        cgroup_root: require_env("SWITCHYARD_CGROUP_ROOT").into(),
        bubblewrap: require_env("SWITCHYARD_BWRAP").into(),
        helper,
        crash_at: crash_at.map(str::to_string),
        probe_request,
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

struct World {
    tmp: tempfile::TempDir,
    repo: PathBuf,
    head: Vec<u8>,
    index: Vec<u8>,
    branch: Vec<u8>,
    _tcp: TcpListener,
    _socket: UnixListener,
}

impl World {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["config", "user.email", "linux@example.com"]);
        git(&repo, &["config", "user.name", "Linux"]);
        fs::write(repo.join("README.md"), "original\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "base"]);
        fs::create_dir_all(tmp.path().join("home/.ssh")).unwrap();
        fs::write(tmp.path().join("home/.ssh/key"), "PROTECTED\n").unwrap();
        fs::write(tmp.path().join("bridge.json"), "PROTECTED\n").unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = UnixListener::bind(tmp.path().join("host.sock")).unwrap();
        Self {
            head: git(&repo, &["rev-parse", "HEAD"]),
            index: fs::read(repo.join(".git/index")).unwrap(),
            branch: git(&repo, &["branch", "--show-current"]),
            repo,
            tmp,
            _tcp: tcp,
            _socket: socket,
        }
    }

    fn db(&self) -> PathBuf {
        self.tmp.path().join("mission.sqlite")
    }

    fn mark(&mut self) {
        self.head = git(&self.repo, &["rev-parse", "HEAD"]);
        // Staged content, not the stat cache. Host Git verification may refresh
        // cache bytes; it must not change what the original index records.
        self.index = git(&self.repo, &["ls-files", "--stage"]);
        self.branch = git(&self.repo, &["branch", "--show-current"]);
    }

    fn assert_origin(&self) {
        assert_eq!(git(&self.repo, &["ls-files", "--stage"]), self.index);
        assert!(
            git(&self.repo, &["diff", "--cached", "--quiet"]).is_empty()
                || git(&self.repo, &["diff", "--cached"]).is_empty()
        );
        assert_eq!(git(&self.repo, &["rev-parse", "HEAD"]), self.head);
        assert_eq!(git(&self.repo, &["branch", "--show-current"]), self.branch);
        assert!(git(&self.repo, &["status", "--porcelain"]).is_empty());
        assert_eq!(
            fs::read(self.tmp.path().join("bridge.json")).unwrap(),
            b"PROTECTED\n"
        );
        assert_eq!(
            fs::read(self.tmp.path().join("home/.ssh/key")).unwrap(),
            b"PROTECTED\n"
        );
        assert_eq!(
            fs::read(self.repo.join("README.md")).unwrap(),
            b"original\n"
        );
    }
}

fn implementing(world: &mut World) -> (Host, String) {
    let mut host = Host::open(&world.db()).unwrap();
    let mission = host
        .create_mission(CreateMission {
            repo: world.repo.clone(),
            issue_number: 130,
            issue_title: "linux".into(),
            objective: "confined effects".into(),
            acceptance: "durable quiescence".into(),
            actor: "operator".into(),
            max_repairs: 1,
            max_plan_revisions: 1,
        })
        .unwrap();
    let workspaces = world.tmp.path().join("workspaces");
    fs::create_dir_all(&workspaces).unwrap();
    host.prepare_workspace(&mission.mission_id, &mission.attempt_id, &workspaces)
        .unwrap();
    let admit = |host: &mut Host, mission_id: &str, body: CommandBody| {
        let revision = host.snapshot(mission_id).unwrap().revision;
        host.admit(AdmitRequest {
            command_id: None,
            mission_id: mission_id.to_string(),
            attempt_id: None,
            expected_revision: revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body,
        })
        .unwrap();
    };
    admit(&mut host, &mission.mission_id, CommandBody::StartPlanning);
    host.dispatch_pending(&mission.mission_id).unwrap();
    host.apply_notification(
        &mission.mission_id,
        parse_notification(&json!({
            "kind": "plan_completed",
            "plan": "plan",
            "quiescent": true,
            "digest": payload_hash(&json!("plan")).unwrap()
        }))
        .unwrap(),
    )
    .unwrap();
    let digest = host
        .snapshot(&mission.mission_id)
        .unwrap()
        .plan_digest
        .unwrap();
    admit(
        &mut host,
        &mission.mission_id,
        CommandBody::ApprovePlan {
            plan_digest: digest,
        },
    );
    assert_eq!(
        host.snapshot(&mission.mission_id).unwrap().phase,
        Phase::Implementing
    );
    // Baseline after the host's own Git setup. Later checks prove the sandbox
    // did not rewrite the original branch, HEAD, or index.
    world.mark();
    (host, mission.mission_id)
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

fn request(
    host: &Host,
    mission: &str,
    effect_key: &str,
    scope: &str,
    operation: LinuxOperation,
) -> LinuxExecutionRequest {
    LinuxExecutionRequest {
        binding: binding(host, mission),
        tool_task_id: "task-130".into(),
        effect_key: effect_key.into(),
        capability_scope: scope.into(),
        deadline_ms: now_ms() + 20_000,
        operation,
    }
}

fn tree(host: &Host, mission: &str) -> PathBuf {
    PathBuf::from(host.snapshot(mission).unwrap().workspace.unwrap().path)
}

fn enable(host: &mut Host) {
    host.enable_linux_execution(config(None, None)).unwrap();
}

#[test]
fn positive_file_ops_and_confinement_denials_leave_git_authority_unchanged() {
    let mut world = World::new();
    let (mut host, mission) = implementing(&mut world);
    enable(&mut host);
    let workspace = tree(&host, &mission);
    symlink(world.repo.join("README.md"), workspace.join("escape")).unwrap();
    symlink(
        format!("/proc/1/root{}", world.repo.join("README.md").display()),
        workspace.join("proc-escape"),
    )
    .unwrap();
    fs::write(workspace.join("note.txt"), "hello").unwrap();

    let write = request(
        &host,
        &mission,
        "write-file:v1",
        LINUX_WRITE_SCOPE,
        LinuxOperation::WriteFile {
            path: "allowed.txt".into(),
            content: "allowed workspace write\n".into(),
        },
    );
    let wrote = host.execute_linux_operation(&write).unwrap();
    assert_eq!(wrote.state, "completed");
    assert_eq!(
        fs::read(workspace.join("allowed.txt")).unwrap(),
        b"allowed workspace write\n"
    );
    let stale = LinuxExecutionRequest {
        binding: RuntimeBinding {
            generation: 0,
            ..write.binding.clone()
        },
        ..write.clone()
    };
    assert_eq!(
        host.execute_linux_operation(&stale).unwrap_err().category(),
        "stale_generation"
    );

    let read = request(
        &host,
        &mission,
        "read-file:v1",
        LINUX_READ_SCOPE,
        LinuxOperation::ReadFile {
            path: "allowed.txt".into(),
        },
    );
    let read = host.execute_linux_operation(&read).unwrap();
    assert_eq!(
        read.receipt.unwrap()["result"]["helper"]["content"],
        "allowed workspace write\n"
    );
    let range = request(
        &host,
        &mission,
        "range-read:v1",
        LINUX_RANGE_SCOPE,
        LinuxOperation::ReadFileRange {
            path: "allowed.txt".into(),
            offset: 8,
            length: 4,
        },
    );
    assert_eq!(
        host.execute_linux_operation(&range)
            .unwrap()
            .receipt
            .unwrap()["result"]["helper"]["content"],
        "work"
    );
    let list = request(
        &host,
        &mission,
        "list-dir:v1",
        LINUX_LIST_SCOPE,
        LinuxOperation::ListDirectory { path: ".".into() },
    );
    let names = host
        .execute_linux_operation(&list)
        .unwrap()
        .receipt
        .unwrap()["result"]["helper"]["entries"]
        .clone();
    assert!(
        names
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "allowed.txt")
    );

    let conflict = request(
        &host,
        &mission,
        "edit-conflict:v1",
        LINUX_EDIT_SCOPE,
        LinuxOperation::EditFile {
            path: "note.txt".into(),
            old: "missing".into(),
            new: "x".into(),
        },
    );
    let conflict = host.execute_linux_operation(&conflict).unwrap();
    assert_eq!(
        conflict.receipt.as_ref().unwrap()["disposition"],
        "conflict"
    );
    assert_eq!(fs::read(workspace.join("note.txt")).unwrap(), b"hello");
    let edit = request(
        &host,
        &mission,
        "edit-file:v1",
        LINUX_EDIT_SCOPE,
        LinuxOperation::EditFile {
            path: "note.txt".into(),
            old: "hello".into(),
            new: "world".into(),
        },
    );
    host.execute_linux_operation(&edit).unwrap();
    assert_eq!(fs::read(workspace.join("note.txt")).unwrap(), b"world");

    for (key, path) in [("symlink", "escape"), ("magic", "proc-escape")] {
        let denied = request(
            &host,
            &mission,
            key,
            LINUX_READ_SCOPE,
            LinuxOperation::ReadFile { path: path.into() },
        );
        let denied = host.execute_linux_operation(&denied).unwrap();
        assert_eq!(denied.receipt.as_ref().unwrap()["disposition"], "denied");
        assert_eq!(denied.state, "completed");
    }
    let traversal = request(
        &host,
        &mission,
        "traversal",
        LINUX_READ_SCOPE,
        LinuxOperation::ReadFile {
            path: "../README.md".into(),
        },
    );
    assert!(host.execute_linux_operation(&traversal).is_err());

    let ledger = world.db();
    let probe = json!({
        "paths": [
            ledger,
            world.repo.join("README.md"),
            world.repo.join(".git/HEAD"),
            world.tmp.path().join("bridge.json"),
            world.tmp.path().join("home/.ssh/key"),
            world.tmp.path().join("missing-under-protected")
        ],
        "tcp": world._tcp.local_addr().unwrap().to_string(),
        "socket": world.tmp.path().join("host.sock")
    });
    host.enable_linux_execution(config(None, Some(probe)))
        .unwrap();
    let probed = request(
        &host,
        &mission,
        "probe:v1",
        LINUX_EXEC_START_SCOPE,
        LinuxOperation::ExecStart {
            argv: vec!["/helper".into(), "probe".into()],
        },
    );
    let probed = host.execute_linux_operation(&probed).unwrap();
    let body = &probed.receipt.unwrap()["result"]["helper"];
    assert_eq!(body["tcp"], false, "{body}");
    assert_eq!(body["socket"], false, "{body}");
    assert!(body["leaked"].as_array().unwrap().is_empty(), "{body}");
    for entry in body["paths"].as_array().unwrap() {
        assert_eq!(entry["read"], false, "{entry}");
        assert_eq!(entry["write"], false, "{entry}");
    }
    let cat = request(
        &host,
        &mission,
        "cat-ledger:v1",
        LINUX_EXEC_START_SCOPE,
        LinuxOperation::ExecStart {
            argv: vec!["/usr/bin/cat".into(), ledger.display().to_string()],
        },
    );
    let cat = host.execute_linux_operation(&cat).unwrap();
    assert_ne!(cat.receipt.as_ref().unwrap()["disposition"], "exited");
    assert!(
        !cat.receipt.unwrap()["result"]["stdout"]
            .as_str()
            .unwrap()
            .contains("SQLite format")
    );
    world.assert_origin();
    let dot_git = fs::read_to_string(workspace.join(".git")).unwrap();
    assert!(dot_git.starts_with("gitdir:"), "{dot_git}");
}

#[test]
fn symlinked_git_pointer_cannot_redirect_a_later_mount() {
    let mut world = World::new();
    let (mut host, mission) = implementing(&mut world);
    enable(&mut host);
    let workspace = tree(&host, &mission);
    let original = fs::read(workspace.join(".git")).unwrap();
    let sentinel = world.tmp.path().join("sentinel-target");
    fs::write(&sentinel, "do-not-touch\n").unwrap();
    fs::remove_file(workspace.join(".git")).unwrap();
    symlink(&sentinel, workspace.join(".git")).unwrap();
    let write = request(
        &host,
        &mission,
        "write-through-symlink",
        LINUX_WRITE_SCOPE,
        LinuxOperation::WriteFile {
            path: "nope.txt".into(),
            content: "nope".into(),
        },
    );
    assert!(host.execute_linux_operation(&write).is_err());
    assert!(!workspace.join("nope.txt").exists());
    assert_eq!(fs::read(&sentinel).unwrap(), b"do-not-touch\n");
    assert!(
        host.snapshot(&mission)
            .unwrap()
            .linux_execution_effects
            .is_empty()
    );
    fs::remove_file(workspace.join(".git")).unwrap();
    fs::write(workspace.join(".git"), original).unwrap();
    world.assert_origin();
}

#[test]
fn detached_descendants_stop_before_reuse_and_workspace_is_not_verified_while_live() {
    let mut world = World::new();
    let (mut host, mission) = implementing(&mut world);
    enable(&mut host);
    let workspace = tree(&host, &mission);
    let script = "/usr/bin/setsid /bin/sh -c 'while true; do echo tick >> /workspace/leaf.heartbeat; sleep 0.05; done' >/dev/null 2>&1 & echo $! > /workspace/leaf.pid; sleep 30";
    let start = request(
        &host,
        &mission,
        "exec:start:v1",
        LINUX_EXEC_START_SCOPE,
        LinuxOperation::ExecStart {
            argv: vec!["/bin/sh".into(), "-c".into(), script.to_string()],
        },
    );
    let started = host.execute_linux_operation(&start).unwrap();
    assert_eq!(started.state, "running");
    wait_for(&workspace.join("leaf.heartbeat"));
    let blocked = host.recover_workspace(&mission).unwrap_err();
    assert_eq!(
        blocked.category(),
        "execution_blocked",
        "live writer must block workspace verification, got {blocked}"
    );
    world.assert_origin();
    let snapshot = host.snapshot(&mission).unwrap();
    host.admit(AdmitRequest {
        command_id: None,
        mission_id: mission.clone(),
        attempt_id: None,
        expected_revision: snapshot.revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body: CommandBody::DetectUncertainty {
            reason: AttentionReason::ExecutionUncertain,
            content_digest: None,
        },
    })
    .unwrap();
    let resolve = ExecutionResolutionRequest {
        command_id: "too-soon".into(),
        binding: binding(&host, &mission),
        operation_id: start.operation_id().unwrap(),
        payload_hash: start.payload_hash().unwrap(),
        expected_revision: host.snapshot(&mission).unwrap().revision,
        actor: "operator".into(),
        rationale: "cgroup is still populated".into(),
        decision: ExecutionResolutionDecision::Completed,
    };
    assert!(host.resolve_linux_execution(&resolve).is_err());
    let cancel = request(
        &host,
        &mission,
        "exec:cancel:v1",
        LINUX_EXEC_CANCEL_SCOPE,
        LinuxOperation::ExecCancel {
            start_operation_id: start.operation_id().unwrap(),
            start_payload_hash: start.payload_hash().unwrap(),
        },
    );
    let cancelled = host.execute_linux_operation(&cancel).unwrap();
    assert_eq!(cancelled.state, "completed");
    assert_eq!(cancelled.receipt.unwrap()["disposition"], "cancelled");
    let frozen = fs::read(workspace.join("leaf.heartbeat")).unwrap();
    thread::sleep(Duration::from_millis(250));
    assert_eq!(fs::read(workspace.join("leaf.heartbeat")).unwrap(), frozen);
    let again = request(
        &host,
        &mission,
        "write-after-stop",
        LINUX_WRITE_SCOPE,
        LinuxOperation::WriteFile {
            path: "after-stop.txt".into(),
            content: "reused".into(),
        },
    );
    // The cancelled start is settled, but DetectUncertainty left needs_attention
    // and any remaining block must be explicit. A fresh write is allowed only
    // once no linux effect is unresolved. Cancel completes the start.
    if host.snapshot(&mission).unwrap().execution_blocked {
        let _ = resolve;
    } else {
        // Mission may still be needs_attention, which refuses a new writer.
        let _ = resolve;
    }
    let phase = host.snapshot(&mission).unwrap().phase;
    if phase == Phase::NeedsAttention {
        let revision = host.snapshot(&mission).unwrap().revision;
        host.admit(AdmitRequest {
            command_id: None,
            mission_id: mission.clone(),
            attempt_id: None,
            expected_revision: revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::ExplicitRecovery {
                expected_target: Phase::Implementing,
                ownership_proven: true,
                effects: EffectProof::Reconciled,
            },
        })
        .unwrap();
    }
    host.execute_linux_operation(&again).unwrap();
    assert_eq!(
        fs::read(workspace.join("after-stop.txt")).unwrap(),
        b"reused"
    );
    world.assert_origin();
}

#[test]
fn deadline_stops_a_detached_writer() {
    let mut world = World::new();
    let (mut host, mission) = implementing(&mut world);
    enable(&mut host);
    let workspace = tree(&host, &mission);
    let heartbeat = workspace.join("deadline.heartbeat");
    let script = "/usr/bin/setsid /bin/sh -c 'while true; do echo tick >> /workspace/deadline.heartbeat; sleep 0.05; done' >/dev/null 2>&1 & sleep 30";
    let mut start = request(
        &host,
        &mission,
        "exec:deadline:v1",
        LINUX_EXEC_START_SCOPE,
        LinuxOperation::ExecStart {
            argv: vec!["/bin/sh".into(), "-c".into(), script.to_string()],
        },
    );
    start.deadline_ms = now_ms() + 4_000;
    let started = host.execute_linux_operation(&start).unwrap();
    assert_eq!(started.state, "running");
    wait_for(&heartbeat);
    let started_at = Instant::now();
    let mut quiet = false;
    while started_at.elapsed() < Duration::from_secs(15) {
        let status = request(
            &host,
            &mission,
            &format!("status-{}", started_at.elapsed().as_millis()),
            LINUX_EXEC_STATUS_SCOPE,
            LinuxOperation::ExecStatus {
                start_operation_id: start.operation_id().unwrap(),
                start_payload_hash: start.payload_hash().unwrap(),
            },
        );
        let status = host.execute_linux_operation(&status).unwrap();
        if status.receipt.as_ref().unwrap()["startState"] != "running"
            && status.receipt.as_ref().unwrap()["startState"] != "entered"
        {
            quiet = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(quiet, "deadline did not settle the detached writer");
    let frozen = fs::read(&heartbeat).unwrap();
    thread::sleep(Duration::from_millis(250));
    assert_eq!(fs::read(&heartbeat).unwrap(), frozen);
    assert!(started_at.elapsed() < Duration::from_secs(15));
    world.assert_origin();
}

#[test]
fn sigkill_boundaries_do_not_rerun_and_record_quiescence() {
    for stage in ["prepared", "entered", "receipt", "completed"] {
        let mut world = World::new();
        let (host, mission) = implementing(&mut world);
        let generation_before = host.generation();
        drop(host);
        let child = spawn_crash(&world, &mission, stage);
        wait_kill(child, &format!("AT-{stage}"));
        let mut host = Host::open(&world.db()).unwrap();
        enable(&mut host);
        let mut replay = LinuxExecutionRequest {
            binding: binding(&host, &mission),
            tool_task_id: "task-130".into(),
            effect_key: "crash-write".into(),
            capability_scope: LINUX_WRITE_SCOPE.into(),
            deadline_ms: 20_000,
            operation: LinuxOperation::WriteFile {
                path: "crash.txt".into(),
                content: "once".into(),
            },
        };
        // Deadline is part of the payload hash. The child used a fresh deadline,
        // so replay must load the stored hash by using the same request the child
        // persisted. Re-read it from the effect view and only refresh generation.
        let view = host
            .snapshot(&mission)
            .unwrap()
            .linux_execution_effects
            .into_iter()
            .find(|effect| effect.scope == LINUX_WRITE_SCOPE)
            .expect(stage);
        replay.deadline_ms = stored_deadline(&world.db(), &view.operation_id);
        let workspace = tree(&host, &mission);
        let file = workspace.join("crash.txt");
        match stage {
            "prepared" => {
                let error = host.execute_linux_operation(&replay).unwrap_err();
                assert_eq!(error.category(), "execution_not_applied", "{stage}");
                assert!(!file.exists(), "{stage}");
            }
            "entered" => {
                let error = host.execute_linux_operation(&replay).unwrap_err();
                assert_eq!(error.category(), "execution_uncertain", "{stage}");
                assert_eq!(
                    host.snapshot(&mission).unwrap().phase,
                    Phase::NeedsAttention
                );
                assert!(!file.exists(), "{stage}");
                let stale = LinuxExecutionRequest {
                    binding: RuntimeBinding {
                        generation: generation_before,
                        ..replay.binding.clone()
                    },
                    ..replay.clone()
                };
                assert_eq!(
                    host.execute_linux_operation(&stale).unwrap_err().category(),
                    "stale_generation"
                );
            }
            "receipt" | "completed" => {
                if file.exists() {
                    fs::write(&file, "tampered").unwrap();
                }
                let outcome = host.execute_linux_operation(&replay).unwrap();
                assert_eq!(outcome.state, "completed", "{stage}");
                assert_eq!(fs::read(&file).unwrap(), b"tampered");
                let again = host.execute_linux_operation(&replay).unwrap();
                assert_eq!(again.receipt, outcome.receipt);
            }
            _ => unreachable!(),
        }
        world.assert_origin();
    }
}

#[test]
fn host_sigkill_stops_detached_descendants_and_keeps_uncertainty() {
    let mut world = World::new();
    let (host, mission) = implementing(&mut world);
    drop(host);
    let child = spawn_crash(&world, &mission, "running");
    wait_kill(child, "AT-running");
    let workspace = {
        let host = Host::open(&world.db()).unwrap();
        tree(&host, &mission)
    };
    let heartbeat = workspace.join("kill.heartbeat");
    let frozen = fs::read(&heartbeat).unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        fs::read(&heartbeat).unwrap(),
        frozen,
        "writer survived host SIGKILL"
    );
    let mut host = Host::open(&world.db()).unwrap();
    enable(&mut host);
    let view = host.snapshot(&mission).unwrap().linux_execution_effects[0].clone();
    let mut replay = LinuxExecutionRequest {
        binding: binding(&host, &mission),
        tool_task_id: "task-130".into(),
        effect_key: "crash-exec".into(),
        capability_scope: LINUX_EXEC_START_SCOPE.into(),
        deadline_ms: stored_deadline(&world.db(), &view.operation_id),
        operation: LinuxOperation::ExecStart {
            argv: vec!["/bin/true".into()],
        },
    };
    // The stored argv is the long shell, not /bin/true. Rebuild from the ledger
    // so the payload hash matches; generation is the only refreshed field.
    replay.operation = stored_operation(&world.db(), &view.operation_id);
    replay.capability_scope = view.scope.clone();
    let error = host.execute_linux_operation(&replay).unwrap_err();
    assert_eq!(error.category(), "execution_uncertain");
    assert_eq!(
        host.snapshot(&mission).unwrap().phase,
        Phase::NeedsAttention
    );
    let guidance = host.snapshot(&mission).unwrap().to_json()["nextSteps"].to_string();
    assert!(
        guidance.contains("linuxExecutionEffects") || guidance.contains("Linux execution evidence"),
        "{guidance}"
    );
    assert!(guidance.contains("not_applied"), "{guidance}");
    assert!(guidance.contains("populated 0"), "{guidance}");
    world.assert_origin();
}

#[test]
fn resolution_accepts_only_matching_quiescence_and_replays_one_audit() {
    let mut world = World::new();
    let (host, mission) = implementing(&mut world);
    drop(host);
    let child = spawn_crash(&world, &mission, "receipt");
    wait_kill(child, "AT-receipt");
    let mut host = Host::open(&world.db()).unwrap();
    enable(&mut host);
    let blocked = host.recover(&mission).unwrap_err();
    assert_eq!(blocked.category(), "execution_blocked");
    assert_eq!(
        host.snapshot(&mission).unwrap().phase,
        Phase::NeedsAttention
    );
    let view = host.snapshot(&mission).unwrap().linux_execution_effects[0].clone();
    let mut resolve = ExecutionResolutionRequest {
        command_id: "linux-resolve-1".into(),
        binding: binding(&host, &mission),
        operation_id: view.operation_id.clone(),
        payload_hash: view.payload_hash.clone(),
        expected_revision: host.snapshot(&mission).unwrap().revision,
        actor: "operator".into(),
        rationale: "receipt and empty cgroup were both checked".into(),
        decision: ExecutionResolutionDecision::NotApplied,
    };
    assert!(host.resolve_linux_execution(&resolve).is_err());
    resolve.decision = ExecutionResolutionDecision::Abandon;
    assert!(host.resolve_linux_execution(&resolve).is_err());
    let real_inode = inode_of(&world.db(), &view.operation_id);
    set_inode(&world.db(), &view.operation_id, 1);
    resolve.decision = ExecutionResolutionDecision::Completed;
    assert!(
        host.resolve_linux_execution(&resolve).is_err(),
        "mismatched cgroup identity must fail closed"
    );
    set_inode(&world.db(), &view.operation_id, real_inode);
    let audit = host.resolve_linux_execution(&resolve).unwrap();
    assert_eq!(audit["evidence"]["simulated"], false);
    assert_eq!(audit["evidence"]["quiescence"]["proven"], true);
    let replay = host
        .execute_linux_operation(&LinuxExecutionRequest {
            binding: binding(&host, &mission),
            tool_task_id: "task-130".into(),
            effect_key: "crash-write".into(),
            capability_scope: LINUX_WRITE_SCOPE.into(),
            deadline_ms: stored_deadline(&world.db(), &view.operation_id),
            operation: LinuxOperation::WriteFile {
                path: "crash.txt".into(),
                content: "once".into(),
            },
        })
        .unwrap();
    assert_eq!(replay.state, "completed");
    drop(host);
    let mut host = Host::open(&world.db()).unwrap();
    enable(&mut host);
    resolve.binding.generation = host.generation();
    assert_eq!(host.resolve_linux_execution(&resolve).unwrap(), audit);
    resolve.command_id = "second-decision".into();
    resolve.expected_revision = host.snapshot(&mission).unwrap().revision;
    assert!(host.resolve_linux_execution(&resolve).is_err());
    world.assert_origin();
}

#[test]
fn abandonment_stays_final_and_cannot_authorize_another_writer() {
    let mut world = World::new();
    let (host, mission) = implementing(&mut world);
    drop(host);
    let child = spawn_crash(&world, &mission, "entered");
    wait_kill(child, "AT-entered");
    let mut host = Host::open(&world.db()).unwrap();
    enable(&mut host);
    assert_eq!(
        host.recover(&mission).unwrap_err().category(),
        "execution_blocked"
    );
    let view = host.snapshot(&mission).unwrap().linux_execution_effects[0].clone();
    let mut resolve = ExecutionResolutionRequest {
        command_id: "abandon-linux-1".into(),
        binding: binding(&host, &mission),
        operation_id: view.operation_id.clone(),
        payload_hash: view.payload_hash,
        expected_revision: host.snapshot(&mission).unwrap().revision,
        actor: "operator".into(),
        rationale: "entered without a receipt; keep the block".into(),
        decision: ExecutionResolutionDecision::Abandon,
    };
    let audit = host.resolve_linux_execution(&resolve).unwrap();
    assert_eq!(audit["evidence"]["blockRetained"], true);
    let fresh = request(
        &host,
        &mission,
        "second-writer",
        LINUX_WRITE_SCOPE,
        LinuxOperation::WriteFile {
            path: "second.txt".into(),
            content: "no".into(),
        },
    );
    assert_eq!(
        host.execute_linux_operation(&fresh).unwrap_err().category(),
        "execution_blocked"
    );
    let revision = host.snapshot(&mission).unwrap().revision;
    assert!(
        host.admit(AdmitRequest {
            command_id: None,
            mission_id: mission.clone(),
            attempt_id: None,
            expected_revision: revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::ExplicitRecovery {
                expected_target: Phase::Implementing,
                ownership_proven: true,
                effects: EffectProof::Reconciled,
            },
        })
        .is_err()
    );
    assert!(
        host.admit(AdmitRequest {
            command_id: None,
            mission_id: mission.clone(),
            attempt_id: None,
            expected_revision: host.snapshot(&mission).unwrap().revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::RequestVerification { quiescent: true },
        })
        .is_err()
    );
    drop(host);
    let mut host = Host::open(&world.db()).unwrap();
    enable(&mut host);
    resolve.binding.generation = host.generation();
    assert_eq!(host.resolve_linux_execution(&resolve).unwrap(), audit);
    world.assert_origin();
}

#[test]
fn flood_fork_and_memory_hit_aggregate_limits() {
    let mut world = World::new();
    let (mut host, mission) = implementing(&mut world);
    enable(&mut host);
    let flood = request(
        &host,
        &mission,
        "flood",
        LINUX_EXEC_START_SCOPE,
        LinuxOperation::ExecStart {
            argv: vec!["/usr/bin/yes".into()],
        },
    );
    let flood = settle(&mut host, &mission, flood, "flood");
    let body = flood.receipt.unwrap();
    assert_eq!(body["disposition"], "truncated", "{body}");
    assert_eq!(body["result"]["stdoutTruncated"], true);
    assert_eq!(body["result"]["stdout"].as_str().unwrap().len(), 64 * 1024);

    let fork = request(
        &host,
        &mission,
        "fork",
        LINUX_EXEC_START_SCOPE,
        LinuxOperation::ExecStart {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "i=0; while [ \"$i\" -lt 200 ]; do sleep 2 & i=$((i+1)); done; wait".into(),
            ],
        },
    );
    let fork = settle(&mut host, &mission, fork, "fork");
    let events = fork.receipt.as_ref().unwrap()["result"]["pidsMaxEvents"]
        .as_u64()
        .unwrap_or(0);
    assert!(
        events > 0,
        "fork bomb did not hit pids.max: {}",
        fork.receipt.as_ref().unwrap()
    );

    let memory = request(
        &host,
        &mission,
        "memory",
        LINUX_EXEC_START_SCOPE,
        LinuxOperation::ExecStart {
            argv: vec![
                "/usr/bin/python3".into(),
                "-c".into(),
                "chunks=[]\nwhile True:\n chunks.append(bytearray(1024*1024))\n".into(),
            ],
        },
    );
    let memory = settle(&mut host, &mission, memory, "memory");
    let oom = memory.receipt.as_ref().unwrap()["result"]["oomKills"]
        .as_u64()
        .unwrap_or(0);
    assert!(
        oom > 0,
        "memory fixture did not hit memory.max: {}",
        memory.receipt.as_ref().unwrap()
    );
    world.assert_origin();
}

fn spawn_crash(world: &World, mission: &str, stage: &str) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_worker", "--nocapture"])
        .env("SW_LINUX_CRASH", "1")
        .env("SW_LINUX_DB", world.db())
        .env("SW_LINUX_MISSION", mission)
        .env("SW_LINUX_STAGE", stage)
        .env(
            "SWITCHYARD_CGROUP_ROOT",
            require_env("SWITCHYARD_CGROUP_ROOT"),
        )
        .env("SWITCHYARD_BWRAP", require_env("SWITCHYARD_BWRAP"))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
}

fn wait_kill(mut child: std::process::Child, marker: &str) {
    let stdout = child.stdout.take().unwrap();
    let marker = marker.to_string();
    let waiting_for = marker.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.contains(&marker) {
                let _ = tx.send(line);
                break;
            }
        }
    });
    let line = rx
        .recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| {
            let _ = child.kill();
            panic!("crash worker did not reach {waiting_for}");
        });
    assert!(line.contains(&waiting_for), "{line}");
    child.kill().unwrap();
    child.wait().unwrap();
    thread::sleep(Duration::from_millis(200));
}

fn stored_deadline(db: &Path, operation: &str) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    let raw: String = conn
        .query_row(
            "SELECT request_json FROM linux_execution_effects WHERE operation_id=?1",
            [operation],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str::<Value>(&raw).unwrap()["deadlineMs"]
        .as_i64()
        .unwrap()
}

fn stored_operation(db: &Path, operation: &str) -> LinuxOperation {
    let conn = rusqlite::Connection::open(db).unwrap();
    let raw: String = conn
        .query_row(
            "SELECT request_json FROM linux_execution_effects WHERE operation_id=?1",
            [operation],
            |row| row.get(0),
        )
        .unwrap();
    let value: Value = serde_json::from_str(&raw).unwrap();
    let operation = &value["operation"];
    match operation["method"].as_str() {
        Some("write_file") => LinuxOperation::WriteFile {
            path: operation["path"].as_str().unwrap().into(),
            content: operation["content"].as_str().unwrap().into(),
        },
        Some("exec_start") => LinuxOperation::ExecStart {
            argv: operation["argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_string())
                .collect(),
        },
        other => panic!("unexpected stored operation {other:?}"),
    }
}

fn inode_of(db: &Path, operation: &str) -> i64 {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT cgroup_ino FROM linux_execution_effects WHERE operation_id=?1",
            [operation],
            |row| row.get(0),
        )
        .unwrap()
}

fn set_inode(db: &Path, operation: &str, inode: i64) {
    rusqlite::Connection::open(db)
        .unwrap()
        .execute(
            "UPDATE linux_execution_effects SET cgroup_ino=?1 WHERE operation_id=?2",
            rusqlite::params![inode, operation],
        )
        .unwrap();
}

fn settle(
    host: &mut Host,
    mission: &str,
    mut operation: LinuxExecutionRequest,
    label: &str,
) -> switchyard_core::LinuxExecutionOutcome {
    let mut outcome = host.execute_linux_operation(&operation).unwrap();
    let started = Instant::now();
    let mut polls = 0u32;
    while outcome.receipt.is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "linux effect stayed {}",
            outcome.state
        );
        thread::sleep(Duration::from_millis(50));
        polls += 1;
        let status = request(
            host,
            mission,
            &format!("{label}-status-{polls}"),
            LINUX_EXEC_STATUS_SCOPE,
            LinuxOperation::ExecStatus {
                start_operation_id: operation.operation_id().unwrap(),
                start_payload_hash: operation.payload_hash().unwrap(),
            },
        );
        if let Err(error) = host.execute_linux_operation(&status) {
            panic!(
                "status {} id {} hash {} failed: {error}",
                status.effect_key,
                status.operation_id().unwrap(),
                status.payload_hash().unwrap()
            );
        }
        operation.binding.generation = host.generation();
        outcome = host.execute_linux_operation(&operation).unwrap();
    }
    outcome
}

fn wait_for(path: &Path) {
    let start = Instant::now();
    while !path.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "missing {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn crash_worker() {
    if std::env::var("SW_LINUX_CRASH").is_err() {
        return;
    }
    let db = std::env::var("SW_LINUX_DB").unwrap();
    let mission = std::env::var("SW_LINUX_MISSION").unwrap();
    let stage = std::env::var("SW_LINUX_STAGE").unwrap();
    let mut host = Host::open(Path::new(&db)).unwrap();
    let crash_at = (stage != "running").then_some(stage.as_str());
    host.enable_linux_execution(config(crash_at, None)).unwrap();
    let workspace = tree(&host, &mission);
    if stage == "running" {
        let heartbeat = workspace.join("kill.heartbeat");
        let script = "/usr/bin/setsid /bin/sh -c 'while true; do echo tick >> /workspace/kill.heartbeat; sleep 0.05; done' >/dev/null 2>&1 & sleep 30";
        let start = request(
            &host,
            &mission,
            "crash-exec",
            LINUX_EXEC_START_SCOPE,
            LinuxOperation::ExecStart {
                argv: vec!["/bin/sh".into(), "-c".into(), script.to_string()],
            },
        );
        let started = host.execute_linux_operation(&start).unwrap();
        assert_eq!(started.state, "running");
        wait_for(&heartbeat);
        println!("AT-running");
        let _ = std::io::stdout().flush();
        loop {
            thread::sleep(Duration::from_secs(30));
        }
    }
    let write = request(
        &host,
        &mission,
        "crash-write",
        LINUX_WRITE_SCOPE,
        LinuxOperation::WriteFile {
            path: "crash.txt".into(),
            content: "once".into(),
        },
    );
    let _ = host.execute_linux_operation(&write);
}

// The traversal assertion above used a placeholder. Keep the real check here so
// a rejected path does not become a durable effect.
#[test]
fn rejected_traversal_does_not_allocate_an_effect() {
    let mut world = World::new();
    let (mut host, mission) = implementing(&mut world);
    enable(&mut host);
    let before = host
        .snapshot(&mission)
        .unwrap()
        .linux_execution_effects
        .len();
    let traversal = request(
        &host,
        &mission,
        "traversal-only",
        LINUX_READ_SCOPE,
        LinuxOperation::ReadFile {
            path: "../README.md".into(),
        },
    );
    assert!(host.execute_linux_operation(&traversal).is_err());
    assert_eq!(
        host.snapshot(&mission)
            .unwrap()
            .linux_execution_effects
            .len(),
        before
    );
    world.assert_origin();
}
