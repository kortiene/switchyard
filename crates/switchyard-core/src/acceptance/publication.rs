use super::fixtures::{
    admit, admit_expect, create, deliver, drive_to_publishing, fingerprint, ledger_text, notify,
    open_world,
};
use crate::delivery::{publication_operation_id, publication_payload, publication_payload_hash};
use crate::execution::RuntimeNotification;
use crate::{CommandBody, Host, OperationState, Phase, PublicationProof};

#[test]
fn fake_runtime_drives_the_state_machine_without_credentials_or_git_writes() {
    let world = open_world();
    let before = fingerprint(&world.repo);
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let attempt = created.attempt_id.clone();
    let digest = drive_to_publishing(&mut host, &created.mission_id);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().attempt_id,
        attempt
    );
    assert_eq!(fingerprint(&world.repo), before);
    let published = notify(
        &mut host,
        &created.mission_id,
        RuntimeNotification::PublicationObserved {
            content_digest: digest.clone(),
            commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            tree_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            pr_id: "101".into(),
        },
    );
    assert_eq!(published, Phase::Published);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().attempt_id,
        attempt
    );
    assert_eq!(fingerprint(&world.repo), before);
    let terminal = admit_expect(&mut host, &created.mission_id, CommandBody::Cancel).unwrap_err();
    assert_eq!(terminal.category(), "terminal_immutable");
    let source_revision = host.snapshot(&created.mission_id).unwrap().revision;
    let successor = host
        .create_successor(&created.mission_id, "operator")
        .unwrap();
    assert_ne!(successor.mission_id, created.mission_id);
    assert_ne!(successor.attempt_id, attempt);
    assert_eq!(successor.phase, Phase::Draft);
    assert_eq!(
        successor.successor_of_mission_id.as_deref(),
        Some(created.mission_id.as_str())
    );
    let source = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(source.phase, Phase::Published);
    assert_eq!(source.revision, source_revision);
    let text = ledger_text(&world.db);
    assert!(
        text.contains("\"credentials\":\"none\"") || text.contains("\"credentials\": \"none\""),
        "runtime config must record that the fake adapter has no credentials"
    );
    assert!(!text.contains("ghp_"));
    assert!(!text.contains("sk-"));
}

#[test]
fn cancellation_during_publication_can_finish_or_discover_the_pr() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let digest = drive_to_publishing(&mut host, &created.mission_id);
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    let cancelling = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(
        cancelling.delivery.as_ref().unwrap().approved_digest,
        digest
    );
    assert_eq!(cancelling.phase, Phase::Cancelling);
    assert_eq!(
        cancelling.delivery.as_ref().unwrap().publication_state,
        "intent_recorded"
    );
    deliver(&mut host, &created.mission_id);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    );
    let cancelled = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(cancelled.phase, Phase::Cancelled);
    assert!(
        cancelled.delivery.is_some(),
        "created branch/commit intent is retained"
    );
    drop(host);

    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let digest = drive_to_publishing(&mut host, &created.mission_id);
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    let delivery_id = host
        .snapshot(&created.mission_id)
        .unwrap()
        .delivery
        .unwrap()
        .id;
    let proof = proof_for(&digest, "101");
    deliver(&mut host, &created.mission_id);
    host.record_operation(
        &created.mission_id,
        &publication_operation_id(&delivery_id).unwrap(),
        &publication_payload(&proof),
        OperationState::Completed,
    )
    .unwrap();
    let blocked = admit_expect(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    )
    .unwrap_err();
    assert_eq!(blocked.category(), "guard_failed");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Cancelling
    );
    assert_eq!(
        notify(
            &mut host,
            &created.mission_id,
            RuntimeNotification::PublicationObserved {
                content_digest: proof.content_digest.clone(),
                commit_sha: proof.commit_sha.clone(),
                tree_sha: proof.tree_sha.clone(),
                pr_id: proof.pr_id.clone(),
            }
        ),
        Phase::Published
    );
    let _ = publication_payload_hash(&proof).unwrap();
}

fn proof_for(digest: &str, pr_id: &str) -> PublicationProof {
    PublicationProof {
        content_digest: digest.into(),
        commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        tree_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        pr_id: pr_id.into(),
    }
}

#[test]
fn predecessor_pending_stop_cannot_enter_a_successors_outbox() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let source = create(&mut host, &world.repo, 2);
    let digest = drive_to_publishing(&mut host, &source.mission_id);
    let stop = admit(&mut host, &source.mission_id, CommandBody::Cancel);
    // A publication acknowledgement may win the race with a still-pending cancel.
    assert_eq!(
        notify(
            &mut host,
            &source.mission_id,
            RuntimeNotification::PublicationObserved {
                content_digest: digest,
                commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                tree_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                pr_id: "129".into(),
            }
        ),
        Phase::Published
    );
    let source_before = host.snapshot(&source.mission_id).unwrap().to_json();
    assert!(
        source_before["pendingCommands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == stop.command_id)
    );
    let successor = host
        .create_successor(&source.mission_id, "operator")
        .unwrap();
    assert_ne!(source.mission_id, successor.mission_id);
    assert_ne!(source.attempt_id, successor.attempt_id);
    let work = admit(&mut host, &successor.mission_id, CommandBody::StartPlanning);
    drop(host);
    let mut host = Host::open(&world.db).unwrap();
    assert_eq!(host.dispatch_reconciled(&successor.mission_id).unwrap(), 1);
    let next = host.snapshot(&successor.mission_id).unwrap();
    assert!(
        next.commands
            .iter()
            .all(|c| c.attempt_id == successor.attempt_id)
    );
    assert!(
        host.runtime_mut()
            .unwrap()
            .stored_receipt(&work.request_id)
            .is_some()
    );
    assert!(
        host.runtime_mut()
            .unwrap()
            .stored_receipt(&stop.request_id)
            .is_none()
    );
    let source_after = host.snapshot(&source.mission_id).unwrap().to_json();
    assert_eq!(source_after["commands"], source_before["commands"]);
    assert_eq!(source_after["revision"], source_before["revision"]);
    assert_eq!(source_after["phase"], "published");
}
