//! CLI opt-in for the confined Pi runtime. Gated like the other confined
//! suites: missing `SWITCHYARD_*` prerequisites fail this test, and the flag
//! never falls back to FakeRuntime. The binary runs the composed scenario's
//! `s3-acceptance` fixture (one write, one process start, one process status).
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

fn error_message(output: &std::process::Output, category: &str) -> String {
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(stderr["ok"], false);
    assert_eq!(stderr["error"]["category"], category, "{stderr}");
    stderr["error"]["message"].as_str().unwrap().to_string()
}

#[test]
fn model_endpoint_flag_fails_closed_on_https_loopback() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let db = tmp.path().join("mission.sqlite");
    init_repo(&repo);
    let output = run_without_prerequisites(&repo, &db, "https://127.0.0.1/v1");
    let message = error_message(&output, "model_endpoint_not_allowed");
    assert!(!message.contains("guard failed"), "{message}");
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
        let message = error_message(&output, "guard_failed");
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
    assert_eq!(config["fixture"], "product");
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
    assert!(message.contains("frozen"), "{message}");
    assert!(message.contains("supplied"), "{message}");
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

/// Five-second bridge abort deadline (`pins.json` `deadlineMs`). A cancel
/// must finish inside it while the model socket is still held open.
const CANCEL_BOUND: Duration = Duration::from_secs(5);

fn read_http_request(stream: &mut std::net::TcpStream) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 2048];
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        let Some(split) = buf.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&buf[..split]);
        let length = head.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        });
        if let Some(length) = length
            && buf.len() >= split + 4 + length
        {
            break;
        }
    }
    Ok(buf)
}

fn sse_reply() -> Vec<u8> {
    let body = "data: {\"id\":\"stub\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"stub-done\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"stub\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let mut bytes = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend(body.as_bytes());
    bytes
}

#[test]
fn product_fixture_sends_the_work_brief_and_cancel_returns_while_the_stub_holds() {
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
    let secret = "sk-real-key-must-not-appear";
    // SAFETY: test-only sentinel. Rust 1.99 makes env mutation unsafe because
    // it races other threads. execution-confinement runs this suite on one thread.
    unsafe {
        std::env::set_var("OPENAI_API_KEY", secret);
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let endpoint = format!("http://127.0.0.1:{port}/v1");
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let server_seen = Arc::clone(&seen);
    let server_stop = Arc::clone(&stop);
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        while !server_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let raw = read_http_request(&mut stream).unwrap_or_default();
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    let hold = text.contains("switchyard.work-brief.v1");
                    server_seen.lock().unwrap().push(text);
                    if hold {
                        let _ = stream.set_read_timeout(None);
                        let mut byte = [0_u8; 1];
                        let _ = stream.read(&mut byte);
                    } else {
                        let _ = stream.write_all(&sse_reply());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

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
        "--model-endpoint",
        &endpoint,
        "--db",
        db_arg,
        "--repo",
        repo.to_str().unwrap(),
        "--issue",
        "217",
        "--objective",
        "product work brief",
        "--acceptance",
        "the stub sees the brief",
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
    let mut revision = created["mission"]["revision"].as_u64().unwrap().to_string();
    let (code, prepared, error) = run(&[
        "mission",
        "workspace",
        "--runtime",
        "confined-pi",
        "--model-endpoint",
        &endpoint,
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
    assert_eq!(prepared["workspace"]["state"], "ready");
    let (code, planned, error) = run(&[
        "mission",
        "command",
        "--runtime",
        "confined-pi",
        "--model-endpoint",
        &endpoint,
        "--db",
        db_arg,
        "--mission",
        &mission,
        "--kind",
        "start_planning",
        "--expected-revision",
        &revision,
        "--payload",
        "{}",
    ]);
    assert_eq!(code, 0, "{error}");
    revision = planned["mission"]["revision"].as_u64().unwrap().to_string();
    let frozen = switchyard_core::read_attempt_freeze(Path::new(db_arg), &mission).unwrap();
    let plan = "fixture plan";
    let digest = switchyard_core::bound_plan_digest(plan, &attempt, &frozen).unwrap();
    let brief = switchyard_core::work_brief(&frozen, plan, &digest, None).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE missions SET phase='awaiting_plan_approval' WHERE id=?1",
        [&mission],
    )
    .unwrap();
    conn.execute(
        "UPDATE attempts SET plan_body=?1, plan_digest=?2 WHERE id=?3",
        rusqlite::params![plan, digest, attempt],
    )
    .unwrap();
    drop(conn);

    let bridge = {
        let name = db.file_name().unwrap().to_str().unwrap();
        std::fs::canonicalize(db.parent().unwrap().join(format!("{name}.pi-bridge"))).unwrap()
    };
    let model = switchyard_core::ModelEndpoint::parse(&endpoint).unwrap();
    let mut host = switchyard_core::Host::open_confined_pi(
        Path::new(db_arg),
        switchyard_core::ConfinedPiConfig {
            root: bridge,
            cgroup_root: require_env("SWITCHYARD_CGROUP_ROOT").into(),
            runtime: require_env("SWITCHYARD_PI_RUNTIME").into(),
            node: require_env("SWITCHYARD_PI_NODE").into(),
            bubblewrap: require_env("SWITCHYARD_BWRAP").into(),
            launcher: require_env("SWITCHYARD_BRIDGE_LAUNCHER").into(),
            policy: require_env("SWITCHYARD_BRIDGE_POLICY").into(),
            model_endpoint: Some(model),
        },
        switchyard_core::LinuxExecutionConfig {
            cgroup_root: require_env("SWITCHYARD_CGROUP_ROOT").into(),
            bubblewrap: require_env("SWITCHYARD_BWRAP").into(),
            helper: require_env("SWITCHYARD_EXECUTION_HELPER").into(),
            crash_at: None,
            probe_request: None,
            fail_launch: false,
        },
        "product",
    )
    .unwrap();
    let expected_revision = revision.parse::<u64>().unwrap();
    host.admit(switchyard_core::AdmitRequest {
        command_id: None,
        mission_id: mission.clone(),
        attempt_id: None,
        expected_revision,
        actor: "operator".into(),
        origin: switchyard_core::CommandOrigin::Operator,
        body: switchyard_core::CommandBody::ApprovePlan {
            plan_digest: digest,
        },
    })
    .unwrap();
    host.dispatch_pending(&mission).unwrap();
    let started_wait = Instant::now();
    let request = loop {
        let hit = seen
            .lock()
            .unwrap()
            .iter()
            .find(|raw| raw.contains("switchyard.work-brief.v1"))
            .cloned();
        if let Some(hit) = hit {
            break hit;
        }
        assert!(
            started_wait.elapsed() < Duration::from_secs(20),
            "approve_plan did not reach the stub"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        request.starts_with("POST /v1/chat/completions "),
        "{request}"
    );
    assert!(
        request.contains("\"stream\":true") || request.contains("\"stream\": true"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer unused"),
        "{request}"
    );
    assert!(!request.contains(secret), "{request}");
    let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
    let parsed: Value = serde_json::from_str(body).unwrap();
    let content = parsed["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .find(|content| content.contains("switchyard.work-brief.v1"))
        .unwrap_or("");
    assert_eq!(content, brief);

    let started = Instant::now();
    host.admit(switchyard_core::AdmitRequest {
        command_id: None,
        mission_id: mission.clone(),
        attempt_id: None,
        expected_revision: host.snapshot(&mission).unwrap().revision,
        actor: "operator".into(),
        origin: switchyard_core::CommandOrigin::Operator,
        body: switchyard_core::CommandBody::Cancel,
    })
    .unwrap();
    host.dispatch_stops(&mission).unwrap();
    assert!(
        started.elapsed() < CANCEL_BOUND,
        "cancel took {:?}, bound is {CANCEL_BOUND:?}",
        started.elapsed()
    );
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    // SAFETY: clears the sentinel set above. Same single-threaded suite.
    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
    }
}
