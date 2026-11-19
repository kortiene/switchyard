//! One tool keeps a receipt and a second tool is still running when the host
//! is SIGKILLed. Reopen must admit `unsettled_turn`. A receipt from any tool
//! does not hide the one that is stuck, with or without a model endpoint.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use switchyard_core::{Host, ModelEndpoint, Phase};

use super::{World, bridge, linux};

fn open_hold(world: &World, endpoint: Option<&str>) -> Host {
    let mut config = bridge(&world.dir.path().join("bridge"));
    if let Some(endpoint) = endpoint {
        config.model_endpoint = Some(ModelEndpoint::parse(endpoint).unwrap());
    }
    Host::open_confined_test_fixture(&world.db(), config, linux(None), "write-then-hold").unwrap()
}

fn holding(db: &std::path::Path) -> bool {
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut statement = conn
        .prepare(
            "SELECT state, receipt_json IS NOT NULL, request_json FROM linux_execution_effects",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let wrote = rows
        .iter()
        .any(|(_, receipt, body)| body.contains("write_file") && *receipt);
    let held = rows.iter().any(|(state, receipt, body)| {
        body.contains("exec_start") && state == "running" && !*receipt
    });
    wrote && held
}

#[test]
fn write_then_hold_worker() {
    let Ok(root) = std::env::var("SW_HOLD_ROOT") else {
        return;
    };
    let mission = std::env::var("SW_HOLD_MISSION").unwrap();
    let root = std::path::PathBuf::from(root);
    let mut config = bridge(&root.join("bridge"));
    if let Ok(endpoint) = std::env::var("SW_HOLD_ENDPOINT") {
        config.model_endpoint = Some(ModelEndpoint::parse(&endpoint).unwrap());
    }
    let mut host = Host::open_confined_test_fixture(
        &root.join("mission.sqlite"),
        config,
        linux(None),
        "write-then-hold",
    )
    .unwrap();
    host.dispatch_pending(&mission).unwrap();
    loop {
        thread::sleep(Duration::from_secs(30));
    }
}

fn sigkill_while_a_second_tool_holds(endpoint: Option<&str>) {
    let (world, host) = World::configured("write-then-hold", |_, config| {
        if let Some(endpoint) = endpoint {
            config.model_endpoint = Some(ModelEndpoint::parse(endpoint).unwrap());
        }
    });
    drop(host);
    let mut worker = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "held_tool::write_then_hold_worker",
            "--nocapture",
        ])
        .env("SW_HOLD_ROOT", world.dir.path())
        .env("SW_HOLD_MISSION", &world.mission)
        .envs(endpoint.map(|value| ("SW_HOLD_ENDPOINT", value)))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !holding(&world.db()) {
        assert!(
            Instant::now() < deadline,
            "the write did not finish before the held process"
        );
        if let Some(status) = worker.try_wait().unwrap() {
            panic!("worker exited before the second tool held: {status}");
        }
        thread::sleep(Duration::from_millis(20));
    }
    worker.kill().unwrap();
    let killed = worker.wait().unwrap();
    assert_eq!(killed.signal(), Some(9));
    let mut host = open_hold(&world, endpoint);
    host.recover(&world.mission).unwrap();
    let recovered = host.snapshot(&world.mission).unwrap();
    assert_eq!(recovered.phase, Phase::NeedsAttention);
    assert_eq!(
        recovered.to_json()["attentionReason"],
        "unsettled_turn",
        "{recovered:?}"
    );
}

#[test]
fn sigkill_after_a_kept_receipt_and_a_held_tool_is_unsettled() {
    sigkill_while_a_second_tool_holds(None);
}

#[test]
fn sigkill_after_a_kept_receipt_and_a_held_tool_is_unsettled_with_an_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buffer = [0u8; 256];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        }
    });
    sigkill_while_a_second_tool_holds(Some(&format!("http://{address}/v1")));
}
