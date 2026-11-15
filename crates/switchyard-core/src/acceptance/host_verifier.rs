//! #199a: changed-path gate, evidence, and notify refusal. No verifier execution.
use super::fixtures::{admit, create, git, git_out, notify, open_world, pass_note, plan_note};
use crate::execution::RuntimeNotification;
use crate::persistence::RUST_VERIFY_JSON;
use crate::{CommandBody, CreateMission, Host, Phase};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use std::path::Path;

#[test]
fn path_gate_records_fail_and_clean_gate_refuses_confinement() {
    assert!(crate::persistence::supported_verifier_documents().contains(&RUST_VERIFY_JSON));
    let world = open_world();
    std::fs::write(world.repo.join(".gitignore"), ".cargo/\n").unwrap();
    git(&world.repo, &["add", ".gitignore"]);
    git(&world.repo, &["commit", "-m", "ignore"]);
    let base = git_out(&world.repo, &["rev-parse", "HEAD"]);
    let base = base.trim();
    let mut host = Host::open(&world.db).unwrap();
    let created = host
        .create_mission_with_verifier(request(&world.repo), Some(RUST_VERIFY_JSON.into()))
        .unwrap();
    let id = created.mission_id.clone();
    to_verifying(&mut host, &id);
    let frozen = host.snapshot(&id).unwrap().frozen_inputs_json.unwrap();
    assert!(frozen.contains("switchyard.verifier.rust-verify.v1"));
    assert!(frozen.contains("\"executes\":true"));

    let over = "x\n".repeat(401);
    let cases = [
        ("scripts/rust-verify.sh", "#!/bin/sh\n", "outside", ""),
        ("Cargo.toml", "[package]\nname=\"x\"\n", "outside", "add"),
        (".cargo/config.toml", "offline=true\n", "outside", ""),
        ("docs/over.md", over.as_str(), "lines", "commit"),
        (
            "crates/switchyard/tests/cli_confined.rs",
            "fn d() {}\n",
            "denied",
            "",
        ),
    ];
    for (rel, body, class, git_op) in cases {
        write(&world.repo, rel, body);
        if git_op == "add" || git_op == "commit" {
            git(&world.repo, &["add", rel]);
        }
        if git_op == "commit" {
            git(&world.repo, &["commit", "-m", rel]);
        }
        let (snap, outcome) = host.verify_mission(&id).unwrap();
        assert_eq!((outcome.as_str(), snap.phase), ("fail", Phase::Verifying));
        let (document, provenance, completeness) = evidence(&world.db, &id).unwrap();
        let violation = if class == "lines" {
            "maxAddedLines:401".to_string()
        } else {
            format!("{class}:{rel}")
        };
        assert_eq!(completeness, "complete");
        assert_eq!(provenance, json!({"collector": "switchyard-host-verifier"}));
        assert_eq!(document["outcome"], "fail");
        assert_eq!(
            document["contentDigestBefore"],
            document["contentDigestAfter"]
        );
        assert_eq!(document["contentDigestBefore"], snap.content_digest);
        assert!(
            document["verifierDigest"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert_eq!(document["violations"], json!([violation]), "{rel}");
        git(&world.repo, &["reset", "--hard", base]);
        git(&world.repo, &["clean", "-fdx"]);
    }

    write(&world.repo, "docs/ok.md", "ok\n");
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
fn executes_false_notify_pass_is_still_admitted() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    to_verifying(&mut host, &created.mission_id);
    let digest = host.snapshot(&created.mission_id).unwrap().content_digest;
    assert_eq!(
        notify(&mut host, &created.mission_id, pass_note(&digest)),
        Phase::AwaitingReview
    );
    let (document, provenance, completeness) = evidence(&world.db, &created.mission_id).unwrap();
    assert_eq!(completeness, "complete");
    assert!(document.get("verifierDigest").is_none());
    assert_eq!(
        provenance,
        json!({"collector": "runtime-observation", "runtimeClaim": true})
    );
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
