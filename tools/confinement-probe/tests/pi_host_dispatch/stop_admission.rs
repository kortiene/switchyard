//! Confined Host/Pi/Linux proof that a busy pending Continue cannot block
//! operator Pause or Cancel. The sequence matches the CLI: reconcile the
//! workspace, admit the stop, then `dispatch_stops`. There is no shared
//! stop-first Host method. `model_endpoint` stays unset. FakeRuntime remains
//! the product default; this module does not select the confined adapter
//! outside the `pi-linux-process` suite.
//!
//! `busy-once` never starts a workspace process. The process-hold tests below
//! compose that fixture with a live writer and an undelivered Continue. No
//! new bridge-child mode is added.
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use switchyard_core::{AdmitRequest, CommandBody, CommandOrigin, Host, Phase};

use super::*;

fn row<'a>(value: &'a Value, kind: &str) -> &'a Value {
    value["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["kind"] == kind)
        .unwrap_or_else(|| panic!("missing {kind}"))
}

fn command_stored(db: &std::path::Path, kind: &str) -> (String, Option<String>) {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    conn.query_row(
        "SELECT state, runtime_receipt_json FROM commands WHERE kind=?1",
        [kind],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

fn stop_case(cancel: bool) -> (CommandBody, &'static str, Phase, &'static str) {
    if cancel {
        (
            CommandBody::Cancel,
            "cancel",
            Phase::Cancelled,
            "confirm_cancelled",
        )
    } else {
        (
            CommandBody::Pause,
            "pause",
            Phase::Paused,
            "confirm_stopped",
        )
    }
}

fn admitted_phase(cancel: bool) -> Phase {
    if cancel {
        Phase::Cancelling
    } else {
        Phase::Pausing
    }
}

fn pending_busy_continue() -> (World, Host, String) {
    let (world, mut host) = World::new("busy-once");
    assert_eq!(host.dispatch_pending(&world.mission).unwrap(), 1);
    until(&mut host, &world.mission, |value| {
        value["piHost"]["observation"]["runtimeIdle"] == true
    });
    admit(
        &mut host,
        &world.mission,
        CommandBody::Continue {
            activity: "implementing".into(),
        },
    );
    let error = host.dispatch_pending(&world.mission).unwrap_err();
    assert_eq!(error.category(), "guard_failed", "{error}");
    assert!(error.rejected_admission().is_none(), "{error}");
    assert!(error.to_string().contains("bridge: busy"), "{error}");
    let value = host.snapshot(&world.mission).unwrap().to_json();
    assert_eq!(value["phase"], "implementing");
    let continued = row(&value, "continue");
    assert_eq!(continued["state"], "pending");
    assert!(continued["receiptId"].is_null());
    let request_id = continued["requestId"].as_str().unwrap().to_string();
    (world, host, request_id)
}

fn workspace_json(host: &Host, mission: &str) -> Value {
    host.snapshot(mission).unwrap().to_json()["workspace"].clone()
}

/// `busy-once` answers with text and starts no workspace process. Bridge
/// termination with `populated 0` is the descendant proof for that fixture.
fn assert_delivered(
    host: &Host,
    world: &World,
    cancel: bool,
    continue_request: &str,
    linux_receipts: &[(String, Option<String>)],
) {
    let (_, kind, phase, confirm) = stop_case(cancel);
    let snap = host.snapshot(&world.mission).unwrap();
    assert_eq!(snap.phase, phase, "cancel={cancel}");
    let value = snap.to_json();
    let proof = &value["piHost"]["lifecycle"]["termination"];
    assert_eq!(proof["proven"], true, "{proof}");
    let events = proof["events"].as_str().unwrap_or("");
    assert!(
        events.split('\n').any(|line| line == "populated 0"),
        "{proof}"
    );
    assert_eq!(value["executionBlocked"], false);
    assert_eq!(
        command_origin(host, &world.mission, confirm),
        "host_supervisor"
    );
    let confirms: Vec<_> = value["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|command| command["kind"] == confirm)
        .collect();
    assert_eq!(confirms.len(), 1);
    assert_eq!(confirms[0]["origin"], "host_supervisor");
    let stop = row(&value, kind);
    assert_eq!(stop["state"], "acknowledged");
    let (state, receipt) = command_stored(&world.db(), kind);
    assert_eq!(state, "acknowledged");
    let abort: Value = serde_json::from_str(receipt.as_deref().unwrap()).unwrap();
    assert_eq!(abort["adapter"], "pi-confined-v1");
    assert_eq!(abort["credentials"], "none");
    assert_eq!(abort["requestId"], stop["requestId"]);
    assert!(abort["receiptId"].as_str().is_some_and(|id| !id.is_empty()));
    let continued = row(&value, "continue");
    assert_eq!(continued["state"], "superseded");
    assert_eq!(continued["requestId"], continue_request);
    assert!(continued["receiptId"].is_null());
    let (continue_state, continue_receipt) = command_stored(&world.db(), "continue");
    assert_eq!(continue_state, "superseded");
    let marker: Value = serde_json::from_str(continue_receipt.as_deref().unwrap()).unwrap();
    assert_eq!(marker["superseded"], true);
    assert!(marker.get("receiptId").is_none());
    assert!(marker.get("admitted").is_none());
    assert!(value["pendingCommands"].as_array().unwrap().is_empty());
    assert_eq!(row(&value, "approve_plan")["state"], "acknowledged");
    assert!(
        value["linuxExecutionEffects"]
            .as_array()
            .is_some_and(|rows| rows.is_empty())
    );
    assert!(!world.workspace.join("process.heartbeat").exists());
    assert_eq!(world.receipts(), linux_receipts);
    world.origin();
}

fn refuse_forged_confirm_stopped(host: &mut Host, mission: &str) {
    assert_eq!(host.snapshot(mission).unwrap().phase, Phase::Pausing);
    for origin in [CommandOrigin::Operator, CommandOrigin::RuntimeObservation] {
        let revision = host.snapshot(mission).unwrap().revision;
        let error = host
            .admit(AdmitRequest {
                command_id: None,
                mission_id: mission.to_string(),
                attempt_id: None,
                expected_revision: revision,
                actor: "forged".into(),
                origin,
                body: CommandBody::ConfirmStopped,
            })
            .unwrap_err();
        assert_eq!(error.category(), "guard_failed", "{origin:?}: {error}");
        assert!(error.rejected_admission().is_none(), "{origin:?}: {error}");
        assert!(
            error
                .to_string()
                .contains("no mission-fact or execution-quiescence authority"),
            "{origin:?}: {error}"
        );
    }
    let value = host.snapshot(mission).unwrap().to_json();
    assert_eq!(value["phase"], "pausing");
    assert_eq!(row(&value, "pause")["state"], "pending");
    assert_eq!(row(&value, "continue")["state"], "pending");
    assert!(
        value["commands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|command| command["kind"] != "confirm_stopped")
    );
}

fn deliver_stop(host: &mut Host, world: &World, cancel: bool) {
    host.recover_workspace(&world.mission).unwrap();
    let (body, kind, _, _) = stop_case(cancel);
    admit(host, &world.mission, body);
    let admitted = host.snapshot(&world.mission).unwrap();
    assert_eq!(admitted.phase, admitted_phase(cancel));
    let value = admitted.to_json();
    assert_eq!(row(&value, kind)["state"], "pending");
    assert_eq!(row(&value, "continue")["state"], "pending");
    host.dispatch_stops(&world.mission).unwrap();
}

fn inject_defect(world: &World, defect: &str) -> Option<std::fs::File> {
    let conn = rusqlite::Connection::open(world.db()).unwrap();
    match defect {
        "uncertain" => {
            conn.execute(
                "UPDATE workspaces SET state='uncertain', detail='injected ownership uncertainty'",
                [],
            )
            .unwrap();
            None
        }
        "busy" => {
            let lock = world.workspace.parent().unwrap().join("owner.lock");
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(lock)
                .unwrap();
            file.try_lock().unwrap();
            Some(file)
        }
        "unverified" => {
            conn.execute("UPDATE workspaces SET verified_generation=NULL", [])
                .unwrap();
            None
        }
        "transient" => {
            conn.execute_batch(
                "CREATE TRIGGER fail_verify BEFORE UPDATE OF verified_generation ON workspaces \
                 BEGIN SELECT RAISE(ABORT,'injected workspace failure'); END;",
            )
            .unwrap();
            None
        }
        _ => unreachable!(),
    }
}

fn replay_does_not_resend(
    host: &mut Host,
    world: &World,
    kind: &str,
    linux_receipts: &[(String, Option<String>)],
) {
    let original = command_stored(&world.db(), kind);
    let continued = command_stored(&world.db(), "continue");
    assert_eq!(host.dispatch_stops(&world.mission).unwrap(), 0);
    host.recover(&world.mission).unwrap();
    assert_eq!(command_stored(&world.db(), kind), original);
    assert_eq!(command_stored(&world.db(), "continue"), continued);
    assert_eq!(continued.0, "superseded");
    assert_eq!(world.receipts(), linux_receipts);
}

#[test]
fn host_busy_continue_cannot_block_same_host_pause_or_cancel() {
    for cancel in [false, true] {
        let (world, mut host, continue_id) = pending_busy_continue();
        let linux_receipts = world.receipts();
        let approved = command_stored(&world.db(), "approve_plan");
        deliver_stop(&mut host, &world, cancel);
        assert_delivered(&host, &world, cancel, &continue_id, &linux_receipts);
        assert_eq!(command_stored(&world.db(), "approve_plan"), approved);
        let (_, kind, _, _) = stop_case(cancel);
        replay_does_not_resend(&mut host, &world, kind, &linux_receipts);
    }
}

#[test]
fn host_forged_confirm_stopped_is_refused_while_the_stop_is_pending() {
    let (world, mut host, continue_id) = pending_busy_continue();
    let linux_receipts = world.receipts();
    host.recover_workspace(&world.mission).unwrap();
    admit(&mut host, &world.mission, CommandBody::Pause);
    refuse_forged_confirm_stopped(&mut host, &world.mission);
    host.dispatch_stops(&world.mission).unwrap();
    assert_delivered(&host, &world, false, &continue_id, &linux_receipts);
}

#[test]
fn host_busy_continue_stop_bypasses_blocked_workspaces() {
    for defect in ["unverified", "busy", "uncertain", "transient"] {
        for cancel in [false, true] {
            let (world, mut host, continue_id) = pending_busy_continue();
            let linux_receipts = world.receipts();
            let _lease = inject_defect(&world, defect);
            let recovered = host.recover_workspace(&world.mission);
            if defect == "unverified" {
                // A healthy tree is re-verified here, which is what the CLI
                // reconcile does. Withdraw that generation again so the stop
                // itself is what runs against an unverified workspace.
                assert!(recovered.is_ok(), "{defect} cancel={cancel}: {recovered:?}");
                rusqlite::Connection::open(world.db())
                    .unwrap()
                    .execute("UPDATE workspaces SET verified_generation=NULL", [])
                    .unwrap();
            } else {
                let error = recovered.unwrap_err();
                let category = match defect {
                    "busy" => "workspace_busy",
                    "uncertain" => "workspace_uncertain",
                    "transient" => "storage_failed",
                    _ => unreachable!(),
                };
                assert_eq!(error.category(), category, "{defect} cancel={cancel}");
                assert!(
                    error.is_isolated_workspace_failure(),
                    "{defect} cancel={cancel}: {error}"
                );
            }
            let blocked = workspace_json(&host, &world.mission);
            if defect == "unverified" {
                assert!(blocked["verifiedGeneration"].is_null(), "{blocked}");
            }
            let (body, kind, _, _) = stop_case(cancel);
            admit(&mut host, &world.mission, body);
            let admitted = host.snapshot(&world.mission).unwrap();
            assert_eq!(admitted.phase, admitted_phase(cancel), "{defect}");
            let value = admitted.to_json();
            assert_eq!(row(&value, kind)["state"], "pending");
            assert_eq!(row(&value, "continue")["state"], "pending");
            host.dispatch_stops(&world.mission).unwrap();
            assert_eq!(
                workspace_json(&host, &world.mission),
                blocked,
                "{defect} cancel={cancel}"
            );
            assert_delivered(&host, &world, cancel, &continue_id, &linux_receipts);
            // `recover` may re-verify a healthy tree. This replay is only
            // `dispatch_stops`, which must not resend the abort or repair the defect.
            let abort = command_stored(&world.db(), kind);
            let continued = command_stored(&world.db(), "continue");
            assert_eq!(host.dispatch_stops(&world.mission).unwrap(), 0);
            assert_eq!(command_stored(&world.db(), kind), abort);
            assert_eq!(command_stored(&world.db(), "continue"), continued);
            assert_eq!(world.receipts(), linux_receipts);
            assert_eq!(
                workspace_json(&host, &world.mission),
                blocked,
                "{defect} cancel={cancel}"
            );
        }
    }
}

#[test]
fn host_busy_stop_recovers_after_drop_before_delivery() {
    for cancel in [false, true] {
        let (world, mut host, continue_id) = pending_busy_continue();
        let linux_receipts = world.receipts();
        host.recover_workspace(&world.mission).unwrap();
        let (body, kind, _, _) = stop_case(cancel);
        admit(&mut host, &world.mission, body);
        assert_eq!(
            host.snapshot(&world.mission).unwrap().phase,
            admitted_phase(cancel)
        );
        let (state, receipt) = command_stored(&world.db(), kind);
        assert_eq!(state, "pending");
        assert!(receipt.is_none());
        assert_eq!(command_stored(&world.db(), "continue").0, "pending");
        drop(host);
        let mut host = world.reopen();
        let opened = host.snapshot(&world.mission).unwrap();
        assert_eq!(opened.phase, admitted_phase(cancel));
        assert_ne!(
            opened.to_json()["piHost"]["lifecycle"]["termination"]["proven"],
            true
        );
        host.recover(&world.mission).unwrap();
        assert_delivered(&host, &world, cancel, &continue_id, &linux_receipts);
        replay_does_not_resend(&mut host, &world, kind, &linux_receipts);
    }
}

#[test]
fn host_busy_stop_redelivers_abort_after_lost_acknowledgement() {
    for cancel in [false, true] {
        let (world, mut host, continue_id) = pending_busy_continue();
        let linux_receipts = world.receipts();
        host.recover_workspace(&world.mission).unwrap();
        let (body, kind, _, _) = stop_case(cancel);
        admit(&mut host, &world.mission, body);
        let conn = rusqlite::Connection::open(world.db()).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_ack BEFORE UPDATE OF state ON commands \
             WHEN NEW.state='acknowledged' BEGIN SELECT RAISE(ABORT,'lost stop ack'); END;",
        )
        .unwrap();
        let error = host.dispatch_stops(&world.mission).unwrap_err();
        assert_eq!(error.category(), "storage_failed", "{error}");
        assert!(error.to_string().contains("lost stop ack"), "{error}");
        let mid = host.snapshot(&world.mission).unwrap();
        assert_eq!(mid.phase, admitted_phase(cancel));
        let value = mid.to_json();
        assert_ne!(value["piHost"]["lifecycle"]["termination"]["proven"], true);
        assert_eq!(row(&value, kind)["state"], "pending");
        assert!(row(&value, kind)["receiptId"].is_null());
        // `acknowledge` writes runtime_receipt_json and state='acknowledged'
        // in one update. fail_ack rolls both back, so no abort receipt exists
        // to reuse. Recovery records one receipt; the next pass keeps it.
        let (state, receipt) = command_stored(&world.db(), kind);
        assert_eq!(state, "pending");
        assert!(receipt.is_none(), "{receipt:?}");
        assert_eq!(row(&value, "continue")["state"], "pending");
        assert!(value["commands"].as_array().unwrap().iter().all(|command| {
            command["kind"] != "confirm_stopped" && command["kind"] != "confirm_cancelled"
        }));
        drop(host);
        conn.execute_batch("DROP TRIGGER fail_ack").unwrap();
        drop(conn);
        let mut host = world.reopen();
        assert_eq!(
            host.snapshot(&world.mission).unwrap().phase,
            admitted_phase(cancel)
        );
        host.recover(&world.mission).unwrap();
        assert_delivered(&host, &world, cancel, &continue_id, &linux_receipts);
        replay_does_not_resend(&mut host, &world, kind, &linux_receipts);
    }
}

fn cgroup_populated(path: &Path) -> u64 {
    let events = fs::read_to_string(path.join("cgroup.events"))
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    events
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(' ')?;
            (key == "populated")
                .then(|| value.parse::<u64>().ok())
                .flatten()
        })
        .unwrap_or_else(|| {
            panic!(
                "cgroup {} has no populated count:\n{events}",
                path.display()
            )
        })
}

fn heartbeat_advances(path: &Path) -> Vec<u8> {
    let before = fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let deadline = Instant::now() + Duration::from_secs(2);
    while fs::read(path).unwrap() == before {
        assert!(
            Instant::now() < deadline,
            "process heartbeat did not advance at {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    fs::read(path).unwrap()
}

fn heartbeat_stays(path: &Path, frozen: &[u8]) {
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        fs::read(path).unwrap(),
        frozen,
        "workspace heartbeat resumed after the stop"
    );
}

fn writer_cgroup(db: &Path, operation: &str) -> PathBuf {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let path: String = conn
        .query_row(
            "SELECT cgroup_path FROM linux_execution_effects WHERE operation_id=?1",
            [operation],
            |row| row.get(0),
        )
        .unwrap();
    PathBuf::from(path)
}

fn running_start_id(value: &Value) -> String {
    value["linuxExecutionEffects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["method"] == "exec_start" && effect["state"] == "running")
        .unwrap_or_else(|| panic!("no running exec_start: {value}"))["operationId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn linux_effect_ids(value: &Value) -> Vec<String> {
    value["linuxExecutionEffects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|effect| effect["operationId"].as_str().unwrap().to_string())
        .collect()
}

/// `process-hold` has started a real writer. Heartbeat growth and a populated
/// writer cgroup are both required; the ledger row is still `unproven`.
fn wait_for_live_writer(
    host: &mut Host,
    mission: &str,
    workspace: &Path,
    db: &Path,
) -> (String, PathBuf) {
    let value = until(host, mission, |value| {
        value["linuxExecutionEffects"]
            .as_array()
            .is_some_and(|rows| {
                rows.iter()
                    .any(|row| row["method"] == "exec_start" && row["state"] == "running")
            })
            && workspace.join("process.heartbeat").exists()
    });
    let operation = running_start_id(&value);
    let cgroup = writer_cgroup(db, &operation);
    assert!(
        cgroup_populated(&cgroup) > 0,
        "writer descendants were not populated: {}",
        cgroup.display()
    );
    let bridge = PathBuf::from(
        value["piHost"]["lifecycle"]["cgroup"]["path"]
            .as_str()
            .unwrap_or_else(|| panic!("missing bridge cgroup: {value}")),
    );
    assert!(
        cgroup_populated(&bridge) > 0,
        "bridge cgroup was empty: {}",
        bridge.display()
    );
    heartbeat_advances(&workspace.join("process.heartbeat"));
    assert_eq!(
        fs::read_to_string(workspace.join("process.marker")).unwrap(),
        "once\n"
    );
    let writer = value["linuxExecutionEffects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["operationId"] == operation)
        .unwrap();
    assert_eq!(writer["quiescence"], "unproven");
    (operation, cgroup)
}

/// CLI order for a non-stop command. The live writer blocks confined work
/// dispatch before any frame is sent, so this Continue stays pending and is
/// never a protocol `busy` refusal. That is the undelivered row.
fn admit_undelivered_continue(host: &mut Host, mission: &str) -> String {
    host.recover_workspace(mission).unwrap();
    assert_eq!(host.dispatch_reconciled(mission).unwrap(), 0);
    admit(
        host,
        mission,
        CommandBody::Continue {
            activity: "implementing".into(),
        },
    );
    let error = host.dispatch_reconciled(mission).unwrap_err();
    assert_eq!(error.category(), "guard_failed", "{error}");
    assert!(error.rejected_admission().is_none(), "{error}");
    assert!(
        error
            .to_string()
            .contains("unresolved effects block confined dispatch"),
        "{error}"
    );
    let value = host.snapshot(mission).unwrap().to_json();
    let continued = row(&value, "continue");
    assert_eq!(continued["state"], "pending");
    assert!(continued["receiptId"].is_null());
    assert_eq!(value["phase"], "implementing");
    continued["requestId"].as_str().unwrap().to_string()
}

fn assert_one_cancellation(
    before: &[(String, Option<String>)],
    after: &[(String, Option<String>)],
    operation: &str,
) {
    assert_eq!(
        before.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        after.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>()
    );
    let mut gained = Vec::new();
    for (previous, next) in before.iter().zip(after.iter()) {
        assert_eq!(previous.0, next.0);
        if previous.1 != next.1 {
            assert!(
                previous.1.is_none(),
                "receipt replaced: {previous:?} -> {next:?}"
            );
            gained.push(next);
        }
    }
    assert_eq!(
        gained.len(),
        1,
        "linux receipts changed unexpectedly\nbefore={before:?}\nafter={after:?}"
    );
    assert_eq!(gained[0].0, operation);
    let receipt: Value = serde_json::from_str(gained[0].1.as_deref().unwrap()).unwrap();
    assert_eq!(receipt["operationId"], operation);
    assert_eq!(receipt["disposition"], "cancelled");
    assert_eq!(receipt["simulated"], false);
    assert_eq!(receipt["result"]["disposition"], "cancelled");
}

struct LiveStop {
    cancel: bool,
    continue_request: String,
    operation: String,
    cgroup: PathBuf,
    receipts_before: Vec<(String, Option<String>)>,
    effect_ids_before: Vec<String>,
}

fn assert_live_hold_stopped(host: &Host, world: &World, expected: &LiveStop) {
    let cancel = expected.cancel;
    let (_, kind, phase, confirm) = stop_case(cancel);
    let snap = host.snapshot(&world.mission).unwrap();
    assert_eq!(snap.phase, phase, "cancel={cancel}");
    let value = snap.to_json();
    let proof = &value["piHost"]["lifecycle"]["termination"];
    assert_eq!(proof["proven"], true, "{proof}");
    let events = proof["events"].as_str().unwrap_or("");
    assert!(
        events.split('\n').any(|line| line == "populated 0"),
        "{proof}"
    );
    assert_eq!(value["executionBlocked"], false);
    assert_eq!(
        command_origin(host, &world.mission, confirm),
        "host_supervisor"
    );
    let confirms: Vec<_> = value["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|command| command["kind"] == confirm)
        .collect();
    assert_eq!(confirms.len(), 1);
    assert_eq!(confirms[0]["origin"], "host_supervisor");
    let stop = row(&value, kind);
    assert_eq!(stop["state"], "acknowledged");
    let (state, receipt) = command_stored(&world.db(), kind);
    assert_eq!(state, "acknowledged");
    let abort: Value = serde_json::from_str(receipt.as_deref().unwrap()).unwrap();
    assert_eq!(abort["adapter"], "pi-confined-v1");
    assert_eq!(abort["credentials"], "none");
    assert_eq!(abort["requestId"], stop["requestId"]);
    assert!(abort["receiptId"].as_str().is_some_and(|id| !id.is_empty()));
    let continued = row(&value, "continue");
    assert_eq!(continued["state"], "superseded");
    assert_eq!(continued["requestId"], expected.continue_request);
    assert!(continued["receiptId"].is_null());
    assert_eq!(
        value["commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|command| command["kind"] == "continue")
            .count(),
        1,
        "continue was resent"
    );
    let (continue_state, continue_receipt) = command_stored(&world.db(), "continue");
    assert_eq!(continue_state, "superseded");
    let marker: Value = serde_json::from_str(continue_receipt.as_deref().unwrap()).unwrap();
    assert_eq!(marker["superseded"], true);
    assert!(marker.get("receiptId").is_none());
    assert!(marker.get("admitted").is_none());
    assert!(value["pendingCommands"].as_array().unwrap().is_empty());
    assert_eq!(row(&value, "approve_plan")["state"], "acknowledged");
    assert_eq!(linux_effect_ids(&value), expected.effect_ids_before);
    let writer = value["linuxExecutionEffects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["operationId"] == expected.operation)
        .unwrap();
    assert_eq!(writer["state"], "completed");
    assert_eq!(writer["quiescence"], "populated 0");
    assert_eq!(writer["superseded"], false);
    assert_eq!(cgroup_populated(&expected.cgroup), 0);
    assert_eq!(
        fs::read_to_string(world.workspace.join("process.marker")).unwrap(),
        "once\n"
    );
    let frozen = fs::read(world.workspace.join("process.heartbeat")).unwrap();
    heartbeat_stays(&world.workspace.join("process.heartbeat"), &frozen);
    assert_one_cancellation(
        &expected.receipts_before,
        &world.receipts(),
        &expected.operation,
    );
    world.origin();
}

fn signal_process(pid: u32, flag: &str) {
    let status = Command::new("kill")
        .args([flag, &pid.to_string()])
        .status()
        .unwrap_or_else(|error| panic!("spawn kill {flag} {pid}: {error}"));
    assert!(status.success(), "kill {flag} {pid} failed: {status}");
}

struct HostCrash {
    child: Option<std::process::Child>,
    cgroup: Option<PathBuf>,
    supervisor: Option<u32>,
}

impl Drop for HostCrash {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(pid) = self.supervisor {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
        if let Some(path) = &self.cgroup {
            let _ = fs::write(path.join("cgroup.kill"), b"1\n");
        }
    }
}

#[test]
fn host_pending_continue_cannot_block_pause_or_cancel_during_live_process_hold() {
    for cancel in [false, true] {
        let (world, mut host) = World::new("process-hold");
        assert_eq!(host.dispatch_pending(&world.mission).unwrap(), 1);
        let (operation, cgroup) =
            wait_for_live_writer(&mut host, &world.mission, &world.workspace, &world.db());
        let continue_id = admit_undelivered_continue(&mut host, &world.mission);
        assert!(cgroup_populated(&cgroup) > 0);
        heartbeat_advances(&world.workspace.join("process.heartbeat"));
        let receipts_before = world.receipts();
        let ids_before = linux_effect_ids(&host.snapshot(&world.mission).unwrap().to_json());
        let approved = command_stored(&world.db(), "approve_plan");
        host.recover_workspace(&world.mission).unwrap();
        let (body, kind, _, _) = stop_case(cancel);
        admit(&mut host, &world.mission, body);
        let admitted = host.snapshot(&world.mission).unwrap();
        assert_eq!(admitted.phase, admitted_phase(cancel));
        let value = admitted.to_json();
        assert_ne!(value["piHost"]["lifecycle"]["termination"]["proven"], true);
        assert_eq!(row(&value, kind)["state"], "pending");
        assert!(row(&value, kind)["receiptId"].is_null());
        assert_eq!(row(&value, "continue")["state"], "pending");
        assert_eq!(row(&value, "continue")["requestId"], continue_id);
        assert!(
            cgroup_populated(&cgroup) > 0,
            "stop admission killed the writer"
        );
        heartbeat_advances(&world.workspace.join("process.heartbeat"));
        assert_eq!(host.dispatch_stops(&world.mission).unwrap(), 1);
        assert_live_hold_stopped(
            &host,
            &world,
            &LiveStop {
                cancel,
                continue_request: continue_id,
                operation,
                cgroup,
                receipts_before,
                effect_ids_before: ids_before,
            },
        );
        assert_eq!(command_stored(&world.db(), "approve_plan"), approved);
        let frozen = world.receipts();
        assert_eq!(host.dispatch_stops(&world.mission).unwrap(), 0);
        host.recover(&world.mission).unwrap();
        assert_eq!(world.receipts(), frozen);
        assert_eq!(command_stored(&world.db(), "continue").0, "superseded");
        assert_eq!(command_stored(&world.db(), kind).0, "acknowledged");
        assert_eq!(
            fs::read_to_string(world.workspace.join("process.marker")).unwrap(),
            "once\n"
        );
        world.origin();
    }
}

/// In-process `drop` reaps the supervised writer, so it cannot show that
/// recovery stops a process that is still populated. This worker admits the
/// stop and then waits to be killed.
#[test]
fn host_live_process_hold_stop_drop_worker() {
    let Some(root) = std::env::var_os("SW_HOST_ROOT").map(PathBuf::from) else {
        return;
    };
    let mission = std::env::var("SW_HOST_MISSION").unwrap();
    let cancel = std::env::var("SW_HOST_CANCEL").unwrap() == "1";
    let mut host = Host::open_confined_pi(
        &root.join("mission.sqlite"),
        super::bridge(&root.join("bridge")),
        super::linux(None),
        "process-hold",
    )
    .unwrap();
    let workspace = PathBuf::from(host.snapshot(&mission).unwrap().workspace.unwrap().path);
    assert_eq!(host.dispatch_pending(&mission).unwrap(), 1);
    let (operation, cgroup) = wait_for_live_writer(
        &mut host,
        &mission,
        &workspace,
        &root.join("mission.sqlite"),
    );
    let continue_id = admit_undelivered_continue(&mut host, &mission);
    assert!(cgroup_populated(&cgroup) > 0);
    host.recover_workspace(&mission).unwrap();
    let (body, kind, _, _) = stop_case(cancel);
    admit(&mut host, &mission, body);
    let admitted = host.snapshot(&mission).unwrap();
    assert_eq!(admitted.phase, admitted_phase(cancel));
    let value = admitted.to_json();
    assert_ne!(value["piHost"]["lifecycle"]["termination"]["proven"], true);
    assert_eq!(row(&value, kind)["state"], "pending");
    assert!(row(&value, kind)["receiptId"].is_null());
    assert_eq!(row(&value, "continue")["requestId"], continue_id);
    assert_eq!(
        command_stored(&root.join("mission.sqlite"), "continue").0,
        "pending"
    );
    let conn = rusqlite::Connection::open_with_flags(
        root.join("mission.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let supervisor: i64 = conn
        .query_row(
            "SELECT launcher_pid FROM linux_execution_effects WHERE operation_id=?1",
            [&operation],
            |row| row.get(0),
        )
        .unwrap();
    drop(conn);
    assert!(supervisor > 0, "writer supervisor pid missing");
    fs::write(
        root.join("live-stop.json"),
        serde_json::to_vec(&json!({
            "operation": operation,
            "cgroup": cgroup,
            "continue": continue_id,
            "supervisor": supervisor,
            "populated": cgroup_populated(&cgroup),
        }))
        .unwrap(),
    )
    .unwrap();
    println!("AT-live-stop");
    let _ = std::io::stdout().flush();
    loop {
        std::thread::sleep(Duration::from_secs(30));
    }
}

#[test]
fn host_live_process_hold_stop_stops_descendants_once_after_host_kill() {
    for cancel in [false, true] {
        let (world, host) = World::new("process-hold");
        drop(host);
        let mut worker = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "stop_admission::host_live_process_hold_stop_drop_worker",
                "--nocapture",
            ])
            .env("SW_HOST_ROOT", world.dir.path())
            .env("SW_HOST_MISSION", &world.mission)
            .env("SW_HOST_CANCEL", if cancel { "1" } else { "0" })
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = worker.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut crash = HostCrash {
            child: Some(worker),
            cgroup: None,
            supervisor: None,
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(line) if line.contains("AT-live-stop") => break,
                Ok(_) => {}
                Err(error) => {
                    panic!("missing live process-hold stop barrier: {error}");
                }
            }
        }
        let info: Value =
            serde_json::from_slice(&fs::read(world.dir.path().join("live-stop.json")).unwrap())
                .unwrap();
        let operation = info["operation"].as_str().unwrap().to_string();
        let continue_id = info["continue"].as_str().unwrap().to_string();
        let cgroup = PathBuf::from(info["cgroup"].as_str().unwrap());
        let supervisor = u32::try_from(info["supervisor"].as_u64().unwrap()).unwrap();
        crash.cgroup = Some(cgroup.clone());
        crash.supervisor = Some(supervisor);
        assert!(info["populated"].as_u64().unwrap() > 0);
        assert!(cgroup_populated(&cgroup) > 0);
        heartbeat_advances(&world.workspace.join("process.heartbeat"));
        // Freeze the helper so host death does not reap the sandbox. The
        // detached heartbeat is not that helper, so it keeps running.
        signal_process(supervisor, "-STOP");
        assert!(cgroup_populated(&cgroup) > 0);
        heartbeat_advances(&world.workspace.join("process.heartbeat"));
        let child = crash.child.as_mut().unwrap();
        child.kill().unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(9));
        crash.child = None;
        assert!(
            cgroup_populated(&cgroup) > 0,
            "host death stopped the writer before recovery"
        );
        heartbeat_advances(&world.workspace.join("process.heartbeat"));
        let receipts_before = world.receipts();
        assert!(receipts_before.iter().all(|(_, receipt)| receipt.is_none()));
        let mut host = world.reopen();
        let opened = host.snapshot(&world.mission).unwrap();
        assert_eq!(opened.phase, admitted_phase(cancel));
        let opened_value = opened.to_json();
        assert_ne!(
            opened_value["piHost"]["lifecycle"]["termination"]["proven"],
            true
        );
        assert_eq!(
            row(&opened_value, if cancel { "cancel" } else { "pause" })["state"],
            "pending"
        );
        assert_eq!(row(&opened_value, "continue")["state"], "pending");
        assert_eq!(row(&opened_value, "continue")["requestId"], continue_id);
        let (stop_state, stop_receipt) =
            command_stored(&world.db(), if cancel { "cancel" } else { "pause" });
        assert_eq!(stop_state, "pending");
        assert!(stop_receipt.is_none(), "{stop_receipt:?}");
        let ids_before = linux_effect_ids(&opened_value);
        // No abort receipt exists yet. Recovery records one; it does not
        // reuse or re-deliver a stored receipt. The writer is still populated.
        assert!(cgroup_populated(&cgroup) > 0);
        host.recover(&world.mission).unwrap();
        assert_live_hold_stopped(
            &host,
            &world,
            &LiveStop {
                cancel,
                continue_request: continue_id,
                operation,
                cgroup: cgroup.clone(),
                receipts_before,
                effect_ids_before: ids_before,
            },
        );
        let (_, kind, _, _) = stop_case(cancel);
        let frozen = world.receipts();
        let abort = command_stored(&world.db(), kind);
        assert_eq!(host.dispatch_stops(&world.mission).unwrap(), 0);
        host.recover(&world.mission).unwrap();
        assert_eq!(command_stored(&world.db(), kind), abort);
        assert_eq!(command_stored(&world.db(), "continue").0, "superseded");
        assert_eq!(world.receipts(), frozen);
        assert_eq!(
            fs::read_to_string(world.workspace.join("process.marker")).unwrap(),
            "once\n"
        );
        assert_eq!(cgroup_populated(&cgroup), 0);
        world.origin();
        // Recovery already emptied the cgroup. Drop must not be what records
        // a second cancellation.
        crash.cgroup = None;
        crash.supervisor = None;
    }
}
