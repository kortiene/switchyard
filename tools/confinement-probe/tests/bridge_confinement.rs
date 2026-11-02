use std::fs;
use std::net::{TcpListener, TcpStream};
use std::os::unix::{
    fs::symlink,
    net::{UnixListener, UnixStream},
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use switchyard_confinement_probe::{
    bridge::{BridgeSandbox, prepare},
    probe,
};
use switchyard_core::{PiBridge, RuntimeBinding};

fn policy() -> PathBuf {
    std::env::var_os("SWITCHYARD_BRIDGE_POLICY")
        .map(PathBuf::from)
        .expect("bridge-confinement requires a compiled policy; no skip/fallback")
}
fn spawn(sandbox: &BridgeSandbox, generation: u64, mode: &str) -> PiBridge {
    PiBridge::spawn(
        &sandbox.launcher,
        Path::new("bridge-fixture"),
        &sandbox.database,
        RuntimeBinding {
            mission_id: "m-confined".into(),
            attempt_id: "a-confined".into(),
            session_id: "s-confined".into(),
            generation,
        },
        mode,
    )
    .unwrap()
}
fn call(bridge: &mut PiBridge, method: &str, command: Option<&str>, params: Value) -> Value {
    bridge
        .call(method, command, params, &mut |_| {
            panic!("fixture has no forwarded effects")
        })
        .unwrap_or_else(|e| panic!("{method}: {e}; {}", bridge.stderr_tail()))
}
fn open(bridge: &mut PiBridge) {
    let hello = bridge
        .hello()
        .unwrap_or_else(|e| panic!("hello: {e}; {}", bridge.stderr_tail()));
    assert_eq!(hello["storage"], "sqlite-full");
    call(bridge, "runtime.open", None, json!({}));
}
fn observe(sandbox: &BridgeSandbox) -> (PathBuf, Vec<probe::ProcessIdentity>) {
    let namespace = probe::namespace_from_status(&sandbox.status_root).unwrap();
    let members = probe::namespace_members(&namespace).unwrap();
    assert!(
        !members.is_empty(),
        "must observe the live namespace before killing"
    );
    (namespace, members)
}
fn retain(name: &str, value: &Value) {
    let directory = std::env::var_os("SWITCHYARD_CONFINEMENT_EVIDENCE")
        .expect("evidence directory is mandatory");
    fs::write(
        Path::new(&directory).join(name),
        serde_json::to_vec_pretty(value).unwrap(),
    )
    .unwrap();
}

#[test]
fn bridge_boundary_denials_preserve_host_authority_and_private_sqlite_recovers() {
    assert_eq!(
        std::env::var("SWITCHYARD_CREDENTIAL_SENTINEL").unwrap(),
        "host-only-fixture"
    );
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let sandbox = prepare(root, &policy(), true).unwrap();
    let protected: Vec<_> = [
        "mission.sqlite",
        "workspace/source",
        "original/.git/config",
        "home/.ssh/key",
        "home/.config/gh/hosts.yml",
        "bridge-config",
    ]
    .into_iter()
    .map(|name| root.join("protected").join(name))
    .collect();
    for path in &protected {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "host authority sentinel").unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "host authority sentinel");
    }
    symlink(&protected[0], root.join("state/escape")).unwrap();
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let socket = root.join("host.sock");
    let unix = UnixListener::bind(&socket).unwrap();
    // Both endpoints really exist and accept connections outside the sandbox.
    let _tcp_positive = TcpStream::connect(tcp.local_addr().unwrap()).unwrap();
    let _unix_positive = UnixStream::connect(&socket).unwrap();
    assert!(!unix.local_addr().unwrap().is_unnamed());
    fs::write(
        root.join("case.json"),
        json!({"protected":protected,
        "port":tcp.local_addr().unwrap().port(), "socket":socket})
        .to_string(),
    )
    .unwrap();
    let mut bridge = spawn(&sandbox, 1, "answer");
    open(&mut bridge);
    let report: Vec<String> =
        serde_json::from_slice(&fs::read(root.join("state/probe.json")).unwrap()).unwrap();
    assert_eq!(
        report.len(),
        26,
        "all actual negative operations must run: {report:?}"
    );
    assert_eq!(
        fs::read_to_string(root.join("state/positive")).unwrap(),
        "private state works"
    );
    assert!(
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join("state/session.sqlite.owner.lock"))
            .unwrap()
            .try_lock()
            .is_err(),
        "negative owner-lock attempts must preserve exclusivity"
    );
    let receipt = call(
        &mut bridge,
        "runtime.submit",
        Some("work"),
        json!({"type":"input","content":"hello"}),
    );
    let limit = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = call(
            &mut bridge,
            "runtime.inspect",
            None,
            json!({"commandIds":["work"]}),
        );
        if snapshot["runtimeIdle"] == true {
            break;
        }
        assert!(Instant::now() < limit, "answer did not complete");
        std::thread::sleep(Duration::from_millis(10));
    }
    let (namespace, members) = observe(&sandbox);
    bridge.terminate();
    assert!(bridge.exit_status().unwrap().is_some());
    drop(bridge);
    probe::require_quiescent(&namespace, &members).unwrap();
    let mut bridge = spawn(&sandbox, 2, "recover");
    open(&mut bridge);
    let snapshot = call(
        &mut bridge,
        "runtime.inspect",
        None,
        json!({"commandIds":["work"]}),
    );
    assert_eq!(snapshot["scheduling"], "paused");
    let replay = call(
        &mut bridge,
        "runtime.submit",
        Some("work"),
        json!({"type":"input","content":"hello"}),
    );
    assert_eq!(replay, receipt, "reopen must retain original Pi admission");
    for path in &protected {
        assert_eq!(fs::read_to_string(path).unwrap(), "host authority sentinel");
    }
    retain(
        "bridge-boundaries.json",
        &json!({"denials":report,"receipt":receipt,"reopened":snapshot,
        "namespace":namespace,"observedPids":members.iter().map(|p|p.pid).collect::<Vec<_>>() }),
    );
}

#[test]
fn confined_uncooperative_abort_reaps_namespace_before_inert_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let sandbox = prepare(directory.path(), &policy(), false).unwrap();
    let mut bridge = spawn(&sandbox, 1, "ignore-abort");
    open(&mut bridge);
    call(
        &mut bridge,
        "runtime.submit",
        Some("work"),
        json!({"type":"input","content":"wait"}),
    );
    let limit = Instant::now() + Duration::from_secs(5);
    while !bridge.stderr_tail().contains("waiting tool entered") {
        assert!(
            Instant::now() < limit,
            "tool did not start: {}",
            bridge.stderr_tail()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let (namespace, members) = observe(&sandbox);
    let started = Instant::now();
    let error = bridge
        .call("runtime.abort", Some("stop"), json!({}), &mut |_| {
            panic!("no effects")
        })
        .unwrap_err();
    assert!(
        error.to_string().contains("cancellation_timeout"),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(bridge.exit_status().unwrap().is_some());
    drop(bridge);
    probe::require_quiescent(&namespace, &members).unwrap();
    // Same armed tool on reopen: unintended scheduling would hang again.
    let mut bridge = spawn(&sandbox, 2, "ignore-abort");
    open(&mut bridge);
    let before = call(
        &mut bridge,
        "runtime.inspect",
        None,
        json!({"commandIds":["stop"]}),
    );
    assert_eq!(before["scheduling"], "paused");
    assert_eq!(before["commands"][0]["receipt"]["settled"], false);
    let stopped = call(&mut bridge, "runtime.abort", Some("stop"), json!({}));
    assert_eq!(stopped["runtimeSettled"], true);
    assert!(!bridge.stderr_tail().contains("waiting tool entered"));
    retain(
        "bridge-stop.json",
        &json!({"before":before,"stopped":stopped,"namespace":namespace,
        "observedPids":members.iter().map(|p|p.pid).collect::<Vec<_>>() }),
    );
}

#[test]
fn invalid_or_missing_policy_never_starts_an_unconfined_bridge() {
    let missing = tempfile::tempdir().unwrap();
    assert!(prepare(missing.path(), &missing.path().join("missing.so"), false).is_err());
    for (name, bytes) in [
        ("invalid ELF", b"not an ELF library".as_slice()),
        ("empty ELF", b"".as_slice()),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let policy = directory.path().join("broken.so");
        fs::write(&policy, bytes).unwrap();
        let sandbox = prepare(directory.path(), &policy, false).unwrap();
        let mut bridge = spawn(&sandbox, 1, "answer");
        assert!(bridge.hello().is_err(), "{name}");
        assert!(
            bridge
                .stderr_tail()
                .contains("bridge lockdown guard rejected startup"),
            "{}",
            bridge.stderr_tail()
        );
        assert!(bridge.exit_status().unwrap().is_some());
        assert!(
            !sandbox.database.exists(),
            "Pi must not open SQLite without lockdown"
        );
    }
}
