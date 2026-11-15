//! #199a: workspace gate, evidence, and notify refusal. No verifier execution.
use super::fixtures::{
    admit, create, deliver, git, git_out, notify, open_world, pass_note, plan_note,
};
use crate::execution::RuntimeNotification;
use crate::persistence::RUST_VERIFY_JSON;
use crate::{CommandBody, CreateMission, Host, Phase, SuccessorChanges};
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
