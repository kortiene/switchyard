//! Compose real outbox recovery with interrupted durable tool tasks.
//! The launcher gate is before execution; real effects remain an S3 boundary.
use super::*;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

fn entries(path: &Path) -> Vec<Value> {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    // The child appends JSONL while we poll. A read can split the last write,
    // so only newline-terminated lines are complete records.
    let complete = raw.rsplit_once('\n').map(|(done, _)| done).unwrap_or("");
    complete
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn wait_entries(path: &Path, count: usize) -> Vec<Value> {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let found = entries(path);
        if found.len() >= count {
            return found;
        }
        assert!(
            Instant::now() < until,
            "tool entry barrier not reached: {found:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn entries_ignore_a_trailing_partial_line_while_polling() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("entered");
    std::fs::write(
        &path,
        "{\"task\":\"t1\",\"name\":\"write_file\",\"generation\":1}\n{\"task\":\"t2\"",
    )
    .unwrap();
    let partial = entries(&path);
    assert_eq!(partial.len(), 1);
    assert_eq!(partial[0]["task"], "t1");
    assert_eq!(partial[0]["name"], "write_file");
    std::fs::write(
        &path,
        "{\"task\":\"t1\",\"name\":\"write_file\",\"generation\":1}\n{\"task\":\"t2\",\"name\":\"write_pair\",\"generation\":2}\n",
    )
    .unwrap();
    let complete = entries(&path);
    assert_eq!(complete.len(), 2);
    assert_eq!(complete[1]["task"], "t2");
    assert_eq!(complete[1]["generation"], 2);
}

#[test]
fn pending_stops_prevent_interrupted_tool_replay_across_two_recoveries() {
    for stop in [CommandBody::Pause, CommandBody::Cancel] {
        let w = world();
        let mut h = host(&w, "gated-tools");
        let m = create(&mut h, &w);
        let start = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
        h.dispatch_pending(&m.mission_id).unwrap();
        let req = request(&h, &m.mission_id, &start, "submit");
        let marker = PathBuf::from(format!(
            "{}.entered",
            pi(&mut h).database(&req.binding).display()
        ));
        let initial = wait_entries(&marker, 2);
        assert_eq!(initial.len(), 2);
        let original = first_receipt(&mut h, &req)["receipt"].clone();
        let continuation = admit(
            &mut h,
            &m.mission_id,
            CommandBody::Continue {
                activity: "planning".into(),
            },
        )
        .unwrap();
        let stop = admit(&mut h, &m.mission_id, stop).unwrap();
        drop(h); // Reap the child with two actual durable tool tasks interrupted.

        for _ in 0..2 {
            let mut h = host(&w, "gated-tools");
            let req = request(&h, &m.mission_id, &start, "submit");
            pi(&mut h).open(&req.binding, true).unwrap();
            let before = pi(&mut h).inspect(&req).unwrap();
            assert_eq!(before["scheduling"], "paused");
            assert_eq!(before["commands"][0]["receipt"], original);
            let after = h.recover(&m.mission_id).unwrap();
            assert_eq!(after.phase, stop.phase);
            assert!(after.pending_commands.is_empty());
            assert_eq!(after.commands[1].id, continuation.command_id);
            assert_eq!(after.commands[1].state, "superseded");
            assert_eq!(
                after.commands[2].receipt_id.as_deref(),
                Some(stop.request_id.as_str())
            );
            let state = pi(&mut h).inspect(&req).unwrap();
            assert_eq!(state["commands"][0]["receipt"], original);
            assert_eq!(state["commands"][0]["reason"], "aborted");
            assert_eq!(state["runtimeIdle"], true);
            assert_eq!(state["liveTaskCount"], 0);
            assert_eq!(
                entries(&marker),
                initial,
                "stop must settle before any tool replay"
            );
            assert!(after.execution_effects.is_empty());
            assert!(admit(&mut h, &m.mission_id, CommandBody::ConfirmStopped).is_err());
            assert!(admit(&mut h, &m.mission_id, CommandBody::ConfirmCancelled).is_err());
            assert_eq!(counts(&w), (1, 1, 3));
        }
    }
}

#[test]
fn reconciled_active_host_resumes_the_same_durable_tool_tasks() {
    let w = world();
    let mut h = host(&w, "gated-tools");
    let m = create(&mut h, &w);
    let start = admit(&mut h, &m.mission_id, CommandBody::StartPlanning).unwrap();
    h.dispatch_pending(&m.mission_id).unwrap();
    let req = request(&h, &m.mission_id, &start, "submit");
    let marker = PathBuf::from(format!(
        "{}.entered",
        pi(&mut h).database(&req.binding).display()
    ));
    let initial = wait_entries(&marker, 2);
    let original = first_receipt(&mut h, &req)["receipt"].clone();
    drop(h);

    let mut h = host(&w, "gated-tools");
    let req = request(&h, &m.mission_id, &start, "submit");
    pi(&mut h).open(&req.binding, true).unwrap();
    assert_eq!(pi(&mut h).inspect(&req).unwrap()["scheduling"], "paused");
    assert_eq!(entries(&marker), initial);
    let after = h.recover(&m.mission_id).unwrap();
    assert!(after.pending_commands.is_empty());
    assert_eq!(
        after.commands[0].receipt_id.as_deref(),
        original["receipt"].as_str()
    );
    let resumed = wait_entries(&marker, 4);
    assert_eq!(resumed.len(), 4);
    let tasks = |rows: &[Value]| {
        rows.iter()
            .map(|row| {
                (
                    row["task"].as_str().unwrap().to_string(),
                    row["name"].as_str().unwrap().to_string(),
                )
            })
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(tasks(&initial), tasks(&resumed[2..]));
    assert!(
        resumed[2..]
            .iter()
            .all(|row| row["generation"] == h.generation())
    );
    admit(&mut h, &m.mission_id, CommandBody::Pause).unwrap();
    h.dispatch_pending(&m.mission_id).unwrap();
    assert_eq!(pi(&mut h).inspect(&req).unwrap()["runtimeIdle"], true);
    assert_eq!(counts(&w), (1, 1, 2));
}
