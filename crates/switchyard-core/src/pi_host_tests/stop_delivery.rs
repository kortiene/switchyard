use super::*;
use std::fs::OpenOptions;

#[test]
fn pi_stops_bypass_blocked_workspaces_without_resuming_pending_work() {
    for defect in ["uncertain", "busy", "unverified", "transient"] {
        for stop in [CommandBody::Pause, CommandBody::Cancel] {
            let w = world();
            let mut h = host(&w, "cooperative-wait");
            let m = create(&mut h, &w);
            let root = w._tmp.path().join("workspaces");
            std::fs::create_dir(&root).unwrap();
            let view = h
                .prepare_workspace(&m.mission_id, &m.attempt_id, &root)
                .unwrap();
            let start = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
            h.dispatch_pending(&m.mission_id).unwrap();
            let continued = admit(
                &mut h,
                &m.mission_id,
                CommandBody::Continue {
                    activity: "planning".into(),
                },
            )
            .unwrap();
            let stop = admit(&mut h, &m.mission_id, stop).unwrap();
            let mut lease = None;
            let db = Connection::open(&w.db).unwrap();
            match defect {
                "uncertain" => h.store.uncertain_workspace(&view.id, "injected ownership uncertainty").unwrap(),
                "busy" => {
                    let lock = OpenOptions::new().read(true).write(true)
                        .open(Path::new(&view.path).parent().unwrap().join("owner.lock")).unwrap();
                    lock.try_lock().unwrap(); lease = Some(lock);
                }
                "unverified" => { db.execute("UPDATE workspaces SET verified_generation=0", []).unwrap(); }
                "transient" => db.execute_batch("CREATE TRIGGER fail_verify BEFORE UPDATE OF verified_generation ON workspaces BEGIN SELECT RAISE(ABORT,'injected workspace failure'); END;").unwrap(),
                _ => unreachable!(),
            }
            let before = h.snapshot(&m.mission_id).unwrap().to_json();
            let result = if defect == "unverified" {
                h.dispatch_reconciled(&m.mission_id)
            } else {
                h.dispatch_pending(&m.mission_id)
            };
            assert!(result.is_err(), "{defect}");
            let after = h.snapshot(&m.mission_id).unwrap();
            assert_eq!(after.phase, stop.phase);
            assert_eq!(after.to_json()["workspace"], before["workspace"]);
            let stop_req = request(&h, &m.mission_id, &stop, "abort");
            let original = first_receipt(&mut h, &stop_req);
            assert_eq!(original["receipt"]["settled"], true);
            assert!(command_receipt(&w.db, &stop.request_id).is_some());
            assert!(command_receipt(&w.db, &continued.request_id).is_none());
            let start_req = request(&h, &m.mission_id, &start, "submit");
            assert_eq!(first_receipt(&mut h, &start_req)["reason"], "aborted");
            assert_eq!(h.dispatch_stops(&m.mission_id).unwrap(), 0);
            // The abort receipt acknowledges only runtime cancellation.
            assert!(admit(&mut h, &m.mission_id, CommandBody::ConfirmStopped).is_err());
            assert!(admit(&mut h, &m.mission_id, CommandBody::ConfirmCancelled).is_err());
            drop(h);
            drop(lease);
            if defect == "transient" {
                db.execute_batch("DROP TRIGGER fail_verify").unwrap();
            }
            let mut h = host(&w, "recover");
            // Reopen inert and prove the original stop survives another owner.
            let req = request(&h, &m.mission_id, &stop, "abort");
            pi(&mut h).open(&req.binding, true).unwrap();
            assert_eq!(first_receipt(&mut h, &req), original);
            assert_eq!(counts(&w), (1, 1, 3));
        }
    }
}

#[test]
fn pi_stop_lost_host_ack_reconciles_without_workspace_readiness() {
    for body in [CommandBody::Pause, CommandBody::Cancel] {
        let w = world();
        let mut h = host(&w, "answer");
        let m = create(&mut h, &w);
        // Neither the queued work nor a workspace may be needed to deliver stop.
        let start = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
        let stop = admit(&mut h, &m.mission_id, body).unwrap();
        let db = Connection::open(&w.db).unwrap();
        db.execute_batch("CREATE TRIGGER fail_ack BEFORE UPDATE OF state ON commands WHEN NEW.state='acknowledged' BEGIN SELECT RAISE(ABORT,'lost stop ack'); END;").unwrap();
        assert_eq!(
            h.dispatch_stops(&m.mission_id).unwrap_err().category(),
            "storage_failed"
        );
        let req = request(&h, &m.mission_id, &stop, "abort");
        let original = first_receipt(&mut h, &req);
        assert_eq!(original["receipt"]["settled"], true);
        drop(h);
        db.execute_batch("DROP TRIGGER fail_ack").unwrap();
        // Armed fixture kills on an actual abort, so successful reconciliation
        // proves we inspected the settled receipt instead of sending it again.
        let mut h = host(&w, "crash-after-abort-settled");
        assert_eq!(h.dispatch_stops(&m.mission_id).unwrap(), 1);
        let req = request(&h, &m.mission_id, &stop, "abort");
        assert_eq!(first_receipt(&mut h, &req), original);
        assert!(command_receipt(&w.db, &start.request_id).is_none());
        assert_eq!(h.snapshot(&m.mission_id).unwrap().phase, stop.phase);
        assert_eq!(h.dispatch_stops(&m.mission_id).unwrap(), 0);
    }
}

#[test]
fn pi_stale_owner_cannot_open_or_deliver_stop() {
    let w = world();
    let mut h = host(&w, "answer");
    let m = create(&mut h, &w);
    let stop = admit(&mut h, &m.mission_id, CommandBody::Cancel).unwrap();
    Connection::open(&w.db)
        .unwrap()
        .execute("UPDATE host_owner SET generation=generation+1", [])
        .unwrap();
    assert_eq!(
        h.dispatch_stops(&m.mission_id).unwrap_err().category(),
        "stale_generation"
    );
    assert!(pi(&mut h).open_bindings().is_empty());
    assert!(command_receipt(&w.db, &stop.request_id).is_none());
}
