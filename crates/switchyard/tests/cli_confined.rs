//! CLI opt-in for the confined Pi runtime. Gated like the other confined
//! suites: missing `SWITCHYARD_*` prerequisites fail this test, and the flag
//! never falls back to FakeRuntime. The binary runs the composed scenario's
//! `s3-acceptance` fixture (one write, one process start, one process status).
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::path::Path;
use std::process::Command;

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
    let digest = switchyard_core::payload_hash(&json!("fixture plan")).unwrap();
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE missions SET phase='awaiting_plan_approval', plan_body='fixture plan', plan_digest=?1 WHERE id=?2",
            rusqlite::params![digest, mission],
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
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no adapter replacement"),
        "{error}"
    );
}
