//! CLI opt-in for the confined Pi runtime. Gated like the other confined
//! suites: missing `SWITCHYARD_*` prerequisites fail this test, and the flag
//! never falls back to FakeRuntime. The binary runs the composed scenario's
//! `s3-acceptance` fixture (one write, one process start, one process status).
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_switchyard")
}

fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{key} required; no fallback"))
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
        // The endpoint is the flag alone. A leftover environment value must
        // not change the composed fixture's `network: denied` record.
        .env_remove("SWITCHYARD_MODEL_ENDPOINT")
        .output()
        .unwrap();
    let stdout = serde_json::from_slice::<Value>(&output.stdout).unwrap_or(Value::Null);
    let stderr = serde_json::from_slice::<Value>(&output.stderr).unwrap_or(Value::Null);
    (output.status.code().unwrap_or(1), stdout, stderr)
}

#[test]
fn cli_confined_pi_runs_composed_s3_acceptance_scenario() {
    for key in [
        "SWITCHYARD_PI_RUNTIME",
        "SWITCHYARD_BWRAP",
        "SWITCHYARD_CGROUP_ROOT",
        "SWITCHYARD_PI_NODE",
        "SWITCHYARD_BRIDGE_LAUNCHER",
        "SWITCHYARD_BRIDGE_POLICY",
        "SWITCHYARD_EXECUTION_HELPER",
    ] {
        require_env(key);
    }
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
        "--runtime",
        "confined-pi",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "97",
        "--objective",
        "composed scenario from the CLI",
        "--acceptance",
        "one write, one process start, one process status",
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(created["mission"]["runtimeAdapter"], "pi-confined-v1");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let attempt = created["mission"]["attemptId"].as_str().unwrap();
    let (code, prepared, error) = run(&[
        "mission",
        "workspace",
        "--runtime",
        "confined-pi",
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
    assert_eq!(prepared["workspace"]["state"], "ready");
    let workspace = Path::new(prepared["workspace"]["path"].as_str().unwrap());
    // Operators cannot admit `plan_completed`: that observation must come from
    // the runtime, and a confined host refuses fake notifications as mission
    // facts. The s3-acceptance fixture does not plan. Reaching
    // `awaiting_plan_approval` is the same seeded row the probe harness writes,
    // not an S4 command. This import is what lets `approve_plan` run the
    // composed scenario from the CLI.
    let frozen = switchyard_core::read_attempt_freeze(Path::new(db_arg), mission).unwrap();
    let digest = switchyard_core::bound_plan_digest("fixture plan", attempt, &frozen).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE missions SET phase='awaiting_plan_approval' WHERE id=?1",
        [mission],
    )
    .unwrap();
    conn.execute(
        "UPDATE attempts SET frozen_inputs_json=?1, plan_body='fixture plan', plan_digest=?2 WHERE id=?3",
        rusqlite::params![frozen, digest, attempt],
    )
    .unwrap();
    let (code, status, error) = run(&[
        "mission",
        "status",
        "--runtime",
        "confined-pi",
        "--db",
        db_arg,
        "--mission",
        mission,
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(status["missions"][0]["phase"], "awaiting_plan_approval");
    let revision = status["missions"][0]["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let payload = json!({"planDigest": digest}).to_string();
    let (code, approved, error) = run(&[
        "mission",
        "command",
        "--runtime",
        "confined-pi",
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
    let mission_json = &approved["mission"];
    assert_eq!(mission_json["runtimeAdapter"], "pi-confined-v1");
    assert_eq!(mission_json["phase"], "implementing");
    assert_eq!(approved["composedScenario"], "complete");
    let effects = mission_json["linuxExecutionEffects"].as_array().unwrap();
    assert_eq!(effects.len(), 3, "{approved}");
    for method in ["write_file", "exec_start", "exec_status"] {
        assert_eq!(
            effects
                .iter()
                .filter(|effect| effect["method"] == method)
                .count(),
            1,
            "{approved}"
        );
    }
    assert!(effects.iter().all(|effect| effect["simulated"] == false));
    assert_eq!(
        std::fs::read_to_string(workspace.join("output.txt")).unwrap(),
        "once"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("process.marker")).unwrap(),
        "once\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "hello\n"
    );
    let config: String = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT runtime_config_json FROM attempts", [], |row| {
            row.get(0)
        })
        .unwrap();
    let config: Value = serde_json::from_str(&config).unwrap();
    assert_eq!(config["adapter"], "pi-confined-v1");
    assert_eq!(config["credentials"], "none");
    assert_eq!(config["fixture"], "s3-acceptance");
    // No `--model-endpoint` flag. `run` also clears `SWITCHYARD_MODEL_ENDPOINT`
    // so this denial does not depend on the process environment.
    assert_eq!(config["network"], "denied");
    assert!(
        !config
            .to_string()
            .contains("SENTINEL_GH_TOKEN_SHOULD_NOT_PERSIST")
    );
    assert!(
        !config
            .to_string()
            .contains("SENTINEL_ANTHROPIC_KEY_SHOULD_NOT_PERSIST")
    );
    let (code, _, error) = run(&["mission", "recover", "--db", db_arg, "--mission", mission]);
    assert_eq!(code, 2, "{error}");
    let message = error["error"]["message"].as_str().unwrap();
    assert!(message.contains("no adapter replacement"), "{error}");
    assert!(message.contains("--runtime confined-pi"), "{error}");
}

/// The endpoint is parsed before the `SWITCHYARD_*` path prerequisites, so
/// these tests stay deterministic without the confined runtime installed.
fn run_without_prerequisites(repo: &Path, db: &Path, endpoint: &str) -> std::process::Output {
    Command::new(bin())
        .args([
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
            "97",
            "--objective",
            "model endpoint guard",
            "--acceptance",
            "one write",
        ])
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

fn guard_message(output: &std::process::Output) -> String {
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(stderr["ok"], false);
    assert_eq!(stderr["error"]["category"], "guard_failed");
    stderr["error"]["message"].as_str().unwrap().to_string()
}

#[test]
fn model_endpoint_flag_fails_closed_on_https_loopback() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let output = run_without_prerequisites(&repo, &db, "https://127.0.0.1/v1");
    let message = guard_message(&output);
    assert_eq!(message.matches("guard failed").count(), 1, "{message}");
    assert!(message.contains("TLS client"), "{message}");
    assert!(!message.contains("https://127.0.0.1"), "{message}");
    assert!(!message.contains("SWITCHYARD_MODEL_ENDPOINT"), "{message}");
    assert!(!db.exists());
    assert!(!tmp.path().join("mission.sqlite.pi-bridge").exists());
}

#[test]
fn model_endpoint_loopback_literals_pass_parse_before_path_prerequisites() {
    let literals = [
        "http://127.0.0.1:8000/v1",
        "http://127.0.0.1/v1",
        "http://[::1]:8000/v1",
        "http://[::1]/v1",
        "http://127.42.1.2:9/v1",
    ];
    for endpoint in literals {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let db = tmp.path().join("mission.sqlite");
        init_repo(&repo);
        let output = run_without_prerequisites(&repo, &db, endpoint);
        let message = guard_message(&output);
        assert!(
            message.contains("SWITCHYARD_PI_RUNTIME"),
            "{endpoint}: {message}"
        );
        assert!(
            !message.contains("loopback IP literal"),
            "{endpoint}: {message}"
        );
        assert!(!message.contains("TLS client"), "{endpoint}: {message}");
        assert!(!message.contains("SWITCHYARD_MODEL_ENDPOINT"), "{message}");
        assert!(!db.exists(), "{endpoint}");
        assert!(!tmp.path().join("mission.sqlite.pi-bridge").exists());
    }
}

/// Create stores the pin. A later `cancel` reopens the host, so a different
/// flag fails in that open and the original flag is admitted. Both need the
/// confined-execution prerequisites, same as the composed scenario above,
/// and run in the `execution-confinement` job. `cancel --persist-only` still
/// opens the host; it does not dispatch the faux bridge.
#[test]
fn model_endpoint_reopen_refuses_a_different_value_and_accepts_the_original() {
    for key in [
        "SWITCHYARD_PI_RUNTIME",
        "SWITCHYARD_BWRAP",
        "SWITCHYARD_CGROUP_ROOT",
        "SWITCHYARD_PI_NODE",
        "SWITCHYARD_BRIDGE_LAUNCHER",
        "SWITCHYARD_BRIDGE_POLICY",
        "SWITCHYARD_EXECUTION_HELPER",
    ] {
        require_env(key);
    }
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let db_arg = db.to_str().unwrap();
    let endpoint = "http://127.0.0.1:8000/v1";
    let other = "http://127.0.0.2:8000/v1";
    let (code, created, error) = run(&[
        "mission",
        "create",
        "--runtime",
        "confined-pi",
        "--model-endpoint",
        endpoint,
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "97",
        "--objective",
        "pin the model endpoint for the mission lifetime",
        "--acceptance",
        "reopen with the same literal",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let revision = created["mission"]["revision"].as_u64().unwrap().to_string();
    let config: String = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT runtime_config_json FROM attempts", [], |row| {
            row.get(0)
        })
        .unwrap();
    let config: Value = serde_json::from_str(&config).unwrap();
    assert_eq!(config["network"]["mode"], "model-endpoint");
    assert_eq!(config["network"]["url"], endpoint);
    assert_eq!(config["network"]["pinned"], "127.0.0.1:8000");
    assert!(config["network"]["url"].as_str().unwrap().contains("/v1"));

    let (code, stdout, error) = run(&[
        "mission",
        "command",
        "--runtime",
        "confined-pi",
        "--model-endpoint",
        other,
        "--db",
        db_arg,
        "--mission",
        mission,
        "--kind",
        "cancel",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(code, 2, "{stdout} {error}");
    let message = error["error"]["message"].as_str().unwrap();
    assert_eq!(error["error"]["category"], "guard_failed");
    assert!(message.contains("network"), "{message}");
    assert!(message.contains("no adapter replacement"), "{message}");
    let (code, status, error) = run(&["mission", "status", "--db", db_arg, "--mission", mission]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(status["missions"][0]["phase"], "draft");

    let (code, cancelled, error) = run(&[
        "mission",
        "command",
        "--runtime",
        "confined-pi",
        "--model-endpoint",
        endpoint,
        "--db",
        db_arg,
        "--mission",
        mission,
        "--kind",
        "cancel",
        "--expected-revision",
        &revision,
        "--persist-only",
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(cancelled["ok"], true);
    assert_eq!(cancelled["mission"]["phase"], "cancelling");
}

/// Real `switchyard` process. SIGINT during the foreground wait admits
/// cancel and settles with the existing cgroup proof. `process-hold` keeps
/// the runtime busy so the wait does not finish first. The default fixture
/// stays `s3-acceptance`.
#[test]
fn cli_sigint_during_foreground_wait_cancels_with_cgroup_proof() {
    for key in [
        "SWITCHYARD_PI_RUNTIME",
        "SWITCHYARD_BWRAP",
        "SWITCHYARD_CGROUP_ROOT",
        "SWITCHYARD_PI_NODE",
        "SWITCHYARD_BRIDGE_LAUNCHER",
        "SWITCHYARD_BRIDGE_POLICY",
        "SWITCHYARD_EXECUTION_HELPER",
    ] {
        require_env(key);
    }
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
        "--runtime",
        "confined-pi",
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "218",
        "--objective",
        "hold the bridge open",
        "--acceptance",
        "Ctrl-C cancels with a cgroup proof",
    ]);
    assert_eq!(code, 0, "{error}");
    let mission = created["mission"]["missionId"].as_str().unwrap();
    let attempt = created["mission"]["attemptId"].as_str().unwrap();
    let (code, prepared, error) = run(&[
        "mission",
        "workspace",
        "--runtime",
        "confined-pi",
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
    assert_eq!(prepared["workspace"]["state"], "ready");
    let frozen = switchyard_core::read_attempt_freeze(Path::new(db_arg), mission).unwrap();
    let digest = switchyard_core::bound_plan_digest("fixture plan", attempt, &frozen).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE missions SET phase='awaiting_plan_approval' WHERE id=?1",
        [mission],
    )
    .unwrap();
    conn.execute(
        "UPDATE attempts SET frozen_inputs_json=?1, plan_body='fixture plan', plan_digest=?2 WHERE id=?3",
        rusqlite::params![frozen, digest, attempt],
    )
    .unwrap();
    drop(conn);
    let (code, status, error) = run(&[
        "mission",
        "status",
        "--runtime",
        "confined-pi",
        "--db",
        db_arg,
        "--mission",
        mission,
    ]);
    assert_eq!(code, 0, "{error}");
    let revision = status["missions"][0]["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let payload = json!({"planDigest": digest}).to_string();
    let mut child = Command::new(bin())
        .args([
            "mission",
            "command",
            "--runtime",
            "confined-pi",
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
        ])
        .env("SWITCHYARD_CONFINED_FIXTURE", "process-hold")
        .env("GH_TOKEN", "SENTINEL_GH_TOKEN_SHOULD_NOT_PERSIST")
        .env(
            "ANTHROPIC_API_KEY",
            "SENTINEL_ANTHROPIC_KEY_SHOULD_NOT_PERSIST",
        )
        .env_remove("SWITCHYARD_MODEL_ENDPOINT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let phase: String = rusqlite::Connection::open(&db)
            .unwrap()
            .query_row("SELECT phase FROM missions WHERE id=?1", [mission], |row| {
                row.get(0)
            })
            .unwrap_or_default();
        let effects: i64 = rusqlite::Connection::open(&db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM linux_execution_effects", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);
        if phase == "implementing" && effects > 0 {
            break;
        }
        match child.try_wait().unwrap() {
            Some(status) => panic!("foreground wait ended before SIGINT: {status} phase={phase}"),
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            None => panic!("process-hold did not enter the foreground wait"),
        }
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        child.try_wait().unwrap().is_none(),
        "wait ended before SIGINT"
    );
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let output = child.wait_with_output().unwrap();
    let stdout = serde_json::from_slice::<Value>(&output.stdout).unwrap_or(Value::Null);
    assert!(
        output.status.success(),
        "stderr {} stdout {stdout}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(stdout["composedScenario"], "cancelled", "{stdout}");
    assert_eq!(stdout["mission"]["phase"], "cancelled", "{stdout}");
    let export =
        switchyard_core::Host::export_document(Path::new(db_arg), mission, "s2-2").unwrap();
    let receipt = export["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["kind"] == "confirm_cancelled")
        .unwrap()["receipt"]
        .clone();
    assert_eq!(receipt["proven"], true, "{receipt}");
    assert!(receipt["cgroup"].is_object(), "{receipt}");
}
