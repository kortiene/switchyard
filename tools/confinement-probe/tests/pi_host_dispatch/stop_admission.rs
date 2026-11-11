//! Confined Host/Pi/Linux proof that a busy pending Continue cannot block
//! operator Pause or Cancel. The sequence matches the CLI: reconcile the
//! workspace, admit the stop, then `dispatch_stops`. There is no shared
//! stop-first Host method. `model_endpoint` stays unset. FakeRuntime remains
//! the product default; this module does not select the confined adapter
//! outside the `pi-linux-process` suite.
use std::fs::OpenOptions;

use serde_json::Value;
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
