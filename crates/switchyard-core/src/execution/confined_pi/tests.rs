use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

#[test]
fn confined_pi_storage_refuses_symlinks_hardlinks_and_shared_roots() {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = Directory::root(tmp.path()).unwrap();
    let sentinel = tmp.path().join("sentinel");
    fs::write(&sentinel, "unchanged").unwrap();
    symlink(&sentinel, tmp.path().join("owner.lock")).unwrap();
    assert!(root.open_file("owner.lock", true).is_err());
    fs::remove_file(tmp.path().join("owner.lock")).unwrap();
    fs::hard_link(&sentinel, tmp.path().join("owner.lock")).unwrap();
    assert!(root.open_file("owner.lock", true).is_err());
    symlink(tmp.path(), tmp.path().join("redirect")).unwrap();
    assert!(Directory::root(&tmp.path().join("redirect")).is_err());
    assert!(root.child("redirect").is_err());
    symlink(&sentinel, tmp.path().join("current.json")).unwrap();
    assert!(root.read("current.json").is_err());
    assert!(root.write("current.json", &json!({})).is_err());
    let state = root.child("state").unwrap();
    symlink(&sentinel, state.path.join("session.sqlite")).unwrap();
    assert!(state.inspect_state(true).is_err());
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "unchanged");
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Directory::root(tmp.path()).is_err());
}

#[test]
fn confined_pi_storage_lock_and_atomic_record_retain_identity() {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = Directory::root(tmp.path()).unwrap();
    let lock = try_acquire_file(root.open_file("owner.lock", true).unwrap()).unwrap();
    assert!(try_acquire_file(root.open_file("owner.lock", true).unwrap()).is_err());
    root.write("current.json", &json!({"generation":1}))
        .unwrap();
    root.write("current.json", &json!({"generation":2}))
        .unwrap();
    assert_eq!(root.read("current.json").unwrap().unwrap()["generation"], 2);
    drop(lock);
    assert!(try_acquire_file(root.open_file("owner.lock", true).unwrap()).is_ok());
    assert!(root.child("../escape").is_err());
    assert!(root.child("state").unwrap().inspect_state(true).is_err());
}

#[test]
fn confined_pi_recovery_rejects_foreign_leaf_and_invalid_identity() {
    let root = Path::new("/sys/fs/cgroup/delegated");
    let row =
        json!({"cgroup":{"path":"/sys/fs/cgroup/foreign/bridge-a","dev":1,"ino":2,"boot":"b"}});
    assert!(recorded_leaf(&row, root).is_err());
    let binding = RuntimeBinding {
        mission_id: "m".into(),
        attempt_id: "a".into(),
        session_id: "s".into(),
        generation: 0,
    };
    assert!(validate(&binding, "answer").is_err());
    let binding = RuntimeBinding {
        generation: 1,
        ..binding
    };
    assert!(validate(&binding, "eval").is_err());
    assert!(validate(&binding, "answer").is_ok());
}
