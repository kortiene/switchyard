use super::*;

#[test]
fn abort_crashes_recover_original_stop_without_claiming_mission_quiescence() {
    for (fixture, settled) in [
        ("crash-after-abort-recorded", false),
        ("crash-after-abort-settled", true),
    ] {
        for stop in [CommandBody::Pause, CommandBody::Cancel] {
            let w = world();
            let mut h = host(&w, fixture);
            let mission = create(&mut h, &w);
            admit(&mut h, &mission.mission_id, CommandBody::StartPlanning).unwrap();
            h.dispatch_pending(&mission.mission_id).unwrap();
            let stop = admit(&mut h, &mission.mission_id, stop).unwrap();
            // SIGKILL drops the peer mid-response. Only that transport loss is
            // the crash precondition; a binding or payload error is a different failure.
            match h.dispatch_pending(&mission.mission_id) {
                Err(Error::GuardFailed { detail })
                    if detail == "interrupted or oversized bridge frame"
                        || detail.starts_with("interrupted or oversized bridge frame\n") => {}
                other => {
                    panic!("SIGKILL fixture must interrupt the bridge transport, got {other:?}")
                }
            }
            let snapshot = h.snapshot(&mission.mission_id).unwrap();
            assert_eq!(snapshot.phase, stop.phase);
            assert_eq!(snapshot.pending_commands.len(), 1);
            assert_eq!(snapshot.pending_commands[0].request_id, stop.request_id);
            assert!(command_receipt(&w.db, &stop.request_id).is_none());
            drop(h);

            // Keep the crash fixture armed. A recorded abort is resumed, and a
            // settled abort is only inspected; neither creates a new identity.
            for generation in 0..2 {
                let mut h = host(&w, fixture);
                let req = request(&h, &mission.mission_id, &stop, "abort");
                pi(&mut h).open(&req.binding, true).unwrap();
                let before = pi(&mut h).inspect(&req).unwrap();
                assert_eq!(before["scheduling"], "paused");
                assert_eq!(before["commands"][0]["receipt"]["receipt"], stop.request_id);
                assert_eq!(
                    before["commands"][0]["receipt"]["settled"],
                    settled || generation > 0
                );
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
                let after = pi(&mut h).inspect(&req).unwrap();
                assert_eq!(after["commands"][0]["receipt"]["settled"], true);
                assert_eq!(after["stopping"], false);
                assert_eq!(after["runtimeIdle"], true);
                assert!(admit(&mut h, &mission.mission_id, CommandBody::ConfirmStopped).is_err());
                assert!(admit(&mut h, &mission.mission_id, CommandBody::ConfirmCancelled).is_err());
                assert_eq!(counts(&w), (1, 1, 2));
            }
        }
    }
}

#[test]
fn sustained_real_busy_continuation_cannot_starve_an_admitted_stop() {
    for stop in [CommandBody::Pause, CommandBody::Cancel] {
        let w = world();
        let mut h = host(&w, "cooperative-wait");
        let mission = create(&mut h, &w);
        let start = admit(&mut h, &mission.mission_id, CommandBody::StartPlanning).unwrap();
        h.dispatch_pending(&mission.mission_id).unwrap();
        let continued = admit(
            &mut h,
            &mission.mission_id,
            CommandBody::Continue {
                activity: "planning".into(),
            },
        )
        .unwrap();
        for _ in 0..8 {
            let error = h.dispatch_pending(&mission.mission_id).unwrap_err();
            assert!(error.to_string().contains("bridge: busy"), "{error}");
            assert!(command_receipt(&w.db, &continued.request_id).is_none());
        }
        let stop = admit(&mut h, &mission.mission_id, stop).unwrap();
        assert_eq!(h.dispatch_pending(&mission.mission_id).unwrap(), 1);
        let snapshot = h.snapshot(&mission.mission_id).unwrap();
        assert!(snapshot.pending_commands.is_empty());
        assert_eq!(snapshot.phase, stop.phase);
        assert_eq!(snapshot.commands[1].request_id, continued.request_id);
        assert_eq!(snapshot.commands[1].state, "superseded");
        let req = request(&h, &mission.mission_id, &start, "submit");
        assert_eq!(first_receipt(&mut h, &req)["reason"], "aborted");
        drop(h);
        let mut h = host(&w, "recover");
        let snapshot = h.recover(&mission.mission_id).unwrap();
        assert_eq!(snapshot.phase, stop.phase);
        assert!(snapshot.pending_commands.is_empty());
        assert_eq!(counts(&w), (1, 1, 3));
    }
}
