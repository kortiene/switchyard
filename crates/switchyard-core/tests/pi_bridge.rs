#![cfg(feature = "pi-contract")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use switchyard_core::{PiBridge, RuntimeBinding};

fn binding(generation: u64) -> RuntimeBinding {
    RuntimeBinding {
        mission_id: "m-contract".into(),
        attempt_id: "a:contract/é😀".into(),
        session_id: "s/contract:\"".into(),
        generation,
    }
}

fn paths() -> (PathBuf, PathBuf) {
    let node = std::env::var_os("SWITCHYARD_PI_NODE")
        .expect("pi-contract requires SWITCHYARD_PI_NODE; this gate must not skip");
    let launcher = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/pi-source/test/bridge-child.ts");
    (PathBuf::from(node), launcher)
}

fn spawn(database: &Path, generation: u64, fixture: &str) -> PiBridge {
    let (node, launcher) = paths();
    let mut bridge =
        PiBridge::spawn(&node, &launcher, database, binding(generation), fixture).unwrap();
    let hello = bridge.hello().unwrap();
    assert_eq!(hello["storage"], "sqlite-full");
    bridge
        .call("runtime.open", None, json!({}), &mut |_| {
            panic!("open must not execute tools")
        })
        .unwrap();
    bridge
}

fn call(
    bridge: &mut PiBridge,
    method: &str,
    command: Option<&str>,
    params: Value,
) -> switchyard_core::Result<Value> {
    bridge.call(method, command, params, &mut |_| {
        panic!("no tool effects expected")
    })
}

#[test]
fn admission_receipt_survives_process_kill_and_rejects_changed_content() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("session.sqlite");
    let mut bridge = spawn(&db, 1, "answer");
    let first = call(
        &mut bridge,
        "runtime.submit",
        Some("original"),
        json!({"type":"input","content":"hello"}),
    )
    .unwrap();
    bridge.terminate();
    drop(bridge);
    let mut reopened = spawn(&db, 2, "recover");
    let same = call(
        &mut reopened,
        "runtime.submit",
        Some("original"),
        json!({"type":"input","content":"hello"}),
    )
    .unwrap();
    assert_eq!(first, same);
    let snapshot = call(
        &mut reopened,
        "runtime.watch",
        None,
        json!({"commandIds":["original"]}),
    )
    .unwrap();
    assert_eq!(snapshot["resyncRequired"], true);
    assert_eq!(
        snapshot["snapshot"]["commands"][0]["receipt"]["receipt"],
        first["receiptId"]
    );
    let conflict = call(
        &mut reopened,
        "runtime.submit",
        Some("original"),
        json!({"type":"input","content":"changed"}),
    );
    assert!(
        conflict
            .unwrap_err()
            .to_string()
            .contains("command_conflict")
    );
}

#[test]
fn idle_steering_never_becomes_a_new_submission() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("session.sqlite");
    let mut bridge = spawn(&db, 1, "answer");
    let rejected = call(
        &mut bridge,
        "runtime.steer",
        Some("steer"),
        json!({"type":"input","content":"steer"}),
    );
    assert!(rejected.unwrap_err().to_string().contains("idle"));
    drop(bridge);
    let mut bridge = spawn(&db, 2, "recover");
    let snapshot = call(
        &mut bridge,
        "runtime.inspect",
        None,
        json!({"commandIds":["steer"]}),
    )
    .unwrap();
    assert_eq!(snapshot["commands"][0]["receipt"], Value::Null);
    assert_eq!(snapshot["runtimeIdle"], true);
}

#[test]
fn a_second_owner_and_a_stale_reopen_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("session.sqlite");
    let bridge = spawn(&db, 1, "answer");
    let (node, launcher) = paths();
    let competing = PiBridge::spawn(
        &node,
        &launcher,
        &directory.path().join("./session.sqlite"),
        binding(2),
        "answer",
    );
    assert!(competing.is_err());
    drop(bridge);
    let mut stale = PiBridge::spawn(&node, &launcher, &db, binding(1), "answer").unwrap();
    stale.hello().unwrap();
    assert!(
        call(&mut stale, "runtime.open", None, json!({}))
            .unwrap_err()
            .to_string()
            .contains("stale_generation")
    );
}

#[test]
fn corrupt_storage_is_retained_and_never_opened_as_a_fresh_session() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("corrupt.sqlite");
    let corrupt = b"not a sqlite database";
    std::fs::write(&db, corrupt).unwrap();
    let (node, launcher) = paths();
    let mut bridge = PiBridge::spawn(&node, &launcher, &db, binding(1), "answer").unwrap();
    bridge.hello().unwrap();
    assert!(call(&mut bridge, "runtime.open", None, json!({})).is_err());
    assert_eq!(std::fs::read(&db).unwrap(), corrupt);
}

#[test]
fn malformed_peer_output_and_forged_authority_are_rejected_by_rust() {
    // A deliberately hostile peer exercises the host independently of Pi's
    // well-formed responses. It has no model or execution capabilities.
    let mutations = [
        "frame.protocol = 9",
        "frame.binding.generation = 0",
        "frame.id = 'host:999'",
        "frame.error = {category:'interrupted'}",
        "frame.result = {verified:true}",
        "frame.result = {published:true}",
        "frame.result = {quiescent:true}",
        "frame.result = {number:0.5}",
        "frame.result = {huge:'x'.repeat(65536)}",
    ];
    for mutation in mutations {
        let directory = tempfile::tempdir().unwrap();
        let launcher = directory.path().join("hostile.mjs");
        let script = format!(
            "import {{ createInterface }} from 'node:readline';
            createInterface({{input:process.stdin}}).on('line', line => {{
                const request=JSON.parse(line);
                const frame={{protocol:0,kind:'response',id:request.id,binding:request.binding,result:{{}}}};
                {mutation};
                process.stdout.write(JSON.stringify(frame)+'\\n');
            }});"
        );
        std::fs::write(&launcher, script).unwrap();
        let (node, _) = paths();
        let mut bridge = PiBridge::spawn(
            &node,
            &launcher,
            &directory.path().join("session.sqlite"),
            binding(1),
            "answer",
        )
        .unwrap();
        assert!(
            call(&mut bridge, "runtime.inspect", None, json!({})).is_err(),
            "must reject {mutation}"
        );
    }
}

#[test]
fn multi_effect_and_nested_tool_replay_reuse_rust_receipts_after_sigkill() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("session.sqlite");
    // This is the S2 simulated execution boundary, not S3 workspace execution.
    let receipts = Connection::open(directory.path().join("receipts.sqlite")).unwrap();
    receipts.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE receipts(id TEXT PRIMARY KEY, hash TEXT NOT NULL, result TEXT NOT NULL, calls INTEGER NOT NULL);").unwrap();
    let mut bridge = spawn(&db, 1, "tools");
    call(
        &mut bridge,
        "runtime.submit",
        Some("tools"),
        json!({"type":"input","content":"write"}),
    )
    .unwrap();
    let pid = bridge.process_id();
    let mut killed = false;
    let mut first_id = String::new();
    let until = Instant::now() + Duration::from_secs(5);
    while !killed && Instant::now() < until {
        let _ = bridge.call("runtime.inspect", None, json!({}), &mut |effect| {
            let id = effect["operationId"].as_str().unwrap();
            first_id = id.to_string();
            receipts
                .execute(
                    "INSERT INTO receipts VALUES (?1,?2,?3,1)",
                    params![
                        id,
                        effect["payloadHash"].as_str().unwrap(),
                        json!({"receipt":"original"}).to_string()
                    ],
                )
                .unwrap();
            assert!(
                Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status()
                    .unwrap()
                    .success()
            );
            killed = true;
            Ok(json!({"receipt":"original"}))
        });
    }
    assert!(killed, "the simulated effect must commit before killing Pi");
    drop(bridge);
    let mut recovered = spawn(&db, 2, "recover");
    call(
        &mut recovered,
        "runtime.submit",
        Some("tools"),
        json!({"type":"input","content":"write"}),
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let mut idle = false;
    while Instant::now() < until {
        let snapshot = recovered
            .call(
                "runtime.inspect",
                None,
                json!({"commandIds":["tools"]}),
                &mut |effect| {
                    let id = effect["operationId"].as_str().unwrap();
                    let hash = effect["payloadHash"].as_str().unwrap();
                    let existing = receipts.query_row(
                        "SELECT hash,result FROM receipts WHERE id=?1",
                        [id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    );
                    if let Ok((previous_hash, result)) = existing {
                        assert_eq!(hash, previous_hash);
                        receipts
                            .execute("UPDATE receipts SET calls=calls+1 WHERE id=?1", [id])
                            .unwrap();
                        return Ok(serde_json::from_str(&result).unwrap());
                    }
                    let result = json!({"receipt":id});
                    receipts
                        .execute(
                            "INSERT INTO receipts VALUES (?1,?2,?3,1)",
                            params![id, hash, result.to_string()],
                        )
                        .unwrap();
                    Ok(result)
                },
            )
            .unwrap();
        if snapshot["runtimeIdle"] == true {
            assert_eq!(snapshot["commands"][0]["status"], "done");
            idle = true;
            break;
        }
    }
    assert!(idle, "recovered runtime must settle");
    let count: i64 = receipts
        .query_row("SELECT count(*) FROM receipts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 3, "two distinct pair slots and one nested task");
    let repeats: i64 = receipts
        .query_row("SELECT calls FROM receipts WHERE id=?1", [first_id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        repeats, 2,
        "replayed effect returns its original Rust receipt"
    );
}
