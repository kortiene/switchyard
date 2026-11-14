//! S4-A slice 172a: freeze attempt inputs on draft -> planning.
use super::fixtures::{admit, admit_expect, create, notify, open_world, plan_note};
use crate::canonical::payload_hash;
use crate::persistence::AttemptInputPatch;
use crate::{
    AdmitRequest, CommandBody, CommandOrigin, ExecutionOperation, ExecutionRequest,
    FAKE_WRITE_SCOPE, RuntimeBinding,
};
use serde_json::Value;

#[test]
fn unlisted_transition_and_stale_revision_do_not_freeze_inputs() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    assert!(!created.inputs_frozen);
    assert!(created.attempt_owner_generation.is_none());
    assert!(created.commands.is_empty());

    let illegal = host.admit(AdmitRequest {
        command_id: None,
        mission_id: created.mission_id.clone(),
        attempt_id: None,
        expected_revision: created.revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body: CommandBody::ApprovePlan {
            plan_digest: "sha256:not-a-plan".into(),
        },
    });
    let illegal = illegal.unwrap_err();
    assert_eq!(illegal.category(), "illegal_transition");
    let after_illegal = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(after_illegal.phase.as_str(), "draft");
    assert_eq!(after_illegal.revision, created.revision);
    assert!(!after_illegal.inputs_frozen);
    assert!(after_illegal.commands.is_empty());

    let stale = host.admit(AdmitRequest {
        command_id: None,
        mission_id: created.mission_id.clone(),
        attempt_id: None,
        expected_revision: created.revision + 4,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body: CommandBody::StartPlanning,
    });
    let stale = stale.unwrap_err();
    assert_eq!(stale.category(), "stale_revision");
    let after_stale = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(after_stale.phase.as_str(), "draft");
    assert_eq!(after_stale.revision, created.revision);
    assert!(after_stale.frozen_inputs_json.is_none());
    assert!(after_stale.commands.is_empty());
    let decisions = host.decisions(&created.mission_id).unwrap();
    assert!(
        decisions
            .iter()
            .all(|decision| decision.action != "start_planning")
    );
}

#[test]
fn start_planning_freezes_inputs_and_refuses_later_writes() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    host.revise_attempt_inputs(
        &created.mission_id,
        &AttemptInputPatch {
            objective: Some("Frozen objective".into()),
            acceptance_criteria: Some("Frozen acceptance".into()),
            base_sha: Some("0123456789abcdef".into()),
            issue_title: Some("Frozen issue".into()),
            max_repairs: Some(4),
            max_plan_revisions: Some(3),
            verifier_json: Some(
                r#"{"executes":false,"frozen":true,"id":"switchyard.verifier.v1"}"#.into(),
            ),
        },
    )
    .unwrap();
    let drafted = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(drafted.objective, "Frozen objective");
    assert!(!drafted.inputs_frozen);

    let admitted = admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    assert_eq!(admitted.phase.as_str(), "planning");
    assert_eq!(admitted.revision, drafted.revision + 1);
    let frozen = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(frozen.revision, drafted.revision + 1);
    assert!(frozen.inputs_frozen);
    assert_eq!(frozen.attempt_owner_generation, Some(host.generation()));
    assert_eq!(
        frozen.attempt_owner_generation,
        Some(frozen.recorded_owner_generation)
    );
    assert_eq!(frozen.base_sha, "0123456789abcdef");
    assert_eq!(frozen.max_repairs, 4);
    assert_eq!(frozen.max_plan_revisions, 3);
    let document: Value =
        serde_json::from_str(frozen.frozen_inputs_json.as_deref().unwrap()).unwrap();
    assert_eq!(document["objective"], "Frozen objective");
    assert_eq!(document["acceptanceCriteria"], "Frozen acceptance");
    assert_eq!(document["baseSha"], "0123456789abcdef");
    assert_eq!(document["issueSnapshot"]["title"], "Frozen issue");
    assert_eq!(document["executionLimits"]["maxRepairs"].as_u64(), Some(4));
    assert_eq!(
        document["executionLimits"]["maxPlanRevisions"].as_u64(),
        Some(3)
    );
    assert_eq!(document["verifier"]["id"], "switchyard.verifier.v1");
    assert_eq!(document["schema"], "switchyard.attempt-freeze.v1");
    assert_eq!(
        document["sandboxPolicy"]["policy"]["planningTools"].as_str(),
        Some("read-only")
    );
    assert_eq!(
        document["sandboxPolicy"]["environment"]["host"].as_str(),
        Some("single")
    );
    assert!(
        document["sandboxPolicy"]["policy"]
            .get("maxRepairs")
            .is_none()
    );
    assert!(document.get("sessionInitialized").is_none());
    assert!(
        document["runtimeConfig"]
            .get("sessionInitialized")
            .is_none()
    );

    let command = frozen
        .commands
        .iter()
        .find(|command| command.kind == "start_planning")
        .unwrap();
    assert_eq!(command.state, "pending");
    assert_eq!(command.expected_revision, drafted.revision);
    assert_eq!(command.resulting_revision, Some(frozen.revision));
    assert_eq!(command.origin, "operator");
    let decision = host
        .decisions(&created.mission_id)
        .unwrap()
        .into_iter()
        .find(|decision| decision.action == "start_planning")
        .unwrap();
    assert_eq!(decision.command_id.as_deref(), Some(command.id.as_str()));
    assert_eq!(
        decision.digest.as_deref(),
        Some(payload_hash(&document).unwrap().as_str())
    );

    let again =
        admit_expect(&mut host, &created.mission_id, CommandBody::StartPlanning).unwrap_err();
    assert_eq!(again.category(), "illegal_transition");
    let revise = host.revise_attempt_inputs(
        &created.mission_id,
        &AttemptInputPatch {
            objective: Some("Changed after freeze".into()),
            base_sha: Some("ffffffffffffffff".into()),
            max_plan_revisions: Some(9),
            ..AttemptInputPatch::default()
        },
    );
    let revise = revise.unwrap_err();
    assert_eq!(revise.category(), "guard_failed");
    assert!(revise.to_string().contains("immutable"), "{revise}");
    let unchanged = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(unchanged.objective, "Frozen objective");
    assert_eq!(unchanged.base_sha, "0123456789abcdef");
    assert_eq!(unchanged.max_plan_revisions, 3);
    assert_eq!(unchanged.frozen_inputs_json, frozen.frozen_inputs_json);
    assert_eq!(unchanged.revision, frozen.revision);

    admit(&mut host, &created.mission_id, CommandBody::Pause);
    let paused = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(paused.phase.as_str(), "pausing");
    assert_eq!(paused.frozen_inputs_json, frozen.frozen_inputs_json);
    assert_eq!(
        paused.attempt_owner_generation,
        frozen.attempt_owner_generation
    );
}

#[test]
fn tampered_frozen_inputs_route_to_needs_attention_and_still_cancel() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let frozen = host.snapshot(&created.mission_id).unwrap();
    let snapshot = frozen.frozen_inputs_json.clone();
    host.tamper_attempt_base_sha(&created.mission_id, "deadbeef")
        .unwrap();

    let error = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::Steer {
            content: "keep going".into(),
        },
    )
    .unwrap_err();
    assert_eq!(error.category(), "guard_failed");
    assert!(error.to_string().contains("was not applied"), "{error}");
    let attention = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(attention.phase.as_str(), "needs_attention");
    assert_eq!(
        attention.attention_reason.as_deref(),
        Some("frozen_input_drift")
    );
    assert_eq!(attention.frozen_inputs_json, snapshot);
    assert_eq!(attention.base_sha, "deadbeef");
    assert!(
        attention
            .commands
            .iter()
            .all(|command| command.kind != "steer")
    );
    assert!(
        attention
            .commands
            .iter()
            .any(|command| command.kind == "detect_uncertainty")
    );

    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    let cancelling = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(cancelling.phase.as_str(), "cancelling");
    assert_eq!(cancelling.frozen_inputs_json, snapshot);
    assert_eq!(cancelling.base_sha, "deadbeef");
}

#[test]
fn tampered_frozen_inputs_still_admit_cancel_and_in_table_reject() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    host.dispatch_pending(&created.mission_id).unwrap();
    let frozen = host.snapshot(&created.mission_id).unwrap();
    host.tamper_attempt_base_sha(&created.mission_id, "deadbeef")
        .unwrap();
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    let cancelling = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(cancelling.phase.as_str(), "cancelling");
    assert_eq!(cancelling.frozen_inputs_json, frozen.frozen_inputs_json);
    assert_eq!(cancelling.base_sha, "deadbeef");

    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    host.dispatch_pending(&created.mission_id).unwrap();
    assert_eq!(
        notify(
            &mut host,
            &created.mission_id,
            plan_note("implement the ledger")
        ),
        crate::Phase::AwaitingPlanApproval
    );
    let frozen = host.snapshot(&created.mission_id).unwrap();
    host.tamper_attempt_base_sha(&created.mission_id, "cafebabe")
        .unwrap();
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::Reject {
            rationale: "inputs drifted".into(),
        },
    );
    let rejected = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(rejected.phase.as_str(), "rejected");
    assert_eq!(rejected.frozen_inputs_json, frozen.frozen_inputs_json);
    assert_eq!(rejected.base_sha, "cafebabe");
}

#[test]
fn planning_refuses_a_write_tool_through_the_profile() {
    let world = open_world();
    let mut host = crate::Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let mission = host.snapshot(&created.mission_id).unwrap();
    let request = ExecutionRequest {
        binding: RuntimeBinding {
            mission_id: mission.mission_id.clone(),
            attempt_id: mission.attempt_id.clone(),
            session_id: mission.session_id.clone(),
            generation: host.generation(),
        },
        tool_task_id: "plan-write".into(),
        effect_key: "write".into(),
        capability_scope: FAKE_WRITE_SCOPE.into(),
        deadline_ms: i64::MAX,
        operation: ExecutionOperation::WriteFile {
            path: "notes.txt".into(),
            content: "not during planning".into(),
        },
    };
    let error = host.execute_fake_operation(&request).unwrap_err();
    assert_eq!(error.category(), "guard_failed");
    assert!(
        error.to_string().contains("planning tools are read-only"),
        "{error}"
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase.as_str(),
        "planning"
    );
    assert!(
        host.snapshot(&created.mission_id)
            .unwrap()
            .execution_effects
            .is_empty()
    );
}
