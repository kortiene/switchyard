#![cfg(feature = "pi-contract")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use switchyard_core::{PiBridge, PiEffectOutcome, RuntimeBinding, operation_id, payload_hash};

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
fn uncooperative_abort_times_out_reaps_child_and_reopens_with_unsettled_intent() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("session.sqlite");
    let mut bridge = spawn(&db, 1, "ignore-abort");
    call(
        &mut bridge,
        "runtime.submit",
        Some("work"),
        json!({"type":"input","content":"wait"}),
    )
    .unwrap();
    let limit = std::time::Instant::now() + Duration::from_secs(3);
    while !bridge.stderr_tail().contains("waiting tool entered") {
        assert!(
            std::time::Instant::now() < limit,
            "fixture did not enter its tool"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let started = std::time::Instant::now();
    let error = call(&mut bridge, "runtime.abort", Some("stop"), json!({})).unwrap_err();
    assert!(
        error.to_string().contains("cancellation_timeout"),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(
        bridge.exit_status().unwrap().is_some(),
        "Rust must reap the failed peer"
    );
    assert!(db.is_file());
    drop(bridge);
    // Keep the uncooperative fixture armed: replaying its tool would hang again.
    let mut bridge = spawn(&db, 2, "ignore-abort");
    let before = call(
        &mut bridge,
        "runtime.inspect",
        None,
        json!({"commandIds":["stop"]}),
    )
    .unwrap();
    assert_eq!(before["scheduling"], "paused");
    assert_eq!(before["stopping"], true);
    assert_eq!(before["commands"][0]["receipt"]["settled"], false);
    assert_eq!(before["commands"][0]["receipt"]["receipt"], "stop");
    let stopped = call(&mut bridge, "runtime.abort", Some("stop"), json!({})).unwrap();
    assert_eq!(
        stopped,
        json!({"commandId":"stop","accepted":true,"runtimeSettled":true})
    );
    assert!(
        !bridge.stderr_tail().contains("waiting tool entered"),
        "recovery must not rerun the tool"
    );
    let limit = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let snapshot = call(&mut bridge, "runtime.inspect", None, json!({})).unwrap();
        if snapshot["runtimeIdle"] == true && snapshot["stopping"] == false {
            break;
        }
        assert!(
            std::time::Instant::now() < limit,
            "recovered abort did not settle: {snapshot}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !bridge.stderr_tail().contains("waiting tool entered"),
        "recovery must not rerun the tool after the abort settles"
    );
}

fn synthetic_peer(directory: &Path, script: &str) -> PiBridge {
    let launcher = directory.join("synthetic.mjs");
    std::fs::write(&launcher, script).unwrap();
    let (node, _) = paths();
    PiBridge::spawn(
        &node,
        &launcher,
        &directory.join("synthetic.sqlite"),
        binding(1),
        "answer",
    )
    .unwrap()
}

fn effect_peer(directory: &Path, count: usize) -> PiBridge {
    let owner = binding(1);
    let payload = json!({"method":"simulated-receipt"});
    let effects: Vec<Value> = (1..=count).map(|n| {
        let key = format!("slot-{n}");
        json!({"protocol":0,"kind":"request","id":format!("bridge:{n}"),"method":"execution.request",
            "binding":{"missionId":owner.mission_id,"attemptId":owner.attempt_id,"sessionId":owner.session_id,"generation":1},
            "params":{"taskId":"task","effectKey":key,"payload":payload,"payloadHash":payload_hash(&payload).unwrap(),
                "operationId":operation_id(&owner.attempt_id,&owner.session_id,"task",&key).unwrap()}})
    }).collect();
    synthetic_peer(
        directory,
        &format!(
            r#"
        import {{createInterface}} from 'node:readline';
        const effects={effects}; let host; let next=0; const receipts=[];
        const send=frame=>process.stdout.write(JSON.stringify(frame)+'\n');
        createInterface({{input:process.stdin}}).on('line',line=>{{
            const frame=JSON.parse(line);
            if(frame.kind==='request') host=frame;
            else receipts.push(frame.error ? {{error:frame.error}} : frame.result);
            if(next<effects.length) send(effects[next++]);
            else setTimeout(()=>send({{protocol:0,kind:'response',id:host.id,binding:host.binding,result:{{receipts}}}}),125);
        }});
    "#,
            effects = serde_json::to_string(&effects).unwrap()
        ),
    )
}

#[test]
fn successful_effects_get_a_fresh_runtime_response_window() {
    let directory = tempfile::tempdir().unwrap();
    let mut peer = effect_peer(directory.path(), 2);
    let mut calls = 0;
    let result = peer
        .call("runtime.inspect", None, json!({}), &mut |_| {
            // Each receipt takes less than Pi's 5 s effect deadline, but their
            // combined duration exceeds the Rust response window of 8 s.
            std::thread::sleep(Duration::from_millis(4100));
            calls += 1;
            Ok(json!({"receipt":calls}))
        })
        .unwrap();
    assert_eq!(calls, 2);
    assert_eq!(result["receipts"], json!([{"receipt":1},{"receipt":2}]));
}

#[test]
fn effect_flood_cannot_extend_a_call_indefinitely() {
    let pins: Value =
        serde_json::from_str(include_str!("../../../runtime/pi-source/pins.json")).unwrap();
    let limit = pins["bridge"]["maxEffectsPerCall"].as_u64().unwrap() as usize;
    let directory = tempfile::tempdir().unwrap();
    let mut peer = effect_peer(directory.path(), limit + 1);
    let mut calls = 0;
    let error = peer
        .call("runtime.inspect", None, json!({}), &mut |_| {
            calls += 1;
            Ok(json!({"receipt":calls}))
        })
        .unwrap_err();
    assert_eq!(calls, limit);
    assert!(error.to_string().contains("effect limit exceeded"));
}

#[test]
fn typed_effect_responses_preserve_all_four_dispositions() {
    let directory = tempfile::tempdir().unwrap();
    let mut peer = effect_peer(directory.path(), 4);
    let mut outcomes = [
        PiEffectOutcome::Completed(json!({"receipt":"original"})),
        PiEffectOutcome::Rejected,
        PiEffectOutcome::Conflict,
        PiEffectOutcome::Uncertain,
    ]
    .into_iter();
    let result = peer
        .call_with_effects("runtime.inspect", None, json!({}), &mut |_| {
            outcomes.next().unwrap()
        })
        .unwrap();
    assert_eq!(
        result["receipts"],
        json!([
            {"receipt":"original"},
            {"error":{"category":"execution_rejected","admission":"rejected"}},
            {"error":{"category":"operation_conflict","admission":"rejected"}},
            {"error":{"category":"execution_uncertain","admission":"uncertain"}}
        ])
    );
}

#[test]
fn legacy_callback_errors_never_establish_non_admission() {
    for error in [
        switchyard_core::Error::guard("after effect"),
        switchyard_core::Error::OperationConflict {
            operation_id: "same".into(),
        },
        switchyard_core::Error::storage("after effect"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut peer = effect_peer(directory.path(), 1);
        let mut error = Some(error);
        let result = peer
            .call("runtime.inspect", None, json!({}), &mut |_| {
                Err(error.take().unwrap())
            })
            .unwrap();
        assert_eq!(
            result["receipts"],
            json!([{"error":{"category":"execution_uncertain","admission":"uncertain"}}])
        );
    }
}

#[test]
fn failed_peer_keeps_a_bounded_diagnostic_tail_without_terminal_controls() {
    let directory = tempfile::tempdir().unwrap();
    let mut peer = synthetic_peer(
        directory.path(),
        r#"
        import {writeSync} from 'node:fs';
        process.stdin.once('data',()=>{
            writeSync(2,'discarded-prefix'+'x'.repeat(12000)+'\x1b[31mstorage-open-failed\n');
            process.exit(1);
        });
    "#,
    );
    let error = call(&mut peer, "runtime.open", None, json!({})).unwrap_err();
    let tail = peer.stderr_tail();
    assert!(tail.len() <= 4096);
    assert!(!tail.contains("discarded-prefix"));
    assert!(!tail.contains('\x1b'));
    assert!(error.to_string().contains("storage-open-failed"));
}

#[test]
fn handshake_rejects_pin_drift_and_retains_diagnostics() {
    for mutation in [
        "result.node='wrong'",
        "result.source='wrong'",
        "result.deadlineMs=1",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let pins = include_str!("../../../runtime/pi-source/pins.json");
        let mut peer = synthetic_peer(
            directory.path(),
            &format!(
                r#"
            import {{createInterface}} from 'node:readline'; import {{writeSync}} from 'node:fs';
            const pins={pins};
            createInterface({{input:process.stdin}}).on('line',line=>{{
                const request=JSON.parse(line);
                const result={{...request.params,protocol:pins.bridge.protocol,node:pins.node,
                    storage:'sqlite-full',deadlineMs:pins.bridge.deadlineMs}};
                {mutation}; writeSync(2,'handshake-diagnostic\n');
                process.stdout.write(JSON.stringify({{protocol:0,kind:'response',id:request.id,binding:request.binding,result}})+'\n');
            }});
        "#
            ),
        );
        let message = peer.hello().unwrap_err().to_string();
        assert!(
            message.contains("incompatible bridge handshake"),
            "{mutation}: {message}"
        );
        assert!(message.contains("handshake-diagnostic"));
    }
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
    let error = call(&mut bridge, "runtime.open", None, json!({})).unwrap_err();
    let message = error.to_string();
    let tail = bridge.stderr_tail();
    assert!(
        !tail.trim().is_empty(),
        "corrupt open exited with empty stderr: {message}"
    );
    assert!(
        tail.lines()
            .any(|line| line.contains("pi-bridge:") && line.contains("file is not a database")),
        "stderr missing bounded storage diagnostic: {tail}"
    );
    assert!(
        message.contains("file is not a database"),
        "bridge failure did not retain stderr diagnostic: {message}"
    );
    let status = bridge.exit_status().expect("peer status");
    assert!(
        status.is_some_and(|status| !status.success()),
        "corrupt open should exit non-zero, got {status:?}"
    );
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
        "frame.result = {snapshot:{verified:true}}",
        "frame.result = {snapshots:[{state:{approved:true}}]}",
        "frame.result = [{published:true}]",
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
fn shared_contract_vectors_match_typescript_canonical_and_operation_hashes() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../runtime/pi-source/contract-vectors.json"
    ))
    .unwrap();
    for vector in vectors["values"].as_array().unwrap() {
        assert_eq!(
            switchyard_core::canonicalize(&vector["value"]).unwrap(),
            vector["canonical"]
        );
        assert_eq!(
            switchyard_core::payload_hash(&vector["value"]).unwrap(),
            vector["hash"]
        );
    }
    for vector in vectors["operations"].as_array().unwrap() {
        let binding = &vector["binding"];
        assert_eq!(
            switchyard_core::operation_id(
                binding["attemptId"].as_str().unwrap(),
                binding["sessionId"].as_str().unwrap(),
                vector["task"].as_str().unwrap(),
                vector["key"].as_str().unwrap(),
            )
            .unwrap(),
            vector["id"]
        );
    }
}
