//! A repair admitted after the live bridge is proven stopped stays pending.
//! Reopen can still deliver it. Cancel supersedes it.
use switchyard_core::{CommandBody, Host, Phase};

use super::{World, admit, until};

fn command_state(host: &Host, mission: &str, kind: &str) -> String {
    host.snapshot(mission)
        .unwrap()
        .commands
        .iter()
        .find(|command| command.kind == kind)
        .unwrap_or_else(|| panic!("missing {kind}"))
        .state
        .clone()
}

fn proven_bridge_with_pending_repair() -> (World, Host) {
    let (world, mut host) = World::new("answer");
    host.dispatch_pending(&world.mission).unwrap();
    until(&mut host, &world.mission, |value| {
        value["piHost"]["observation"]["runtimeIdle"] == true
    });
    let proof = host.stop_live_bridge_for_test(&world.mission).unwrap();
    assert_eq!(proof["proven"], true, "{proof}");
    rusqlite::Connection::open(world.db())
        .unwrap()
        .execute(
            "UPDATE missions SET phase='awaiting_review' WHERE id=?1",
            [&world.mission],
        )
        .unwrap();
    admit(
        &mut host,
        &world.mission,
        CommandBody::RequestChanges {
            feedback: "hold the repair".into(),
            within_frozen: true,
        },
    );
    let error = host.dispatch_pending(&world.mission).unwrap_err();
    assert_eq!(error.category(), "repair_after_settle", "{error}");
    assert_eq!(
        command_state(&host, &world.mission, "request_changes"),
        "pending"
    );
    (world, host)
}

#[test]
fn repair_after_proven_stop_stays_pending_and_reopen_can_deliver_it() {
    let (world, host) = proven_bridge_with_pending_repair();
    drop(host);
    let mut host = world.reopen();
    match host.dispatch_pending(&world.mission) {
        Ok(_) => {}
        Err(error) => assert_ne!(
            error.category(),
            "repair_after_settle",
            "a new generation is not the settled bridge: {error}"
        ),
    }
    let state = command_state(&host, &world.mission, "request_changes");
    assert!(
        state == "pending" || state == "acknowledged" || state == "reconciled",
        "repair must stay deliverable, got {state}"
    );
}

#[test]
fn cancel_supersedes_a_repair_left_pending_after_settle() {
    let (world, mut host) = proven_bridge_with_pending_repair();
    admit(&mut host, &world.mission, CommandBody::Cancel);
    host.dispatch_pending(&world.mission).unwrap();
    assert_eq!(
        command_state(&host, &world.mission, "request_changes"),
        "superseded"
    );
    assert_eq!(
        host.snapshot(&world.mission).unwrap().phase,
        Phase::Cancelled
    );
}
