//! #199b confined verifier. The execution-confinement job runs this module.
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::fixtures::{admit, notify, plan_note};
use crate::execution::RuntimeNotification;
use crate::linux_execution::cgroup;
use crate::persistence::RUST_VERIFY_JSON;
use crate::{CommandBody, CreateMission, Host, LinuxExecutionConfig, Phase};

fn require_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("{name} is required; missing cgroup delegation or Bubblewrap fails this gate")
    })
}

fn helper_bin() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("target");
    path.push("debug");
    path.push("switchyard-execution-helper");
    assert!(
        path.is_file(),
        "switchyard-execution-helper is missing at {}",
        path.display()
    );
    path
}

fn linux_config(crash_at: Option<&str>) -> LinuxExecutionConfig {
    LinuxExecutionConfig {
        cgroup_root: require_env("SWITCHYARD_CGROUP_ROOT").into(),
        bubblewrap: require_env("SWITCHYARD_BWRAP").into(),
        helper: helper_bin(),
        crash_at: crash_at.map(str::to_string),
        probe_request: None,
        fail_launch: false,
    }
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_file(repo: &Path, rel: &str, body: &str) {
    let path = repo.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

fn fixture(test_body: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.email", "switchyard@example.com"]);
    git(&repo, &["config", "user.name", "Switchyard"]);
    write_file(
        &repo,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/demo\"]\nresolver = \"2\"\n",
    );
    write_file(
        &repo,
        "rust-toolchain.toml",
        "[toolchain]\nchannel = \"1.99.1\"\ncomponents = [\"rustfmt\", \"clippy\"]\nprofile = \"minimal\"\n",
    );
    write_file(
        &repo,
        "scripts/rust-verify.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\ncd \"$(dirname \"$0\")/..\"\ncargo fmt --all -- --check\ncargo clippy --workspace --all-targets --locked -- -D warnings\ncargo test --workspace --locked\n",
    );
    let script = repo.join("scripts/rust-verify.sh");
    let mut perms = fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).unwrap();
    write_file(
        &repo,
        "crates/demo/Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &repo,
        "crates/demo/src/lib.rs",
        "pub fn kept() -> i32 { 1 }\n",
    );
    write_file(&repo, "crates/demo/tests/kept.rs", test_body);
    let formatted = Command::new("cargo")
        .args(["fmt", "--all"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(formatted.success(), "cargo fmt");
    let status = Command::new("cargo")
        .arg("generate-lockfile")
        .arg("--offline")
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(status.success(), "cargo generate-lockfile --offline");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "fixture"]);
    (tmp, repo)
}

fn mission(repo: &Path, db: &Path) -> (Host, String, String) {
    let mut host = Host::open(db).unwrap();
    let created = host
        .create_mission_with_verifier(
            CreateMission {
                repo: repo.to_path_buf(),
                issue_number: 199,
                issue_title: "confined".into(),
                objective: "Run the verifier".into(),
                acceptance: "Evidence".into(),
                actor: "operator".into(),
                max_repairs: 2,
                max_plan_revisions: 3,
            },
            Some(RUST_VERIFY_JSON.into()),
        )
        .unwrap();
    let id = created.mission_id.clone();
    admit(&mut host, &id, CommandBody::StartPlanning);
    host.dispatch_pending(&id).unwrap();
    notify(&mut host, &id, plan_note("plan the verifier"));
    let digest = host.snapshot(&id).unwrap().plan_digest.unwrap();
    admit(
        &mut host,
        &id,
        CommandBody::ApprovePlan {
            plan_digest: digest,
        },
    );
    notify(
        &mut host,
        &id,
        RuntimeNotification::ImplementationSettled { quiescent: true },
    );
    fs::create_dir_all(db.parent().unwrap().join("ws")).unwrap();
    host.prepare_workspace(&id, &created.attempt_id, &db.parent().unwrap().join("ws"))
        .unwrap();
    (host, id, created.attempt_id)
}

fn evidence(db: &Path, mission: &str) -> serde_json::Value {
    let conn = rusqlite::Connection::open(db).unwrap();
    let body: String = conn
        .query_row(
            "SELECT e.body_json FROM verifications v JOIN evidence e ON e.id = v.evidence_id
             WHERE v.mission_id = ?1 AND v.invalidated = 0 ORDER BY v.seq DESC LIMIT 1",
            [mission],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&body).unwrap()
}

const PASS_TEST: &str = "#[test]\nfn kept() { assert_eq!(demo::kept(), 1); }\n";
const FAIL_TEST: &str = "#[test]\nfn kept() { assert_eq!(demo::kept(), 0); }\n";

#[test]
fn host_verifier_confined_pass_admits_complete_pass() {
    let (tmp, repo) = fixture(PASS_TEST);
    let db = tmp.path().join("mission.sqlite");
    let (mut host, id, _) = mission(&repo, &db);
    host.enable_linux_execution(linux_config(None)).unwrap();
    let (snap, outcome) = host.verify_mission(&id).unwrap();
    let document = evidence(&db, &id);
    assert_eq!(outcome, "pass", "{document}");
    assert_eq!(snap.phase, Phase::AwaitingReview);
    assert_eq!(document["outcome"], "pass");
    assert!(
        document["stdoutSha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    let conn = rusqlite::Connection::open(&db).unwrap();
    let provenance: String = conn
        .query_row(
            "SELECT e.provenance_json FROM verifications v JOIN evidence e ON e.id = v.evidence_id
             WHERE v.mission_id = ?1 AND v.invalidated = 0 ORDER BY v.seq DESC LIMIT 1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    let provenance: serde_json::Value = serde_json::from_str(&provenance).unwrap();
    assert_eq!(provenance["collector"], "switchyard-host-verifier");
    assert_eq!(provenance["bubblewrap"], "0.9.0");
    assert!(
        provenance["toolchain"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    let binds = provenance["etcBinds"]
        .as_array()
        .expect("etcBinds records the host check");
    assert!(
        binds.iter().all(|entry| {
            entry.as_str().is_some_and(|path| {
                crate::linux_execution::VERIFIER_ETC_ALLOWLIST.contains(&path)
                    && std::path::Path::new(path).exists()
            })
        }),
        "{provenance}"
    );
    assert!(
        !std::path::Path::new("/etc/alternatives").exists()
            || binds.iter().any(|entry| entry == "/etc/alternatives"),
        "{provenance}"
    );
    assert!(provenance.get("linkerMissing").is_none(), "{provenance}");
    assert_eq!(
        provenance["cpuMax"],
        crate::linux_execution::VERIFIER_CPU_MAX
    );
    assert_eq!(provenance["memoryMax"], "4G");
    assert_eq!(provenance["pidsMax"], "2048");
}

#[test]
fn host_verifier_confined_fail_stays_verifying() {
    let (tmp, repo) = fixture(FAIL_TEST);
    let db = tmp.path().join("mission.sqlite");
    let (mut host, id, _) = mission(&repo, &db);
    host.enable_linux_execution(linux_config(None)).unwrap();
    let (snap, outcome) = host.verify_mission(&id).unwrap();
    assert_eq!((outcome.as_str(), snap.phase), ("fail", Phase::Verifying));
    assert_eq!(evidence(&db, &id)["outcome"], "fail");
}

#[test]
fn host_verifier_confined_sleep_past_the_deadline_is_incomplete() {
    let body = "#[test]\nfn slow() { std::thread::sleep(std::time::Duration::from_secs(90)); }\n";
    let (tmp, repo) = fixture(body);
    let db = tmp.path().join("mission.sqlite");
    let (mut host, id, _) = mission(&repo, &db);
    host.enable_linux_execution(linux_config(None)).unwrap();
    host.verifier_deadline_ms = Some(15_000);
    let (snap, outcome) = host.verify_mission(&id).unwrap();
    assert_eq!(outcome, "incomplete", "{}", evidence(&db, &id));
    assert_eq!(snap.phase, Phase::NeedsAttention);
    assert_eq!(
        snap.attention_reason.as_deref(),
        Some("verification_incomplete")
    );
}

#[test]
fn host_verifier_confined_network_is_denied() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let (tmp, repo) = fixture(PASS_TEST);
    let script = format!("echo >/dev/tcp/127.0.0.1/{port}\n");
    let result = profile_command(&repo, &["/usr/bin/bash", "-c", &script]);
    let accepted = listener.accept().is_ok();
    assert!(!accepted, "sandbox connected: {result}");
    assert!(
        result.contains("ENTERED") && result.contains("DONE"),
        "{result}"
    );
    let control = profile_command(&repo, &["/bin/echo", "ok"]);
    assert!(
        control.contains("ENTERED") && control.contains("DONE"),
        "{control}"
    );
    assert!(control.contains("ok"), "{control}");
    let _ = tmp;
}

#[test]
fn host_verifier_confined_mutation_during_the_run_is_incomplete() {
    let body = "#[test]\nfn beat() {\n    let dir = std::env::var(\"CARGO_TARGET_DIR\").unwrap();\n    std::fs::write(format!(\"{dir}/beat\"), \"1\").unwrap();\n    std::thread::sleep(std::time::Duration::from_secs(20));\n}\n";
    let (tmp, repo) = fixture(body);
    let db = tmp.path().join("mission.sqlite");
    let target = tmp.path().join("cargo-target");
    fs::create_dir_all(&target).unwrap();
    let (mut host, id, _) = mission(&repo, &db);
    let tree = PathBuf::from(host.snapshot(&id).unwrap().workspace.unwrap().path);
    host.enable_linux_execution(linux_config(None)).unwrap();
    host.verifier_target_dir = Some(target.clone());
    host.verifier_deadline_ms = Some(60_000);
    let beat = target.join("beat");
    let tree_for_thread = tree.clone();
    let watcher = thread::spawn(move || {
        let start = Instant::now();
        while !beat.exists() {
            if start.elapsed() > Duration::from_secs(45) {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        fs::write(
            tree_for_thread.join("crates/demo/src/lib.rs"),
            "pub fn kept() -> i32 { 2 }\n",
        )
        .unwrap();
    });
    let (snap, outcome) = host.verify_mission(&id).unwrap();
    watcher.join().unwrap();
    assert_eq!(outcome, "incomplete", "{}\n{snap:?}", evidence(&db, &id));
    assert_eq!(snap.phase, Phase::NeedsAttention);
}

#[test]
fn host_verifier_confined_symlink_outside_the_worktree_cannot_be_followed() {
    let (tmp, repo) = fixture(PASS_TEST);
    let secret = tmp.path().join("secret.txt");
    fs::write(&secret, "TOP-SECRET-BYTES\n").unwrap();
    let link = repo.join("crates/demo/tests/outside");
    symlink(&secret, &link).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "symlink"]);
    let result = profile_command(&repo, &["/bin/cat", "crates/demo/tests/outside"]);
    assert!(
        result.contains("ENTERED") && result.contains("DONE"),
        "{result}"
    );
    assert!(
        !result.contains("TOP-SECRET-BYTES"),
        "sandbox followed the symlink: {result}"
    );
    let control = profile_command(&repo, &["/bin/cat", "crates/demo/src/lib.rs"]);
    assert!(control.contains("fn kept"), "{control}");
}

#[test]
fn host_verifier_confined_missing_toolchain_is_unavailable() {
    let (tmp, repo) = fixture(PASS_TEST);
    let db = tmp.path().join("mission.sqlite");
    let (mut host, id, _) = mission(&repo, &db);
    host.enable_linux_execution(linux_config(None)).unwrap();
    let empty = tmp.path().join("empty-rustup");
    fs::create_dir_all(&empty).unwrap();
    host.verifier_rustup_home = Some(empty);
    let error = host.verify_mission(&id).unwrap_err();
    assert_eq!(
        error.category(),
        "verifier_confinement_unavailable",
        "{error}"
    );
    assert!(
        error.to_string().contains("toolchain_unavailable"),
        "{error}"
    );
    assert_eq!(host.snapshot(&id).unwrap().phase, Phase::Verifying);
    let rows: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM verifications WHERE mission_id = ?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn host_verifier_confined_missing_linker_is_toolchain_unavailable() {
    let (tmp, repo) = fixture(PASS_TEST);
    let db = tmp.path().join("mission.sqlite");
    let (mut host, id, _) = mission(&repo, &db);
    host.enable_linux_execution(linux_config(None)).unwrap();
    host.verifier_omit_linker = true;
    let error = host.verify_mission(&id).unwrap_err();
    assert_eq!(
        error.category(),
        "verifier_confinement_unavailable",
        "{error}"
    );
    assert!(
        error.to_string().contains("toolchain_unavailable"),
        "{error}"
    );
    assert_eq!(host.snapshot(&id).unwrap().phase, Phase::Verifying);
    let (rows, state): (i64, String) = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM verifications WHERE mission_id = ?1),
                (SELECT state FROM linux_execution_effects WHERE mission_id = ?1)",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(rows, 0);
    assert_eq!(state, "not_applied");
    let _ = tmp;
}

#[test]
fn host_verifier_confined_credentials_are_not_in_the_sandbox() {
    let (tmp, repo) = fixture(PASS_TEST);
    let cargo_home = tmp.path().join("cargo-home");
    fs::create_dir_all(cargo_home.join("registry")).unwrap();
    fs::create_dir_all(cargo_home.join("git")).unwrap();
    fs::write(cargo_home.join("credentials.toml"), "SECRET-TOKEN\n").unwrap();
    let result = profile_command_with_cargo(
        &repo,
        Some(&cargo_home),
        &["/bin/cat", "/cargo-home/credentials.toml"],
    );
    assert!(
        result.contains("ENTERED") && result.contains("DONE"),
        "{result}"
    );
    assert!(!result.contains("SECRET-TOKEN"), "{result}");
    let _ = tmp;
}

#[test]
fn host_verifier_confined_etc_lists_only_the_allowlist() {
    let (tmp, repo) = fixture(PASS_TEST);
    let result = profile_command(&repo, &["/bin/ls", "/etc"]);
    assert!(
        result.contains("ENTERED") && result.contains("DONE"),
        "{result}"
    );
    let stdout = result.split("\"stdout\":\"").nth(1).unwrap_or(&result);
    for name in ["passwd", "shadow", "netrc", "gitconfig"] {
        assert!(
            !stdout.contains(name),
            "sandbox /etc exposed {name}: {result}"
        );
    }
    let _ = tmp;
}

#[test]
fn host_verifier_confined_result_symlink_is_incomplete() {
    let (tmp, repo) = fixture(PASS_TEST);
    let db = tmp.path().join("mission.sqlite");
    let (mut host, id, _) = mission(&repo, &db);
    host.enable_linux_execution(linux_config(None)).unwrap();
    host.verifier_result_plant = Some(crate::VerifierResultPlant::Symlink);
    let (snap, outcome) = host.verify_mission(&id).unwrap();
    assert_eq!(outcome, "incomplete", "{snap:?}");
    assert_eq!(snap.phase, Phase::NeedsAttention);
    assert_eq!(snap.repair_count, 0);
    let _ = tmp;
}

#[test]
fn host_verifier_confined_prepare_failure_removes_the_leaf() {
    let root =
        cgroup::validate_delegation(Path::new(&require_env("SWITCHYARD_CGROUP_ROOT"))).unwrap();
    let before = fs::read_dir(&root).unwrap().count();
    let (tmp, repo) = fixture(PASS_TEST);
    let db = tmp.path().join("mission.sqlite");
    let (mut host, id, _) = mission(&repo, &db);
    host.enable_linux_execution(linux_config(None)).unwrap();
    host.fail_prepare_verifier = true;
    let error = host.verify_mission(&id).unwrap_err();
    assert_eq!(error.category(), "guard_failed", "{error}");
    let after = fs::read_dir(&root).unwrap().count();
    assert_eq!(after, before, "cgroup leaf leaked");
    let _ = tmp;
}

#[test]
fn host_verifier_confined_unknown_kind_stays_blocked() {
    let (tmp, repo) = fixture(PASS_TEST);
    let db = tmp.path().join("mission.sqlite");
    let (mut host, id, attempt) = mission(&repo, &db);
    let snap = host.snapshot(&id).unwrap();
    let generation = host.generation();
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, created_at
             ) VALUES ('op-v1:confinedunknownkind00000000000000', ?1, ?2, ?3, 'sha256:row', '{\"kind\":\"other\"}', ?4, 'prepared', 1)",
            rusqlite::params![id, attempt, snap.session_id, generation],
        )
        .unwrap();
    host.enable_linux_execution(linux_config(None)).unwrap();
    let error = host.verify_mission(&id).unwrap_err();
    assert_eq!(error.category(), "execution_blocked", "{error}");
    let recovered = host.recover(&id).unwrap_err();
    assert_eq!(recovered.category(), "execution_blocked", "{recovered}");
    let state: String = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT state FROM linux_execution_effects WHERE operation_id = 'op-v1:confinedunknownkind00000000000000'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "uncertain");
}

#[test]
fn host_verifier_confined_kill_then_blocked_then_recover_then_verify() {
    if std::env::var_os("SW_VERIFIER_KILL").is_some() {
        kill_worker();
        return;
    }
    let body = "#[test]\nfn slow() { std::thread::sleep(std::time::Duration::from_secs(30)); }\n";
    let (tmp, repo) = fixture(body);
    let db = tmp.path().join("mission.sqlite");
    let (host, id, _) = mission(&repo, &db);
    drop(host);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "acceptance::host_verifier_confined::host_verifier_confined_kill_then_blocked_then_recover_then_verify",
            "--nocapture",
        ])
        .env("SW_VERIFIER_KILL", "1")
        .env("SW_VERIFIER_DB", &db)
        .env("SW_VERIFIER_MISSION", &id)
        .env(
            "SWITCHYARD_CGROUP_ROOT",
            require_env("SWITCHYARD_CGROUP_ROOT"),
        )
        .env("SWITCHYARD_BWRAP", require_env("SWITCHYARD_BWRAP"))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.contains("AT-verifier-running") {
                let _ = tx.send(());
                break;
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(60))
        .expect("verifier did not reach the running barrier");
    child.kill().unwrap();
    child.wait().unwrap();
    thread::sleep(Duration::from_millis(200));
    let mut host = Host::open(&db).unwrap();
    host.enable_linux_execution(linux_config(None)).unwrap();
    let blocked = host.verify_mission(&id).unwrap_err();
    assert_eq!(blocked.category(), "execution_blocked", "{blocked}");
    let recovered = host.recover(&id).expect("recover settles the killed row");
    assert_eq!(recovered.phase, Phase::NeedsAttention);
    let state: String = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT state FROM linux_execution_effects ORDER BY created_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "completed", "{state}");
    admit(
        &mut host,
        &id,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Verifying,
            ownership_proven: true,
            effects: crate::EffectProof::Reconciled,
        },
    );
    let (snap, outcome) = host.verify_mission(&id).expect("verify runs after recover");
    assert_eq!(outcome, "pass", "{snap:?}");
}

#[test]
fn host_verifier_confined_kill_then_recover_without_a_prior_verify() {
    if std::env::var_os("SW_VERIFIER_KILL").is_some() {
        kill_worker();
        return;
    }
    let body = "#[test]\nfn slow() { std::thread::sleep(std::time::Duration::from_secs(30)); }\n";
    let (tmp, repo) = fixture(body);
    let db = tmp.path().join("mission.sqlite");
    let (host, id, _) = mission(&repo, &db);
    drop(host);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "acceptance::host_verifier_confined::host_verifier_confined_kill_then_recover_without_a_prior_verify",
            "--nocapture",
        ])
        .env("SW_VERIFIER_KILL", "1")
        .env("SW_VERIFIER_DB", &db)
        .env("SW_VERIFIER_MISSION", &id)
        .env("SWITCHYARD_CGROUP_ROOT", require_env("SWITCHYARD_CGROUP_ROOT"))
        .env("SWITCHYARD_BWRAP", require_env("SWITCHYARD_BWRAP"))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.contains("AT-verifier-running") {
                let _ = tx.send(());
                break;
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(120))
        .expect("verifier did not reach the running barrier");
    child.kill().unwrap();
    child.wait().unwrap();
    thread::sleep(Duration::from_millis(200));
    let mut host = Host::open(&db).unwrap();
    host.enable_linux_execution(linux_config(None)).unwrap();
    let recovered = host
        .recover(&id)
        .expect("recover settles without a prior verify");
    assert_eq!(recovered.phase, Phase::NeedsAttention);
    let state: String = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT state FROM linux_execution_effects ORDER BY created_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "completed");
    admit(
        &mut host,
        &id,
        CommandBody::ExplicitRecovery {
            expected_target: Phase::Verifying,
            ownership_proven: true,
            effects: crate::EffectProof::Reconciled,
        },
    );
    let (snap, outcome) = host.verify_mission(&id).expect("verify runs after recover");
    assert_eq!(outcome, "pass", "{snap:?}");
    let _ = tmp;
}

fn kill_worker() {
    let db = std::env::var("SW_VERIFIER_DB").unwrap();
    let id = std::env::var("SW_VERIFIER_MISSION").unwrap();
    let mut host = Host::open(Path::new(&db)).unwrap();
    host.enable_linux_execution(linux_config(Some("verifier-running")))
        .unwrap();
    host.verifier_deadline_ms = Some(120_000);
    let _ = host.verify_mission(&id);
}

fn profile_command(repo: &Path, argv: &[&str]) -> String {
    profile_command_with_cargo(repo, None, argv)
}

fn profile_command_with_cargo(repo: &Path, cargo_home: Option<&Path>, argv: &[&str]) -> String {
    let root =
        cgroup::validate_delegation(Path::new(&require_env("SWITCHYARD_CGROUP_ROOT"))).unwrap();
    let leaf = cgroup::create_verifier_leaf(
        &root,
        &format!("probe-{}-{}", std::process::id(), crate::domain::now_ms()),
    )
    .unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    fs::write(scratch.path().join("marker"), "marker\n").unwrap();
    fs::write(scratch.path().join("status.json"), b"").unwrap();
    fs::write(scratch.path().join("request.json"), b"").unwrap();
    fs::write(scratch.path().join("result.json"), b"").unwrap();
    let toolchain = Command::new("rustc")
        .arg("--print")
        .arg("sysroot")
        .output()
        .unwrap();
    assert!(toolchain.status.success());
    let toolchain = PathBuf::from(String::from_utf8(toolchain.stdout).unwrap().trim());
    let rustup = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".rustup"));
    let cargo_home = cargo_home.map(Path::to_path_buf).unwrap_or_else(|| {
        std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cargo"))
    });
    let deadline = crate::domain::now_ms() + 20_000;
    let mut child = Command::new(helper_bin())
        .arg("supervise")
        .arg("--cgroup")
        .arg(&leaf.path)
        .arg("--bwrap")
        .arg(require_env("SWITCHYARD_BWRAP"))
        .arg("--workspace")
        .arg(repo)
        .arg("--marker")
        .arg(scratch.path().join("marker"))
        .arg("--status")
        .arg(scratch.path().join("status.json"))
        .arg("--mode")
        .arg("verifier")
        .arg("--deadline-ms")
        .arg(deadline.to_string())
        .arg("--helper")
        .arg(helper_bin())
        .arg("--request")
        .arg(scratch.path().join("request.json"))
        .arg("--result")
        .arg(scratch.path().join("result.json"))
        .arg("--cargo-home")
        .arg(&cargo_home)
        .arg("--rustup-home")
        .arg(&rustup)
        .arg("--toolchain")
        .arg(&toolchain)
        .arg("--target-dir")
        .arg(target.path())
        .arg("--cpu-seconds")
        .arg("20")
        .arg("--")
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let reader = thread::spawn(move || {
        let mut lines = Vec::new();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let done = line == "DONE";
            lines.push(line);
            if done {
                break;
            }
        }
        lines
    });
    thread::sleep(Duration::from_millis(300));
    writeln!(stdin, "GO").unwrap();
    let lines = reader.join().unwrap();
    let _ = child.wait();
    let _ = cgroup::kill_and_prove(&leaf, Duration::from_secs(2));
    let result = fs::read_to_string(scratch.path().join("result.json")).unwrap_or_default();
    format!("{lines:?}\n{result}")
}
