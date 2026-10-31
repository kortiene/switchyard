use super::fixtures::{admit_expect, create, fingerprint, git, git_out, open_world};
use crate::{CommandBody, Host, Phase};

#[test]
fn second_active_owner_is_rejected_and_inspection_does_not_take_the_lock() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let generation = host.generation();
    let revision = created.revision;
    match Host::open(&world.db) {
        Err(error) => assert_eq!(error.category(), "second_active_owner"),
        Ok(_) => panic!("a second active owner was admitted"),
    }
    let inspected = Host::inspect(&world.db, Some(&created.mission_id)).unwrap();
    assert_eq!(inspected[0].revision, revision);
    assert_eq!(inspected[0].attempt_id, created.attempt_id);
    assert_eq!(inspected[0].recorded_owner_generation, generation);
    assert_eq!(host.generation(), generation);
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().revision,
        revision
    );
    drop(host);
    std::fs::remove_file(crate::execution::lock_path(&world.db)).unwrap();
    let _ = Host::inspect(&world.db, Some(&created.mission_id)).unwrap();
    assert!(!crate::execution::lock_path(&world.db).exists());
}

#[test]
fn real_git_identity_is_frozen_and_a_non_repo_is_rejected() {
    let world = open_world();
    let before = fingerprint(&world.repo);
    let head = git_out(&world.repo, &["rev-parse", "HEAD"]);
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    assert_eq!(created.head_sha, head.trim());
    assert_eq!(created.base_sha, head.trim());
    assert_eq!(fingerprint(&world.repo), before);
    std::fs::write(world.repo.join("later.txt"), "later\n").unwrap();
    git(&world.repo, &["add", "later.txt"]);
    git(&world.repo, &["commit", "-m", "later"]);
    let snap = host.snapshot(&created.mission_id).unwrap();
    assert_eq!(snap.base_sha, head.trim());
    assert_ne!(
        snap.base_sha,
        git_out(&world.repo, &["rev-parse", "HEAD"]).trim()
    );
    drop(host);

    let mut fresh = Host::open(&world._tmp.path().join("other.sqlite")).unwrap();
    let empty = world._tmp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let error = fresh
        .create_mission(crate::CreateMission {
            repo: empty,
            issue_number: 1,
            issue_title: String::new(),
            objective: "x".into(),
            acceptance: "y".into(),
            actor: "operator".into(),
            max_repairs: 1,
            max_plan_revisions: 1,
        })
        .unwrap_err();
    assert_eq!(error.category(), "git_failed");
    let bare = world._tmp.path().join("nocommit");
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "--initial-branch=main"]);
    let error = fresh
        .create_mission(crate::CreateMission {
            repo: bare,
            issue_number: 1,
            issue_title: String::new(),
            objective: "x".into(),
            acceptance: "y".into(),
            actor: "operator".into(),
            max_repairs: 1,
            max_plan_revisions: 1,
        })
        .unwrap_err();
    assert_eq!(error.category(), "git_failed");
}

#[test]
fn aliased_database_paths_share_one_owner_and_generation_is_reread() {
    let world = open_world();
    let mut host = Host::open(&world.db).unwrap();
    let created = create(&mut host, &world.repo, 2);
    let alias = world._tmp.path().join("alias.sqlite");
    std::os::unix::fs::symlink(&world.db, &alias).unwrap();
    match Host::open(&alias) {
        Err(error) => assert_eq!(error.category(), "second_active_owner"),
        Ok(_) => panic!("symlink opened a second owner"),
    }
    let alias_lock = crate::execution::lock_path(&alias);
    assert!(
        !alias_lock.exists(),
        "alias must not grow its own owner lock"
    );
    let generation = host.generation();
    let conn = rusqlite::Connection::open(&world.db).unwrap();
    conn.execute("UPDATE host_owner SET generation = generation + 1", [])
        .unwrap();
    let error =
        admit_expect(&mut host, &created.mission_id, CommandBody::StartPlanning).unwrap_err();
    assert_eq!(error.category(), "stale_generation");
    assert_eq!(
        host.snapshot(&created.mission_id).unwrap().phase,
        Phase::Draft
    );
    let binding = crate::RuntimeBinding {
        mission_id: created.mission_id.clone(),
        attempt_id: created.attempt_id.clone(),
        session_id: created.session_id.clone(),
        generation,
    };
    assert_eq!(
        host.accept_runtime_binding(&binding, &created.mission_id)
            .unwrap_err()
            .category(),
        "stale_generation"
    );
}
