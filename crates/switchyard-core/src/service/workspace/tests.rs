use super::*;
use rusqlite::Connection;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct World {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    root: PathBuf,
    db: PathBuf,
}

fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let root = dir.path().join("workspaces");
    fs::create_dir(&repo).unwrap();
    fs::create_dir(&root).unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.email", "workspace@example.com"]);
    git(&repo, &["config", "user.name", "Workspace"]);
    fs::write(repo.join("README.md"), "frozen\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-m", "base"]);
    World {
        db: dir.path().join("mission.sqlite"),
        _dir: dir,
        repo,
        root,
    }
}

fn mission(h: &mut Host, w: &World) -> MissionSnapshot {
    h.create_mission(CreateMission {
        repo: w.repo.clone(),
        issue_number: 118,
        issue_title: "workspace".into(),
        objective: "owned workspace".into(),
        acceptance: "same identity after restart".into(),
        actor: "operator".into(),
        max_repairs: 1,
        max_plan_revisions: 1,
    })
    .unwrap()
}

fn prepare(h: &mut Host, m: &MissionSnapshot, w: &World) -> WorkspaceView {
    h.prepare_workspace(&m.mission_id, &m.attempt_id, &w.root)
        .unwrap()
}

fn count(w: &World) -> i64 {
    Connection::open(&w.db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM workspaces", [], |r| r.get(0))
        .unwrap()
}

/// `mission command` opens a host and dispatches before any explicit recover.
/// A prepared workspace must reconcile in that new owner generation.
#[test]
fn command_dispatch_reconciles_a_ready_workspace_in_the_new_generation() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    let view = prepare(&mut h, &m, &w);
    assert_eq!(view.state, "ready");
    assert_eq!(view.verified_generation, Some(h.generation()));
    drop(h);

    let mut h = Host::open(&w.db).unwrap();
    assert!(h.generation() > view.verified_generation.unwrap());
    assert_eq!(h.dispatch_pending(&m.mission_id).unwrap(), 0);
    let reconciled = h.snapshot(&m.mission_id).unwrap().workspace.unwrap();
    assert_eq!(reconciled.id, view.id);
    assert_eq!(reconciled.path, view.path);
    assert_eq!(reconciled.state, "ready");
    assert_eq!(reconciled.verified_generation, Some(h.generation()));
    let admitted = h
        .admit(AdmitRequest {
            command_id: None,
            mission_id: m.mission_id.clone(),
            attempt_id: Some(m.attempt_id.clone()),
            expected_revision: m.revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body: CommandBody::StartPlanning,
        })
        .unwrap();
    assert_eq!(admitted.state, CommandState::Pending);
    assert_eq!(admitted.phase, Phase::Planning);
}

#[test]
fn dirty_source_and_frozen_base_survive_idempotent_workspace_reopen() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    fs::write(w.repo.join("later.txt"), "later commit\n").unwrap();
    git(&w.repo, &["add", "later.txt"]);
    git(&w.repo, &["commit", "-m", "source advances"]);
    fs::write(w.repo.join("README.md"), "staged\n").unwrap();
    git(&w.repo, &["add", "README.md"]);
    fs::write(w.repo.join("README.md"), "unstaged\n").unwrap();
    fs::write(w.repo.join("untracked.txt"), "untracked\n").unwrap();
    let before = [
        git(&w.repo, &["rev-parse", "HEAD"]),
        git(&w.repo, &["symbolic-ref", "HEAD"]),
        fs::read(w.repo.join(".git/index")).unwrap(),
        fs::read(w.repo.join("README.md")).unwrap(),
        fs::read(w.repo.join("untracked.txt")).unwrap(),
    ];
    let view = prepare(&mut h, &m, &w);
    let tree = Path::new(&view.path);
    assert_eq!(view.state, "ready");
    assert_eq!(
        fs::read_to_string(tree.join("README.md")).unwrap(),
        "frozen\n"
    );
    assert!(!tree.join("later.txt").exists());
    assert_eq!(
        git(tree, &["rev-parse", "HEAD"]),
        format!("{}\n", m.base_sha).as_bytes()
    );
    assert_eq!(prepare(&mut h, &m, &w), view);
    assert_eq!(count(&w), 1);
    assert_eq!(
        before,
        [
            git(&w.repo, &["rev-parse", "HEAD"]),
            git(&w.repo, &["symbolic-ref", "HEAD"]),
            fs::read(w.repo.join(".git/index")).unwrap(),
            fs::read(w.repo.join("README.md")).unwrap(),
            fs::read(w.repo.join("untracked.txt")).unwrap()
        ]
    );
    fs::write(tree.join("README.md"), "retained edits\n").unwrap();
    fs::write(tree.join("notes.txt"), "retained untracked\n").unwrap();
    let generation = h.generation();
    drop(h);
    let inspected = Host::inspect(&w.db, Some(&m.mission_id)).unwrap();
    assert_eq!(inspected[0].workspace.as_ref().unwrap(), &view);
    let mut h = Host::open(&w.db).unwrap();
    assert!(h.generation() > generation);
    assert_eq!(h.dispatch_pending(&m.mission_id).unwrap(), 0);
    assert_eq!(
        h.snapshot(&m.mission_id)
            .unwrap()
            .workspace
            .unwrap()
            .verified_generation,
        Some(h.generation())
    );
    let recovered = h.recover(&m.mission_id).unwrap().workspace.unwrap();
    assert_eq!(recovered.id, view.id);
    assert_eq!(recovered.path, view.path);
    assert_eq!(recovered.verified_generation, Some(h.generation()));
    assert_eq!(
        fs::read_to_string(tree.join("README.md")).unwrap(),
        "retained edits\n"
    );
    assert_eq!(
        fs::read_to_string(tree.join("notes.txt")).unwrap(),
        "retained untracked\n"
    );
    assert_eq!(count(&w), 1);
}

#[cfg(unix)]
#[test]
fn roots_aliases_existing_slots_and_attempt_collisions_are_rejected() {
    use std::os::unix::fs::symlink;
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    let alias = w._dir.path().join("alias");
    symlink(&w.root, &alias).unwrap();
    for root in [
        alias,
        w.root.join(".."),
        w.repo.clone(),
        w.repo.join(".git"),
    ] {
        assert!(
            h.prepare_workspace(&m.mission_id, &m.attempt_id, &root)
                .is_err()
        );
    }
    assert_eq!(count(&w), 0);
    let slot = w.root.join(&m.attempt_id);
    fs::create_dir(&slot).unwrap();
    fs::write(slot.join("foreign.txt"), "do not adopt").unwrap();
    assert!(
        h.prepare_workspace(&m.mission_id, &m.attempt_id, &w.root)
            .is_err()
    );
    assert_eq!(count(&w), 0);
    assert_eq!(
        fs::read_to_string(slot.join("foreign.txt")).unwrap(),
        "do not adopt"
    );
    fs::rename(&slot, w.root.join("foreign-retained")).unwrap();
    let first = prepare(&mut h, &m, &w);
    assert!(
        h.prepare_workspace(&m.mission_id, "a-other", &w.root)
            .is_err()
    );
    let other_root = w._dir.path().join("other-root");
    fs::create_dir(&other_root).unwrap();
    assert!(
        h.prepare_workspace(&m.mission_id, &m.attempt_id, &other_root)
            .is_err()
    );
    Connection::open(&w.db)
        .unwrap()
        .execute(
            "UPDATE missions SET phase='cancelled' WHERE id=?1",
            [&m.mission_id],
        )
        .unwrap();
    let successor = h.create_successor(&m.mission_id, "operator").unwrap();
    assert!(
        h.prepare_workspace(&successor.mission_id, &m.attempt_id, &w.root)
            .is_err()
    );
    let second = prepare(&mut h, &successor, &w);
    assert_ne!(first.id, second.id);
    assert_ne!(first.path, second.path);
    assert!(Path::new(&first.path).join("README.md").is_file());
}

#[test]
fn stale_generation_never_reserves_or_changes_a_workspace() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    Connection::open(&w.db)
        .unwrap()
        .execute("UPDATE host_owner SET generation=generation+1", [])
        .unwrap();
    let error = h
        .prepare_workspace(&m.mission_id, &m.attempt_id, &w.root)
        .unwrap_err();
    assert_eq!(error.category(), "stale_generation");
    assert_eq!(count(&w), 0);
    assert_eq!(fs::read_dir(&w.root).unwrap().count(), 0);
    drop(h);
    let mut h = Host::open(&w.db).unwrap();
    let before = prepare(&mut h, &m, &w);
    Connection::open(&w.db)
        .unwrap()
        .execute("UPDATE host_owner SET generation=generation+1", [])
        .unwrap();
    assert_eq!(
        h.recover_workspace(&m.mission_id).unwrap_err().category(),
        "stale_generation"
    );
    assert_eq!(
        h.snapshot(&m.mission_id).unwrap().workspace.unwrap(),
        before
    );
}

#[test]
fn a_copied_ledger_cannot_adopt_a_workspace_through_a_different_owner_lock() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    let original = prepare(&mut h, &m, &w);
    let copy = w._dir.path().join("copied.sqlite");
    Connection::open(&w.db)
        .unwrap()
        .execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
        .unwrap();
    let mut other = Host::open(&copy).unwrap();
    assert!(other.recover_workspace(&m.mission_id).is_err());
    assert_eq!(
        h.snapshot(&m.mission_id).unwrap().workspace.unwrap(),
        original
    );
    assert_eq!(prepare(&mut h, &m, &w), original);
}

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn worker(w: &World, m: &MissionSnapshot, stage: &str, path: Option<&str>) -> Worker {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "service::workspace::tests::crash_worker",
            "--nocapture",
        ])
        .env("SW_WORKSPACE_DB", &w.db)
        .env("SW_WORKSPACE_ROOT", &w.root)
        .env("SW_WORKSPACE_MISSION", &m.mission_id)
        .env("SW_WORKSPACE_ATTEMPT", &m.attempt_id)
        .env("SW_WORKSPACE_STAGE", stage)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    Worker(command.spawn().unwrap())
}

fn crash_at(w: &World, m: &MissionSnapshot, stage: &str) {
    let mut worker = worker(w, m, stage, None);
    let stdout = worker.0.stdout.take().unwrap();
    let (send, recv) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if line.unwrap().contains("WORKSPACE-BARRIER") {
                let _ = send.send(());
                return;
            }
        }
    });
    recv.recv_timeout(Duration::from_secs(20))
        .expect("worker did not reach crash boundary");
    worker.0.kill().unwrap();
    worker.0.wait().unwrap();
}

#[test]
fn crash_worker() {
    let Some(db) = std::env::var_os("SW_WORKSPACE_DB") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("SW_WORKSPACE_ROOT").unwrap());
    let mission = std::env::var("SW_WORKSPACE_MISSION").unwrap();
    let attempt = std::env::var("SW_WORKSPACE_ATTEMPT").unwrap();
    let stage = std::env::var("SW_WORKSPACE_STAGE").unwrap();
    let mut h = Host::open(Path::new(&db)).unwrap();
    h.prepare_workspace_at(&mission, &attempt, &root, |point| {
        if point == stage {
            println!("WORKSPACE-BARRIER");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
        Ok(())
    })
    .unwrap();
}

#[test]
fn sigkill_before_creation_and_after_git_ack_recovers_one_workspace() {
    for stage in ["reserved", "after_git"] {
        let w = world();
        let mut h = Host::open(&w.db).unwrap();
        let m = mission(&mut h, &w);
        drop(h);
        crash_at(&w, &m, stage);
        let recorded = Host::inspect(&w.db, Some(&m.mission_id))
            .unwrap()
            .remove(0)
            .workspace
            .unwrap();
        let mut h = Host::open(&w.db).unwrap();
        let recovered = h.recover(&m.mission_id).unwrap().workspace.unwrap();
        assert_eq!(recorded.id, recovered.id);
        assert_eq!(recorded.path, recovered.path);
        assert_eq!(recovered.base_sha, m.base_sha);
        assert_eq!(recovered.state, "ready");
        assert_eq!(count(&w), 1);
        assert_eq!(
            String::from_utf8(git(&w.repo, &["worktree", "list", "--porcelain"]))
                .unwrap()
                .matches("worktree ")
                .count(),
            2
        );
    }
}

#[test]
fn ambiguous_or_partial_creation_retains_evidence_and_never_repeats_git() {
    for stage in ["before_git", "after_git"] {
        let w = world();
        let mut h = Host::open(&w.db).unwrap();
        let m = mission(&mut h, &w);
        drop(h);
        crash_at(&w, &m, stage);
        let slot = w.root.join(&m.attempt_id);
        let marker = fs::read(slot.join("owner.json")).unwrap();
        if stage == "after_git" {
            fs::remove_file(slot.join("tree/README.md")).unwrap();
        }
        let mut h = Host::open(&w.db).unwrap();
        assert_eq!(
            h.recover_workspace(&m.mission_id).unwrap_err().category(),
            "workspace_uncertain"
        );
        assert_eq!(
            h.snapshot(&m.mission_id).unwrap().workspace.unwrap().state,
            "uncertain"
        );
        assert!(h.dispatch_pending(&m.mission_id).is_err());
        assert_eq!(
            h.recover_workspace(&m.mission_id).unwrap_err().category(),
            "workspace_uncertain"
        );
        assert_eq!(fs::read(slot.join("owner.json")).unwrap(), marker);
        assert!(!slot.join("tree/README.md").exists());
        assert_eq!(count(&w), 1);
    }
}

#[test]
fn missing_or_rebound_metadata_is_quarantined_without_cleanup() {
    for defect in ["marker", "gitlink", "lock-reason", "head", "tree", "root"] {
        let w = world();
        let mut h = Host::open(&w.db).unwrap();
        let m = mission(&mut h, &w);
        let view = prepare(&mut h, &m, &w);
        let record = h.store.load_workspace(&m.attempt_id).unwrap().unwrap();
        let admin = Path::new(record.git_dir.as_ref().unwrap());
        match defect {
            "marker" => fs::remove_file(record.slot().join("owner.json")).unwrap(),
            "gitlink" => fs::remove_file(Path::new(&view.path).join(".git")).unwrap(),
            "lock-reason" => fs::write(admin.join("locked"), "another owner\n").unwrap(),
            "head" => fs::write(admin.join("HEAD"), "ref: refs/heads/main\n").unwrap(),
            "tree" => fs::rename(&view.path, record.slot().join("retained-tree")).unwrap(),
            "root" => fs::rename(&w.root, w._dir.path().join("retained-root")).unwrap(),
            _ => unreachable!(),
        }
        let error = h.recover_workspace(&m.mission_id).unwrap_err();
        assert_eq!(error.category(), "workspace_uncertain", "{defect}: {error}");
        assert_eq!(
            h.snapshot(&m.mission_id).unwrap().workspace.unwrap().state,
            "uncertain"
        );
        assert!(admin.is_dir(), "Git evidence was removed");
        assert_eq!(count(&w), 1);
    }
}

#[cfg(unix)]
#[test]
fn checkout_does_not_run_repository_hooks_filters_or_fsmonitor() {
    use std::os::unix::fs::PermissionsExt;
    let w = world();
    fs::write(
        w.repo.join(".gitattributes"),
        "*.md filter=external\nconditional.txt filter=late\n",
    )
    .unwrap();
    fs::write(w.repo.join("conditional.txt"), "no filter execution\n").unwrap();
    git(&w.repo, &["add", ".gitattributes", "conditional.txt"]);
    git(&w.repo, &["commit", "-m", "attributes"]);
    let sentinel = w._dir.path().join("executed");
    let hook = w._dir.path().join("hook");
    fs::write(
        &hook,
        format!("#!/bin/sh\ntouch '{}'\n", sentinel.display()),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let hooks = w._dir.path().join("hooks");
    fs::create_dir(&hooks).unwrap();
    fs::copy(&hook, hooks.join("post-checkout")).unwrap();
    git(
        &w.repo,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    git(
        &w.repo,
        &["config", "core.fsmonitor", hook.to_str().unwrap()],
    );
    for name in [
        "filter.external.clean",
        "filter.external.smudge",
        "filter.external.process",
    ] {
        git(&w.repo, &["config", name, hook.to_str().unwrap()]);
    }
    git(&w.repo, &["config", "filter.external.required", "true"]);
    let conditional = w._dir.path().join("conditional.gitconfig");
    fs::write(
        &conditional,
        format!(
            "[filter \"late\"]\n smudge = {}\n process = {}\n clean = {}\n required = true\n",
            hook.display(),
            hook.display(),
            hook.display()
        ),
    )
    .unwrap();
    let key = format!("includeIf.gitdir:{}/.git/worktrees/.path", w.repo.display());
    git(&w.repo, &["config", &key, conditional.to_str().unwrap()]);
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    let workspace = prepare(&mut h, &m, &w);
    assert_eq!(
        fs::read_to_string(Path::new(&workspace.path).join("README.md")).unwrap(),
        "frozen\n"
    );
    assert!(!sentinel.exists());
    assert_eq!(
        fs::read_to_string(Path::new(&workspace.path).join("conditional.txt")).unwrap(),
        "no filter execution\n"
    );
}

#[cfg(unix)]
#[test]
fn surviving_git_keeps_creation_lease_after_host_sigkill() {
    use std::os::unix::fs::PermissionsExt;
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    drop(h);
    let tools = w._dir.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let barrier = w._dir.path().join("git-started");
    let release = w._dir.path().join("git-release");
    let wrapper = tools.join("git");
    fs::write(&wrapper, format!("#!/bin/sh\ncase \" $* \" in\n*\" reset --hard \"*)\n touch '{}'\n n=0\n while [ ! -f '{}' ]; do\n  n=$((n+1)); [ $n -lt 400 ] || exit 99\n  sleep 0.05\n done\n ;;\nesac\nexec /usr/bin/git \"$@\"\n", barrier.display(), release.display())).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", tools.display(), std::env::var("PATH").unwrap());
    let mut worker = worker(&w, &m, "never", Some(&path));
    let deadline = Instant::now() + Duration::from_secs(15);
    while !barrier.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    worker.0.kill().unwrap();
    worker.0.wait().unwrap();
    let mut h = Host::open(&w.db).unwrap();
    assert_eq!(
        h.recover_workspace(&m.mission_id).unwrap_err().category(),
        "workspace_busy"
    );
    assert_eq!(
        h.snapshot(&m.mission_id).unwrap().workspace.unwrap().state,
        "creating"
    );
    fs::write(release, "finish").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match h.recover_workspace(&m.mission_id) {
            Ok(Some(view)) => {
                assert_eq!(view.state, "ready");
                break;
            }
            Err(Error::WorkspaceBusy { .. }) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            other => panic!("creation did not reconcile: {other:?}"),
        }
    }
    assert_eq!(count(&w), 1);
}

#[test]
fn v2_ledger_is_inspectable_then_migrates_without_creating_workspaces() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    drop(h);
    let db = Connection::open(&w.db).unwrap();
    db.execute_batch(
        "DROP TRIGGER IF EXISTS supersession_command_unique;
         DROP TRIGGER IF EXISTS execution_supersessions_no_update;
         DROP TRIGGER IF EXISTS execution_supersessions_no_delete;
         DROP TRIGGER IF EXISTS execution_resolutions_no_update;
         DROP TRIGGER IF EXISTS execution_resolutions_no_delete;
         DROP TRIGGER IF EXISTS linux_resolutions_no_update;
         DROP TRIGGER IF EXISTS linux_resolutions_no_delete;
         DROP TABLE IF EXISTS execution_supersessions;
         DROP TRIGGER IF EXISTS linux_effect_identity_unique;
         DROP TRIGGER IF EXISTS simulated_effect_identity_unique;
         DROP TRIGGER IF EXISTS linux_resolution_command_unique;
         DROP TRIGGER IF EXISTS simulated_resolution_command_unique;
         DROP TABLE IF EXISTS linux_execution_resolutions;
         DROP TABLE IF EXISTS linux_execution_effects;
         DROP TABLE execution_resolutions;
         DROP TABLE execution_effects;
         DROP TABLE workspaces;
         DELETE FROM schema_migrations WHERE version>=3;
         PRAGMA user_version=2;",
    )
    .unwrap();
    assert!(
        Host::inspect(&w.db, Some(&m.mission_id)).unwrap()[0]
            .workspace
            .is_none()
    );
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    let mut h = Host::open(&w.db).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        9
    );
    assert_eq!(h.snapshot(&m.mission_id).unwrap().base_sha, m.base_sha);
    assert!(h.recover(&m.mission_id).unwrap().workspace.is_none());
    assert_eq!(count(&w), 0);
    assert_eq!(fs::read_dir(&w.root).unwrap().count(), 0);
}

fn sha(repo: &Path) -> String {
    String::from_utf8(git(repo, &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_string()
}

fn worktree_count(repo: &Path) -> usize {
    String::from_utf8(git(repo, &["worktree", "list", "--porcelain"]))
        .unwrap()
        .matches("worktree ")
        .count()
}

#[test]
fn ready_head_may_descend_from_the_frozen_base() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    let view = prepare(&mut h, &m, &w);
    let tree = Path::new(&view.path);
    git(tree, &["commit", "--allow-empty", "-m", "agent"]);
    let head = sha(tree);
    assert_ne!(head, m.base_sha);
    drop(h);
    let mut h = Host::open(&w.db).unwrap();
    let reconciled = h.recover_workspace(&m.mission_id).unwrap().unwrap();
    assert_eq!(reconciled.state, "ready");
    assert_eq!(reconciled.id, view.id);
    assert_eq!(reconciled.base_sha, m.base_sha);
    assert_eq!(sha(tree), head);
    git(tree, &["commit", "--allow-empty", "-m", "again"]);
    let later = sha(tree);
    assert_eq!(
        h.prepare_workspace(&m.mission_id, &m.attempt_id, &w.root)
            .unwrap()
            .state,
        "ready"
    );
    assert_eq!(sha(tree), later);
    assert_eq!(
        h.snapshot(&m.mission_id)
            .unwrap()
            .workspace
            .unwrap()
            .base_sha,
        m.base_sha
    );
}

#[test]
fn ready_head_outside_the_frozen_history_is_uncertain() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    let view = prepare(&mut h, &m, &w);
    git(&w.repo, &["checkout", "--orphan", "unrelated"]);
    fs::write(w.repo.join("side.txt"), "side\n").unwrap();
    git(&w.repo, &["add", "side.txt"]);
    git(&w.repo, &["commit", "-m", "unrelated"]);
    let unrelated = sha(&w.repo);
    git(&w.repo, &["checkout", "main"]);
    git(Path::new(&view.path), &["reset", "--hard", &unrelated]);
    let error = h.recover_workspace(&m.mission_id).unwrap_err();
    assert_eq!(error.category(), "workspace_uncertain", "{error}");
    let recorded = h.snapshot(&m.mission_id).unwrap().workspace.unwrap();
    assert_eq!(recorded.state, "uncertain");
    assert_eq!(recorded.id, view.id);
    assert!(Path::new(&view.path).join(".git").is_file());
}

#[test]
fn creating_head_must_still_equal_the_frozen_base() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    drop(h);
    crash_at(&w, &m, "after_git");
    let tree = w.root.join(&m.attempt_id).join("tree");
    git(&tree, &["commit", "--allow-empty", "-m", "before ready"]);
    let head = sha(&tree);
    assert_ne!(head, m.base_sha);
    let mut h = Host::open(&w.db).unwrap();
    assert_eq!(
        h.recover_workspace(&m.mission_id).unwrap_err().category(),
        "workspace_uncertain"
    );
    assert_eq!(
        h.snapshot(&m.mission_id).unwrap().workspace.unwrap().state,
        "uncertain"
    );
    assert_eq!(sha(&tree), head);
}

#[test]
fn crash_after_matching_owner_marker_reclaims_the_reserved_slot() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    drop(h);
    crash_at(&w, &m, "slot");
    let slot = w.root.join(&m.attempt_id);
    let marker = fs::read(slot.join("owner.json")).unwrap();
    assert!(!slot.join("tree").exists());
    let mut h = Host::open(&w.db).unwrap();
    let recovered = h.recover(&m.mission_id).unwrap().workspace.unwrap();
    assert_eq!(recovered.state, "ready");
    assert_eq!(recovered.base_sha, m.base_sha);
    assert_eq!(recovered.path, slot.join("tree").to_str().unwrap());
    assert_eq!(fs::read(slot.join("owner.json")).unwrap(), marker);
    assert_eq!(count(&w), 1);
    assert_eq!(worktree_count(&w.repo), 2);
}

#[test]
fn reserved_slot_without_matching_marker_stays_uncertain() {
    for defect in ["extra", "marker"] {
        let w = world();
        let mut h = Host::open(&w.db).unwrap();
        let m = mission(&mut h, &w);
        drop(h);
        crash_at(&w, &m, "slot");
        let slot = w.root.join(&m.attempt_id);
        match defect {
            "extra" => fs::write(slot.join("extra.txt"), "keep").unwrap(),
            "marker" => fs::write(slot.join("owner.json"), "{\"version\":1}").unwrap(),
            _ => unreachable!(),
        }
        let mut h = Host::open(&w.db).unwrap();
        assert_eq!(
            h.recover_workspace(&m.mission_id).unwrap_err().category(),
            "workspace_uncertain",
            "{defect}"
        );
        assert_eq!(
            h.snapshot(&m.mission_id).unwrap().workspace.unwrap().state,
            "uncertain"
        );
        assert_eq!(
            h.recover_workspace(&m.mission_id).unwrap_err().category(),
            "workspace_uncertain"
        );
        assert_eq!(worktree_count(&w.repo), 1);
        if defect == "extra" {
            assert_eq!(fs::read_to_string(slot.join("extra.txt")).unwrap(), "keep");
        } else {
            assert_eq!(
                fs::read_to_string(slot.join("owner.json")).unwrap(),
                "{\"version\":1}"
            );
        }
    }
}

#[test]
fn equivalent_workspace_root_spellings_match_the_reservation() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    let slash = PathBuf::from(format!("{}/", w.root.display()));
    let view = h
        .prepare_workspace(&m.mission_id, &m.attempt_id, &slash)
        .unwrap();
    assert_eq!(view.state, "ready");
    assert!(!view.path.ends_with('/'));
    assert_eq!(
        h.prepare_workspace(&m.mission_id, &m.attempt_id, &w.root)
            .unwrap()
            .id,
        view.id
    );
    let parent = w.root.parent().unwrap();
    let name = w.root.file_name().unwrap().to_str().unwrap();
    let doubled = PathBuf::from(format!("{}//{name}/", parent.display()));
    assert_eq!(
        h.prepare_workspace(&m.mission_id, &m.attempt_id, &doubled)
            .unwrap()
            .id,
        view.id
    );
}

#[cfg(unix)]
#[test]
fn transient_slot_creation_failure_stays_retryable() {
    use std::os::unix::fs::PermissionsExt;
    let w = world();
    let original = fs::metadata(&w.root).unwrap().permissions();
    let root = w.root.clone();
    let _restore = RestorePerms(root, original);
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    let error = h
        .prepare_workspace_at(&m.mission_id, &m.attempt_id, &w.root, |point| {
            if point == "reserved" {
                let mut perms = fs::metadata(&w.root).unwrap().permissions();
                perms.set_mode(0o555);
                fs::set_permissions(&w.root, perms).unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.category(), "storage_failed", "{error}");
    let recorded = h.snapshot(&m.mission_id).unwrap().workspace.unwrap();
    assert_eq!(recorded.state, "reserved");
    assert!(recorded.detail.is_none());
    let mut perms = fs::metadata(&w.root).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&w.root, perms).unwrap();
    let ready = h
        .prepare_workspace(&m.mission_id, &m.attempt_id, &w.root)
        .unwrap();
    assert_eq!(ready.state, "ready");
    assert_eq!(ready.id, recorded.id);
}

#[cfg(unix)]
struct RestorePerms(PathBuf, fs::Permissions);
#[cfg(unix)]
impl Drop for RestorePerms {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, self.1.clone());
    }
}

#[cfg(unix)]
#[test]
fn git_failure_worker() {
    let Some(db) = std::env::var_os("SW_WORKSPACE_DB") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("SW_WORKSPACE_ROOT").unwrap());
    let mission = std::env::var("SW_WORKSPACE_MISSION").unwrap();
    let attempt = std::env::var("SW_WORKSPACE_ATTEMPT").unwrap();
    let mut h = Host::open(Path::new(&db)).unwrap();
    match h.prepare_workspace(&mission, &attempt, &root) {
        Ok(_) => panic!("git worktree add was expected to fail"),
        Err(error) => {
            let state = h.snapshot(&mission).unwrap().workspace.unwrap().state;
            println!("WORKSPACE-RESULT {} {state}", error.category());
        }
    }
}

#[cfg(unix)]
#[test]
fn clean_git_creation_failure_stays_retryable() {
    use std::os::unix::fs::PermissionsExt;
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    drop(h);
    let tools = w._dir.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let wrapper = tools.join("git");
    fs::write(
        &wrapper,
        "#!/bin/sh\ncase \" $* \" in\n*\" worktree add \"*) exit 1 ;;\nesac\nexec /usr/bin/git \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", tools.display(), std::env::var("PATH").unwrap());
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "service::workspace::tests::git_failure_worker",
            "--nocapture",
        ])
        .env("SW_WORKSPACE_DB", &w.db)
        .env("SW_WORKSPACE_ROOT", &w.root)
        .env("SW_WORKSPACE_MISSION", &m.mission_id)
        .env("SW_WORKSPACE_ATTEMPT", &m.attempt_id)
        .env("PATH", &path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "worker failed: {stdout}{stderr}");
    assert!(
        stdout.contains("WORKSPACE-RESULT git_failed creating"),
        "{stdout}{stderr}"
    );
    let recorded = Host::inspect(&w.db, Some(&m.mission_id))
        .unwrap()
        .remove(0)
        .workspace
        .unwrap();
    assert_eq!(recorded.state, "creating");
    assert!(recorded.detail.is_some());
    let mut h = Host::open(&w.db).unwrap();
    let ready = h
        .prepare_workspace(&m.mission_id, &m.attempt_id, &w.root)
        .unwrap();
    assert_eq!(ready.id, recorded.id);
    assert_eq!(ready.state, "ready");
    assert!(ready.detail.is_none());
    assert_eq!(worktree_count(&w.repo), 2);
}

#[test]
fn recover_still_polls_when_the_workspace_is_uncertain() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    let view = prepare(&mut h, &m, &w);
    fs::remove_file(Path::new(&view.path).parent().unwrap().join("owner.json")).unwrap();
    h.runtime_mut()
        .unwrap()
        .push_notification(RuntimeNotification::Uncertainty {
            reason: AttentionReason::Uncertainty,
            detail: "workspace blocked".into(),
        });
    let error = h.recover(&m.mission_id).unwrap_err();
    assert_eq!(error.category(), "workspace_uncertain", "{error}");
    let snap = h.snapshot(&m.mission_id).unwrap();
    assert_eq!(snap.phase, Phase::NeedsAttention);
    assert_eq!(snap.workspace.unwrap().state, "uncertain");
    assert_eq!(h.runtime_mut().unwrap().queued_notifications(), 0);
}

#[cfg(unix)]
#[test]
fn lock_holder_worker() {
    let Some(path) = std::env::var_os("SW_WORKSPACE_LOCK") else {
        return;
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.try_lock().unwrap();
    println!("WORKSPACE-BARRIER");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}

#[cfg(unix)]
#[test]
fn recover_still_polls_when_the_workspace_is_busy() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    let view = prepare(&mut h, &m, &w);
    let lock = Path::new(&view.path).parent().unwrap().join("owner.lock");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "service::workspace::tests::lock_holder_worker",
            "--nocapture",
        ])
        .env("SW_WORKSPACE_LOCK", &lock)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut holder = Worker(command.spawn().unwrap());
    let stdout = holder.0.stdout.take().unwrap();
    let (send, recv) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if line.unwrap().contains("WORKSPACE-BARRIER") {
                let _ = send.send(());
                return;
            }
        }
    });
    recv.recv_timeout(Duration::from_secs(20))
        .expect("lock holder did not start");
    h.runtime_mut()
        .unwrap()
        .push_notification(RuntimeNotification::Uncertainty {
            reason: AttentionReason::Uncertainty,
            detail: "creator still holds the lease".into(),
        });
    let error = h.recover(&m.mission_id).unwrap_err();
    assert_eq!(error.category(), "workspace_busy", "{error}");
    let snap = h.snapshot(&m.mission_id).unwrap();
    assert_eq!(snap.phase, Phase::NeedsAttention);
    let workspace = snap.workspace.unwrap();
    assert_eq!(workspace.state, "ready");
    assert_eq!(workspace.id, view.id);
    assert!(workspace.detail.is_none());
}

#[test]
fn missing_head_object_and_corrupt_index_are_uncertain() {
    for defect in ["missing-head", "corrupt-index"] {
        let w = world();
        let mut h = Host::open(&w.db).unwrap();
        let m = mission(&mut h, &w);
        prepare(&mut h, &m, &w);
        let record = h.store.load_workspace(&m.attempt_id).unwrap().unwrap();
        let admin = Path::new(record.git_dir.as_ref().unwrap());
        match defect {
            "missing-head" => {
                fs::write(
                    admin.join("HEAD"),
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
                )
                .unwrap();
            }
            "corrupt-index" => fs::write(admin.join("index"), "not a git index").unwrap(),
            _ => unreachable!(),
        }
        let error = h.recover_workspace(&m.mission_id).unwrap_err();
        assert_eq!(error.category(), "workspace_uncertain", "{defect}: {error}");
        assert_eq!(
            h.snapshot(&m.mission_id).unwrap().workspace.unwrap().state,
            "uncertain"
        );
        assert_eq!(
            h.recover_workspace(&m.mission_id).unwrap_err().category(),
            "workspace_uncertain",
            "{defect} was retried instead of staying uncertain"
        );
    }
}

#[test]
fn mkdir_failure_worker() {
    let Some(db) = std::env::var_os("SW_WORKSPACE_DB") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("SW_WORKSPACE_ROOT").unwrap());
    let mission = std::env::var("SW_WORKSPACE_MISSION").unwrap();
    let attempt = std::env::var("SW_WORKSPACE_ATTEMPT").unwrap();
    let mut h = Host::open(Path::new(&db)).unwrap();
    match h.prepare_workspace(&mission, &attempt, &root) {
        Ok(_) => panic!("slot creation was expected to fail after mkdir"),
        Err(error) => {
            let state = h.snapshot(&mission).unwrap().workspace.unwrap().state;
            println!("WORKSPACE-RESULT {} {state}", error.category());
        }
    }
}

#[test]
fn transient_failure_after_mkdir_removes_the_new_slot() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    drop(h);
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "service::workspace::tests::mkdir_failure_worker",
            "--nocapture",
        ])
        .env("SW_WORKSPACE_DB", &w.db)
        .env("SW_WORKSPACE_ROOT", &w.root)
        .env("SW_WORKSPACE_MISSION", &m.mission_id)
        .env("SW_WORKSPACE_ATTEMPT", &m.attempt_id)
        .env("SW_WORKSPACE_FAIL_AFTER_MKDIR", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "worker failed: {stdout}{stderr}");
    assert!(
        stdout.contains("WORKSPACE-RESULT storage_failed reserved"),
        "{stdout}{stderr}"
    );
    assert!(
        !w.root.join(&m.attempt_id).exists(),
        "failed slot creation left a directory behind"
    );
    let recorded = Host::inspect(&w.db, Some(&m.mission_id))
        .unwrap()
        .remove(0)
        .workspace
        .unwrap();
    assert_eq!(recorded.state, "reserved");
    assert!(recorded.detail.is_none());
    let mut h = Host::open(&w.db).unwrap();
    let ready = h
        .prepare_workspace(&m.mission_id, &m.attempt_id, &w.root)
        .unwrap();
    assert_eq!(ready.id, recorded.id);
    assert_eq!(ready.state, "ready");
}

#[cfg(unix)]
#[test]
fn recover_still_polls_when_workspace_storage_fails() {
    use std::os::unix::fs::PermissionsExt;
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    drop(h);
    crash_at(&w, &m, "reserved");
    let original = fs::metadata(&w.root).unwrap().permissions();
    let _restore = RestorePerms(w.root.clone(), original);
    let mut perms = fs::metadata(&w.root).unwrap().permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&w.root, perms).unwrap();
    let mut h = Host::open(&w.db).unwrap();
    h.runtime_mut()
        .unwrap()
        .push_notification(RuntimeNotification::Uncertainty {
            reason: AttentionReason::Uncertainty,
            detail: "storage blocked creation".into(),
        });
    let error = h.recover(&m.mission_id).unwrap_err();
    assert_eq!(error.category(), "storage_failed", "{error}");
    let snap = h.snapshot(&m.mission_id).unwrap();
    assert_eq!(snap.phase, Phase::NeedsAttention);
    let workspace = snap.workspace.unwrap();
    assert_eq!(workspace.state, "reserved");
    assert!(workspace.detail.is_none());
    assert_eq!(h.runtime_mut().unwrap().queued_notifications(), 0);
}

#[test]
fn recover_aborts_before_poll_when_the_owner_generation_is_stale() {
    let w = world();
    let mut h = Host::open(&w.db).unwrap();
    let m = mission(&mut h, &w);
    h.runtime_mut()
        .unwrap()
        .push_notification(RuntimeNotification::Uncertainty {
            reason: AttentionReason::Uncertainty,
            detail: "stale owner".into(),
        });
    Connection::open(&w.db)
        .unwrap()
        .execute("UPDATE host_owner SET generation=generation+1", [])
        .unwrap();
    let error = h.recover(&m.mission_id).unwrap_err();
    assert_eq!(error.category(), "stale_generation", "{error}");
    assert_eq!(h.snapshot(&m.mission_id).unwrap().phase, Phase::Draft);
    assert_eq!(h.runtime_mut().unwrap().queued_notifications(), 1);
}

#[cfg(unix)]
#[test]
fn signaled_git_worker() {
    let Some(db) = std::env::var_os("SW_WORKSPACE_DB") else {
        return;
    };
    let mission = std::env::var("SW_WORKSPACE_MISSION").unwrap();
    let mut h = Host::open(Path::new(&db)).unwrap();
    match h.recover_workspace(&mission) {
        Ok(_) => panic!("signaled git was expected to fail"),
        Err(error) => {
            let state = h.snapshot(&mission).unwrap().workspace.unwrap().state;
            println!("WORKSPACE-RESULT {} {state}", error.category());
        }
    }
}

#[cfg(unix)]
#[test]
fn signaled_git_during_verify_stays_retryable() {
    use std::os::unix::fs::PermissionsExt;
    for needle in ["cat-file", "merge-base"] {
        let w = world();
        let mut h = Host::open(&w.db).unwrap();
        let m = mission(&mut h, &w);
        let view = prepare(&mut h, &m, &w);
        git(
            Path::new(&view.path),
            &["commit", "--allow-empty", "-m", "agent"],
        );
        drop(h);
        let tools = w._dir.path().join("tools");
        fs::create_dir(&tools).unwrap();
        let wrapper = tools.join("git");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\ncase \" $* \" in\n*\" {needle} \"*)\n kill -TERM $$\n ;;\nesac\nexec /usr/bin/git \"$@\"\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:{}", tools.display(), std::env::var("PATH").unwrap());
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "service::workspace::tests::signaled_git_worker",
                "--nocapture",
            ])
            .env("SW_WORKSPACE_DB", &w.db)
            .env("SW_WORKSPACE_MISSION", &m.mission_id)
            .env("PATH", &path)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{needle} worker failed: {stdout}{stderr}"
        );
        assert!(
            stdout.contains("WORKSPACE-RESULT git_failed ready"),
            "{needle}: {stdout}{stderr}"
        );
        let mut h = Host::open(&w.db).unwrap();
        let recovered = h.recover_workspace(&m.mission_id).unwrap().unwrap();
        assert_eq!(recovered.state, "ready");
        assert_eq!(recovered.id, view.id);
    }
}

mod stop_delivery;
