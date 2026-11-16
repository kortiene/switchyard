//! #199a: workspace gate, evidence, and notify refusal. No verifier execution.
use super::fixtures::{
    admit, create, deliver, git, git_out, notify, open_world, pass_note, plan_note,
};
use crate::execution::RuntimeNotification;
use crate::persistence::RUST_VERIFY_JSON;
use crate::{AdmitRequest, CommandBody, CreateMission, Host, Phase, SuccessorChanges};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use std::path::Path;

#[test]
fn path_gate_uses_the_owned_workspace() {
    assert!(crate::persistence::supported_verifier_documents().contains(&RUST_VERIFY_JSON));
    let world = open_world();
    seed(&world.repo);
    let base = git_out(&world.repo, &["rev-parse", "HEAD"]);
    let base = base.trim();
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    let id = created.mission_id.clone();
    to_verifying(&mut host, &id);
    let tree = workspace(
        &mut host,
        &id,
        &created.attempt_id,
        &world._tmp.path().join("ws"),
    );
    let frozen = host.snapshot(&id).unwrap().frozen_inputs_json.unwrap();
    assert!(frozen.contains("switchyard.verifier.rust-verify.v1"));

    write(&world.repo, "scripts/rust-verify.sh", "#!/bin/sh\n");
    assert_eq!(
        host.verify_mission(&id).unwrap_err().category(),
        "verifier_confinement_unavailable"
    );
    assert_eq!(host.snapshot(&id).unwrap().phase, Phase::Verifying);

    let over = "x\n".repeat(401);
    let cases = [
        (
            "scripts/rust-verify.sh",
            "#!/bin/sh\n",
            "outside:scripts/rust-verify.sh",
            "file",
        ),
        (
            "Cargo.toml",
            "[package]\nname=\"x\"\n",
            "outside:Cargo.toml",
            "add",
        ),
        (
            ".cargo/config.toml",
            "offline=true\n",
            "outside:.cargo/config.toml",
            "file",
        ),
        ("docs/over.md", over.as_str(), "maxAddedLines:401", "file"),
        ("README.md", "hello\nchanged\n", "outside:README.md", "file"),
    ];
    for (rel, body, violation, kind) in cases {
        write(&tree, rel, body);
        if kind == "add" {
            git(&tree, &["add", rel]);
        }
        expect_fail(&mut host, &world.db, &id, violation);
        git(&tree, &["reset", "--hard", base]);
        git(&tree, &["clean", "-fdx"]);
    }

    std::fs::remove_file(tree.join("docs/f.bin")).unwrap();
    let deleted_bin = host.verify_mission(&id).unwrap_err();
    assert_ne!(deleted_bin.category(), "storage_failed", "{deleted_bin}");
    assert_eq!(deleted_bin.category(), "verifier_confinement_unavailable");
    git(&tree, &["reset", "--hard", base]);
    git(&tree, &["clean", "-fdx"]);

    std::fs::remove_file(tree.join("crates/switchyard/tests/cli_confined.rs")).unwrap();
    expect_fail(
        &mut host,
        &world.db,
        &id,
        "denied:crates/switchyard/tests/cli_confined.rs",
    );
    git(&tree, &["reset", "--hard", base]);
    git(&tree, &["clean", "-fdx"]);

    std::fs::create_dir_all(tree.join("crates/x/tests")).unwrap();
    git(&tree, &["mv", "src/lib.rs", "crates/x/tests/moved.rs"]);
    expect_fail(&mut host, &world.db, &id, "outside:src/lib.rs");
    git(&tree, &["reset", "--hard", base]);
    git(&tree, &["clean", "-fdx"]);

    write(&tree, "crates/switchyard/tests/counted.rs", &over);
    expect_fail(&mut host, &world.db, &id, "maxAddedLines:401");
    git(&tree, &["reset", "--hard", base]);
    git(&tree, &["clean", "-fdx"]);

    std::fs::create_dir_all(tree.join("docs")).unwrap();
    std::os::unix::fs::symlink("/dev/zero", tree.join("docs/zero")).unwrap();
    expect_fail(&mut host, &world.db, &id, "special:docs/zero");
    git(&tree, &["reset", "--hard", base]);
    git(&tree, &["clean", "-fdx"]);

    let outside = world._tmp.path().join("outside-box");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("f.bin"), "x\n".repeat(500)).unwrap();
    std::fs::remove_dir_all(tree.join("docs/box")).unwrap();
    std::os::unix::fs::symlink(&outside, tree.join("docs/box")).unwrap();
    let (snap, outcome) = host.verify_mission(&id).unwrap();
    assert_eq!((outcome.as_str(), snap.phase), ("fail", Phase::Verifying));
    let (document, _, _) = evidence(&world.db, &id).unwrap();
    let violations = document["violations"].as_array().unwrap();
    assert!(
        violations
            .iter()
            .any(|item| item == "special:docs/box/f.bin"),
        "{document}"
    );
    assert_eq!(document["addedLines"], json!(0), "{document}");
    git(&tree, &["reset", "--hard", base]);
    git(&tree, &["clean", "-fdx"]);

    for index in 0..50 {
        write(
            &tree,
            &format!("docs/batch/{index}.md"),
            &"line\n".repeat(10),
        );
    }
    expect_fail(&mut host, &world.db, &id, "maxAddedLines:401");
    git(&tree, &["reset", "--hard", base]);
    git(&tree, &["clean", "-fdx"]);

    git(&tree, &["update-index", "--assume-unchanged", "README.md"]);
    let hidden = host.verify_mission(&id).unwrap_err();
    assert_eq!(hidden.category(), "verifier_index_flags_set");
    assert!(
        hidden.to_string().contains("--no-assume-unchanged"),
        "{hidden}"
    );
    git(
        &tree,
        &["update-index", "--no-assume-unchanged", "README.md"],
    );

    write(&tree, "docs/ok.md", "ok\n");
    let prior = host.snapshot(&id).unwrap().content_digest;
    assert_eq!(
        host.verify_mission(&id).unwrap_err().category(),
        "verifier_confinement_unavailable"
    );
    let snap = host.snapshot(&id).unwrap();
    assert_eq!(snap.phase, Phase::Verifying);
    assert_ne!(snap.content_digest, prior);
    assert!(evidence(&world.db, &id).is_none());
    let fail = RuntimeNotification::VerificationFail {
        content_digest: snap.content_digest.clone(),
        evidence: "no".into(),
        requirements: vec![],
        completeness: "complete".into(),
    };
    for note in [pass_note(&snap.content_digest), fail] {
        assert_eq!(
            host.apply_notification(&id, note).unwrap_err().category(),
            "verification_not_host_run"
        );
        assert_eq!(host.snapshot(&id).unwrap().phase, Phase::Verifying);
    }
}

#[test]
fn verify_drift_records_frozen_input_drift_once_then_cancels() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    to_verifying(&mut host, &created.mission_id);
    let before = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(before.phase, Phase::Verifying);
    host.tamper_attempt_base_sha(&created.mission_id, "deadbeef")
        .unwrap();
    let first = host.verify_mission(&created.mission_id).unwrap_err();
    assert_eq!(first.category(), "frozen_input_drift");
    assert!(first.to_string().contains("drifted"), "{first}");
    let attention = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(attention.phase, Phase::NeedsAttention);
    assert_eq!(
        attention.attention_reason.as_deref(),
        Some("frozen_input_drift")
    );
    assert_eq!(attention.revision, before.revision + 1);
    assert_eq!(detect_uncertainty_count(&attention), 1);
    let again = host.verify_mission(&created.mission_id).unwrap_err();
    assert_eq!(again.category(), "frozen_input_drift");
    let still = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(still.phase, Phase::NeedsAttention);
    assert_eq!(still.revision, attention.revision);
    assert_eq!(detect_uncertainty_count(&still), 1);
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    deliver(&mut host, &created.mission_id);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    );
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Cancelled
    );
}

fn detect_uncertainty_count(snapshot: &crate::MissionSnapshot) -> usize {
    snapshot
        .commands
        .iter()
        .filter(|command| command.kind == "detect_uncertainty")
        .count()
}

#[test]
fn verify_refuses_a_blocked_execution_before_the_verifier_runs() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    to_verifying(&mut host, &created.mission_id);
    let _tree = workspace(
        &mut host,
        &created.mission_id,
        &created.attempt_id,
        &world._tmp.path().join("ws"),
    );
    let before = host.snapshot(&created.mission_id).unwrap();
    rusqlite::Connection::open(&world.db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, created_at
             ) VALUES ('op-v1:verify-blocked', ?1, ?2, ?3, 'sha256:row', '{}', 1, 'prepared', 1)",
            rusqlite::params![created.mission_id, created.attempt_id, before.session_id],
        )
        .unwrap();
    let error = host.verify_mission(&created.mission_id).unwrap_err();
    assert_eq!(error.category(), "execution_blocked", "{error}");
    assert!(evidence(&world.db, &created.mission_id).is_none());
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().content_digest,
        before.content_digest
    );
}

#[test]
fn verify_with_an_admitted_stop_stays_blocked_and_leaves_the_stop_pending() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    to_verifying(&mut host, &created.mission_id);
    let _tree = workspace(
        &mut host,
        &created.mission_id,
        &created.attempt_id,
        &world._tmp.path().join("ws"),
    );
    let before = host.snapshot(&created.mission_id).unwrap();
    rusqlite::Connection::open(&world.db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, created_at
             ) VALUES ('op-v1:verify-stop', ?1, ?2, ?3, 'sha256:row', '{}', 1, 'prepared', 1)",
            rusqlite::params![created.mission_id, created.attempt_id, before.session_id],
        )
        .unwrap();
    admit(&mut host, &created.mission_id, CommandBody::Pause);
    let error = host.verify_mission(&created.mission_id).unwrap_err();
    // Phase is pausing, so the phase gate wins over the execution block.
    assert_eq!(error.category(), "illegal_transition", "{error}");
    assert!(evidence(&world.db, &created.mission_id).is_none());
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().content_digest,
        before.content_digest
    );
    let pause = host
        .snapshot(&created.mission_id)
        .unwrap()
        .commands
        .into_iter()
        .find(|command| command.kind == "pause")
        .unwrap();
    assert_eq!(pause.state, "pending");
    assert!(pause.receipt_id.is_none());
}

#[test]
fn verify_without_a_workspace_is_uncertain() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    to_verifying(&mut host, &created.mission_id);
    let missing = host.verify_mission(&created.mission_id).unwrap_err();
    assert_eq!(missing.category(), "workspace_uncertain");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Verifying
    );
}

#[test]
fn successor_and_default_verifier_follow_the_allowlist() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 1);
    to_verifying(&mut host, &created.mission_id);
    let tree = workspace(
        &mut host,
        &created.mission_id,
        &created.attempt_id,
        &world._tmp.path().join("ws"),
    );
    let _ = tree;
    assert_eq!(
        host.verify_mission(&created.mission_id)
            .unwrap_err()
            .category(),
        "verifier_gate_unavailable"
    );
    admit(&mut host, &created.mission_id, CommandBody::Cancel);
    deliver(&mut host, &created.mission_id);
    admit(
        &mut host,
        &created.mission_id,
        CommandBody::ConfirmCancelled,
    );
    let next = host
        .create_successor_with(
            &created.mission_id,
            "operator",
            &SuccessorChanges {
                verifier_json: Some(RUST_VERIFY_JSON.into()),
                ..SuccessorChanges::default()
            },
        )
        .unwrap();
    assert!(
        next.verifier_json
            .contains("switchyard.verifier.rust-verify.v1")
    );
}

#[test]
fn require_pass_refuses_a_runtime_complete_pass_after_a_host_pass_row() {
    let world = open_world();
    seed(&world.repo);
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    to_verifying(&mut host, &created.mission_id);
    let _tree = workspace(
        &mut host,
        &created.mission_id,
        &created.attempt_id,
        &world._tmp.path().join("ws"),
    );
    let snap = host.snapshot(&created.mission_id).unwrap();
    insert_pass(
        &world.db,
        &created.mission_id,
        &created.attempt_id,
        &snap.content_digest,
        "runtime-observation",
    );
    let refused = host
        .admit(AdmitRequest {
            command_id: None,
            mission_id: created.mission_id.clone(),
            attempt_id: Some(created.attempt_id.clone()),
            expected_revision: snap.revision,
            actor: "runtime".into(),
            origin: crate::CommandOrigin::RuntimeObservation,
            body: CommandBody::CompletePass,
        })
        .unwrap_err();
    assert_eq!(refused.category(), "guard_failed", "{refused}");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Verifying
    );

    rusqlite::Connection::open(&world.db)
        .unwrap()
        .execute(
            "UPDATE evidence SET provenance_json = '{\"collector\":\"switchyard-host-verifier\"}'
             WHERE mission_id = ?1",
            [&created.mission_id],
        )
        .unwrap();
    let snap = host.snapshot(&created.mission_id).unwrap();
    let still = host
        .admit(AdmitRequest {
            command_id: None,
            mission_id: created.mission_id.clone(),
            attempt_id: Some(created.attempt_id.clone()),
            expected_revision: snap.revision,
            actor: "runtime".into(),
            origin: crate::CommandOrigin::RuntimeObservation,
            body: CommandBody::CompletePass,
        })
        .unwrap_err();
    assert_eq!(still.category(), "verification_not_host_run", "{still}");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Verifying
    );
}

#[test]
fn plan_digest_mismatch_is_refused_before_the_sandbox_starts() {
    let world = open_world();
    seed(&world.repo);
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    to_verifying(&mut host, &created.mission_id);
    let _tree = workspace(
        &mut host,
        &created.mission_id,
        &created.attempt_id,
        &world._tmp.path().join("ws"),
    );
    let changed = rusqlite::Connection::open(&world.db)
        .unwrap()
        .execute(
            "UPDATE attempts SET plan_digest = 'sha256:not-the-plan' WHERE id = ?1",
            [&created.attempt_id],
        )
        .unwrap();
    assert_eq!(changed, 1);
    host.synthetic_verifier = Some(crate::service::SyntheticVerifier::Pass);
    let first = host.verify_mission(&created.mission_id).unwrap_err();
    assert_eq!(first.category(), "guard_failed", "{first}");
    assert!(first.to_string().contains("plan digest"), "{first}");
    let attention = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(attention.phase, Phase::NeedsAttention);
    assert_eq!(
        attention.attention_reason.as_deref(),
        Some("plan_digest_mismatch")
    );
    assert!(evidence(&world.db, &created.mission_id).is_none());
    assert!(host.synthetic_verifier.is_some());
    let again = host.verify_mission(&created.mission_id).unwrap_err();
    assert_eq!(again.category(), "guard_failed", "{again}");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().revision,
        attention.revision
    );
}

#[test]
fn digest_mismatch_timeout_and_truncated_output_enter_needs_attention() {
    let world = open_world();
    seed(&world.repo);
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    to_verifying(&mut host, &created.mission_id);
    let tree = workspace(
        &mut host,
        &created.mission_id,
        &created.attempt_id,
        &world._tmp.path().join("ws"),
    );
    host.verifier_between_digests = Some(write_tamper);
    let (snap, outcome) = host.verify_mission(&created.mission_id).unwrap();
    assert_eq!(outcome, "incomplete");
    assert_eq!(snap.phase, Phase::NeedsAttention);
    assert_eq!(
        snap.attention_reason.as_deref(),
        Some("verification_incomplete")
    );
    let (document, _, completeness) = evidence(&world.db, &created.mission_id).unwrap();
    assert_eq!(completeness, "incomplete");
    assert_ne!(
        document["contentDigestBefore"], document["contentDigestAfter"],
        "{document}"
    );
    let _ = tree;

    for (kind, label) in [
        (crate::service::SyntheticVerifier::Timeout, "timeout"),
        (crate::service::SyntheticVerifier::Truncated, "truncated"),
    ] {
        let world = open_world();
        seed(&world.repo);
        let mut host = Host::open(&world.db).unwrap();
        let created = host
            .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
            .unwrap();
        to_verifying(&mut host, &created.mission_id);
        let _tree = workspace(
            &mut host,
            &created.mission_id,
            &created.attempt_id,
            &world._tmp.path().join("ws"),
        );
        host.synthetic_verifier = Some(kind);
        let (snap, outcome) = host.verify_mission(&created.mission_id).unwrap();
        assert_eq!(
            (label, outcome.as_str(), snap.phase),
            (label, "incomplete", Phase::NeedsAttention),
            "{label}"
        );
        let (document, provenance, completeness) =
            evidence(&world.db, &created.mission_id).unwrap();
        assert_eq!(completeness, "incomplete", "{label} {document}");
        assert_eq!(document["outcome"], "incomplete", "{label}");
        assert_eq!(
            provenance["collector"], "switchyard-host-verifier",
            "{label}"
        );
        assert_eq!(snap.repair_count, 0, "{label}");
    }
}

#[test]
fn a_missing_result_signal_empty_hash_or_oom_is_incomplete_and_uses_no_repair() {
    for kind in [
        crate::service::SyntheticVerifier::MissingResult,
        crate::service::SyntheticVerifier::Signal,
        crate::service::SyntheticVerifier::EmptyHash,
        crate::service::SyntheticVerifier::Oom,
    ] {
        let world = open_world();
        seed(&world.repo);
        let mut host = Host::open(&world.db).unwrap();
        let created = host
            .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
            .unwrap();
        to_verifying(&mut host, &created.mission_id);
        let _tree = workspace(
            &mut host,
            &created.mission_id,
            &created.attempt_id,
            &world._tmp.path().join("ws"),
        );
        host.synthetic_verifier = Some(kind);
        let (snap, outcome) = host.verify_mission(&created.mission_id).unwrap();
        assert_eq!(outcome, "incomplete", "{kind:?} {snap:?}");
        assert_eq!(snap.phase, Phase::NeedsAttention);
        assert_eq!(snap.repair_count, 0);
    }
}

#[test]
fn a_failing_test_that_prints_a_resolution_marker_still_records_fail() {
    let world = open_world();
    seed(&world.repo);
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    to_verifying(&mut host, &created.mission_id);
    let _tree = workspace(
        &mut host,
        &created.mission_id,
        &created.attempt_id,
        &world._tmp.path().join("ws"),
    );
    host.synthetic_verifier = Some(crate::service::SyntheticVerifier::ResolutionMarker);
    let (snap, outcome) = host.verify_mission(&created.mission_id).unwrap();
    assert_eq!(outcome, "fail");
    assert_eq!(snap.phase, Phase::Verifying);
    assert_eq!(snap.repair_count, 0);
}

#[test]
fn a_corrupted_verifier_json_refuses_complete_pass() {
    let world = open_world();
    seed(&world.repo);
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    to_verifying(&mut host, &created.mission_id);
    let _tree = workspace(
        &mut host,
        &created.mission_id,
        &created.attempt_id,
        &world._tmp.path().join("ws"),
    );
    rusqlite::Connection::open(&world.db)
        .unwrap()
        .execute(
            "UPDATE attempts SET verifier_json = '{' WHERE id = ?1",
            [&created.attempt_id],
        )
        .unwrap();
    let snap = host.snapshot(&created.mission_id).unwrap();
    insert_pass(
        &world.db,
        &created.mission_id,
        &created.attempt_id,
        &snap.content_digest,
        "runtime-observation",
    );
    let refused = host
        .admit(AdmitRequest {
            command_id: None,
            mission_id: created.mission_id.clone(),
            attempt_id: Some(created.attempt_id.clone()),
            expected_revision: snap.revision,
            actor: "runtime".into(),
            origin: crate::CommandOrigin::RuntimeObservation,
            body: CommandBody::CompletePass,
        })
        .unwrap_err();
    assert_eq!(refused.category(), "guard_failed", "{refused}");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Verifying
    );
}

#[test]
fn a_file_over_one_mebibyte_without_a_newline_is_max_bytes() {
    let world = open_world();
    seed(&world.repo);
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    to_verifying(&mut host, &created.mission_id);
    let tree = workspace(
        &mut host,
        &created.mission_id,
        &created.attempt_id,
        &world._tmp.path().join("ws"),
    );
    let mut body = vec![b'a'; 1024 * 1024 + 1];
    assert!(!body.contains(&b'\n'));
    let path = tree.join("docs/wide.bin");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &body).unwrap();
    body.clear();
    expect_fail(
        &mut host,
        &world.db,
        &created.mission_id,
        "maxBytes:docs/wide.bin",
    );
}

#[test]
fn more_than_4096_changed_paths_is_a_complete_fail() {
    let world = open_world();
    seed(&world.repo);
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    to_verifying(&mut host, &created.mission_id);
    let tree = workspace(
        &mut host,
        &created.mission_id,
        &created.attempt_id,
        &world._tmp.path().join("ws"),
    );
    let count = 4097;
    for index in 0..count {
        write(&tree, &format!("docs/many/{index}.md"), "");
    }
    let (snap, outcome) = host.verify_mission(&created.mission_id).unwrap();
    assert_eq!((outcome.as_str(), snap.phase), ("fail", Phase::Verifying));
    let (document, _, completeness) = evidence(&world.db, &created.mission_id).unwrap();
    assert_eq!(completeness, "complete");
    let violations = document["violations"].as_array().unwrap();
    assert!(
        violations.iter().any(|item| item == "maxChangedPaths:4097"),
        "{document}"
    );
    assert_eq!(document["changedPaths"].as_array().unwrap().len(), 4096);
    assert_eq!(document["changedPathCount"], json!(4097));
}

#[test]
fn an_unknown_execution_kind_stays_blocked_and_is_never_skipped() {
    let world = open_world();
    seed(&world.repo);
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    to_verifying(&mut host, &created.mission_id);
    let _tree = workspace(
        &mut host,
        &created.mission_id,
        &created.attempt_id,
        &world._tmp.path().join("ws"),
    );
    let snap = host.snapshot(&created.mission_id).unwrap();
    rusqlite::Connection::open(&world.db)
        .unwrap()
        .execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, created_at
             ) VALUES ('op-v1:0123456789abcdef0123456789abcdefaaaaaaaa', ?1, ?2, ?3, 'sha256:row', '{\"kind\":\"nope\"}', 1, 'prepared', 1)",
            rusqlite::params![created.mission_id, created.attempt_id, snap.session_id],
        )
        .unwrap();
    let error = host.verify_mission(&created.mission_id).unwrap_err();
    assert_eq!(error.category(), "execution_blocked", "{error}");
    let state = effect_state(&world.db, "op-v1:0123456789abcdef0123456789abcdefaaaaaaaa");
    assert_eq!(state, "uncertain", "{state}");
    let recovered = host.recover(&created.mission_id).unwrap_err();
    assert_eq!(recovered.category(), "execution_blocked", "{recovered}");
    let state = effect_state(&world.db, "op-v1:0123456789abcdef0123456789abcdefaaaaaaaa");
    assert_eq!(state, "uncertain");
    assert_ne!(state, "completed");
    assert_ne!(state, "not_applied");
}

fn write_tamper(tree: &Path) {
    write(tree, "docs/tamper.md", "changed\n");
}

fn insert_pass(db: &Path, mission: &str, attempt: &str, digest: &str, collector: &str) {
    let provenance = format!("{{\"collector\":\"{collector}\"}}");
    rusqlite::Connection::open(db)
        .unwrap()
        .execute(
            "INSERT INTO evidence(
                id, mission_id, attempt_id, content_hash, completeness, provenance_json,
                body_json, invalidated, created_at
             ) VALUES ('e-pass', ?1, ?2, ?3, 'complete', ?4, '{\"body\":\"\"}', 0, 1)",
            rusqlite::params![mission, attempt, digest, provenance],
        )
        .unwrap();
    rusqlite::Connection::open(db)
        .unwrap()
        .execute(
            "INSERT INTO verifications(
                id, mission_id, attempt_id, seq, evidence_id, outcome, requirements_json,
                content_digest, invalidated, created_at
             ) VALUES ('v-pass', ?1, ?2, 1, 'e-pass', 'pass', '[]', ?3, 0, 1)",
            rusqlite::params![mission, attempt, digest],
        )
        .unwrap();
}

fn effect_state(db: &Path, operation: &str) -> String {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT state FROM linux_execution_effects WHERE operation_id = ?1",
            [operation],
            |row| row.get(0),
        )
        .unwrap()
}

fn seed(repo: &Path) {
    std::fs::write(repo.join(".gitignore"), ".cargo/\n").unwrap();
    write(repo, "src/lib.rs", "fn kept() {}\n");
    write(
        repo,
        "crates/switchyard/tests/cli_confined.rs",
        "fn denied() {}\n",
    );
    write(
        repo,
        "crates/switchyard/tests/counted.rs",
        "fn small() {}\n",
    );
    write(repo, "crates/switchyard/tests/.gitattributes", "* -diff\n");
    write(repo, "docs/.gitattributes", "*.bin binary\n");
    write(repo, "docs/f.bin", "binary\n");
    write(repo, "docs/box/f.bin", "old\n");
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", "gate fixtures"]);
}

fn request(repo: &Path) -> CreateMission {
    CreateMission {
        repo: repo.to_path_buf(),
        issue_number: 199,
        issue_title: "v".into(),
        objective: "Gate the diff".into(),
        acceptance: "Refuse unconfined execution".into(),
        actor: "operator".into(),
        max_repairs: 2,
        max_plan_revisions: 3,
    }
}

fn to_verifying(host: &mut Host, mission: &str) {
    admit(host, mission, CommandBody::StartPlanning);
    host.dispatch_pending(mission).unwrap();
    notify(host, mission, plan_note("plan"));
    let digest = host.snapshot(mission).unwrap().plan_digest.unwrap();
    admit(
        host,
        mission,
        CommandBody::ApprovePlan {
            plan_digest: digest,
        },
    );
    notify(
        host,
        mission,
        RuntimeNotification::ImplementationSettled { quiescent: true },
    );
}

fn workspace(host: &mut Host, mission: &str, attempt: &str, root: &Path) -> std::path::PathBuf {
    std::fs::create_dir_all(root).unwrap();
    let view = host.prepare_workspace(mission, attempt, root).unwrap();
    view.path.into()
}

fn expect_fail(host: &mut Host, db: &Path, mission: &str, violation: &str) {
    let (snap, outcome) = host.verify_mission(mission).unwrap();
    assert_eq!((outcome.as_str(), snap.phase), ("fail", Phase::Verifying));
    let (document, provenance, completeness) = evidence(db, mission).unwrap();
    assert_eq!(completeness, "complete");
    assert_eq!(provenance["collector"], "switchyard-host-verifier");
    assert_eq!(document["outcome"], "fail");
    assert_eq!(
        document["contentDigestBefore"],
        document["contentDigestAfter"]
    );
    assert_eq!(document["contentDigestBefore"], snap.content_digest);
    assert_eq!(document["baseSha"], snap.base_sha);
    assert!(
        document["verifierDigest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(document["violations"], json!([violation]), "{document}");
}

fn evidence(db: &Path, mission: &str) -> Option<(Value, Value, String)> {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT e.body_json, e.provenance_json, e.completeness FROM verifications v
         JOIN evidence e ON e.id = v.evidence_id
         WHERE v.mission_id = ?1 AND v.invalidated = 0 ORDER BY v.seq DESC LIMIT 1",
        [mission],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .unwrap()
    .map(
        |(body, provenance, completeness): (String, String, String)| {
            (
                serde_json::from_str(&body).unwrap(),
                serde_json::from_str(&provenance).unwrap(),
                completeness,
            )
        },
    )
}

fn write(repo: &Path, rel: &str, body: &str) {
    let path = repo.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}
