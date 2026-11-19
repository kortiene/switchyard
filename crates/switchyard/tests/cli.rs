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

fn count_rows(db: &Path, sql: &str) -> i64 {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

fn owner_lock(db: &Path) -> std::path::PathBuf {
    let mut name = db.as_os_str().to_os_string();
    name.push(".owner.lock");
    std::path::PathBuf::from(name)
}

fn host_generation(db: &Path) -> i64 {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT generation FROM host_owner WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
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
fn uncertain_workspace_delivers_pause_and_cancel_and_reports_the_block() {
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

    let paused_revision_arg = paused_revision.to_string();
    let cancel_args = [
        "mission",
        "command",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--kind",
        "cancel",
        "--expected-revision",
        &paused_revision_arg,
        "--command-id",
        "c-cancel-uncertain",
    ];
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_stop_ack BEFORE UPDATE OF state ON commands
        WHEN NEW.state='acknowledged' BEGIN SELECT RAISE(ABORT,'injected stop acknowledgement failure'); END;").unwrap();
    let (code, _, error) = run(&cancel_args);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["error"]["category"], "storage_failed");
    let diagnostic = error["error"]["message"].as_str().unwrap();
    assert!(
        diagnostic.contains("injected stop acknowledgement failure"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("workspace reconciliation also failed"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("uncertain"), "{diagnostic}");
    conn.execute_batch("DROP TRIGGER reject_stop_ack;").unwrap();
    let (code, cancelled, error) = run(&cancel_args);

    assert_eq!(code, 3, "{cancelled} {error}");
    assert!(error.is_null(), "{error}");
    assert_eq!(cancelled["ok"], true);
    assert_eq!(cancelled["dispatched"], true);
    assert_eq!(cancelled["commandState"], "acknowledged");
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
    let guidance = mission_status["nextSteps"].to_string();
    assert!(guidance.contains("was delivered"), "{guidance}");
    assert!(
        guidance.contains("Workspace remains uncertain"),
        "{guidance}"
    );
    let commands = mission_status["commands"].as_array().unwrap();
    assert!(
        commands
            .iter()
            .any(|command| command["kind"] == "pause" && command["state"] == "acknowledged")
    );
    assert!(
        commands
            .iter()
            .any(|command| command["kind"] == "cancel" && command["state"] == "acknowledged")
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
    host.apply_notification(
        &m.mission_id,
        parse_notification(&serde_json::json!({
            "kind":"plan_completed", "plan":"plan", "quiescent":true
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
    assert_eq!(
        host.snapshot(&m.mission_id).unwrap().phase,
        Phase::NeedsAttention
    );
    admit(&mut host, CommandBody::Cancel);
    host.dispatch_stops(&m.mission_id).unwrap();
    let snapshot = host.snapshot(&m.mission_id).unwrap();
    assert_eq!(snapshot.phase, Phase::Cancelling);
    assert!(
        snapshot.to_json()["nextSteps"]
            .to_string()
            .contains("detect_uncertainty")
    );
    let effect = &snapshot.execution_effects[0];
    let stop_revision = snapshot.revision.to_string();
    let guidance = snapshot.to_json()["nextSteps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .find(|step| step.contains("detect_uncertainty"))
        .unwrap()
        .to_string();
    assert!(guidance.contains(&format!("--mission {}", m.mission_id)));
    assert!(guidance.contains(&format!("--attempt {}", m.attempt_id)));
    assert!(guidance.contains(&format!("--expected-revision {stop_revision}")));
    assert!(guidance.contains("--db '<DB_PATH>'"));
    assert!(guidance.contains("--command-id '<STABLE_COMMAND_ID>'"));
    drop(host);
    let (code, attention, err) = run(&[
        "mission",
        "command",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        &m.mission_id,
        "--attempt",
        &m.attempt_id,
        "--kind",
        "detect_uncertainty",
        "--payload",
        r#"{"reason":"execution_uncertain"}"#,
        "--expected-revision",
        &stop_revision,
        "--command-id",
        "operator-detect-stop-uncertainty",
        "--persist-only",
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(attention["mission"]["phase"], "needs_attention");
    assert_eq!(attention["mission"]["priorPhase"], "cancelling");
    assert_eq!(
        attention["mission"]["commands"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["origin"],
        "operator"
    );
    let revision = attention["mission"]["revision"]
        .as_u64()
        .unwrap()
        .to_string();
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

#[test]
fn cancel_is_admitted_before_dispatching_earlier_work() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "129",
        "--objective",
        "stop before work",
        "--acceptance",
        "abort only",
        "--admit",
        "start_planning",
        "--persist-only",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let revision = created["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, stopped, error) = run(&[
        "mission",
        "command",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--kind",
        "cancel",
        "--expected-revision",
        &revision,
        "--command-id",
        "stop-once",
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(stopped["commandState"], "acknowledged");
    assert_eq!(stopped["dispatched"], true);
    let journal: Value =
        serde_json::from_slice(&std::fs::read(switchyard_core::journal_path(&db)).unwrap())
            .unwrap();
    let submissions = journal["submissions"].as_object().unwrap();
    assert_eq!(submissions.len(), 1);
    assert_eq!(submissions.values().next().unwrap()["method"], "abort");
    let (code, replay, error) = run(&[
        "mission",
        "command",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--kind",
        "cancel",
        "--expected-revision",
        &revision,
        "--command-id",
        "stop-once",
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(replay["idempotent"], true);
    assert_eq!(replay["requestId"], stopped["requestId"]);
    assert_eq!(
        replay["mission"]["commands"],
        stopped["mission"]["commands"]
    );
}

#[test]
fn supersede_execution_audits_the_simulated_journal_and_refuses_linux_rows() {
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
            issue_number: 133,
            issue_title: "supersede".into(),
            objective: "audit".into(),
            acceptance: "unknown outcome".into(),
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
    host.apply_notification(
        &m.mission_id,
        parse_notification(&serde_json::json!({
            "kind":"plan_completed", "plan":"plan", "quiescent":true
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
    drop(host);
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TRIGGER fail_completion;")
        .unwrap();
    let mut host = Host::open(&db).unwrap();
    let effect = host.snapshot(&m.mission_id).unwrap().execution_effects[0].clone();
    let revision = host.snapshot(&m.mission_id).unwrap().revision;
    let abandon = host
        .resolve_fake_execution(&ExecutionResolutionRequest {
            command_id: "abandon-cli".into(),
            binding: RuntimeBinding {
                generation: host.generation(),
                mission_id: m.mission_id.clone(),
                attempt_id: m.attempt_id.clone(),
                session_id: m.session_id.clone(),
            },
            operation_id: effect.operation_id.clone(),
            payload_hash: effect.payload_hash.clone(),
            expected_revision: revision,
            actor: "operator".into(),
            rationale: "outcome unknown after restart".into(),
            decision: ExecutionResolutionDecision::Abandon,
        })
        .unwrap();
    let revision = host.snapshot(&m.mission_id).unwrap().revision.to_string();
    drop(host);
    let args = [
        "mission",
        "supersede-execution",
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
        "--abandonment-command-id",
        abandon["commandId"].as_str().unwrap(),
        "--abandonment-request-hash",
        abandon["requestHash"].as_str().unwrap(),
        "--expected-revision",
        &revision,
        "--command-id",
        "supersede-cli-1",
        "--actor",
        "operator",
        "--rationale",
        "workspace verified; outcome stays unknown",
    ];
    let (code, first, err) = run(&args);
    assert_eq!(code, 0, "{err}");
    assert_eq!(first["outcomeKnown"], false);
    assert_eq!(first["executionBlocked"], false);
    assert_eq!(
        first["mission"]["executionEffects"][0]["state"],
        "uncertain"
    );
    assert_eq!(first["mission"]["executionEffects"][0]["superseded"], true);
    assert!(
        first["supersession"]["evidence"]["workspaceDigest"]
            .as_str()
            .unwrap()
            .len()
            >= 40
    );
    assert!(
        first["nextSteps"]
            .as_str()
            .unwrap()
            .contains("outcome stays unknown")
    );
    let (code, replay, err) = run(&args);
    assert_eq!(code, 0, "{err}");
    assert_eq!(replay["supersession"], first["supersession"]);
    assert_eq!(replay["mission"]["revision"], first["mission"]["revision"]);
    let help = Command::new(bin())
        .args(["mission", "supersede-execution", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("outcome stays unknown"));
    assert!(help.contains("Linux"));
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, cgroup_path, cgroup_dev, cgroup_ino, boot_id,
                entered_at, created_at
             ) VALUES (
                'op-v1:cli-linux', ?1, ?2, ?3, 'sha256:cli', '{}', 1, 'uncertain',
                '/sys/fs/cgroup/switchyard/sy-cli', 1, 2, 'boot', 1, 1
             )",
            rusqlite::params![&m.mission_id, &m.attempt_id, &m.session_id],
        )
        .unwrap();
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_resolutions(
                command_id, operation_id, mission_id, request_hash, audit_json
             ) VALUES (
                'abandon-cli-linux', 'op-v1:cli-linux', ?1, 'sha256:cli-abandon',
                '{\"request\":{\"decision\":\"abandoned\"}}'
             )",
            [&m.mission_id],
        )
        .unwrap();
    let linux_revision = first["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, _body, error) = run(&[
        "mission",
        "supersede-execution",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        &m.mission_id,
        "--attempt",
        &m.attempt_id,
        "--session",
        &m.session_id,
        "--operation-id",
        "op-v1:cli-linux",
        "--payload-hash",
        "sha256:cli",
        "--abandonment-command-id",
        "abandon-cli-linux",
        "--abandonment-request-hash",
        "sha256:cli-abandon",
        "--expected-revision",
        &linux_revision,
        "--command-id",
        "supersede-cli-linux",
        "--actor",
        "operator",
        "--rationale",
        "default build cannot prove the cgroup",
    ]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("delegated cgroup backend"),
        "{error}"
    );
}

#[test]
fn default_runtime_is_fake_and_explicit_fake_matches_it() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let create = |db: &Path, extra: &[&str]| {
        let mut args = vec![
            "mission",
            "create",
            "--db",
            db.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--issue",
            "97",
            "--objective",
            "keep the fake runtime",
            "--acceptance",
            "no confined adapter",
        ];
        args.extend_from_slice(extra);
        run(&args)
    };
    let omitted = tmp.path().join("omitted.sqlite");
    let (code, stdout, error) = create(&omitted, &[]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(stdout["mission"]["runtimeAdapter"], "fake");
    assert_eq!(stdout["mission"]["phase"], "draft");
    let explicit = tmp.path().join("explicit.sqlite");
    let (code, flagged, error) = create(&explicit, &["--runtime", "fake"]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(flagged["mission"]["runtimeAdapter"], "fake");
    assert_eq!(
        flagged["mission"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        stdout["mission"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>()
    );
    let stored = ledger_text(&omitted);
    assert!(stored.contains("\"adapter\":\"fake\"") || stored.contains("\"adapter\": \"fake\""));
    assert!(!stored.contains("pi-confined"));
    let mission = stdout["mission"]["missionId"].as_str().unwrap();
    let (code, status, error) = run(&[
        "mission",
        "status",
        "--runtime",
        "confined-pi",
        "--db",
        omitted.to_str().unwrap(),
        "--mission",
        mission,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(status["readOnly"], true);
    assert_eq!(status["missions"][0]["runtimeAdapter"], "fake");
    assert!(status.get("composedScenario").is_none());
}

#[test]
fn confined_pi_flag_fails_closed_without_prerequisites() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let output = Command::new(bin())
        .args([
            "mission",
            "create",
            "--runtime",
            "confined-pi",
            "--db",
            db.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--issue",
            "97",
            "--objective",
            "do not fall back",
            "--acceptance",
            "confined prerequisites are missing",
        ])
        .env_remove("SWITCHYARD_PI_RUNTIME")
        .env_remove("SWITCHYARD_BWRAP")
        .env_remove("SWITCHYARD_CGROUP_ROOT")
        .env_remove("SWITCHYARD_PI_NODE")
        .env_remove("SWITCHYARD_BRIDGE_LAUNCHER")
        .env_remove("SWITCHYARD_BRIDGE_POLICY")
        .env_remove("SWITCHYARD_EXECUTION_HELPER")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr: Value = serde_json::from_slice(&output.stderr).unwrap();
    let message = stderr["error"]["message"].as_str().unwrap();
    assert_eq!(stderr["ok"], false);
    assert_eq!(stderr["error"]["category"], "guard_failed");
    assert!(message.contains("confined Pi runtime"), "{message}");
    assert!(
        message.contains("refusing to fall back to FakeRuntime"),
        "{message}"
    );
    assert!(!db.exists());
    assert!(!tmp.path().join("mission.sqlite.pi-bridge").exists());
}

fn model_endpoint_output(args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .env_remove("SWITCHYARD_MODEL_ENDPOINT")
        .env_remove("SWITCHYARD_PI_RUNTIME")
        .env_remove("SWITCHYARD_BWRAP")
        .env_remove("SWITCHYARD_CGROUP_ROOT")
        .env_remove("SWITCHYARD_PI_NODE")
        .env_remove("SWITCHYARD_BRIDGE_LAUNCHER")
        .env_remove("SWITCHYARD_BRIDGE_POLICY")
        .env_remove("SWITCHYARD_EXECUTION_HELPER")
        .output()
        .unwrap()
}

fn assert_model_endpoint_guard(output: &std::process::Output, db: &Path, needle: &str) -> String {
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr: Value = serde_json::from_slice(&output.stderr).expect("stderr is JSON");
    assert_eq!(stderr["ok"], false);
    assert_eq!(stderr["error"]["category"], "model_endpoint_not_allowed");
    let message = stderr["error"]["message"].as_str().unwrap().to_string();
    assert!(message.contains(needle), "{message}");
    assert!(!message.contains("guard failed"), "{message}");
    assert!(!db.exists(), "{message}");
    assert!(!owner_lock(db).exists(), "{message}");
    message
}

#[test]
fn model_endpoint_flag_without_confined_pi_is_refused_before_any_database() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let repo_arg = repo.to_str().unwrap();
    let secret = "http://127.0.0.1:9/v1/sk-do-not-echo";
    let cases: &[(&str, &[&str])] = &[
        (
            "omitted-runtime.sqlite",
            &[
                "mission",
                "create",
                "--model-endpoint",
                secret,
                "--db",
                "DB",
                "--repo",
                "REPO",
                "--issue",
                "216",
                "--objective",
                "flag requires confined-pi",
                "--acceptance",
                "no database",
            ],
        ),
        (
            "explicit-fake.sqlite",
            &[
                "--model-endpoint",
                secret,
                "mission",
                "create",
                "--runtime",
                "fake",
                "--db",
                "DB",
                "--repo",
                "REPO",
                "--issue",
                "216",
                "--objective",
                "flag requires confined-pi",
                "--acceptance",
                "no database",
            ],
        ),
    ];
    for (name, template) in cases {
        let db = tmp.path().join(name);
        let db_arg = db.to_str().unwrap().to_string();
        let args: Vec<String> = template
            .iter()
            .map(|arg| match *arg {
                "DB" => db_arg.clone(),
                "REPO" => repo_arg.to_string(),
                other => (*other).to_string(),
            })
            .collect();
        let output = model_endpoint_output(&args.iter().map(String::as_str).collect::<Vec<_>>());
        let message = assert_model_endpoint_guard(
            &output,
            &db,
            "--model-endpoint is valid only with --runtime confined-pi",
        );
        assert!(!message.contains("sk-do-not-echo"), "{message}");
        assert!(!message.contains("127.0.0.1"), "{message}");
        assert!(!message.contains(secret), "{message}");
    }
}

#[test]
fn model_address_is_refused_for_every_value_and_runtime_before_any_database() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let repo_arg = repo.to_str().unwrap();
    let cases = [
        ("omitted.sqlite", None, "http://127.0.0.1:9"),
        ("fake.sqlite", Some("fake"), "localhost"),
        ("confined.sqlite", Some("confined-pi"), "127.0.0.1"),
        ("public.sqlite", Some("fake"), "8.8.8.8"),
        ("empty.sqlite", Some("confined-pi"), ""),
    ];
    for (name, runtime, address) in cases {
        let db = tmp.path().join(name);
        let mut args = vec!["mission".to_string(), "create".to_string()];
        if let Some(runtime) = runtime {
            args.extend(["--runtime".into(), runtime.into()]);
        }
        args.extend([
            "--model-address".into(),
            address.into(),
            "--db".into(),
            db.to_str().unwrap().into(),
            "--repo".into(),
            repo_arg.into(),
            "--issue".into(),
            "217".into(),
            "--objective".into(),
            "refuse the address".into(),
            "--acceptance".into(),
            "no database".into(),
        ]);
        let output = model_endpoint_output(&args.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(output.status.code(), Some(2), "{address} {output:?}");
        assert!(output.stdout.is_empty(), "{address} {output:?}");
        let stderr: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(stderr["ok"], false);
        assert_eq!(
            stderr["error"]["category"], "model_address_not_allowed",
            "{address} {stderr}"
        );
        let message = stderr["error"]["message"].as_str().unwrap();
        assert!(message.contains("omit --model-address"), "{message}");
        if !address.is_empty() {
            assert!(!message.contains(address), "{address}: {message}");
        }
        assert!(!message.contains("127.0.0.1"), "{message}");
        assert!(!message.contains("localhost"), "{message}");
        assert!(!message.contains("8.8.8.8"), "{message}");
        assert!(!db.exists(), "{address}");
        assert!(!owner_lock(&db).exists(), "{address}");
    }
}

#[test]
fn model_endpoint_https_is_refused_in_the_ungated_cli_suite() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let db = tmp.path().join("https.sqlite");
    let endpoint = "https://127.0.0.1/v1";
    let output = model_endpoint_output(&[
        "mission",
        "create",
        "--runtime",
        "confined-pi",
        "--model-endpoint",
        endpoint,
        "--db",
        db.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "217",
        "--objective",
        "https is not the splice",
        "--acceptance",
        "no database",
    ]);
    let message = assert_model_endpoint_guard(
        &output,
        &db,
        "HTTPS model endpoints require a TLS client; only credential-free HTTP routes are supported",
    );
    assert!(!message.contains(endpoint), "{message}");
    assert!(!message.contains("127.0.0.1"), "{message}");
}

#[test]
fn model_endpoint_path_other_than_exact_v1_is_refused_without_a_database() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    // Empty path, `/`, `/v1/`, another path, and a query. No normalisation.
    let cases = [
        "http://127.0.0.1:9",
        "http://127.0.0.1:9/",
        "http://127.0.0.1:9/v1/",
        "http://127.0.0.1:9/v2",
        "http://127.0.0.1:9/v1?q=1",
        "http://127.0.0.1:9/v1#frag",
    ];
    for endpoint in cases {
        let db = tmp.path().join(format!("path-{}.sqlite", endpoint.len()));
        let output = model_endpoint_output(&[
            "mission",
            "create",
            "--runtime",
            "confined-pi",
            "--model-endpoint",
            endpoint,
            "--db",
            db.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--issue",
            "217",
            "--objective",
            "path must be exact v1",
            "--acceptance",
            "no database",
        ]);
        let message = assert_model_endpoint_guard(
            &output,
            &db,
            "--model-endpoint path must be /v1 with no query or fragment; use http://<ip>:<port>/v1",
        );
        assert!(
            message.contains("use http://<ip>:<port>/v1"),
            "{endpoint}: {message}"
        );
        assert!(!message.contains(endpoint), "{endpoint}: {message}");
        assert!(!message.contains("q=1"), "{message}");
        assert!(!message.contains("#frag"), "{message}");
        assert!(!db.exists(), "{endpoint}");
        assert!(!owner_lock(&db).exists(), "{endpoint}");
    }
}

#[test]
fn model_endpoint_invalid_port_is_a_parse_refusal_without_a_database() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    for endpoint in ["http://127.0.0.1:99999/v1", "http://[::1]:0/v1"] {
        let db = tmp.path().join(format!("{}.sqlite", endpoint.len()));
        let output = model_endpoint_output(&[
            "mission",
            "create",
            "--runtime",
            "confined-pi",
            "--model-endpoint",
            endpoint,
            "--db",
            db.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--issue",
            "217",
            "--objective",
            "bad port",
            "--acceptance",
            "no database",
        ]);
        let message = assert_model_endpoint_guard(
            &output,
            &db,
            "model endpoint must be one http URL without credentials",
        );
        assert!(!message.contains(endpoint), "{endpoint}: {message}");
        assert!(!message.contains("99999"), "{message}");
    }
}

#[test]
fn model_endpoint_flag_refuses_hostnames_and_non_loopback_addresses() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let repo_arg = repo.to_str().unwrap();
    let refused = [
        ("localhost.sqlite", "http://localhost:8000/v1", "localhost"),
        (
            "hostname.sqlite",
            "http://model.example.invalid/v1",
            "model.example.invalid",
        ),
        ("public.sqlite", "http://8.8.8.8:9/v1", "8.8.8.8"),
        ("rfc1918.sqlite", "http://10.1.2.3/v1", "10.1.2.3"),
        ("mapped.sqlite", "http://[::ffff:127.0.0.1]:9/v1", "::ffff"),
    ];
    for (name, endpoint, hidden) in refused {
        let db = tmp.path().join(name);
        let output = model_endpoint_output(&[
            "mission",
            "create",
            "--runtime",
            "confined-pi",
            "--model-endpoint",
            endpoint,
            "--db",
            db.to_str().unwrap(),
            "--repo",
            repo_arg,
            "--issue",
            "216",
            "--objective",
            "no dns",
            "--acceptance",
            "no database",
        ]);
        let message = assert_model_endpoint_guard(
            &output,
            &db,
            "--model-endpoint accepts only a loopback IP literal in 127.0.0.0/8 or ::1, with or without a port; hostnames and other addresses are refused without a DNS lookup",
        );
        assert!(!message.contains(hidden), "{endpoint}: {message}");
        assert!(!message.contains(endpoint), "{endpoint}: {message}");
        assert!(
            !message.contains("not a single address"),
            "{endpoint}: {message}"
        );
        let bridge = tmp.path().join(format!(
            "{}.pi-bridge",
            db.file_name().unwrap().to_str().unwrap()
        ));
        assert!(!bridge.exists(), "{bridge:?}");
    }
}

#[test]
fn model_endpoint_loopback_literals_pass_argument_checks_without_creating_a_database() {
    let tmp = tempfile::tempdir().unwrap();
    let literals = [
        "http://127.0.0.1:9/v1",
        "http://127.0.0.1/v1",
        "http://[::1]:9/v1",
        "http://[::1]/v1",
        "http://127.255.255.254:9/v1",
    ];
    for (index, endpoint) in literals.iter().enumerate() {
        let db = tmp.path().join(format!("missing-{index}.sqlite"));
        let output = model_endpoint_output(&[
            "mission",
            "status",
            "--runtime",
            "confined-pi",
            "--model-endpoint",
            endpoint,
            "--db",
            db.to_str().unwrap(),
        ]);
        assert_eq!(output.status.code(), Some(2), "{endpoint} {output:?}");
        assert!(output.stdout.is_empty(), "{endpoint} {output:?}");
        let stderr: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(
            stderr["error"]["category"], "not_found",
            "{endpoint} {stderr}"
        );
        let message = stderr["error"]["message"].as_str().unwrap();
        assert!(
            !message.contains("loopback IP literal"),
            "{endpoint}: {message}"
        );
        assert!(!message.contains(endpoint), "{endpoint}: {message}");
        assert!(!db.exists(), "{endpoint}");
    }
}

#[test]
fn successor_flags_copy_overrides_and_refuse_a_live_source() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "172",
        "--objective",
        "original objective",
        "--acceptance",
        "original acceptance",
        "--max-repairs",
        "2",
        "--max-plan-revisions",
        "3",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let (code, _, error) = run(&[
        "mission",
        "successor",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["error"]["category"], "successor_source_not_terminal");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("confirm_cancelled"),
        "{error}"
    );

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
        "1",
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(cancelled["mission"]["phase"], "cancelling");
    let revision = cancelled["mission"]["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let (code, confirmed, error) = run(&[
        "mission",
        "command",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--kind",
        "confirm_cancelled",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(confirmed["mission"]["phase"], "cancelled");

    let acceptance = tmp.path().join("acceptance.txt");
    std::fs::write(&acceptance, "from file\n").unwrap();
    let file_arg = format!("@{}", acceptance.display());
    let (code, successor, error) = run(&[
        "mission",
        "successor",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--actor",
        "sam",
        "--objective",
        "replaced objective",
        "--acceptance",
        &file_arg,
        "--acceptance",
        "inline criterion",
        "--max-repairs",
        "4",
        "--max-plan-revisions",
        "7",
        "--verifier",
        r#"{"id":"switchyard.verifier.v1","frozen":true,"executes":false}"#,
    ]);
    assert_eq!(code, 0, "{error}");
    let mission_json = &successor["mission"];
    assert_eq!(mission_json["phase"], "draft");
    assert_eq!(mission_json["objective"], "replaced objective");
    assert_eq!(mission_json["maxRepairs"], 4);
    assert_eq!(mission_json["maxPlanRevisions"], 7);
    assert_eq!(mission_json["planRevisionCount"], 0);
    assert_eq!(mission_json["repairCount"], 0);
    assert!(mission_json["planDigest"].is_null());
    assert_eq!(mission_json["successorOfMissionId"], mission);
    assert_eq!(mission_json["runtimeAdapter"], "fake");
    assert_eq!(mission_json["baseSha"], head(&repo));
    assert_eq!(mission_json["verifier"]["id"], "switchyard.verifier.v1");
    let successor_id = mission_json["missionId"].as_str().unwrap();
    let attempt_id = mission_json["attemptId"].as_str().unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    let acceptance_text: String = conn
        .query_row(
            "SELECT acceptance_criteria FROM tasks WHERE mission_id = ?1 AND root = 1",
            [successor_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(acceptance_text, "from file\ninline criterion");
    let rationale: String = conn
        .query_row(
            "SELECT rationale FROM decisions WHERE mission_id = ?1 AND action = 'create'",
            [successor_id],
            |row| row.get(0),
        )
        .unwrap();
    let payload: Value = serde_json::from_str(&rationale).unwrap();
    assert_eq!(payload["actor"], "sam");
    assert_eq!(payload["sourceMissionId"], mission);
    assert_eq!(
        payload["overridden"],
        serde_json::json!([
            "acceptanceCriteria",
            "maxPlanRevisions",
            "maxRepairs",
            "objective",
            "verifier"
        ])
    );
    let verifier: String = conn
        .query_row(
            "SELECT verifier_json FROM attempts WHERE id = ?1",
            [attempt_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        verifier,
        r#"{"executes":false,"frozen":true,"id":"switchyard.verifier.v1"}"#
    );
    let (code, _, error) = run(&[
        "mission",
        "successor",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["error"]["category"], "successor_exists");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains(successor_id),
        "{error}"
    );
}

#[test]
fn successor_rejects_empty_acceptance_parts_and_bad_verifier_files() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "172",
        "--objective",
        "original objective",
        "--acceptance",
        "original acceptance",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
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
        "1",
    ]);
    assert_eq!(code, 0, "{error}");
    let revision = cancelled["mission"]["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let (code, confirmed, error) = run(&[
        "mission",
        "command",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--kind",
        "confirm_cancelled",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(confirmed["mission"]["phase"], "cancelled");

    let refuse = |extra: &[&str], category: &str, message: &str| {
        let mut args = vec![
            "mission",
            "successor",
            "--db",
            db.to_str().unwrap(),
            "--mission",
            mission,
        ];
        args.extend_from_slice(extra);
        let (code, _, error) = run(&args);
        assert_eq!(code, 2, "{error}");
        assert_eq!(error["error"]["category"], category, "{error}");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains(message),
            "{error}"
        );
    };
    refuse(
        &["--objective", ""],
        "guard_failed",
        "objective is required",
    );
    refuse(
        &["--acceptance", ""],
        "guard_failed",
        "acceptance criteria is required",
    );
    refuse(
        &["--acceptance", "", "--acceptance", "still empty first"],
        "guard_failed",
        "acceptance criteria is required",
    );
    refuse(
        &["--acceptance", "@"],
        "guard_failed",
        "acceptance file path is empty",
    );
    let missing = tmp.path().join("no-such-acceptance.txt");
    let missing_arg = format!("@{}", missing.display());
    refuse(
        &["--acceptance", &missing_arg],
        "guard_failed",
        "could not be read",
    );
    refuse(
        &["--verifier", "[]"],
        "guard_failed",
        "must be a JSON object",
    );
    refuse(
        &[
            "--verifier",
            r#"{"executes":true,"frozen":true,"id":"switchyard.verifier.v1"}"#,
        ],
        "verifier_not_supported",
        "switchyard.verifier.v1",
    );
    let missions: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM missions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(missions, 1);
}

#[test]
fn drifted_pause_refuses_with_frozen_input_drift_and_exit_2() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "202",
        "--objective",
        "name the drift refusal",
        "--acceptance",
        "pause after drift exits 2",
        "--admit",
        "start_planning",
        "--persist-only",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let attempt = created["mission"]["attemptId"].as_str().unwrap();
    let revision = created["mission"]["revision"].as_u64().unwrap().to_string();
    assert_eq!(created["mission"]["phase"], "planning");
    let changed = rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE attempts SET base_sha = 'deadbeef' WHERE id = ?1",
            [attempt],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let (code, stdout, stderr) = run(&[
        "mission",
        "command",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        mission,
        "--kind",
        "pause",
        "--expected-revision",
        &revision,
        "--persist-only",
    ]);
    assert_eq!(code, 2, "{stdout} {stderr}");
    assert!(stdout.is_null(), "{stdout}");
    assert_eq!(stderr["ok"], false);
    assert_eq!(stderr["error"]["category"], "frozen_input_drift");
    assert!(
        stderr["error"]["message"]
            .as_str()
            .unwrap()
            .contains("drifted"),
        "{stderr}"
    );
}
#[test]
fn drifted_verify_refuses_with_frozen_input_drift_and_exit_2() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "199",
        "--objective",
        "name the verify drift refusal",
        "--acceptance",
        "verify after drift exits 2",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let attempt = created["mission"]["attemptId"].as_str().unwrap();
    let mut revision = created["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, _started, error) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--kind",
        "start_planning",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(code, 0, "{error}");
    let (code, planned, error) = run(&[
        "mission",
        "notify",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--json-body",
        r#"{"kind":"plan_completed","plan":"implement","quiescent":true}"#,
    ]);
    assert_eq!(code, 0, "{error}");
    let digest = planned["mission"]["planDigest"].as_str().unwrap();
    let payload = format!(r#"{{"planDigest":"{digest}"}}"#);
    revision = planned["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, _approved, error) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--kind",
        "approve_plan",
        "--expected-revision",
        &revision,
        "--payload",
        &payload,
    ]);
    assert_eq!(code, 0, "{error}");
    let (code, verifying, error) = run(&[
        "mission",
        "notify",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--json-body",
        r#"{"kind":"implementation_settled","quiescent":true}"#,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(verifying["mission"]["phase"], "verifying");
    let changed = rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE attempts SET base_sha = 'deadbeef' WHERE id = ?1",
            [attempt],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let (code, stdout, stderr) = run(&["mission", "verify", "--db", db_arg, "--mission", mission]);
    assert_eq!(code, 2, "{stdout} {stderr}");
    assert!(stdout.is_null(), "{stdout}");
    assert_eq!(stderr["ok"], false);
    assert_eq!(stderr["error"]["category"], "frozen_input_drift");
    assert!(
        stderr["error"]["message"]
            .as_str()
            .unwrap()
            .contains("drifted"),
        "{stderr}"
    );
}

#[test]
fn create_verifier_and_mission_verify_use_the_owned_workspace() {
    const RUST: &str = concat!(
        r#"{"allowedPaths":["crates/*/tests/**","docs/**"],"argv":["./scripts/rust-verify.sh"],"#,
        r#""confinement":"linux-bubblewrap-0.9.0","deniedPaths":["crates/switchyard-core/tests/pi_bridge.rs","#,
        r#""crates/switchyard/tests/linux_execution.rs","crates/switchyard/tests/cli_confined.rs"],"#,
        r#""executes":true,"frozen":true,"id":"switchyard.verifier.rust-verify.v1","maxAddedLines":400,"#,
        r#""outputCapBytes":1048576,"timeoutSeconds":1800}"#
    );
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    let root = tmp.path().join("workspaces");
    std::fs::create_dir(&root).unwrap();
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, _, stderr) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "199",
        "--objective",
        "Gate",
        "--acceptance",
        "Refuse",
        "--verifier",
        r#"{"id":"nope"}"#,
    ]);
    assert_eq!(code, 2);
    assert_eq!(stderr["error"]["category"], "verifier_not_supported");
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "199",
        "--objective",
        "Gate the diff",
        "--acceptance",
        "Refuse unconfined execution",
        "--verifier",
        RUST,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        created["mission"]["verifier"]["id"],
        "switchyard.verifier.rust-verify.v1"
    );
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let attempt = created["mission"]["attemptId"].as_str().unwrap();
    let (code, workspace, error) = run(&[
        "mission",
        "workspace",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--attempt",
        attempt,
        "--root",
        root.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{error}");
    let tree = workspace["workspace"]["path"].as_str().unwrap();
    let mut revision = created["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, _started, error) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--kind",
        "start_planning",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(code, 0, "{error}");
    let (code, planned, error) = run(&[
        "mission",
        "notify",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--json-body",
        r#"{"kind":"plan_completed","plan":"implement","quiescent":true}"#,
    ]);
    assert_eq!(code, 0, "{error}");
    let digest = planned["mission"]["planDigest"].as_str().unwrap();
    let payload = format!(r#"{{"planDigest":"{digest}"}}"#);
    revision = planned["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, _approved, error) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--kind",
        "approve_plan",
        "--expected-revision",
        &revision,
        "--payload",
        &payload,
    ]);
    assert_eq!(code, 0, "{error}");
    let (code, verifying, error) = run(&[
        "mission",
        "notify",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--json-body",
        r#"{"kind":"implementation_settled","quiescent":true}"#,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(verifying["mission"]["phase"], "verifying");
    let (code, _, stderr) = run(&["mission", "verify", "--db", db_arg, "--mission", mission]);
    assert_eq!(code, 2, "{stderr}");
    assert_eq!(
        stderr["error"]["category"],
        "verifier_confinement_unavailable"
    );
    std::fs::create_dir_all(Path::new(tree).join("scripts")).unwrap();
    std::fs::write(
        Path::new(tree).join("scripts/rust-verify.sh"),
        "#!/bin/sh\n",
    )
    .unwrap();
    let (code, verified, error) = run(&["mission", "verify", "--db", db_arg, "--mission", mission]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(verified["outcome"], "fail");
    assert_eq!(verified["mission"]["phase"], "verifying");
}

#[test]
fn mission_write_copies_one_file_and_refuses_a_closed_phase() {
    use switchyard_core::*;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    let root = tmp.path().join("workspaces");
    init_repo(&repo);
    std::fs::create_dir(&root).unwrap();
    let (code, created, err) = run(&[
        "mission",
        "create",
        "--db",
        db.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "196",
        "--objective",
        "operator write",
        "--acceptance",
        "file lands in the worktree",
    ]);
    assert_eq!(code, 0, "{err}");
    let mission = created["mission"]["missionId"]
        .as_str()
        .unwrap()
        .to_string();
    let attempt = created["mission"]["attemptId"]
        .as_str()
        .unwrap()
        .to_string();
    let from = tmp.path().join("body.txt");
    std::fs::write(&from, b"hello").unwrap();
    let (code, stdout, err) = run(&[
        "mission",
        "write",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        &mission,
        "--attempt",
        &attempt,
        "--path",
        "note.txt",
        "--from",
        from.to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "{stdout} {err}");
    assert_eq!(err["error"]["category"], "operator_write_phase_closed");

    let mut host = Host::open(&db).unwrap();
    let admit = |host: &mut Host, body| {
        host.admit(AdmitRequest {
            command_id: None,
            mission_id: mission.clone(),
            attempt_id: None,
            expected_revision: host.snapshot(&mission).unwrap().revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body,
        })
        .unwrap();
    };
    admit(&mut host, CommandBody::StartPlanning);
    host.dispatch_pending(&mission).unwrap();
    host.apply_notification(
        &mission,
        parse_notification(&serde_json::json!({
            "kind": "plan_completed", "plan": "write the file", "quiescent": true
        }))
        .unwrap(),
    )
    .unwrap();
    let digest = host.snapshot(&mission).unwrap().plan_digest.unwrap();
    admit(
        &mut host,
        CommandBody::ApprovePlan {
            plan_digest: digest,
        },
    );
    let workspace = host.prepare_workspace(&mission, &attempt, &root).unwrap();
    let revision = host.snapshot(&mission).unwrap().revision;
    drop(host);
    let (code, stdout, err) = run(&[
        "mission",
        "write",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        &mission,
        "--attempt",
        &attempt,
        "--path",
        "src/note.txt",
        "--from",
        from.to_str().unwrap(),
        "--actor",
        "sam",
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(stdout["ok"], true);
    assert_eq!(stdout["write"]["path"], "src/note.txt");
    assert_eq!(stdout["write"]["bytes"], 5);
    assert_eq!(stdout["write"]["authoredBy"], "operator");
    assert_eq!(stdout["write"]["actor"], "sam");
    assert_eq!(stdout["write"]["workspacePath"], workspace.path);
    assert_eq!(stdout["mission"]["phase"], "implementing");
    assert_eq!(stdout["mission"]["revision"].as_u64(), Some(revision));
    let file = std::path::Path::new(&workspace.path).join("src/note.txt");
    assert_eq!(std::fs::read(&file).unwrap(), b"hello");
    let hashed = Command::new("sha256sum").arg(&file).output().unwrap();
    let hex = String::from_utf8(hashed.stdout).unwrap();
    let hex = hex.split_whitespace().next().unwrap();
    assert_eq!(stdout["write"]["sha256"], format!("sha256:{hex}"));
    let (action, digest, command_id): (String, String, Option<String>) =
        rusqlite::Connection::open(&db)
            .unwrap()
            .query_row(
                "SELECT action, digest, command_id FROM decisions WHERE action = 'operator_write'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
    assert_eq!(action, "operator_write");
    assert_eq!(digest, stdout["write"]["sha256"]);
    assert_eq!(command_id, None);
}

fn data_version(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("PRAGMA data_version", [], |row| row.get(0))
        .unwrap()
}

fn assert_no_inputs_frozen(value: &Value) {
    assert!(
        !value.to_string().contains("\"inputsFrozen\""),
        "export document must not carry inputsFrozen: {value}"
    );
}

fn assert_export_receipt(receipt: &Value, out: &Path, file_bytes: &[u8]) {
    assert_eq!(receipt["ok"], true, "{receipt}");
    assert!(receipt.get("schema").is_none(), "{receipt}");
    assert!(receipt.get("mission").is_none(), "{receipt}");
    assert_eq!(receipt["out"], out.display().to_string());
    assert_eq!(
        receipt["bytes"].as_u64(),
        Some(u64::try_from(file_bytes.len()).unwrap())
    );
    let hashed = Command::new("sha256sum").arg(out).output().unwrap();
    assert!(
        hashed.status.success(),
        "{}",
        String::from_utf8_lossy(&hashed.stderr)
    );
    let hex = String::from_utf8(hashed.stdout).unwrap();
    let hex = hex.split_whitespace().next().unwrap();
    assert_eq!(receipt["sha256"], format!("sha256:{hex}"));
}

#[test]
fn export_matches_the_stored_freeze_and_status_stays_compact() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "198",
        "--objective",
        "Export the freeze",
        "--acceptance",
        "The freeze document matches the ledger",
        "--admit",
        "start_planning",
        "--persist-only",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let (code, status, error) = run(&["mission", "status", "--db", db_arg, "--mission", mission]);
    assert_eq!(code, 0, "{error}");
    assert!(status["missions"][0].get("frozenInputsJson").is_none());
    assert_eq!(status["missions"][0]["inputsFrozen"], true);

    let out = tmp.path().join("export.json");
    let (code, receipt, error) = run(&[
        "mission",
        "export",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{error}");
    let file_bytes = std::fs::read(&out).unwrap();
    assert_export_receipt(&receipt, &out, &file_bytes);
    let export: Value = serde_json::from_slice(&file_bytes).unwrap();
    let stored: String = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT frozen_inputs_json FROM attempts WHERE mission_id = ?1",
            [mission],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        export["attempt"]["frozenInputsJson"].as_str(),
        Some(stored.as_str())
    );
    let parsed: Value = serde_json::from_str(&stored).unwrap();
    let digest = switchyard_core::payload_hash(&parsed).unwrap();
    assert_eq!(
        export["attempt"]["frozenInputsDigest"].as_str(),
        Some(digest.as_str())
    );
    assert!(export["attempt"].get("frozenInputs").is_none());
    assert_no_inputs_frozen(&export);
    assert!(export["mission"].get("linuxExecutionEffects").is_some());
    assert!(export["mission"].get("executionResolutions").is_some());
    assert!(export["mission"].get("executionBlocked").is_some());
    assert!(export["mission"].get("piHost").is_none());
    assert!(export["mission"].get("quarantine").is_none());
    assert_eq!(export["schema"], "switchyard.ledger-export.v1");
    assert_eq!(export["ledgerSchemaVersion"].as_i64(), Some(12));
    assert_eq!(export["hostVersion"], env!("CARGO_PKG_VERSION"));
    let (seq, payload, kind): (i64, String, String) = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT seq, payload_canonical, kind FROM commands WHERE mission_id = ?1 ORDER BY seq LIMIT 1",
            [mission],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(export["commands"][0]["seq"].as_i64(), Some(seq));
    assert_eq!(
        export["commands"][0]["payloadCanonical"].as_str(),
        Some(payload.as_str())
    );
    assert_eq!(export["commands"][0]["kind"], kind);
    assert!(export["commands"][0].get("receipt").is_some());
}

#[test]
fn export_reads_while_the_owner_lock_is_held_and_skips_out_when_the_read_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "198",
        "--objective",
        "Read under the lock",
        "--acceptance",
        "Export does not take the owner lock",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"]
        .as_str()
        .unwrap()
        .to_string();

    let mut child = Command::new(bin())
        .args(["mission", "lock", "--db", db_arg])
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

    let conn = rusqlite::Connection::open(&db).unwrap();
    let before = data_version(&conn);
    let out = tmp.path().join("held.json");
    let (code, receipt, error) = run(&[
        "mission",
        "export",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{error}");
    let file_bytes = std::fs::read(&out).unwrap();
    assert_export_receipt(&receipt, &out, &file_bytes);
    let export: Value = serde_json::from_slice(&file_bytes).unwrap();
    assert_eq!(export["mission"]["missionId"], mission);
    assert_eq!(export["attempt"]["frozenInputsJson"], Value::Null);
    assert_eq!(export["attempt"]["frozenInputsDigest"], Value::Null);
    assert_no_inputs_frozen(&export);
    assert_eq!(data_version(&conn), before, "export opened a write");

    let missing_out = tmp.path().join("missing-mission.json");
    let (code, stdout, stderr) = run(&[
        "mission",
        "export",
        "--db",
        db_arg,
        "--mission",
        "m-missing",
        "--out",
        missing_out.to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "{stdout}");
    assert_eq!(stderr["error"]["category"], "not_found");
    assert!(!missing_out.exists());
    assert_eq!(data_version(&conn), before);

    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    assert!(child.wait().unwrap().success());

    let absent = tmp.path().join("no-ledger.sqlite");
    let absent_out = tmp.path().join("no-ledger.json");
    let (code, _, stderr) = run(&[
        "mission",
        "export",
        "--db",
        absent.to_str().unwrap(),
        "--mission",
        &mission,
        "--out",
        absent_out.to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    assert_eq!(stderr["error"]["category"], "not_found");
    assert!(!absent_out.exists());

    let empty = tmp.path().join("empty.sqlite");
    std::fs::write(&empty, b"").unwrap();
    let empty_out = tmp.path().join("empty.json");
    let (code, _, stderr) = run(&[
        "mission",
        "export",
        "--db",
        empty.to_str().unwrap(),
        "--mission",
        &mission,
        "--out",
        empty_out.to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    assert_eq!(stderr["error"]["category"], "storage_failed");
    assert!(!empty_out.exists());
}

#[test]
fn export_of_an_unfrozen_draft_has_null_freeze_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "198",
        "--objective",
        "Draft",
        "--acceptance",
        "Not frozen yet",
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(created["mission"]["phase"], "draft");
    assert_eq!(created["mission"]["inputsFrozen"], false);
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let (code, status, error) = run(&["mission", "status", "--db", db_arg, "--mission", mission]);
    assert_eq!(code, 0, "{error}");
    assert!(status["missions"][0].get("frozenInputsJson").is_none());
    assert_eq!(status["missions"][0]["inputsFrozen"], false);
    let (code, export, error) = run(&["mission", "export", "--db", db_arg, "--mission", mission]);
    assert_eq!(code, 0, "{error}");
    assert!(export["attempt"]["frozenInputsJson"].is_null());
    assert!(export["attempt"]["frozenInputsDigest"].is_null());
    assert_no_inputs_frozen(&export);
    assert_eq!(export["mission"]["phase"], "draft");
}

#[test]
fn export_after_drift_shows_frozen_input_drift_and_detect_uncertainty() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "198",
        "--objective",
        "Show drift",
        "--acceptance",
        "Export the drift exit",
        "--admit",
        "start_planning",
        "--persist-only",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let attempt = created["mission"]["attemptId"].as_str().unwrap();
    let revision = created["mission"]["revision"].as_u64().unwrap().to_string();
    let changed = rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE attempts SET base_sha = 'deadbeef' WHERE id = ?1",
            [attempt],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let (code, stdout, stderr) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--kind",
        "pause",
        "--expected-revision",
        &revision,
        "--persist-only",
    ]);
    assert_eq!(code, 2, "{stdout} {stderr}");
    assert_eq!(stderr["error"]["category"], "frozen_input_drift");
    let (code, export, error) = run(&["mission", "export", "--db", db_arg, "--mission", mission]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(export["mission"]["attentionReason"], "frozen_input_drift");
    assert_eq!(export["mission"]["phase"], "needs_attention");
    assert!(
        export["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["kind"] == "detect_uncertainty"),
        "{export}"
    );
    let stored: String = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT frozen_inputs_json FROM attempts WHERE mission_id = ?1",
            [mission],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        export["attempt"]["frozenInputsJson"].as_str(),
        Some(stored.as_str())
    );
}

#[test]
fn export_includes_the_operator_write_decision_and_the_host_verifier_fail() {
    const RUST: &str = concat!(
        r#"{"allowedPaths":["crates/*/tests/**","docs/**"],"argv":["./scripts/rust-verify.sh"],"#,
        r#""confinement":"linux-bubblewrap-0.9.0","deniedPaths":["crates/switchyard-core/tests/pi_bridge.rs","#,
        r#""crates/switchyard/tests/linux_execution.rs","crates/switchyard/tests/cli_confined.rs"],"#,
        r#""executes":true,"frozen":true,"id":"switchyard.verifier.rust-verify.v1","maxAddedLines":400,"#,
        r#""outputCapBytes":1048576,"timeoutSeconds":1800}"#
    );
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    let root = tmp.path().join("workspaces");
    std::fs::create_dir(&root).unwrap();
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "198",
        "--objective",
        "Record the write and the gate",
        "--acceptance",
        "Export both rows",
        "--verifier",
        RUST,
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"]
        .as_str()
        .unwrap()
        .to_string();
    let attempt = created["mission"]["attemptId"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, workspace, error) = run(&[
        "mission",
        "workspace",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--attempt",
        &attempt,
        "--root",
        root.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{error}");
    let mut revision = created["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, _started, error) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--kind",
        "start_planning",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(code, 0, "{error}");
    let (code, planned, error) = run(&[
        "mission",
        "notify",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--json-body",
        r#"{"kind":"plan_completed","plan":"write the file","quiescent":true}"#,
    ]);
    assert_eq!(code, 0, "{error}");
    let digest = planned["mission"]["planDigest"].as_str().unwrap();
    let payload = format!(r#"{{"planDigest":"{digest}"}}"#);
    revision = planned["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, approved, error) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--kind",
        "approve_plan",
        "--expected-revision",
        &revision,
        "--payload",
        &payload,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(approved["mission"]["phase"], "implementing");
    let from = tmp.path().join("body.txt");
    std::fs::write(&from, b"hello").unwrap();
    let (code, wrote, error) = run(&[
        "mission",
        "write",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--attempt",
        &attempt,
        "--path",
        "src/note.txt",
        "--from",
        from.to_str().unwrap(),
        "--actor",
        "sam",
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(wrote["write"]["actor"], "sam");
    let (code, verifying, error) = run(&[
        "mission",
        "notify",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--json-body",
        r#"{"kind":"implementation_settled","quiescent":true}"#,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(verifying["mission"]["phase"], "verifying");
    let tree = workspace["workspace"]["path"].as_str().unwrap();
    std::fs::create_dir_all(Path::new(tree).join("scripts")).unwrap();
    std::fs::write(
        Path::new(tree).join("scripts/rust-verify.sh"),
        "#!/bin/sh\n",
    )
    .unwrap();
    let (code, verified, error) =
        run(&["mission", "verify", "--db", db_arg, "--mission", &mission]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(verified["outcome"], "fail");
    let (code, export, error) = run(&["mission", "export", "--db", db_arg, "--mission", &mission]);
    assert_eq!(code, 0, "{error}");
    let writes: Vec<_> = export["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["action"] == "operator_write")
        .collect();
    assert_eq!(writes.len(), 1, "{export}");
    assert_eq!(writes[0]["actor"], "sam");
    assert!(writes[0]["commandId"].is_null());
    let fails: Vec<_> = export["verifications"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["outcome"] == "fail")
        .collect();
    assert_eq!(fails.len(), 1, "{export}");
    assert_eq!(
        fails[0]["evidence"]["provenance"]["collector"],
        "switchyard-host-verifier"
    );
}

#[test]
fn export_refuses_an_existing_out_path_and_a_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "198",
        "--objective",
        "Do not clobber",
        "--acceptance",
        "Existing paths stay put",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let out = tmp.path().join("export.json");
    std::fs::write(&out, b"keep-me").unwrap();
    let (code, stdout, stderr) = run(&[
        "mission",
        "export",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "{stdout}");
    assert_eq!(stderr["error"]["category"], "export_target_exists");
    assert!(
        stderr["error"]["message"]
            .as_str()
            .unwrap()
            .contains("remove the file"),
        "{stderr}"
    );
    assert_eq!(std::fs::read(&out).unwrap(), b"keep-me");

    let secret = tmp.path().join("secret");
    std::fs::write(&secret, b"secret").unwrap();
    let link = tmp.path().join("link.json");
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    let (code, stdout, stderr) = run(&[
        "mission",
        "export",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--out",
        link.to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "{stdout}");
    assert_eq!(stderr["error"]["category"], "export_target_exists");
    assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let dir = tmp.path().join("outdir");
    std::fs::create_dir(&dir).unwrap();
    let (code, stdout, stderr) = run(&[
        "mission",
        "export",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--out",
        dir.to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "{stdout}");
    assert_eq!(stderr["error"]["category"], "export_target_exists");
    assert!(dir.is_dir());

    std::fs::remove_file(&out).unwrap();
    let (code, receipt, error) = run(&[
        "mission",
        "export",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{error}");
    let file_bytes = std::fs::read(&out).unwrap();
    assert_export_receipt(&receipt, &out, &file_bytes);
    let export: Value = serde_json::from_slice(&file_bytes).unwrap();
    assert_eq!(export["schema"], "switchyard.ledger-export.v1");
}

#[test]
fn export_revision_matches_the_latest_command_on_a_quiet_ledger() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "198",
        "--objective",
        "One snapshot",
        "--acceptance",
        "Revision matches the latest command",
        "--admit",
        "start_planning",
        "--persist-only",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let (code, export, error) = run(&["mission", "export", "--db", db_arg, "--mission", mission]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(export["schema"], "switchyard.ledger-export.v1");
    let revision = export["mission"]["revision"].as_u64().unwrap();
    let max_resulting = export["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|command| command["resultingRevision"].as_u64())
        .max()
        .unwrap();
    assert_eq!(revision, max_resulting, "{export}");
}

#[test]
fn export_of_a_missing_ledger_under_a_missing_parent_creates_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = tmp.path().join("missing-parent");
    let db = parent.join("mission.sqlite");
    let out = tmp.path().join("export.json");
    let (code, stdout, stderr) = run(&[
        "mission",
        "export",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        "m-missing",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "{stdout}");
    assert_eq!(stderr["error"]["category"], "not_found");
    assert!(!db.exists());
    assert!(!parent.exists());
    assert!(!out.exists());

    let (code, stdout, stderr) = run(&[
        "mission",
        "status",
        "--db",
        db.to_str().unwrap(),
        "--mission",
        "m-missing",
    ]);
    assert_eq!(code, 2, "{stdout}");
    assert_eq!(stderr["error"]["category"], "not_found");
    assert!(!db.exists());
    assert!(!parent.exists());
}

#[test]
fn export_out_prints_a_receipt_and_stdout_without_out_is_the_document() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "198",
        "--objective",
        "Receipt",
        "--acceptance",
        "Stdout stays the document until --out",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let (code, document, error) = run(&["mission", "export", "--db", db_arg, "--mission", mission]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(document["schema"], "switchyard.ledger-export.v1");
    assert_eq!(document["mission"]["missionId"], mission);
    assert!(document.get("ok").is_none());

    let out = tmp.path().join("export.json");
    let (code, receipt, error) = run(&[
        "mission",
        "export",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{error}");
    let file_bytes = std::fs::read(&out).unwrap();
    assert_export_receipt(&receipt, &out, &file_bytes);
    let filed: Value = serde_json::from_slice(&file_bytes).unwrap();
    assert_eq!(filed, document);
}

fn copy_ledger(from: &std::path::Path, to: &std::path::Path) {
    std::fs::copy(from, to).unwrap();
    for suffix in ["-wal", "-shm"] {
        let mut src = from.as_os_str().to_owned();
        src.push(suffix);
        let src = std::path::PathBuf::from(src);
        if src.exists() {
            let mut dst = to.as_os_str().to_owned();
            dst.push(suffix);
            std::fs::copy(&src, std::path::PathBuf::from(dst)).unwrap();
        }
    }
}

#[test]
fn mission_plan_matches_notify_and_notify_still_works() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "197",
        "--objective",
        "plan from a file",
        "--acceptance",
        "digest matches notify",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"]
        .as_str()
        .unwrap()
        .to_string();
    let revision = created["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, _started, error) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--kind",
        "start_planning",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(code, 0, "{error}");

    let notify_db = tmp.path().join("notify.sqlite");
    copy_ledger(&db, &notify_db);
    let plan = "implement the ledger\n";
    let file = tmp.path().join("plan.txt");
    std::fs::write(&file, plan).unwrap();
    let body = serde_json::json!({
        "kind": "plan_completed",
        "plan": plan,
        "quiescent": true,
    })
    .to_string();
    let (code, notified, error) = run(&[
        "mission",
        "notify",
        "--db",
        notify_db.to_str().unwrap(),
        "--mission",
        &mission,
        "--json-body",
        &body,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(notified["phase"], "awaiting_plan_approval");
    assert_eq!(notified["mission"]["phase"], "awaiting_plan_approval");

    let (code, filed, error) = run(&[
        "mission",
        "plan",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--file",
        file.to_str().unwrap(),
        "--quiescent",
        "true",
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(filed["mission"]["phase"], "awaiting_plan_approval");
    assert_eq!(
        filed["mission"]["planDigest"],
        notified["mission"]["planDigest"]
    );
    assert!(
        filed["mission"]["planDigest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );

    let (code, exported, error) =
        run(&["mission", "export", "--db", db_arg, "--mission", &mission]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(exported["ledgerSchemaVersion"], 12);
    assert_eq!(exported["attempt"]["planBody"], plan);
    let decision = exported["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["action"] == "plan_completed")
        .unwrap();
    assert_eq!(decision["actor"], "operator-plan-file");
    let command = exported["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "plan_completed")
        .unwrap();
    assert_eq!(command["origin"], "runtime_observation");
    assert!(
        !command["payloadCanonical"]
            .as_str()
            .unwrap()
            .contains("operator-plan-file")
    );

    let (code, notify_export, error) = run(&[
        "mission",
        "export",
        "--db",
        notify_db.to_str().unwrap(),
        "--mission",
        &mission,
    ]);
    assert_eq!(code, 0, "{error}");
    let notify_decision = notify_export["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["action"] == "plan_completed")
        .unwrap();
    assert_eq!(notify_decision["actor"], "runtime-observation");
    assert_eq!(notify_export["attempt"]["planBody"], plan);
    assert_eq!(
        notify_export["attempt"]["planDigest"],
        exported["attempt"]["planDigest"]
    );
    let notify_command = notify_export["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "plan_completed")
        .unwrap();
    assert_eq!(notify_command["payloadHash"], command["payloadHash"]);
}

#[test]
fn mission_plan_refuses_a_bad_file_and_a_missing_quiescent_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "197",
        "--objective",
        "refuse a bad plan file",
        "--acceptance",
        "state stays planning",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"]
        .as_str()
        .unwrap()
        .to_string();
    let revision = created["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, started, error) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--kind",
        "start_planning",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(code, 0, "{error}");
    let planning_revision = started["mission"]["revision"].clone();

    let missing = tmp.path().join("missing.txt");
    let (code, stdout, stderr) = run(&[
        "mission",
        "plan",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--file",
        missing.to_str().unwrap(),
        "--quiescent",
        "true",
    ]);
    assert_eq!(code, 2, "{stdout}");
    assert_eq!(stderr["error"]["category"], "guard_failed");
    assert!(
        stderr["error"]["message"]
            .as_str()
            .unwrap()
            .contains("fix the path and rerun"),
        "{stderr}"
    );

    let file = tmp.path().join("plan.txt");
    std::fs::write(&file, "implement the ledger").unwrap();
    let (code, stdout, stderr) = run(&[
        "mission",
        "plan",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--file",
        file.to_str().unwrap(),
        "--quiescent",
        "false",
    ]);
    assert_eq!(code, 2, "{stdout}");
    assert!(
        stderr["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not quiescent"),
        "{stderr}"
    );

    let (code, status, error) = run(&["mission", "status", "--db", db_arg, "--mission", &mission]);
    assert_eq!(code, 0, "{error}");
    let mission_status = &status["missions"][0];
    assert_eq!(mission_status["phase"], "planning");
    assert_eq!(mission_status["revision"], planning_revision);
    assert!(mission_status["planDigest"].is_null());

    let (code, stdout, _) = run(&[
        "mission",
        "plan",
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--file",
        file.to_str().unwrap(),
    ]);
    assert_ne!(code, 0);
    assert!(stdout.is_null());
}

#[test]
fn mission_plan_help_notes_verbatim_bytes() {
    let help = Command::new(bin())
        .args(["mission", "plan", "--help"])
        .output()
        .unwrap();
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("verbatim"), "{help}");
    assert!(help.contains("--acceptance @path"), "{help}");
    assert!(help.contains("strips one trailing newline"), "{help}");
    assert!(help.contains("--quiescent"), "{help}");
    assert!(help.contains("<true|false>"), "{help}");
}

const EARLIER_OWNER_HINT: &str = "Workspace was verified by an earlier owner generation; it is reconciled automatically by the next `mission command`, or by `mission verify` while verifying.";

struct OwnerBump {
    _tmp: tempfile::TempDir,
    db: String,
    mission_id: String,
    planned: Value,
}

fn plan_completed_owner_bump() -> OwnerBump {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    let root = tmp.path().join("workspaces");
    std::fs::create_dir(&root).unwrap();
    init_repo(&repo);
    let db_arg = db.to_str().unwrap().to_string();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        &db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "200",
        "--objective",
        "status hint",
        "--acceptance",
        "a generation bump is not explicit recovery",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission_id = created["mission"]["missionId"]
        .as_str()
        .unwrap()
        .to_string();
    let attempt = created["mission"]["attemptId"].as_str().unwrap();
    let revision = created["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, _prepared, error) = run(&[
        "mission",
        "workspace",
        "--db",
        &db_arg,
        "--mission",
        &mission_id,
        "--attempt",
        attempt,
        "--root",
        root.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{error}");
    let (code, _started, error) = run(&[
        "mission",
        "command",
        "--db",
        &db_arg,
        "--mission",
        &mission_id,
        "--kind",
        "start_planning",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(code, 0, "{error}");
    let (code, planned, error) = run(&[
        "mission",
        "notify",
        "--db",
        &db_arg,
        "--mission",
        &mission_id,
        "--json-body",
        r#"{"kind":"plan_completed","plan":"implement","quiescent":true}"#,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(planned["mission"]["phase"], "awaiting_plan_approval");
    OwnerBump {
        _tmp: tmp,
        db: db_arg,
        mission_id,
        planned,
    }
}

fn mission_status(db: &str, mission_id: &str) -> Value {
    let (code, status, error) = run(&["mission", "status", "--db", db, "--mission", mission_id]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(status["readOnly"], true);
    status["missions"][0].clone()
}

fn generations(mission: &Value) -> (u64, u64) {
    (
        mission["workspace"]["verifiedGeneration"].as_u64().unwrap(),
        mission["recordedOwnerGeneration"].as_u64().unwrap(),
    )
}

fn assert_earlier_owner_hint(mission: &Value) {
    let (verified, recorded) = generations(mission);
    assert!(verified < recorded, "{mission}");
    assert_eq!(mission["workspace"]["state"], "ready");
    let steps = mission["nextSteps"].to_string();
    assert!(steps.contains(EARLIER_OWNER_HINT), "{steps}");
    assert!(!steps.contains("Explicit recovery is required"), "{steps}");
    assert!(steps.contains("Status is read-only"), "{steps}");
}

fn assert_single_invocation_bump(mission: &Value) {
    let (verified, recorded) = generations(mission);
    assert_eq!(verified + 1, recorded, "{mission}");
}

fn assert_reconciled(mission: &Value) {
    let (verified, recorded) = generations(mission);
    assert_eq!(verified, recorded, "{mission}");
    assert_eq!(mission["workspace"]["state"], "ready");
    let steps = mission["nextSteps"].to_string();
    assert!(!steps.contains("earlier owner generation"), "{steps}");
    assert!(!steps.contains("Explicit recovery is required"), "{steps}");
}

fn stored_generations(db: &str) -> (i64, i64) {
    let conn = rusqlite::Connection::open(db).unwrap();
    let verified: i64 = conn
        .query_row("SELECT verified_generation FROM workspaces", [], |row| {
            row.get(0)
        })
        .unwrap();
    let recorded: i64 = conn
        .query_row("SELECT generation FROM host_owner", [], |row| row.get(0))
        .unwrap();
    (verified, recorded)
}

fn approve_plan(bump: &OwnerBump) -> Value {
    let digest = bump.planned["mission"]["planDigest"].as_str().unwrap();
    let payload = format!(r#"{{"planDigest":"{digest}"}}"#);
    let revision = bump.planned["mission"]["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let (code, approved, error) = run(&[
        "mission",
        "command",
        "--db",
        &bump.db,
        "--mission",
        &bump.mission_id,
        "--kind",
        "approve_plan",
        "--expected-revision",
        &revision,
        "--payload",
        &payload,
    ]);
    assert_eq!(code, 0, "{error}");
    approved
}

fn implementation_settled(db: &str, mission_id: &str) -> Value {
    let (code, settled, error) = run(&[
        "mission",
        "notify",
        "--db",
        db,
        "--mission",
        mission_id,
        "--json-body",
        r#"{"kind":"implementation_settled","quiescent":true}"#,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(settled["mission"]["phase"], "verifying");
    settled
}

/// #200 decision 5: the 8/9-style bump after plan_completed and after a later notify.
#[test]
fn status_names_automatic_dispatch_after_plan_completed_and_a_later_notify() {
    let bump = plan_completed_owner_bump();
    let after_plan = mission_status(&bump.db, &bump.mission_id);
    assert_earlier_owner_hint(&after_plan);
    assert_single_invocation_bump(&after_plan);
    assert_eq!(after_plan["phase"], "awaiting_plan_approval");
    assert_eq!(
        generations(&after_plan),
        generations(&bump.planned["mission"])
    );

    approve_plan(&bump);
    let settled = implementation_settled(&bump.db, &bump.mission_id);
    let after_notify = mission_status(&bump.db, &bump.mission_id);
    assert_earlier_owner_hint(&after_notify);
    assert_single_invocation_bump(&after_notify);
    assert_eq!(after_notify["phase"], "verifying");
    assert_eq!(generations(&after_notify), generations(&settled["mission"]));
}

/// #200 decision 5: status does not bump or reconcile the owner generations.
#[test]
fn status_leaves_owner_generations_unchanged() {
    let bump = plan_completed_owner_bump();
    let before = stored_generations(&bump.db);
    assert!(before.0 < before.1, "{before:?}");
    let first = mission_status(&bump.db, &bump.mission_id);
    assert_earlier_owner_hint(&first);
    assert_eq!(
        stored_generations(&bump.db),
        before,
        "status wrote the owner generation or the verified generation"
    );
    let second = mission_status(&bump.db, &bump.mission_id);
    assert_eq!(stored_generations(&bump.db), before);
    assert_eq!(generations(&first), generations(&second));
    assert_eq!(first["workspace"], second["workspace"]);
    assert_eq!(first["revision"], second["revision"]);
    assert_eq!(generations(&first), (before.0 as u64, before.1 as u64));
}

/// Hint clause: `mission command` (not `--persist-only`) reconciles to N/N.
#[test]
fn dispatch_reconciles_a_ready_owner_bump_to_matching_generations() {
    let bump = plan_completed_owner_bump();
    let before = mission_status(&bump.db, &bump.mission_id);
    assert_earlier_owner_hint(&before);
    let (verified_before, recorded_before) = generations(&before);

    let approved = approve_plan(&bump);
    assert_eq!(approved["mission"]["phase"], "implementing");
    assert_reconciled(&approved["mission"]);
    let (verified, recorded) = generations(&approved["mission"]);
    assert!(recorded > recorded_before, "{approved}");
    assert_eq!(verified, recorded);

    let settled = implementation_settled(&bump.db, &bump.mission_id);
    assert_earlier_owner_hint(&settled["mission"]);
    assert_single_invocation_bump(&settled["mission"]);
    let revision = settled["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, paused, error) = run(&[
        "mission",
        "command",
        "--db",
        &bump.db,
        "--mission",
        &bump.mission_id,
        "--kind",
        "pause",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(paused["mission"]["phase"], "pausing");
    assert_reconciled(&paused["mission"]);
    let (verified, recorded) = generations(&paused["mission"]);
    assert_eq!(verified, recorded);
    assert!(verified > verified_before);
}

/// #200 decision 6: notify does not reconcile a ready owner bump.
#[test]
fn notify_does_not_reconcile_a_ready_owner_bump() {
    let bump = plan_completed_owner_bump();
    let before = mission_status(&bump.db, &bump.mission_id);
    assert_earlier_owner_hint(&before);
    assert_single_invocation_bump(&before);
    let (verified_before, recorded_before) = generations(&before);
    let (code, notified, error) = run(&[
        "mission",
        "notify",
        "--db",
        &bump.db,
        "--mission",
        &bump.mission_id,
        "--json-body",
        r#"{"kind":"idle"}"#,
    ]);
    assert_eq!(code, 0, "{error}");
    let (verified, recorded) = generations(&notified["mission"]);
    assert_eq!(verified, verified_before);
    assert!(recorded > recorded_before, "{notified}");
    assert_ne!(verified, recorded);
    assert_earlier_owner_hint(&notified["mission"]);
    let after = mission_status(&bump.db, &bump.mission_id);
    assert_eq!(generations(&after), (verified, recorded));
}

/// #200 decision 6: verify does not reconcile until the mission is verifying.
#[test]
fn verify_does_not_reconcile_a_ready_owner_bump_before_verifying() {
    let bump = plan_completed_owner_bump();
    let before = mission_status(&bump.db, &bump.mission_id);
    assert_earlier_owner_hint(&before);
    let (verified_before, recorded_before) = generations(&before);
    let (code, stdout, stderr) = run(&[
        "mission",
        "verify",
        "--db",
        &bump.db,
        "--mission",
        &bump.mission_id,
    ]);
    assert_eq!(code, 2, "{stdout} {stderr}");
    assert_eq!(stderr["error"]["category"], "illegal_transition");
    let after = mission_status(&bump.db, &bump.mission_id);
    let (verified, recorded) = generations(&after);
    assert_eq!(verified, verified_before);
    assert!(recorded > recorded_before, "{after}");
    assert_ne!(verified, recorded);
    assert_earlier_owner_hint(&after);
}

/// Hint clause: `mission verify` reconciles while the phase is verifying.
#[test]
fn verify_reconciles_a_ready_owner_bump_while_verifying() {
    let bump = plan_completed_owner_bump();
    approve_plan(&bump);
    let settled = implementation_settled(&bump.db, &bump.mission_id);
    assert_earlier_owner_hint(&settled["mission"]);
    assert_single_invocation_bump(&settled["mission"]);
    let (code, stdout, stderr) = run(&[
        "mission",
        "verify",
        "--db",
        &bump.db,
        "--mission",
        &bump.mission_id,
    ]);
    assert_eq!(code, 2, "{stdout} {stderr}");
    assert_eq!(stderr["error"]["category"], "verifier_gate_unavailable");
    let after = mission_status(&bump.db, &bump.mission_id);
    assert_reconciled(&after);
    assert_eq!(after["phase"], "verifying");
}

/// A terminal mission keeps the plain read-only status text. The reconcile
/// hint would name commands that cannot run.
#[test]
fn status_omits_the_reconcile_hint_on_a_terminal_mission() {
    let bump = plan_completed_owner_bump();
    let before = mission_status(&bump.db, &bump.mission_id);
    assert_earlier_owner_hint(&before);
    let changed = rusqlite::Connection::open(&bump.db)
        .unwrap()
        .execute(
            "UPDATE missions SET phase = 'cancelled' WHERE id = ?1",
            [&bump.mission_id],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let after = mission_status(&bump.db, &bump.mission_id);
    assert_eq!(after["phase"], "cancelled");
    assert_eq!(after["workspace"]["state"], "ready");
    assert_eq!(generations(&after), generations(&before));
    let (verified, recorded) = generations(&after);
    assert!(verified < recorded, "{after}");
    let steps = after["nextSteps"].to_string();
    assert!(steps.contains("Status is read-only"), "{steps}");
    assert!(!steps.contains(EARLIER_OWNER_HINT), "{steps}");
    assert!(!steps.contains("earlier owner generation"), "{steps}");
    assert!(!steps.contains("Explicit recovery is required"), "{steps}");
}

#[test]
fn create_refuses_missing_empty_and_whitespace_acceptance_without_a_mission() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let repo_arg = repo.to_str().unwrap().to_string();
    let objective = "keep acceptance required";

    let missing_db = tmp.path().join("missing.sqlite");
    let missing = Command::new(bin())
        .args([
            "mission",
            "create",
            "--db",
            missing_db.to_str().unwrap(),
            "--repo",
            &repo_arg,
            "--issue",
            "201",
            "--objective",
            objective,
        ])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2), "{missing:?}");
    assert!(missing.stdout.is_empty(), "{missing:?}");
    let stderr = String::from_utf8(missing.stderr).unwrap();
    assert!(
        stderr.contains("the following required arguments were not provided"),
        "{stderr}"
    );
    assert!(stderr.contains("--acceptance"), "{stderr}");
    assert!(!missing_db.exists(), "{stderr}");

    let refuse = |name: &str, acceptance: &str| {
        let db = tmp.path().join(name);
        let (code, stdout, error) = run(&[
            "mission",
            "create",
            "--db",
            db.to_str().unwrap(),
            "--repo",
            &repo_arg,
            "--issue",
            "201",
            "--objective",
            objective,
            "--acceptance",
            acceptance,
        ]);
        assert_eq!(code, 2, "{stdout} {error}");
        assert!(stdout.is_null(), "{stdout}");
        assert_eq!(error["ok"], false);
        assert_eq!(error["error"]["category"], "guard_failed");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("acceptance criteria is required"),
            "{error}"
        );
        assert!(!db.exists(), "{db:?} {error}");
        assert!(!owner_lock(&db).exists(), "{error}");
    };
    refuse("empty.sqlite", "");
    refuse("whitespace.sqlite", " \t\n ");
}

#[test]
fn refused_create_on_an_existing_ledger_leaves_generation_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let repo_arg = repo.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo_arg,
        "--issue",
        "201",
        "--objective",
        "keep acceptance required",
        "--acceptance",
        "a real acceptance string",
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(created["mission"]["runtimeAdapter"], "fake");
    let missions = count_rows(&db, "SELECT COUNT(*) FROM missions");
    let generation = host_generation(&db);
    assert_eq!(missions, 1);
    for acceptance in ["", " \t\n "] {
        let (code, stdout, error) = run(&[
            "mission",
            "create",
            "--db",
            db_arg,
            "--repo",
            repo_arg,
            "--issue",
            "201",
            "--objective",
            "keep acceptance required",
            "--acceptance",
            acceptance,
        ]);
        assert_eq!(code, 2, "{stdout} {error}");
        assert!(stdout.is_null(), "{stdout}");
        assert_eq!(error["error"]["category"], "guard_failed");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("acceptance criteria is required"),
            "{error}"
        );
        assert_eq!(count_rows(&db, "SELECT COUNT(*) FROM missions"), missions);
        assert_eq!(host_generation(&db), generation);
    }
}

#[test]
fn create_and_successor_help_state_the_acceptance_rules() {
    let create = Command::new(bin())
        .args(["mission", "create", "--help"])
        .output()
        .unwrap();
    assert!(create.status.success(), "{create:?}");
    let help = String::from_utf8(create.stdout).unwrap();
    assert!(
        help.contains("required and must not be empty or whitespace-only"),
        "{help}"
    );
    assert!(help.contains("There is no default"), "{help}");

    let successor = Command::new(bin())
        .args(["mission", "successor", "--help"])
        .output()
        .unwrap();
    assert!(successor.status.success(), "{successor:?}");
    let help = String::from_utf8(successor.stdout).unwrap();
    assert!(
        help.contains("Omitting this flag copies the source mission's acceptance"),
        "{help}"
    );
}

#[test]
fn successor_omitting_acceptance_copies_the_source() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let acceptance = "source acceptance copied forward";
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "201",
        "--objective",
        "source objective",
        "--acceptance",
        acceptance,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(created["mission"]["phase"], "draft");
    assert_eq!(created["mission"]["runtimeAdapter"], "fake");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let (code, cancelled, error) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--kind",
        "cancel",
        "--expected-revision",
        "1",
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(cancelled["mission"]["phase"], "cancelling");
    let revision = cancelled["mission"]["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let (code, confirmed, error) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--kind",
        "confirm_cancelled",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(confirmed["mission"]["phase"], "cancelled");

    let (code, successor, error) =
        run(&["mission", "successor", "--db", db_arg, "--mission", mission]);
    assert_eq!(code, 0, "{error}");
    let copied = &successor["mission"];
    assert_eq!(copied["phase"], "draft");
    assert_eq!(copied["objective"], "source objective");
    assert_eq!(copied["runtimeAdapter"], "fake");
    assert_eq!(copied["successorOfMissionId"], mission);
    let successor_id = copied["missionId"].as_str().unwrap();
    assert_ne!(successor_id, mission);
    let conn = rusqlite::Connection::open(&db).unwrap();
    let acceptance_of = |mission_id: &str| -> String {
        conn.query_row(
            "SELECT acceptance_criteria FROM tasks WHERE mission_id = ?1 AND root = 1",
            [mission_id],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(acceptance_of(mission), acceptance);
    assert_eq!(acceptance_of(successor_id), acceptance);
    let rationale: String = conn
        .query_row(
            "SELECT rationale FROM decisions WHERE mission_id = ?1 AND action = 'create'",
            [successor_id],
            |row| row.get(0),
        )
        .unwrap();
    let payload: Value = serde_json::from_str(&rationale).unwrap();
    assert_eq!(payload["overridden"], serde_json::json!([]));
    assert_eq!(count_rows(&db, "SELECT COUNT(*) FROM missions"), 2);
}

#[test]
fn refused_successor_acceptance_leaves_generation_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "201",
        "--objective",
        "source objective",
        "--acceptance",
        "source acceptance copied forward",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let (code, cancelled, error) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--kind",
        "cancel",
        "--expected-revision",
        "1",
    ]);
    assert_eq!(code, 0, "{error}");
    let revision = cancelled["mission"]["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let (code, confirmed, error) = run(&[
        "mission",
        "command",
        "--db",
        db_arg,
        "--mission",
        mission,
        "--kind",
        "confirm_cancelled",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(confirmed["mission"]["phase"], "cancelled");
    let missions = count_rows(&db, "SELECT COUNT(*) FROM missions");
    let generation = host_generation(&db);
    assert_eq!(missions, 1);

    let refuse = |extra: &[&str]| {
        let mut args = vec!["mission", "successor", "--db", db_arg, "--mission", mission];
        args.extend_from_slice(extra);
        let (code, stdout, error) = run(&args);
        assert_eq!(code, 2, "{stdout} {error}");
        assert!(stdout.is_null(), "{stdout}");
        assert_eq!(error["error"]["category"], "guard_failed");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("acceptance criteria is required"),
            "{error}"
        );
        assert_eq!(count_rows(&db, "SELECT COUNT(*) FROM missions"), missions);
        assert_eq!(host_generation(&db), generation);
    };
    refuse(&["--acceptance", ""]);
    let empty = tmp.path().join("empty-acceptance.txt");
    std::fs::write(&empty, "").unwrap();
    let file_arg = format!("@{}", empty.display());
    refuse(&["--acceptance", &file_arg]);
}
