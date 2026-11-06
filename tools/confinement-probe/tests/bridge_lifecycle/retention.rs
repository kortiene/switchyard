//! Real-kernel retention checks. Crafted historical/journal records model
//! interruption snapshots; the parent suite separately exercises Host SIGKILL.
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::process::Child;

use super::*;

fn directory(root: &Path) -> PathBuf {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("session-")
        })
        .unwrap()
}

fn write_record(path: &Path, record: &Value) {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(&serde_json::to_vec(record).unwrap())
        .unwrap();
    file.sync_all().unwrap();
    fs::File::open(path.parent().unwrap())
        .unwrap()
        .sync_all()
        .unwrap();
}

pub(super) fn assert_retired(root: &Path, record: &Value) -> Value {
    assert!(
        !leaf(record).exists(),
        "obsolete leaf must have been removed"
    );
    let receipt = fs::read_dir(directory(root))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("retired-")
        })
        .map(|path| serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap())
        .find(|value| value["cgroup"] == record["cgroup"])
        .expect("removed leaf needs its exact durable retirement receipt");
    assert_eq!(receipt["generation"], record["generation"]);
    assert_eq!(receipt["removed"], true);
    assert_eq!(receipt["authorization"]["proven"], true);
    assert_eq!(receipt["authorization"]["cgroup"], record["cgroup"]);
    assert!(
        receipt["authorization"]["events"]
            .as_str()
            .unwrap()
            .lines()
            .any(|line| line == "populated 0")
    );
    receipt
}

struct Historical {
    path: PathBuf,
    record: Value,
    child: Option<Child>,
}

impl Historical {
    fn new(root: &Path) -> Self {
        let delegated = path("SWITCHYARD_CGROUP_ROOT").canonicalize().unwrap();
        let path = tempfile::Builder::new()
            .prefix("bridge-retention-")
            .tempdir_in(delegated)
            .unwrap()
            .keep();
        let meta = fs::metadata(&path).unwrap();
        let record = json!({"generation":1,"proven":true,"events":"populated 0\n",
            "cgroup":{"path":path,"dev":meta.dev(),"ino":meta.ino(),
            "boot":fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().trim()}});
        write_record(&directory(root).join("recovered-1.json"), &record);
        Self {
            path,
            record,
            child: None,
        }
    }

    fn populate(&mut self) {
        let child = Command::new("/usr/bin/sleep").arg("60").spawn().unwrap();
        self.child = Some(child);
        fs::write(
            self.path.join("cgroup.procs"),
            self.child.as_ref().unwrap().id().to_string(),
        )
        .unwrap();
        assert!(
            fs::read_to_string(self.path.join("cgroup.events"))
                .unwrap()
                .contains("populated 1")
        );
    }

    fn stop_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            child.wait().unwrap();
            let until = Instant::now() + Duration::from_secs(2);
            while !fs::read_to_string(self.path.join("cgroup.events"))
                .unwrap()
                .contains("populated 0")
            {
                assert!(Instant::now() < until);
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

impl Drop for Historical {
    fn drop(&mut self) {
        self.stop_child();
        let _ = fs::remove_dir(&self.path);
    }
}

#[test]
fn confined_lifecycle_retention_bounds_repeated_reopen_and_keeps_current_leaf() {
    let tmp = private_root();
    let config = config(tmp.path());
    let mut previous: Option<Value> = None;
    let mut all = Vec::new();
    for generation in 1..=6 {
        let mut session = ConfinedPiSession::open(&config, binding(generation), "answer").unwrap();
        let record = session.lifecycle();
        let control = directory(tmp.path());
        if let Some(previous) = previous.as_ref() {
            assert_retired(tmp.path(), previous);
            let saved: Value = serde_json::from_slice(
                &fs::read(control.join(format!("recovered-{}.json", previous["generation"])))
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(saved["cgroup"], previous["cgroup"]);
            assert_eq!(saved["proven"], true);
        }
        all.push(leaf(&record));
        assert_eq!(all.iter().filter(|path| path.exists()).count(), 1);
        assert!(!control.join("leaf-allocation.json").exists());
        assert!(!control.join("leaf-retirement.json").exists());
        assert_eq!(session.stop().unwrap()["proven"], true);
        drop(session);
        assert!(
            leaf(&record).exists(),
            "even a proven stopped current leaf must remain"
        );
        previous = Some(record);
    }
    retain(
        "bridge-lifecycle-retention.json",
        &json!({"reopens":6,"retainedCurrentLeaves":1,"last":previous}),
    );
}

#[test]
fn confined_lifecycle_retention_missing_current_still_blocks_recovery() {
    let tmp = private_root();
    let config = config(tmp.path());
    let mut session = ConfinedPiSession::open(&config, binding(1), "answer").unwrap();
    session.stop().unwrap();
    let record = session.lifecycle();
    drop(session);
    fs::remove_dir(leaf(&record)).unwrap();
    let error = ConfinedPiSession::open(&config, binding(2), "recover")
        .err()
        .expect("missing current leaf cannot be excused by stop evidence");
    assert!(
        error
            .to_string()
            .contains("previous bridge is not quiescent: missing"),
        "{error}"
    );
    let saved: Value =
        serde_json::from_slice(&fs::read(directory(tmp.path()).join("current.json")).unwrap())
            .unwrap();
    assert_eq!(saved, record);
    assert!(!directory(tmp.path()).join("leaf-allocation.json").exists());
}

#[test]
fn confined_lifecycle_retention_refuses_missing_replaced_and_nonempty_obsolete_leaves() {
    for mode in ["missing", "replaced", "nonempty"] {
        let tmp = private_root();
        let config = config(tmp.path());
        let mut session = ConfinedPiSession::open(&config, binding(2), "answer").unwrap();
        session.stop().unwrap();
        let current = session.lifecycle();
        drop(session);
        let mut historical = Historical::new(tmp.path());
        match mode {
            "missing" => fs::remove_dir(&historical.path).unwrap(),
            "replaced" => {
                historical.populate();
                historical.record["cgroup"]["ino"] =
                    json!(historical.record["cgroup"]["ino"].as_u64().unwrap() + 1);
                write_record(
                    &directory(tmp.path()).join("recovered-1.json"),
                    &historical.record,
                );
            }
            "nonempty" => historical.populate(),
            _ => unreachable!(),
        }
        let error = ConfinedPiSession::open(&config, binding(3), "answer")
            .err()
            .expect("unsafe obsolete leaf must block replacement");
        let reason = if mode == "replaced" {
            "mismatched"
        } else {
            mode
        };
        assert!(error.to_string().contains(reason), "{mode}: {error}");
        assert!(leaf(&current).exists());
        assert!(!directory(tmp.path()).join("leaf-allocation.json").exists());
        if let Some(child) = historical.child.as_mut() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "cleanup must not kill obsolete/replaced workloads"
            );
        }
        if mode == "nonempty" {
            historical.stop_child();
            let reopened = ConfinedPiSession::open(&config, binding(4), "answer").unwrap();
            assert_eq!(reopened.lifecycle()["phase"], "opened");
            assert_retired(tmp.path(), &historical.record);
        }
    }
}

#[test]
fn confined_lifecycle_retention_replays_an_authorized_post_rmdir_crash_snapshot() {
    let tmp = private_root();
    let config = config(tmp.path());
    let mut session = ConfinedPiSession::open(&config, binding(2), "answer").unwrap();
    session.stop().unwrap();
    drop(session);
    let historical = Historical::new(tmp.path());
    let retirement = json!({"version":1,"identity":["lifecycle-m","lifecycle-a","lifecycle-s"],
        "source":"recovered","generation":1,"cgroup":historical.record["cgroup"],
        "authorization":{"proven":true,"cgroup":historical.record["cgroup"],
            "events":fs::read_to_string(historical.path.join("cgroup.events")).unwrap()}});
    write_record(
        &directory(tmp.path()).join("leaf-retirement.json"),
        &retirement,
    );
    fs::remove_dir(&historical.path).unwrap();
    let recovered = ConfinedPiSession::open(&config, binding(3), "answer").unwrap();
    assert_eq!(recovered.lifecycle()["phase"], "opened");
    let receipt = assert_retired(tmp.path(), &historical.record);
    assert_eq!(
        receipt["completion"],
        "absent after durable deletion authorization"
    );
    assert!(!directory(tmp.path()).join("leaf-retirement.json").exists());
    retain("bridge-lifecycle-retirement-replay.json", &receipt);
}
