//! Pause and resume stay refused after frozen-input drift.
//!
//! Operator-authored. These tests are not runtime output.

use std::path::{Path, PathBuf};
use std::process::Command;

use switchyard_core::{AdmitRequest, CommandBody, CommandOrigin, Error, Host, Phase};
use tempfile::TempDir;

struct World {
    _tmp: TempDir,
    db: PathBuf,
    host: Host,
    mission_id: String,
    attempt_id: String,
    frozen_inputs_json: String,
    freeze_generation: Option<u64>,
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn open_frozen() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.email", "switchyard@example.com"]);
    git(&repo, &["config", "user.name", "Switchyard"]);
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-m", "init"]);

    let db = tmp.path().join("mission.sqlite");
    let mut host = Host::open(&db).unwrap();
    let created = host
        .create_mission(switchyard_core::CreateMission {
            repo,
            issue_number: 193,
            issue_title: "pause/resume refused after drift".into(),
            objective: "pause/resume refused after drift; test-only".into(),
            acceptance: "Pause refused; stays on the drift exit path\n\
                         Resume refused; does not re-freeze or clear drift"
                .into(),
            actor: "operator".into(),
            max_repairs: 2,
            max_plan_revisions: 2,
        })
        .unwrap();
    admit(&mut host, &created.mission_id, CommandBody::StartPlanning).unwrap();
    host.dispatch_pending(&created.mission_id).unwrap();
    let frozen = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(frozen.phase, Phase::Planning);
    assert!(frozen.inputs_frozen);
    let frozen_inputs_json = frozen.frozen_inputs_json.clone().unwrap();

    World {
        _tmp: tmp,
        db,
        host,
        mission_id: created.mission_id,
        attempt_id: created.attempt_id,
        frozen_inputs_json,
        freeze_generation: frozen.attempt_owner_generation,
    }
}

fn tamper_base_sha(db: &Path, attempt_id: &str) {
    let changed = rusqlite::Connection::open(db)
        .unwrap()
        .execute(
            "UPDATE attempts SET base_sha = 'deadbeef' WHERE id = ?1",
            [attempt_id],
        )
        .unwrap();
    assert_eq!(changed, 1);
}

fn open_drifted() -> World {
    let world = open_frozen();
    tamper_base_sha(&world.db, &world.attempt_id);
    world
}

fn pause_until_paused(host: &mut Host, mission: &str) {
    admit(host, mission, CommandBody::Pause).unwrap();
    host.dispatch_pending(mission).unwrap();
    admit(host, mission, CommandBody::ConfirmStopped).unwrap();
    assert_eq!(host.snapshot(mission).unwrap().phase, Phase::Paused);
}

fn admit(host: &mut Host, mission: &str, body: CommandBody) -> Result<(), Error> {
    let revision = host.snapshot(mission).unwrap().revision;
    host.admit(AdmitRequest {
        command_id: None,
        mission_id: mission.to_string(),
        attempt_id: None,
        expected_revision: revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body,
    })
    .map(|_| ())
}

fn assert_drift_refusal(error: &Error) {
    assert_eq!(error.category(), "guard_failed");
    assert!(
        error.to_string().contains("drifted"),
        "refusal should name drift, got {error}"
    );
}

fn assert_drift_held(
    host: &Host,
    mission: &str,
    frozen_inputs_json: &str,
    freeze_generation: Option<u64>,
) {
    let drifted = host.snapshot(mission).unwrap();
    assert_eq!(drifted.phase, Phase::NeedsAttention);
    assert_eq!(
        drifted.attention_reason.as_deref(),
        Some("frozen_input_drift")
    );
    assert_eq!(drifted.base_sha, "deadbeef");
    assert!(drifted.inputs_frozen);
    assert_eq!(
        drifted.frozen_inputs_json.as_deref(),
        Some(frozen_inputs_json)
    );
    assert_eq!(drifted.attempt_owner_generation, freeze_generation);
    assert!(
        drifted
            .commands
            .iter()
            .all(|command| command.kind != "resume"),
        "resume must not be applied"
    );
}

#[test]
fn pause_is_refused_after_drift_and_the_mission_stays_on_the_drift_exit_path() {
    let World {
        mut host,
        mission_id,
        frozen_inputs_json,
        ..
    } = open_drifted();

    let refused = admit(&mut host, &mission_id, CommandBody::Pause).unwrap_err();
    assert_drift_refusal(&refused);

    let drifted = host.snapshot(&mission_id).unwrap();
    assert_eq!(drifted.phase, Phase::NeedsAttention);
    assert_eq!(
        drifted.attention_reason.as_deref(),
        Some("frozen_input_drift")
    );
    assert_eq!(drifted.base_sha, "deadbeef");
    assert_eq!(
        drifted.frozen_inputs_json.as_deref(),
        Some(frozen_inputs_json.as_str())
    );
    assert!(drifted.inputs_frozen);
    assert!(
        drifted
            .commands
            .iter()
            .all(|command| command.kind != "pause"),
        "pause must not be applied"
    );
    assert!(
        drifted
            .commands
            .iter()
            .any(|command| command.kind == "detect_uncertainty"),
        "the host records the drift exit"
    );

    let revision = drifted.revision;
    let again = admit(&mut host, &mission_id, CommandBody::Pause).unwrap_err();
    assert_drift_refusal(&again);
    let still = host.snapshot(&mission_id).unwrap();
    assert_eq!(still.phase, Phase::NeedsAttention);
    assert_eq!(
        still.attention_reason.as_deref(),
        Some("frozen_input_drift")
    );
    assert_eq!(still.revision, revision);
    assert_eq!(
        still.frozen_inputs_json.as_deref(),
        Some(frozen_inputs_json.as_str())
    );
    assert!(still.commands.iter().all(|command| command.kind != "pause"));
}

#[test]
fn resume_is_refused_and_does_not_re_freeze_or_clear_the_drift() {
    let World {
        mut host,
        mission_id,
        frozen_inputs_json,
        freeze_generation,
        ..
    } = open_drifted();

    let refused = admit(
        &mut host,
        &mission_id,
        CommandBody::Resume { authorized: true },
    )
    .unwrap_err();
    assert_drift_refusal(&refused);
    assert_drift_held(&host, &mission_id, &frozen_inputs_json, freeze_generation);

    let drifted = host.snapshot(&mission_id).unwrap();
    let revision = drifted.revision;
    let uncertainty = drifted
        .commands
        .iter()
        .filter(|command| command.kind == "detect_uncertainty")
        .count();
    let again = admit(
        &mut host,
        &mission_id,
        CommandBody::Resume { authorized: true },
    )
    .unwrap_err();
    assert_drift_refusal(&again);
    let still = host.snapshot(&mission_id).unwrap();
    assert_eq!(still.phase, Phase::NeedsAttention);
    assert_eq!(still.revision, revision);
    assert_eq!(still.attempt_owner_generation, freeze_generation);
    assert_eq!(
        still.frozen_inputs_json.as_deref(),
        Some(frozen_inputs_json.as_str())
    );
    assert_eq!(still.base_sha, "deadbeef");
    assert_eq!(
        still
            .commands
            .iter()
            .filter(|command| command.kind == "detect_uncertainty")
            .count(),
        uncertainty
    );
    assert!(
        still
            .commands
            .iter()
            .all(|command| command.kind != "resume")
    );
}

#[test]
fn resume_after_pause_then_drift_is_refused_and_stays_on_the_drift_exit_path() {
    let World {
        _tmp,
        db,
        mut host,
        mission_id,
        attempt_id,
        frozen_inputs_json,
        freeze_generation,
    } = open_frozen();
    let _keep_ledger = _tmp;
    pause_until_paused(&mut host, &mission_id);
    let paused = host.snapshot(&mission_id).unwrap();
    assert_eq!(paused.phase, Phase::Paused);
    assert_eq!(
        paused.frozen_inputs_json.as_deref(),
        Some(frozen_inputs_json.as_str())
    );
    assert_eq!(paused.attempt_owner_generation, freeze_generation);

    tamper_base_sha(&db, &attempt_id);
    let refused = admit(
        &mut host,
        &mission_id,
        CommandBody::Resume { authorized: true },
    )
    .unwrap_err();
    assert_drift_refusal(&refused);
    assert_drift_held(&host, &mission_id, &frozen_inputs_json, freeze_generation);

    let drifted = host.snapshot(&mission_id).unwrap();
    let revision = drifted.revision;
    let again = admit(
        &mut host,
        &mission_id,
        CommandBody::Resume { authorized: true },
    )
    .unwrap_err();
    assert_drift_refusal(&again);
    let still = host.snapshot(&mission_id).unwrap();
    assert_eq!(still.phase, Phase::NeedsAttention);
    assert_eq!(
        still.attention_reason.as_deref(),
        Some("frozen_input_drift")
    );
    assert_eq!(still.revision, revision);
    assert_eq!(still.base_sha, "deadbeef");
    assert_eq!(still.attempt_owner_generation, freeze_generation);
    assert_eq!(
        still.frozen_inputs_json.as_deref(),
        Some(frozen_inputs_json.as_str())
    );
    assert!(
        still
            .commands
            .iter()
            .all(|command| command.kind != "resume")
    );
}
