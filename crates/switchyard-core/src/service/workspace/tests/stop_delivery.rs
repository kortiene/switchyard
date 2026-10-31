use super::*;

fn admit(h: &mut Host, m: &MissionSnapshot, body: CommandBody) -> AdmitResult {
    h.admit(AdmitRequest {
        command_id: None,
        mission_id: m.mission_id.clone(),
        attempt_id: Some(m.attempt_id.clone()),
        expected_revision: h.snapshot(&m.mission_id).unwrap().revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body,
    })
    .unwrap()
}

#[test]
fn blocked_workspace_delivers_stops_and_replays_the_original_abort() {
    for defect in ["uncertain", "busy", "transient", "generation", "execution"] {
        for cancel in [false, true] {
            for lost_ack in [false, true] {
                let w = world();
                let mut h = Host::open(&w.db).unwrap();
                let m = mission(&mut h, &w);
                let view = if defect == "transient" {
                    assert!(
                        h.prepare_workspace_at(&m.mission_id, &m.attempt_id, &w.root, |at| {
                            if at == "reserved" {
                                Err(Error::storage("crash before creation"))
                            } else {
                                Ok(())
                            }
                        })
                        .is_err()
                    );
                    h.snapshot(&m.mission_id).unwrap().workspace.unwrap()
                } else {
                    prepare(&mut h, &m, &w)
                };
                let start = admit(&mut h, &m, CommandBody::StartPlanning);
                let stop = admit(
                    &mut h,
                    &m,
                    if cancel {
                        CommandBody::Cancel
                    } else {
                        CommandBody::Pause
                    },
                );
                let lock_path = Path::new(&view.path).parent().unwrap().join("owner.lock");
                let mut lease = None;
                match defect {
                    "uncertain" => h.store.uncertain_workspace(&view.id, "fixture").unwrap(),
                    "busy" => {
                        let file = OpenOptions::new()
                            .read(true)
                            .write(true)
                            .open(lock_path)
                            .unwrap();
                        file.try_lock().unwrap();
                        lease = Some(file);
                    }
                    "transient" => {
                        // A read-only root fails before slot creation, without root-user assumptions.
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            fs::set_permissions(&w.root, fs::Permissions::from_mode(0o555))
                                .unwrap();
                        }
                    }
                    "generation" => {
                        Connection::open(&w.db)
                            .unwrap()
                            .execute("UPDATE workspaces SET verified_generation=0", [])
                            .unwrap();
                    }
                    "execution" => {
                        h.record_operation(
                            &m.mission_id,
                            "unresolved",
                            &json!({"effect":"unknown"}),
                            OperationState::Uncertain,
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
                let before = h.snapshot(&m.mission_id).unwrap().to_json();
                let git_before = git(&w.repo, &["status", "--porcelain=v1"]);
                let head_before = git(&w.repo, &["rev-parse", "HEAD"]);
                let original = if lost_ack {
                    let binding = h.binding(&h.store.load_mission(&m.mission_id).unwrap());
                    Some(
                        h.runtime_mut()
                            .unwrap()
                            .abort(&RuntimeRequest {
                                binding,
                                request_id: stop.request_id.clone(),
                                payload_hash: stop.payload_hash.clone(),
                                method: "abort",
                            })
                            .unwrap(),
                    )
                } else {
                    None
                };
                if defect == "generation" {
                    assert!(h.dispatch_reconciled(&m.mission_id).is_err());
                } else {
                    let result = if lost_ack {
                        h.recover(&m.mission_id).map(|_| ())
                    } else {
                        h.dispatch_pending(&m.mission_id).map(|_| ())
                    };
                    match defect {
                        "uncertain" => {
                            assert_eq!(result.unwrap_err().category(), "workspace_uncertain")
                        }
                        "busy" => assert_eq!(result.unwrap_err().category(), "workspace_busy"),
                        "transient" => assert_eq!(result.unwrap_err().category(), "storage_failed"),
                        "execution" => result.unwrap(),
                        _ => unreachable!(),
                    }
                }
                let after = h.snapshot(&m.mission_id).unwrap();
                let delivered = after
                    .commands
                    .iter()
                    .find(|c| c.id == stop.command_id)
                    .unwrap();
                assert_eq!(
                    delivered.state, "acknowledged",
                    "{defect}, cancel={cancel}, lost_ack={lost_ack}"
                );
                let receipt = h
                    .runtime_mut()
                    .unwrap()
                    .stored_receipt(&stop.request_id)
                    .unwrap();
                if let Some(original) = original {
                    assert_eq!(receipt, original);
                }
                assert!(
                    h.runtime_mut()
                        .unwrap()
                        .stored_receipt(&start.request_id)
                        .is_none()
                );
                assert_eq!(h.runtime_mut().unwrap().submission_count(), 1);
                assert_eq!(
                    after.workspace.as_ref().unwrap().state,
                    before["workspace"]["state"]
                );
                assert_eq!(git(&w.repo, &["status", "--porcelain=v1"]), git_before);
                assert_eq!(git(&w.repo, &["rev-parse", "HEAD"]), head_before);
                drop(h);
                let mut h = Host::open(&w.db).unwrap();
                let _ = h.recover(&m.mission_id);
                assert_eq!(
                    h.runtime_mut().unwrap().stored_receipt(&stop.request_id),
                    Some(receipt)
                );
                assert_eq!(h.runtime_mut().unwrap().submission_count(), 1);
                drop(lease);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&w.root, fs::Permissions::from_mode(0o755)).unwrap();
                }
            }
        }
    }
}

#[test]
fn stale_owner_cannot_deliver_a_pending_stop() {
    for cancel in [false, true] {
        let w = world();
        let mut h = Host::open(&w.db).unwrap();
        let m = mission(&mut h, &w);
        prepare(&mut h, &m, &w);
        let stop = admit(
            &mut h,
            &m,
            if cancel {
                CommandBody::Cancel
            } else {
                CommandBody::Pause
            },
        );
        Connection::open(&w.db)
            .unwrap()
            .execute("UPDATE host_owner SET generation=generation+1", [])
            .unwrap();
        assert_eq!(
            h.recover(&m.mission_id).unwrap_err().category(),
            "stale_generation"
        );
        assert!(
            h.runtime_mut()
                .unwrap()
                .stored_receipt(&stop.request_id)
                .is_none()
        );
    }
}

#[test]
fn stop_only_dispatch_does_not_verify_the_workspace_or_drain_work() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    prepare(&mut h, &m, &w);
    let start = admit(&mut h, &m, CommandBody::StartPlanning);
    let pause = admit(&mut h, &m, CommandBody::Pause);
    drop(h);
    let mut h = Host::open(&w.db).unwrap();
    let before = h.snapshot(&m.mission_id).unwrap().to_json();
    assert_eq!(h.dispatch_stops(&m.mission_id).unwrap(), 1);
    let after = h.snapshot(&m.mission_id).unwrap().to_json();
    assert_eq!(before["workspace"], after["workspace"]);
    assert_eq!(before["executionEffects"], after["executionEffects"]);
    assert!(
        h.runtime_mut()
            .unwrap()
            .stored_receipt(&start.request_id)
            .is_none()
    );
    assert!(
        h.runtime_mut()
            .unwrap()
            .stored_receipt(&pause.request_id)
            .is_some()
    );
    assert_eq!(h.dispatch_stops(&m.mission_id).unwrap(), 0);
    assert!(
        after["nextSteps"]
            .to_string()
            .contains("not verified for the current owner")
    );
}

#[test]
fn lost_abort_acknowledgement_recovers_after_real_ledger_write_failure() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    let stop = admit(&mut h, &m, CommandBody::Cancel);
    let conn = Connection::open(&w.db).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_ack BEFORE UPDATE OF state ON commands
        WHEN NEW.state='acknowledged' BEGIN SELECT RAISE(ABORT,'lost acknowledgement'); END;",
    )
    .unwrap();
    assert_eq!(
        h.dispatch_stops(&m.mission_id).unwrap_err().category(),
        "storage_failed"
    );
    let receipt = h
        .runtime_mut()
        .unwrap()
        .stored_receipt(&stop.request_id)
        .unwrap();
    assert_eq!(h.snapshot(&m.mission_id).unwrap().pending_commands.len(), 1);
    drop(h);
    conn.execute_batch("DROP TRIGGER reject_ack;").unwrap();
    let mut h = Host::open(&w.db).unwrap();
    assert_eq!(h.dispatch_stops(&m.mission_id).unwrap(), 1);
    assert_eq!(h.dispatch_stops(&m.mission_id).unwrap(), 0);
    assert_eq!(h.runtime_mut().unwrap().submission_count(), 1);
    assert_eq!(
        h.runtime_mut().unwrap().stored_receipt(&stop.request_id),
        Some(receipt)
    );
}

#[test]
fn stop_replay_rejects_wrong_runtime_binding() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    let stop = admit(&mut h, &m, CommandBody::Cancel);
    let mut binding = h.binding(&h.store.load_mission(&m.mission_id).unwrap());
    binding.session_id = "other-session".into();
    h.runtime_mut()
        .unwrap()
        .abort(&RuntimeRequest {
            binding,
            request_id: stop.request_id,
            payload_hash: stop.payload_hash,
            method: "abort",
        })
        .unwrap();
    assert_eq!(
        h.dispatch_stops(&m.mission_id).unwrap_err().category(),
        "binding_mismatch"
    );
    assert_eq!(h.snapshot(&m.mission_id).unwrap().pending_commands.len(), 1);
}

#[test]
fn unresolved_operations_block_new_fake_work_even_without_a_stop() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    prepare(&mut h, &m, &w);
    let start = admit(&mut h, &m, CommandBody::StartPlanning);
    h.record_operation(
        &m.mission_id,
        "blocked",
        &json!({"effect":"unknown"}),
        OperationState::Uncertain,
    )
    .unwrap();
    assert!(
        h.dispatch_reconciled(&m.mission_id)
            .unwrap_err()
            .to_string()
            .contains("uncertain effects block runtime dispatch")
    );
    assert!(
        h.runtime_mut()
            .unwrap()
            .stored_receipt(&start.request_id)
            .is_none()
    );
}
