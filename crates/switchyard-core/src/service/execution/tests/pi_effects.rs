//! Exercise Pi's durable tool identities against the real Host effect journal.
//! The mapper and fault injection are test-only; no product Pi execution wiring.
use super::*;
use crate::{PiBridge, PiEffectOutcome};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn spawn(db: &Path, binding: RuntimeBinding, fixture: &str) -> PiBridge {
    let node = std::env::var_os("SWITCHYARD_PI_NODE")
        .expect("pi-contract requires SWITCHYARD_PI_NODE; this gate must not skip");
    let launcher = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/pi-source/test/bridge-child.ts");
    let mut bridge = PiBridge::spawn(Path::new(&node), &launcher, db, binding, fixture).unwrap();
    bridge.hello().unwrap();
    bridge
        .call("runtime.open", None, json!({}), &mut |_| {
            panic!("opening a session must not execute tools")
        })
        .unwrap();
    bridge
}

fn request_for(template: &ExecutionRequest, effect: &Value) -> ExecutionRequest {
    assert_eq!(effect["payload"]["method"], "file.write");
    assert_eq!(effect["payload"]["capability"], "workspace-write");
    assert_eq!(effect["payload"]["deadlinePolicy"], "attempt-bound");
    // PiBridge checks the wire hash before this mapper. The Host hash also
    // binds its own simulated capability and the original fixed deadline;
    // neither deadline nor operation identity may be renewed on reconnect.
    let request = ExecutionRequest {
        tool_task_id: effect["taskId"].as_str().unwrap().into(),
        effect_key: effect["effectKey"].as_str().unwrap().into(),
        operation: ExecutionOperation::WriteFile {
            path: effect["payload"]["path"].as_str().unwrap().into(),
            content: effect["payload"]["content"].as_str().unwrap().into(),
        },
        ..template.clone()
    };
    assert_eq!(request.operation_id().unwrap(), effect["operationId"]);
    request
}

fn replay_after_sigkill(uncertain: bool) {
    let (directory, mut host, mut template) = setup();
    let db = directory.path().join("pi.sqlite");
    let mut bridge = spawn(&db, template.binding.clone(), "tools");
    let pid = bridge.process_id();
    let mut first_request = None;
    let mut original = None;
    let mut effect = |value: &Value| {
        if first_request.is_some() {
            // Ignore already-buffered requests after the kill boundary.
            return PiEffectOutcome::Uncertain;
        }
        let request = request_for(&template, value);
        if uncertain {
            // Apply once, fail before the receipt commits. The Host records
            // uncertainty and retains its mission-wide execution block.
            host.runtime_mut().unwrap().fail_next_simulated_effect();
        }
        let outcome = host.execute_fake_operation_outcome(&request);
        if uncertain {
            assert_eq!(outcome, PiEffectOutcome::Uncertain);
        } else {
            assert!(matches!(outcome, PiEffectOutcome::Completed(_)));
        }
        first_request = Some(request);
        original = Some(outcome.clone());
        assert!(
            Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status()
                .unwrap()
                .success()
        );
        outcome
    };
    // Service callbacks even if an effect arrives before the submission reply.
    let submitted = bridge.call_with_effects(
        "runtime.submit",
        Some("tools"),
        json!({"type":"input","content":"write"}),
        &mut effect,
    );
    let until = Instant::now() + Duration::from_secs(5);
    if submitted.is_ok() {
        while Instant::now() < until {
            if bridge
                .call_with_effects("runtime.inspect", None, json!({}), &mut effect)
                .is_err()
            {
                break;
            }
        }
    }
    let mut first_request = first_request.expect("effect must apply before Pi is killed");
    let original = original.unwrap();
    assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 1);
    drop(bridge);
    drop(host);

    let mut host = Host::open(&directory.path().join("mission.sqlite")).unwrap();
    assert!(host.generation() > template.binding.generation);
    // An old generation may not even read a completed receipt.
    assert_eq!(
        host.execute_fake_operation_outcome(&first_request),
        PiEffectOutcome::Rejected
    );
    template.binding.generation = host.generation();
    first_request.binding.generation = host.generation();
    let mut bridge = spawn(&db, template.binding.clone(), "recover");
    let mut replayed = 0;
    let mut refused_new = 0;
    let mut effect = |value: &Value| {
        let request = request_for(&template, value);
        let outcome = host.execute_fake_operation_outcome(&request);
        if request.operation_id().unwrap() == first_request.operation_id().unwrap() {
            assert_eq!(
                request.payload_hash().unwrap(),
                first_request.payload_hash().unwrap()
            );
            assert_eq!(
                outcome, original,
                "replay must retain the original durable outcome"
            );
            replayed += 1;
        } else if uncertain {
            assert_eq!(outcome, PiEffectOutcome::Rejected);
            refused_new += 1;
        } else {
            assert!(matches!(outcome, PiEffectOutcome::Completed(_)));
        }
        outcome
    };
    bridge
        .call_with_effects(
            "runtime.submit",
            Some("tools"),
            json!({"type":"input","content":"write"}),
            &mut effect,
        )
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let mut idle = false;
    while Instant::now() < until {
        let snapshot = bridge
            .call_with_effects(
                "runtime.inspect",
                None,
                json!({"commandIds":["tools"]}),
                &mut effect,
            )
            .unwrap();
        if snapshot["runtimeIdle"] == true {
            assert_eq!(snapshot["commands"][0]["status"], "done");
            idle = true;
            break;
        }
    }
    assert!(
        idle,
        "recovered Pi must settle without granting Host authority"
    );
    assert_eq!(replayed, 1);
    let snapshot = host.snapshot(&template.binding.mission_id).unwrap();
    assert_eq!(snapshot.execution_blocked, uncertain);
    assert_eq!(
        snapshot.execution_effects.len(),
        if uncertain { 1 } else { 3 }
    );
    assert_eq!(
        host.runtime_mut().unwrap().simulated_effect_count(),
        if uncertain { 0 } else { 2 }
    );
    if uncertain {
        assert!(
            refused_new > 0,
            "the unresolved effect must block new tool tasks"
        );
        assert_eq!(snapshot.phase, Phase::NeedsAttention);
    } else {
        // A replay with altered arguments cannot change the original receipt.
        let mut changed = first_request.clone();
        changed.operation = ExecutionOperation::WriteFile {
            path: "changed".into(),
            content: "changed".into(),
        };
        assert_eq!(
            host.execute_fake_operation_outcome(&changed),
            PiEffectOutcome::Conflict
        );
        assert_eq!(
            host.execute_fake_operation_outcome(&first_request),
            original
        );
        assert_eq!(host.runtime_mut().unwrap().simulated_effect_count(), 2);
        assert_eq!(snapshot.phase, Phase::Implementing);
    }
    // The fake backend never grants filesystem writes or execution quiescence.
    let worktree = PathBuf::from(snapshot.workspace.unwrap().path);
    assert_eq!(std::fs::read_dir(worktree).unwrap().count(), 2); // README.md and .git
}

#[test]
fn multi_effect_and_nested_replay_reuse_host_receipts_after_sigkill() {
    replay_after_sigkill(false);
}

#[test]
fn uncertain_effect_replay_after_sigkill_never_reapplies_or_unblocks() {
    replay_after_sigkill(true);
}
