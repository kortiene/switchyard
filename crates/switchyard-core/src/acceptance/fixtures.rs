use crate::execution::RuntimeNotification;
use crate::policy::plan_digest_for;
use crate::{AdmitRequest, CommandBody, CommandOrigin, Error, Host, Phase};
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

pub(super) struct World {
    pub(super) _tmp: TempDir,
    pub(super) db: PathBuf,
    pub(super) repo: PathBuf,
}

pub(super) fn open_world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    World {
        db: tmp.path().join("mission.sqlite"),
        repo,
        _tmp: tmp,
    }
}

pub(super) fn init_repo(repo: &Path) {
    git(repo, &["init", "--initial-branch=main"]);
    git(repo, &["config", "user.email", "switchyard@example.com"]);
    git(repo, &["config", "user.name", "Switchyard"]);
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    git(repo, &["add", "README.md"]);
    git(repo, &["commit", "-m", "init"]);
}

pub(super) fn git(repo: &Path, args: &[&str]) {
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

pub(super) fn git_out(repo: &Path, args: &[&str]) -> String {
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
    String::from_utf8(output.stdout).unwrap()
}

pub(super) fn fingerprint(repo: &Path) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        git_out(repo, &["rev-parse", "HEAD"]),
        git_out(repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
        git_out(repo, &["status", "--porcelain=v1"]),
        git_out(repo, &["diff", "--cached", "--name-only"])
    )
}

pub(super) fn create(host: &mut Host, repo: &Path, max_repairs: u32) -> crate::MissionSnapshot {
    host.create_mission(crate::CreateMission {
        repo: repo.to_path_buf(),
        issue_number: 95,
        issue_title: "S1".into(),
        objective: "Establish the ledger".into(),
        acceptance: "Restart restores the same attempt".into(),
        actor: "operator".into(),
        max_repairs,
        max_plan_revisions: 3,
    })
    .unwrap()
}

pub(super) fn deliver(host: &mut Host, mission: &str) {
    host.dispatch_pending(mission).unwrap();
}

pub(super) fn admit(host: &mut Host, mission: &str, body: CommandBody) -> crate::AdmitResult {
    let snap = host.snapshot(mission).unwrap();
    host.admit(AdmitRequest {
        command_id: None,
        mission_id: mission.to_string(),
        attempt_id: None,
        expected_revision: snap.revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body,
    })
    .unwrap()
}

pub(super) fn admit_expect(host: &mut Host, mission: &str, body: CommandBody) -> Result<(), Error> {
    let snap = host.snapshot(mission).unwrap();
    host.admit(AdmitRequest {
        command_id: None,
        mission_id: mission.to_string(),
        attempt_id: None,
        expected_revision: snap.revision,
        actor: "operator".into(),
        origin: CommandOrigin::Operator,
        body,
    })
    .map(|_| ())
}

pub(super) fn notify(host: &mut Host, mission: &str, notification: RuntimeNotification) -> Phase {
    host.runtime_mut().push_notification(notification);
    let outcome = host.poll_runtime(mission).unwrap().pop().unwrap();
    outcome.phase()
}

pub(super) fn plan_note(plan: &str) -> RuntimeNotification {
    RuntimeNotification::PlanCompleted {
        plan: plan.into(),
        digest: plan_digest_for(plan).unwrap(),
        quiescent: true,
    }
}

pub(super) fn pass_note(digest: &str) -> RuntimeNotification {
    RuntimeNotification::VerificationPass {
        content_digest: digest.into(),
        evidence: "host observed a passing check".into(),
        requirements: vec!["Restart restores the same attempt".into()],
        completeness: "complete".into(),
    }
}

pub(super) fn fail_note(digest: &str) -> RuntimeNotification {
    RuntimeNotification::VerificationFail {
        content_digest: digest.into(),
        evidence: "assertion failed".into(),
        requirements: vec!["Restart restores the same attempt".into()],
        completeness: "complete".into(),
    }
}

pub(super) fn drive_to_publishing(host: &mut Host, mission: &str) -> String {
    admit(host, mission, CommandBody::StartPlanning);
    host.dispatch_pending(mission).unwrap();
    let plan = "implement the ledger";
    assert_eq!(
        notify(host, mission, plan_note(plan)),
        Phase::AwaitingPlanApproval
    );
    admit(
        host,
        mission,
        CommandBody::ApprovePlan {
            plan_digest: plan_digest_for(plan).unwrap(),
        },
    );
    assert_eq!(
        notify(
            host,
            mission,
            RuntimeNotification::ImplementationSettled { quiescent: true }
        ),
        Phase::Verifying
    );
    let digest = host.snapshot(mission).unwrap().content_digest;
    assert_eq!(
        notify(host, mission, pass_note(&digest)),
        Phase::AwaitingReview
    );
    admit(
        host,
        mission,
        CommandBody::ApproveReview {
            digest: digest.clone(),
            rationale: "matches the frozen acceptance".into(),
        },
    );
    admit(
        host,
        mission,
        CommandBody::Publish {
            target_branch: "switchyard/s1".into(),
            base_branch: "main".into(),
            authorized: true,
        },
    );
    assert_eq!(host.snapshot(mission).unwrap().phase, Phase::Publishing);
    digest
}

pub(super) fn ledger_text(db: &Path) -> String {
    let mut bytes = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{suffix}", db.display()));
        if let Ok(chunk) = std::fs::read(&path) {
            bytes.extend(chunk);
        }
    }
    let journal = crate::journal_path(db);
    if let Ok(chunk) = std::fs::read(journal) {
        bytes.extend(chunk);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

pub(super) fn command_payload(db: &Path, request_id: &str) -> String {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT payload_canonical FROM commands WHERE request_id = ?1",
        [request_id],
        |row| row.get(0),
    )
    .unwrap()
}
