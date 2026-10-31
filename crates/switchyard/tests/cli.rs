use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_switchyard")
}

fn init_repo(repo: &Path) {
    let git = |args: &[&str]| {
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
    };
    std::fs::create_dir_all(repo).unwrap();
    git(&["init", "--initial-branch=main"]);
    git(&["config", "user.email", "switchyard@example.com"]);
    git(&["config", "user.name", "Switchyard"]);
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    git(&["add", "README.md"]);
    git(&["commit", "-m", "init"]);
}

fn run(args: &[&str]) -> (i32, Value, Value) {
    let output = Command::new(bin())
        .args(args)
        .env("GH_TOKEN", "SENTINEL_GH_TOKEN_SHOULD_NOT_PERSIST")
        .env(
            "ANTHROPIC_API_KEY",
            "SENTINEL_ANTHROPIC_KEY_SHOULD_NOT_PERSIST",
        )
        .output()
        .unwrap();
    let stdout = serde_json::from_slice::<Value>(&output.stdout).unwrap_or(Value::Null);
    let stderr = serde_json::from_slice::<Value>(&output.stderr).unwrap_or(Value::Null);
    (output.status.code().unwrap_or(1), stdout, stderr)
}

fn head(repo: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn create_status_and_recover_keep_the_same_attempt() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let before = head(&repo);
    let (code, stdout, _) = run(&[
        "mission",
        "create",
        "--db",
        db.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "95",
        "--objective",
        "Establish the ledger",
        "--acceptance",
        "Restart restores the attempt",
        "--admit",
        "start_planning",
        "--persist-only",
    ]);
    assert_eq!(code, 0, "{stdout}");
    let mission = &stdout["mission"];
    let mission_id = mission["missionId"].as_str().unwrap();
    let attempt_id = mission["attemptId"].as_str().unwrap();
    assert_eq!(mission["phase"], "planning");
    assert_eq!(mission["pendingCommands"].as_array().unwrap().len(), 1);
    assert_eq!(mission["baseSha"], before.as_str());
    assert_eq!(head(&repo), before);

    let (code, status, _) = run(&[
        "mission",
        "status",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission_id,
    ]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["readOnly"], true);
    assert_eq!(status["missions"][0]["attemptId"], attempt_id);
    assert_eq!(
        status["missions"][0]["pendingCommands"][0]["id"],
        mission["pendingCommands"][0]["id"]
    );

    let (code, recovered, _) = run(&[
        "mission",
        "recover",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission_id,
    ]);
    assert_eq!(code, 0, "{recovered}");
    assert_eq!(recovered["mission"]["attemptId"], attempt_id);
    assert_eq!(recovered["mission"]["missionId"], mission_id);
    assert_eq!(recovered["mission"]["phase"], "planning");
    assert_eq!(
        recovered["mission"]["pendingCommands"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(recovered["mission"]["commands"][0]["state"], "acknowledged");
    assert_eq!(head(&repo), before);

    let stored = ledger_text(&db);
    assert!(
        stored.contains("\"credentials\":\"none\"") || stored.contains("\"credentials\": \"none\"")
    );
    assert!(!stored.contains("SENTINEL_GH_TOKEN_SHOULD_NOT_PERSIST"));
    assert!(!stored.contains("SENTINEL_ANTHROPIC_KEY_SHOULD_NOT_PERSIST"));
}

#[test]
fn explicit_workspace_command_and_recovery_keep_identity_without_inherited_git_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    let root = tmp.path().join("workspaces");
    std::fs::create_dir(&root).unwrap();
    init_repo(&repo);
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "118",
        "--objective",
        "workspace",
        "--acceptance",
        "same workspace",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let attempt = created["mission"]["attemptId"].as_str().unwrap();
    let output = Command::new(bin())
        .args([
            "mission",
            "workspace",
            "--db",
            db.to_str().unwrap(),
            "--mission",
            mission,
            "--attempt",
            attempt,
            "--root",
            root.to_str().unwrap(),
        ])
        .env("GIT_DIR", "/nonexistent/git-authority")
        .env("GIT_WORK_TREE", "/nonexistent/worktree")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.worktree")
        .env("GIT_CONFIG_VALUE_0", "/nonexistent/injected")
        .env("GH_TOKEN", "WORKSPACE_TOKEN_MUST_NOT_PERSIST")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let prepared: Value = serde_json::from_slice(&output.stdout).unwrap();
    let workspace = &prepared["workspace"];
    assert_eq!(workspace["state"], "ready");
    assert_eq!(workspace["baseSha"], created["mission"]["baseSha"]);
    let (code, recovered, error) = run(&[
        "mission",
        "recover",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(recovered["mission"]["workspace"]["id"], workspace["id"]);
    assert_eq!(recovered["mission"]["workspace"]["path"], workspace["path"]);
    assert_eq!(head(&repo), created["mission"]["baseSha"].as_str().unwrap());
    let slot = root.join(attempt);
    for path in [db, slot.join("owner.json"), slot.join("git-create.log")] {
        assert!(
            !String::from_utf8_lossy(&std::fs::read(path).unwrap())
                .contains("WORKSPACE_TOKEN_MUST_NOT_PERSIST")
        );
    }
}

#[test]
fn mission_command_reconciles_a_prepared_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    let root = tmp.path().join("workspaces");
    std::fs::create_dir(&root).unwrap();
    init_repo(&repo);
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "118",
        "--objective",
        "workspace command",
        "--acceptance",
        "command reconciles",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let attempt = created["mission"]["attemptId"].as_str().unwrap();
    let revision = created["mission"]["revision"].as_u64().unwrap();
    let (code, prepared, error) = run(&[
        "mission",
        "workspace",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--attempt",
        attempt,
        "--root",
        root.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{error}");
    let (code, commanded, error) = run(&[
        "mission",
        "command",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--kind",
        "start_planning",
        "--expected-revision",
        &revision.to_string(),
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(commanded["mission"]["phase"], "planning");
    assert_eq!(commanded["mission"]["commands"][0]["state"], "acknowledged");
    assert_eq!(
        commanded["mission"]["pendingCommands"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(commanded["mission"]["workspace"]["state"], "ready");
    assert_eq!(
        commanded["mission"]["workspace"]["id"],
        prepared["workspace"]["id"]
    );
    assert_ne!(
        commanded["mission"]["workspace"]["verifiedGeneration"],
        prepared["workspace"]["verifiedGeneration"]
    );
}

#[test]
fn uncertain_workspace_admits_pause_and_cancel_without_dispatch() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    let root = tmp.path().join("workspaces");
    std::fs::create_dir(&root).unwrap();
    init_repo(&repo);
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "118",
        "--objective",
        "stop despite uncertainty",
        "--acceptance",
        "pause and cancel are recorded",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let attempt = created["mission"]["attemptId"].as_str().unwrap();
    let revision = created["mission"]["revision"].as_u64().unwrap();
    let (code, _, error) = run(&[
        "mission",
        "workspace",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--attempt",
        attempt,
        "--root",
        root.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{error}");
    std::fs::remove_file(root.join(attempt).join("owner.json")).unwrap();

    let (code, paused, error) = run(&[
        "mission",
        "command",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--kind",
        "pause",
        "--expected-revision",
        &revision.to_string(),
        "--persist-only",
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(paused["commandState"], "pending");
    assert_eq!(paused["mission"]["phase"], "pausing");
    assert_eq!(paused["mission"]["workspace"]["state"], "ready");
    let paused_revision = paused["mission"]["revision"].as_u64().unwrap();

    let (code, cancelled, error) = run(&[
        "mission",
        "command",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--kind",
        "cancel",
        "--expected-revision",
        &paused_revision.to_string(),
    ]);
    assert_eq!(code, 3, "{cancelled} {error}");
    assert!(error.is_null(), "{error}");
    assert_eq!(cancelled["ok"], true);
    assert_eq!(cancelled["dispatched"], false);
    assert_eq!(cancelled["commandState"], "pending");
    assert!(cancelled["commandId"].as_str().unwrap().starts_with("c-"));
    assert!(cancelled["revision"].as_u64().unwrap() > paused_revision);
    assert_eq!(
        cancelled["workspaceError"]["category"],
        "workspace_uncertain"
    );
    assert_eq!(cancelled["mission"]["phase"], "cancelling");
    let recorded_revision = cancelled["revision"].as_u64().unwrap();

    let (code, _, error) = run(&[
        "mission",
        "command",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--kind",
        "cancel",
        "--expected-revision",
        &paused_revision.to_string(),
    ]);
    assert_eq!(
        code, 2,
        "a retry must be a rejection, not another recorded cancel"
    );
    assert_eq!(error["error"]["category"], "stale_revision");
    assert_eq!(error["ok"], false);

    let (code, status, error) = run(&[
        "mission",
        "status",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
    ]);
    assert_eq!(code, 0, "{error}");
    let mission_status = &status["missions"][0];
    assert_eq!(mission_status["phase"], "cancelling");
    assert_eq!(mission_status["revision"], recorded_revision);
    assert_eq!(mission_status["workspace"]["state"], "uncertain");
    let commands = mission_status["commands"].as_array().unwrap();
    assert!(
        commands
            .iter()
            .any(|command| command["kind"] == "pause" && command["state"] == "pending")
    );
    assert!(
        commands
            .iter()
            .any(|command| command["kind"] == "cancel" && command["state"] == "pending")
    );
    assert_eq!(
        commands
            .iter()
            .filter(|command| command["kind"] == "cancel")
            .count(),
        1
    );
}

#[test]
fn second_owner_is_rejected_while_status_still_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("mission.sqlite");
    let mut child = Command::new(bin())
        .args(["mission", "lock", "--db", db.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let holding: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(holding["holding"], true);

    let (code, status, _) = run(&["mission", "status", "--db", db.to_str().unwrap()]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["missions"].as_array().unwrap().len(), 0);

    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let (code, _, stderr) = run(&[
        "mission",
        "create",
        "--db",
        db.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "95",
        "--objective",
        "blocked",
        "--acceptance",
        "blocked",
    ]);
    assert_eq!(code, 2);
    assert_eq!(stderr["error"]["category"], "second_active_owner");

    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    assert!(child.wait().unwrap().success());

    let (code, created, _) = run(&[
        "mission",
        "create",
        "--db",
        db.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "95",
        "--objective",
        "after release",
        "--acceptance",
        "one owner",
    ]);
    assert_eq!(code, 0, "{created}");
    assert_eq!(created["mission"]["phase"], "draft");
}

#[test]
fn git_dir_from_the_parent_does_not_replace_the_requested_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let other = tmp.path().join("other");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    init_repo(&other);
    std::fs::write(other.join("other.txt"), "other\n").unwrap();
    let git = |dir: &Path, args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
    };
    git(&other, &["add", "other.txt"]);
    git(&other, &["commit", "-m", "other"]);
    let wanted = head(&repo);
    let decoy = head(&other);
    assert_ne!(wanted, decoy);
    let output = Command::new(bin())
        .args([
            "mission",
            "create",
            "--db",
            db.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--issue",
            "95",
            "--objective",
            "identity",
            "--acceptance",
            "the requested repo",
        ])
        .env("GIT_DIR", other.join(".git"))
        .env("GIT_WORK_TREE", &other)
        .env("GH_TOKEN", "SENTINEL_GH_TOKEN_SHOULD_NOT_PERSIST")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["mission"]["baseSha"], wanted.as_str());
    assert_ne!(stdout["mission"]["baseSha"], decoy.as_str());
}

fn ledger_text(db: &Path) -> String {
    let mut bytes = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        let path = std::path::PathBuf::from(format!("{}{suffix}", db.display()));
        if let Ok(chunk) = std::fs::read(path) {
            bytes.extend(chunk);
        }
    }
    let journal = std::path::PathBuf::from(format!("{}.fake-runtime.json", db.display()));
    if let Ok(chunk) = std::fs::read(journal) {
        bytes.extend(chunk);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn abandon_execution_is_audited_idempotently_and_keeps_the_block() {
    use switchyard_core::*;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    let root = tmp.path().join("workspaces");
    init_repo(&repo);
    std::fs::create_dir(&root).unwrap();
    let mut host = Host::open(&db).unwrap();
    let m = host
        .create_mission(CreateMission {
            repo,
            issue_number: 125,
            issue_title: "resolution".into(),
            objective: "audit".into(),
            acceptance: "retain block".into(),
            actor: "operator".into(),
            max_repairs: 1,
            max_plan_revisions: 1,
        })
        .unwrap();
    host.prepare_workspace(&m.mission_id, &m.attempt_id, &root)
        .unwrap();
    let admit = |host: &mut Host, body| {
        host.admit(AdmitRequest {
            command_id: None,
            mission_id: m.mission_id.clone(),
            attempt_id: None,
            expected_revision: host.snapshot(&m.mission_id).unwrap().revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body,
        })
        .unwrap();
    };
    admit(&mut host, CommandBody::StartPlanning);
    host.dispatch_pending(&m.mission_id).unwrap();
    // Bind the simulation observation to the canonical plan digest.
    host.apply_notification(
        &m.mission_id,
        parse_notification(&serde_json::json!({
            "kind":"plan_completed", "plan":"plan", "quiescent":true,
            "digest":payload_hash(&serde_json::json!("plan")).unwrap()
        }))
        .unwrap(),
    )
    .unwrap();
    let snapshot = host.snapshot(&m.mission_id).unwrap();
    admit(
        &mut host,
        CommandBody::ApprovePlan {
            plan_digest: snapshot.plan_digest.unwrap(),
        },
    );
    rusqlite::Connection::open(&db).unwrap().execute_batch(
        "CREATE TRIGGER fail_completion BEFORE UPDATE OF state ON execution_effects WHEN NEW.state='completed'
         BEGIN SELECT RAISE(ABORT, 'lost simulated receipt'); END;",
    ).unwrap();
    let request = ExecutionRequest {
        binding: RuntimeBinding {
            mission_id: m.mission_id.clone(),
            attempt_id: m.attempt_id.clone(),
            session_id: m.session_id.clone(),
            generation: host.generation(),
        },
        tool_task_id: "task".into(),
        effect_key: "write".into(),
        capability_scope: FAKE_WRITE_SCOPE.into(),
        deadline_ms: i64::MAX,
        operation: ExecutionOperation::WriteFile {
            path: "out.txt".into(),
            content: "data".into(),
        },
    };
    assert!(host.execute_fake_operation(&request).is_err());
    let snapshot = host.snapshot(&m.mission_id).unwrap();
    assert_eq!(snapshot.phase, Phase::NeedsAttention);
    let effect = &snapshot.execution_effects[0];
    let revision = snapshot.revision.to_string();
    drop(host);
    let args = [
        "mission",
        "abandon-execution",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        &m.mission_id,
        "--attempt",
        &m.attempt_id,
        "--session",
        &m.session_id,
        "--operation-id",
        &effect.operation_id,
        "--payload-hash",
        &effect.payload_hash,
        "--expected-revision",
        &revision,
        "--command-id",
        "resolution-cli-1",
        "--actor",
        "operator",
        "--rationale",
        "Evidence lost on restart",
    ];
    let (code, first, err) = run(&args);
    assert_eq!(code, 0, "{err}");
    assert_eq!(first["executionBlockRetained"], true);
    let guidance = first["nextSteps"].as_str().unwrap();
    assert!(guidance.contains("needs_attention"));
    assert!(guidance.contains("prevent cancellation from completing"));
    assert!(guidance.contains("#125"));
    let help = Command::new(bin())
        .args(["mission", "abandon-execution", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("needs_attention"));
    assert!(help.contains("prevent cancellation from completing"));
    assert!(help.contains("#125"));
    let (code, replay, err) = run(&args);
    assert_eq!(code, 0, "{err}");
    assert_eq!(replay["resolution"], first["resolution"]);
    assert_eq!(replay["mission"]["revision"], first["mission"]["revision"]);
    assert_eq!(replay["mission"]["phase"], "needs_attention");
    assert_eq!(
        replay["mission"]["executionEffects"][0]["state"],
        "uncertain"
    );
    assert_eq!(
        replay["mission"]["executionResolutions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
