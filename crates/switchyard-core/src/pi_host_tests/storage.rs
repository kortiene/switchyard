use super::*;

fn assert_peer_reaped(pid: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while process_alive(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "failed peer {pid} must be reaped"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn sqlite_admission_failure_preserves_the_outbox_until_original_command_recovers() {
    let w = world();
    let mut h = host(&w, "storage-submit");
    let mission = create(&mut h, &w);
    let start = admit(&mut h, &mission.mission_id, CommandBody::StartPlanning).unwrap();
    let error = h.dispatch_pending(&mission.mission_id).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("attempt to write a readonly database"),
        "{error}"
    );
    assert!(command_receipt(&w.db, &start.request_id).is_none());
    assert_eq!(
        h.snapshot(&mission.mission_id)
            .unwrap()
            .pending_commands
            .len(),
        1
    );
    drop(h);

    let mut original = None;
    for generation in 0..2 {
        let mut h = host(&w, "recover");
        let req = request(&h, &mission.mission_id, &start, "submit");
        pi(&mut h).open(&req.binding, true).unwrap();
        let before = first_receipt(&mut h, &req);
        if generation == 0 {
            assert!(before["receipt"].is_null());
            assert!(before["status"].is_null());
        }
        let snapshot = h.recover(&mission.mission_id).unwrap();
        assert!(snapshot.pending_commands.is_empty());
        assert_eq!(snapshot.commands[0].request_id, start.request_id);
        assert_eq!(snapshot.commands[0].payload_hash, start.payload_hash);
        let receipt = first_receipt(&mut h, &req)["receipt"].clone();
        assert_eq!(receipt["settled"], true);
        if let Some(original) = &original {
            assert_eq!(&receipt, original);
        }
        original = Some(receipt);
        assert_eq!(counts(&w), (1, 1, 1));
    }
}

#[test]
fn sqlite_abort_failures_preserve_pending_stops_and_recover_without_claiming_quiescence() {
    for (fixture, has_intent) in [
        ("storage-abort-intent", false),
        ("storage-abort-cancel", true),
        ("storage-abort-settlement", true),
    ] {
        for body in [CommandBody::Pause, CommandBody::Cancel] {
            let w = world();
            let mut h = host(&w, fixture);
            let mission = create(&mut h, &w);
            let start = admit(&mut h, &mission.mission_id, CommandBody::StartPlanning).unwrap();
            h.dispatch_pending(&mission.mission_id).unwrap();
            let req = request(&h, &mission.mission_id, &start, "submit");
            let original_work = first_receipt(&mut h, &req)["receipt"].clone();
            let pid = pi(&mut h).process_id(&req.binding).unwrap();
            let stop = admit(&mut h, &mission.mission_id, body).unwrap();
            let error = h.dispatch_pending(&mission.mission_id).unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains("attempt to write a readonly database"),
                "{fixture}: {error}"
            );
            assert!(!message.contains("cancellation_timeout"), "{error}");
            // Reaping is normally finished inside the dispatch error, but a
            // late wait must not flake. The peer still has to be gone.
            assert_peer_reaped(pid);
            assert!(pi(&mut h).database(&req.binding).is_file());
            let pending = h.snapshot(&mission.mission_id).unwrap();
            assert_eq!(pending.phase, stop.phase);
            assert_eq!(pending.pending_commands.len(), 1);
            assert_eq!(pending.pending_commands[0].request_id, stop.request_id);
            assert!(command_receipt(&w.db, &stop.request_id).is_none());
            drop(h);

            let mut original_stop = None;
            for generation in 0..2 {
                let mut h = host(&w, "recover");
                let req = request(&h, &mission.mission_id, &stop, "abort");
                pi(&mut h).open(&req.binding, true).unwrap();
                let before = pi(&mut h).inspect(&req).unwrap();
                assert_eq!(before["scheduling"], "paused");
                if generation == 0 && !has_intent {
                    assert!(before["commands"][0]["receipt"].is_null());
                } else {
                    assert_eq!(before["commands"][0]["receipt"]["receipt"], stop.request_id);
                    assert_eq!(before["commands"][0]["receipt"]["settled"], generation > 0);
                }
                let snapshot = h.recover(&mission.mission_id).unwrap();
                assert_eq!(snapshot.phase, stop.phase);
                assert!(snapshot.pending_commands.is_empty());
                let row = snapshot
                    .commands
                    .iter()
                    .find(|row| row.request_id == stop.request_id)
                    .unwrap();
                assert_eq!(row.state, "acknowledged");
                assert_eq!(row.receipt_id.as_deref(), Some(stop.request_id.as_str()));
                assert_eq!(row.payload_hash, stop.payload_hash);
                let receipt = first_receipt(&mut h, &req)["receipt"].clone();
                if let Some(original) = &original_stop {
                    assert_eq!(&receipt, original);
                }
                original_stop = Some(receipt);
                let work = request(&h, &mission.mission_id, &start, "submit");
                let work = first_receipt(&mut h, &work);
                assert_eq!(work["receipt"], original_work);
                assert_eq!(work["reason"], "aborted");
                assert!(admit(&mut h, &mission.mission_id, CommandBody::ConfirmStopped).is_err());
                assert!(admit(&mut h, &mission.mission_id, CommandBody::ConfirmCancelled).is_err());
                assert_eq!(counts(&w), (1, 1, 2));
            }
        }
    }
}

#[test]
fn rust_ack_write_failure_recovers_a_settled_pi_abort_without_repeating_it() {
    for body in [CommandBody::Pause, CommandBody::Cancel] {
        let w = world();
        let mut h = host(&w, "cooperative-wait");
        let mission = create(&mut h, &w);
        admit(&mut h, &mission.mission_id, CommandBody::StartPlanning).unwrap();
        h.dispatch_pending(&mission.mission_id).unwrap();
        let stop = admit(&mut h, &mission.mission_id, body).unwrap();
        let db = Connection::open(&w.db).unwrap();
        db.execute_batch("CREATE TRIGGER fail_ack BEFORE UPDATE OF state ON commands WHEN NEW.state='acknowledged'
            BEGIN SELECT RAISE(ABORT, 'injected Rust acknowledgement failure'); END;").unwrap();
        let error = h.dispatch_pending(&mission.mission_id).unwrap_err();
        assert_eq!(error.category(), "storage_failed");
        assert!(
            error
                .to_string()
                .contains("injected Rust acknowledgement failure")
        );
        assert!(command_receipt(&w.db, &stop.request_id).is_none());
        let req = request(&h, &mission.mission_id, &stop, "abort");
        let original = first_receipt(&mut h, &req)["receipt"].clone();
        assert_eq!(original["settled"], true);
        assert_eq!(
            h.snapshot(&mission.mission_id)
                .unwrap()
                .pending_commands
                .len(),
            1
        );
        drop(h);
        db.execute_batch("DROP TRIGGER fail_ack").unwrap();
        for _ in 0..2 {
            // If recovery applies cancellation again, the armed settlement crash kills it.
            let mut h = host(&w, "crash-after-abort-settled");
            let snapshot = h.recover(&mission.mission_id).unwrap();
            assert!(snapshot.pending_commands.is_empty());
            assert_eq!(snapshot.phase, stop.phase);
            let req = request(&h, &mission.mission_id, &stop, "abort");
            assert_eq!(first_receipt(&mut h, &req)["receipt"], original);
            assert_eq!(counts(&w), (1, 1, 2));
        }
    }
}
