//! `mission plan --file` admits `plan_completed` through the notify path.
use super::fixtures::{World, admit, create, open_world};
use crate::{CommandBody, Host, Phase};
use serde_json::json;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

fn planning() -> (World, String) {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning);
    let mission = created.mission_id;
    drop(host);
    (world, mission)
}

fn copy_ledger(from: &Path, to: &Path) {
    fs::copy(from, to).unwrap();
    for suffix in ["-wal", "-shm"] {
        let src = sibling(from, suffix);
        if src.exists() {
            fs::copy(&src, sibling(to, suffix)).unwrap();
        }
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

struct Admitted {
    db: PathBuf,
    digest: String,
    body: String,
    payload_hash: String,
    origin: String,
    actor: String,
}

fn admit_pair(text: &str) -> (World, Admitted, Admitted) {
    let (world, mission) = planning();
    let notify_db = world.db.with_file_name("notify.sqlite");
    copy_ledger(&world.db, &notify_db);
    let file = world._tmp.path().join("plan.txt");
    fs::write(&file, text).unwrap();

    let mut notify_host = Host::open(&notify_db).unwrap();
    notify_host
        .apply_notification_json(
            &mission,
            &json!({
                "kind": "plan_completed",
                "plan": text,
                "quiescent": true,
            }),
        )
        .unwrap();
    let mut file_host = Host::open(&world.db).unwrap();
    file_host.apply_plan_file(&mission, &file, true).unwrap();
    let filed = admitted(&file_host, &world.db, &mission);
    let notified = admitted(&notify_host, &notify_db, &mission);
    (world, filed, notified)
}

fn admitted(host: &Host, db: &Path, mission: &str) -> Admitted {
    let snap = host.snapshot(mission).unwrap();
    let command = snap
        .commands
        .iter()
        .find(|command| command.kind == "plan_completed")
        .unwrap();
    let actor = host
        .decisions(mission)
        .unwrap()
        .into_iter()
        .find(|decision| decision.action == "plan_completed")
        .unwrap()
        .actor;
    Admitted {
        db: db.to_path_buf(),
        digest: snap.plan_digest.unwrap(),
        body: snap.plan_body.unwrap(),
        payload_hash: command.payload_hash.clone(),
        origin: command.origin.clone(),
        actor,
    }
}

#[test]
fn plan_file_digest_equals_notify_json_for_identical_text() {
    for text in ["implement the ledger", "implement the ledger\n", "  keep  "] {
        let (_world, filed, notified) = admit_pair(text);
        assert_eq!(filed.digest, notified.digest, "{text:?}");
        assert_eq!(filed.body, text);
        assert_eq!(notified.body, text);
        assert_eq!(filed.payload_hash, notified.payload_hash);
        assert_eq!(filed.origin, "runtime_observation");
        assert_eq!(notified.origin, "runtime_observation");
        assert!(filed.digest.starts_with("sha256:"));
    }
}

#[test]
fn plan_file_keeps_a_trailing_newline_notify_would_keep() {
    let text = "implement the ledger\n";
    let (world, mission) = planning();
    let stripped_db = world.db.with_file_name("stripped.sqlite");
    copy_ledger(&world.db, &stripped_db);
    let file = world._tmp.path().join("plan.txt");
    fs::write(&file, text).unwrap();
    let mut file_host = Host::open(&world.db).unwrap();
    file_host.apply_plan_file(&mission, &file, true).unwrap();
    let mut stripped_host = Host::open(&stripped_db).unwrap();
    stripped_host
        .apply_notification_json(
            &mission,
            &json!({
                "kind": "plan_completed",
                "plan": "implement the ledger",
                "quiescent": true,
            }),
        )
        .unwrap();
    let filed = file_host.snapshot(&mission).unwrap();
    let stripped = stripped_host.snapshot(&mission).unwrap();
    assert_eq!(filed.plan_body.as_deref(), Some(text));
    assert_ne!(filed.plan_digest, stripped.plan_digest);
}

#[test]
fn plan_file_records_actor_operator_plan_file() {
    let (_world, filed, notified) = admit_pair("implement the ledger");
    assert_eq!(filed.actor, "operator-plan-file");
    assert_eq!(notified.actor, "runtime-observation");
    assert_eq!(filed.payload_hash, notified.payload_hash);
    assert_eq!(filed.digest, notified.digest);

    let export = Host::export_document(&filed.db, &mission_of(&filed.db), "test").unwrap();
    assert_eq!(export["ledgerSchemaVersion"], 12);
    let decision = export["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|decision| decision["action"] == "plan_completed")
        .unwrap();
    assert_eq!(decision["actor"], "operator-plan-file");
    let command = export["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["kind"] == "plan_completed")
        .unwrap();
    assert_eq!(command["origin"], "runtime_observation");
    assert_eq!(command["payloadHash"], filed.payload_hash);
    let payload = command["payloadCanonical"].as_str().unwrap();
    assert!(!payload.contains("operator-plan-file"));
    assert!(!payload.contains("runtime-observation"));
    let version: i64 = rusqlite::Connection::open(&filed.db)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 12);
}

fn mission_of(db: &Path) -> String {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row("SELECT id FROM missions", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn exactly_64_kib_is_admitted_and_one_extra_byte_is_refused() {
    let exact = vec![b'a'; crate::linux_execution::MAX_FILE_BYTES];
    let (world, mission) = planning();
    let path = world._tmp.path().join("exact.txt");
    fs::write(&path, &exact).unwrap();
    let mut host = Host::open(&world.db).unwrap();
    let before = host.snapshot(&mission).unwrap();
    host.apply_plan_file(&mission, &path, true).unwrap();
    let stored = host.snapshot(&mission).unwrap();
    assert_eq!(stored.phase, Phase::AwaitingPlanApproval);
    assert_eq!(
        stored.plan_body.as_deref(),
        Some(std::str::from_utf8(&exact).unwrap())
    );
    assert_eq!(
        stored.plan_body.as_deref().unwrap().len(),
        crate::linux_execution::MAX_FILE_BYTES
    );
    assert!(stored.plan_digest.is_some());
    assert!(stored.revision > before.revision);

    let over = vec![b'b'; crate::linux_execution::MAX_FILE_BYTES + 1];
    let (world, mission) = planning();
    let path = world._tmp.path().join("over.txt");
    fs::write(&path, &over).unwrap();
    let mut host = Host::open(&world.db).unwrap();
    let before = fingerprint(&host, &mission);
    let error = host.apply_plan_file(&mission, &path, true).unwrap_err();
    assert_eq!(error.category(), "guard_failed");
    assert!(
        error.to_string().contains("shorten the plan and rerun"),
        "{error}"
    );
    assert_eq!(fingerprint(&host, &mission), before);
    assert_ne!(before.phase, Phase::NeedsAttention);
}

#[test]
fn each_plan_file_refusal_leaves_state_unchanged() {
    let (world, mission) = planning();
    let dir = world._tmp.path();
    let readable = dir.join("readable.txt");
    fs::write(&readable, "secret plan").unwrap();
    let link = dir.join("linked.txt");
    symlink(&readable, &link).unwrap();
    let folder = dir.join("folder");
    fs::create_dir(&folder).unwrap();
    let unreadable = dir.join("unreadable.txt");
    fs::write(&unreadable, "hidden plan").unwrap();
    let mut mode = fs::metadata(&unreadable).unwrap().permissions();
    mode.set_mode(0o000);
    fs::set_permissions(&unreadable, mode).unwrap();
    // Root can read a mode-000 file. Skipping that case keeps the later
    // refusals on an unchanged planning mission.
    let unreadable_is_refused =
        crate::regular_file::read_regular_nofollow(&unreadable, 64).is_err();
    fs::write(dir.join("empty.txt"), "").unwrap();
    fs::write(dir.join("blank.txt"), " \t\r\n").unwrap();
    fs::write(dir.join("newline.txt"), "\n").unwrap();
    fs::write(dir.join("binary.txt"), [0xff, 0xfe]).unwrap();
    fs::write(
        dir.join("huge.txt"),
        vec![b'c'; crate::linux_execution::MAX_FILE_BYTES + 1],
    )
    .unwrap();

    let cases = [
        (dir.join("missing.txt"), "fix the path and rerun"),
        (link, "fix the path and rerun"),
        (folder, "fix the path and rerun"),
        (dir.join("empty.txt"), "write the plan and rerun"),
        (dir.join("blank.txt"), "write the plan and rerun"),
        (dir.join("newline.txt"), "write the plan and rerun"),
        (dir.join("binary.txt"), "re-encode and rerun"),
        (dir.join("huge.txt"), "shorten the plan and rerun"),
    ];

    let mut host = Host::open(&world.db).unwrap();
    let before = fingerprint(&host, &mission);
    assert_eq!(before.phase, Phase::Planning);
    for (path, way_out) in cases {
        let error = host
            .apply_plan_file(&mission, &path, true)
            .expect_err(&path.display().to_string());
        assert_eq!(error.category(), "guard_failed", "{path:?} {error}");
        assert!(
            error.to_string().contains(way_out),
            "{path:?} {error} should tell the operator to {way_out}"
        );
        assert_eq!(fingerprint(&host, &mission), before, "{path:?}");
        assert_ne!(fingerprint(&host, &mission).phase, Phase::NeedsAttention);
    }
    if unreadable_is_refused {
        let error = host
            .apply_plan_file(&mission, &unreadable, true)
            .expect_err("mode 000");
        assert_eq!(error.category(), "storage_failed", "{error}");
        assert!(
            error.to_string().contains("storage failed"),
            "the storage cause is kept: {error}"
        );
        assert_eq!(fingerprint(&host, &mission), before);
    }
}

#[test]
fn plan_file_follows_a_symlinked_parent_and_keeps_bytes() {
    let (world, mission) = planning();
    let real = world._tmp.path().join("real");
    fs::create_dir(&real).unwrap();
    let text = "via parent link\n";
    fs::write(real.join("plan.txt"), text).unwrap();
    symlink(&real, world._tmp.path().join("link")).unwrap();
    let mut host = Host::open(&world.db).unwrap();
    host.apply_plan_file(
        &mission,
        &world._tmp.path().join("link").join("plan.txt"),
        true,
    )
    .unwrap();
    let stored = host.snapshot(&mission).unwrap();
    assert_eq!(stored.plan_body.as_deref(), Some(text));
    assert_eq!(stored.phase, Phase::AwaitingPlanApproval);
}

#[test]
fn plan_file_false_quiescent_and_wrong_phase_leave_state() {
    let (world, mission) = planning();
    let path = world._tmp.path().join("plan.txt");
    fs::write(&path, "implement the ledger").unwrap();
    let mut host = Host::open(&world.db).unwrap();
    let before = fingerprint(&host, &mission);
    let error = host.apply_plan_file(&mission, &path, false).unwrap_err();
    assert_eq!(error.category(), "guard_failed");
    assert!(error.to_string().contains("not quiescent"), "{error}");
    assert_eq!(fingerprint(&host, &mission), before);

    host.apply_plan_file(&mission, &path, true).unwrap();
    let admitted = fingerprint(&host, &mission);
    assert_eq!(admitted.phase, Phase::AwaitingPlanApproval);
    host.apply_plan_file(&mission, &path, true).unwrap();
    assert_eq!(fingerprint(&host, &mission), admitted);

    let other = world._tmp.path().join("other.txt");
    fs::write(&other, "a different plan").unwrap();
    let error = host.apply_plan_file(&mission, &other, true).unwrap_err();
    assert_eq!(error.category(), "illegal_transition");
    assert_eq!(fingerprint(&host, &mission), admitted);
}

#[test]
fn plan_file_drift_is_refused_before_quiescent_policy() {
    let (world, mission) = planning();
    let path = world._tmp.path().join("plan.txt");
    fs::write(&path, "implement the ledger").unwrap();
    let mut host = Host::open(&world.db).unwrap();
    host.tamper_attempt_base_sha(&mission, "deadbeef").unwrap();
    let before_revision = host.snapshot(&mission).unwrap().revision;
    // quiescent false would be a policy refusal if it ran first. Drift is earlier.
    let error = host.apply_plan_file(&mission, &path, false).unwrap_err();
    assert_eq!(error.category(), "frozen_input_drift", "{error}");
    let attention = host.snapshot(&mission).unwrap();
    assert_eq!(attention.phase, Phase::NeedsAttention);
    assert_eq!(
        attention.attention_reason.as_deref(),
        Some("frozen_input_drift")
    );
    assert!(attention.plan_body.is_none());
    assert!(attention.plan_digest.is_none());
    assert!(attention.revision > before_revision);
    assert!(
        attention
            .commands
            .iter()
            .all(|command| command.kind != "plan_completed")
    );
}

#[test]
fn plan_file_then_approve_plan_binds_the_host_digest() {
    let (world, mission) = planning();
    let text = "implement the ledger\n";
    let path = world._tmp.path().join("plan.txt");
    fs::write(&path, text).unwrap();
    let mut host = Host::open(&world.db).unwrap();
    host.apply_plan_file(&mission, &path, true).unwrap();
    let stored = host.snapshot(&mission).unwrap();
    let digest = stored.plan_digest.clone().unwrap();
    assert_eq!(stored.plan_body.as_deref(), Some(text));
    assert_eq!(stored.phase, Phase::AwaitingPlanApproval);
    let wrong = admit_result(
        &mut host,
        &mission,
        CommandBody::ApprovePlan {
            plan_digest: "sha256:not-the-host-digest".into(),
        },
    );
    assert_eq!(wrong.unwrap_err().category(), "stale_plan_digest");
    let still = host.snapshot(&mission).unwrap();
    assert_eq!(still.phase, Phase::AwaitingPlanApproval);
    assert_eq!(still.plan_digest.as_deref(), Some(digest.as_str()));
    assert_eq!(still.revision, stored.revision);
    admit(
        &mut host,
        &mission,
        CommandBody::ApprovePlan {
            plan_digest: digest.clone(),
        },
    );
    let implementing = host.snapshot(&mission).unwrap();
    assert_eq!(implementing.phase, Phase::Implementing);
    assert_eq!(implementing.plan_digest.as_deref(), Some(digest.as_str()));
    assert_eq!(implementing.plan_body.as_deref(), Some(text));
}

fn admit_result(host: &mut Host, mission: &str, body: CommandBody) -> crate::Result<()> {
    let snap = host.snapshot(mission).unwrap();
    host.admit(crate::AdmitRequest {
        command_id: None,
        mission_id: mission.to_string(),
        attempt_id: None,
        expected_revision: snap.revision,
        actor: "operator".into(),
        origin: crate::CommandOrigin::Operator,
        body,
    })
    .map(|_| ())
}

#[test]
#[cfg(feature = "pi-contract")]
fn plan_file_refused_off_fake() {
    // The default build's only runtime is Fake, so this refusal runs only with pi-contract.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "--initial-branch=main"]);
    git(&["config", "user.email", "switchyard@example.com"]);
    git(&["config", "user.name", "Switchyard"]);
    git(&["commit", "--allow-empty", "-m", "init"]);
    let db = tmp.path().join("mission.sqlite");
    let node = Path::new("/bin/true");
    let node = if node.exists() {
        node
    } else {
        Path::new("/usr/bin/true")
    };
    let mut host = Host::open_pi_probe(&db, node).unwrap();
    let created = host
        .create_mission(crate::CreateMission {
            repo,
            issue_number: 197,
            issue_title: "plan".into(),
            objective: "off fake".into(),
            acceptance: "fake only".into(),
            actor: "operator".into(),
            max_repairs: 1,
            max_plan_revisions: 1,
        })
        .unwrap();
    let before = host.snapshot(&created.mission_id).unwrap();
    // A missing file would be a path refusal if the read ran first.
    let missing = tmp.path().join("missing-plan.txt");
    let error = host
        .apply_plan_file(&created.mission_id, &missing, true)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("fake observations cannot certify Pi mission facts"),
        "{error}"
    );
    assert_eq!(error.category(), "guard_failed");
    let after = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(after.phase, before.phase);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.plan_body, before.plan_body);
    assert_eq!(after.plan_digest, before.plan_digest);
    assert_eq!(after.commands.len(), before.commands.len());
    assert!(!missing.exists());
}

#[derive(PartialEq, Debug)]
struct Fingerprint {
    phase: Phase,
    revision: u64,
    plan_digest: Option<String>,
    plan_body: Option<String>,
    commands: usize,
    decisions: usize,
}

fn fingerprint(host: &Host, mission: &str) -> Fingerprint {
    let snap = host.snapshot(mission).unwrap();
    Fingerprint {
        phase: snap.phase,
        revision: snap.revision,
        plan_digest: snap.plan_digest,
        plan_body: snap.plan_body,
        commands: snap.commands.len(),
        decisions: host.decisions(mission).unwrap().len(),
    }
}
