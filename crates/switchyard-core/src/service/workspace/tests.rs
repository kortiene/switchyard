use super::*;
use rusqlite::Connection;
use std::fs;
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
    assert!(
        h.dispatch_pending(&m.mission_id).is_err(),
        "new owner must reconcile first"
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
    db.execute_batch("DROP TABLE workspaces; DELETE FROM schema_migrations WHERE version=3; PRAGMA user_version=2;").unwrap();
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
        3
    );
    assert_eq!(h.snapshot(&m.mission_id).unwrap().base_sha, m.base_sha);
    assert!(h.recover(&m.mission_id).unwrap().workspace.is_none());
    assert_eq!(count(&w), 0);
    assert_eq!(fs::read_dir(&w.root).unwrap().count(), 0);
}
