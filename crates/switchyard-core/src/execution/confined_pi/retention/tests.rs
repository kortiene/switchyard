use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;

use super::*;

#[derive(Clone)]
struct Entry {
    identity: CgroupIdentity,
    populated: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    BeforeRemoval,
    AfterRemoval,
}

struct FakeLeaves {
    entries: RefCell<HashMap<PathBuf, Entry>>,
    serial: Cell<u64>,
    fault: Cell<Fault>,
    removals: Cell<usize>,
}

impl FakeLeaves {
    fn new() -> Self {
        Self {
            entries: RefCell::new(HashMap::new()),
            serial: Cell::new(1),
            fault: Cell::new(Fault::None),
            removals: Cell::new(0),
        }
    }

    fn create(&self, root: &Path, name: &str) -> Result<CgroupIdentity> {
        let ino = self.serial.get();
        self.serial.set(ino + 1);
        let identity = CgroupIdentity {
            path: root.join(name),
            dev: 1,
            ino,
            boot_id: "test-boot".into(),
        };
        assert!(
            self.entries
                .borrow_mut()
                .insert(
                    identity.path.clone(),
                    Entry {
                        identity: identity.clone(),
                        populated: 0,
                    }
                )
                .is_none()
        );
        Ok(identity)
    }
}

impl Leaves for FakeLeaves {
    fn exists(&self, path: &Path) -> Result<bool> {
        Ok(self.entries.borrow().contains_key(path))
    }

    fn observe(&self, leaf: &CgroupIdentity) -> Result<Quiescence> {
        let entries = self.entries.borrow();
        let entry = entries.get(&leaf.path);
        let reason = match entry {
            None => "missing",
            Some(entry)
                if entry.identity.dev != leaf.dev
                    || entry.identity.ino != leaf.ino
                    || entry.identity.boot_id != leaf.boot_id =>
            {
                "mismatched"
            }
            Some(entry) if entry.populated != 0 => "nonempty",
            Some(_) => "populated 0",
        };
        Ok(Quiescence {
            proven: reason == "populated 0",
            reason: reason.into(),
            populated: entry.map(|entry| entry.populated),
            events: entry
                .map(|entry| format!("populated {}\nfrozen 0\n", entry.populated))
                .unwrap_or_default(),
            pids_max: 0,
            oom_kill: 0,
        })
    }

    fn remove_empty(&self, leaf: &CgroupIdentity) -> Result<Quiescence> {
        let proof = self.observe(leaf)?;
        assert!(
            proof.proven,
            "removal must recheck fresh identity/emptiness"
        );
        self.removals.set(self.removals.get() + 1);
        let fault = self.fault.replace(Fault::None);
        if fault == Fault::BeforeRemoval {
            return Err(Error::guard("injected crash before rmdir"));
        }
        self.entries.borrow_mut().remove(&leaf.path).unwrap();
        if fault == Fault::AfterRemoval {
            return Err(Error::guard("injected crash after rmdir before receipt"));
        }
        Ok(proof)
    }
}

struct Fixture {
    _temporary: tempfile::TempDir,
    control: Directory,
    root: PathBuf,
    binding: RuntimeBinding,
    leaves: FakeLeaves,
}

impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let control = Directory::root(temporary.path()).unwrap();
        Self {
            _temporary: temporary,
            control,
            root: PathBuf::from("/delegated"),
            binding: RuntimeBinding {
                mission_id: "m".into(),
                attempt_id: "a".into(),
                session_id: "s".into(),
                generation: 100,
            },
            leaves: FakeLeaves::new(),
        }
    }

    fn cleanup<'a>(&'a self, current: Option<&'a Value>) -> Cleanup<'a, FakeLeaves> {
        Cleanup {
            control: &self.control,
            root: &self.root,
            binding: &self.binding,
            current,
            leaves: &self.leaves,
        }
    }

    fn leaf(&self, generation: u64) -> Value {
        let leaf = self
            .leaves
            .create(&self.root, &format!("bridge-{generation}"))
            .unwrap();
        json!({"generation":generation,"cgroup":leaf_value(&leaf)})
    }

    fn remember(&self, current: &Value) {
        self.control.write(&format!("recovered-{}.json", current["generation"]),
            &json!({"generation":current["generation"],"cgroup":current["cgroup"],"proven":true,"events":"populated 0\n"})).unwrap();
    }

    fn allocate(&self) -> CgroupIdentity {
        allocate_with(&self.control, &self.root, &self.binding, |root, name| {
            self.leaves.create(root, name)
        })
        .unwrap()
    }

    fn audit(&self, record: &Value) -> Option<Value> {
        let name = self.cleanup(None).audit_name(&record["cgroup"]).unwrap();
        self.control.read(&name).unwrap()
    }
}

#[test]
fn confined_pi_retention_repeated_handoffs_keep_one_leaf_and_all_audit_evidence() {
    let mut fixture = Fixture::new();
    let mut current = fixture.leaf(1);
    for generation in 2..35 {
        fixture.binding.generation = generation;
        fixture.remember(&current);
        fixture.cleanup(Some(&current)).recover().unwrap();
        assert_eq!(fixture.leaves.entries.borrow().len(), 1);
        let allocated = fixture.allocate();
        assert_eq!(fixture.leaves.entries.borrow().len(), 2);
        let next = json!({"generation":generation,"cgroup":leaf_value(&allocated)});
        fixture.control.write("current.json", &next).unwrap();
        fixture.cleanup(Some(&next)).recover().unwrap();
        assert_eq!(fixture.leaves.entries.borrow().len(), 1);
        assert!(fixture.leaves.exists(&allocated.path).unwrap());
        let audit = fixture.audit(&current).unwrap();
        assert_eq!(audit["removed"], true);
        assert!(authorized(&audit));
        assert!(
            fixture
                .control
                .read(&format!("recovered-{}.json", current["generation"]))
                .unwrap()
                .is_some()
        );
        assert!(!fixture.control.exists(ALLOCATION).unwrap());
        assert!(!fixture.control.exists(RETIREMENT).unwrap());
        current = next;
    }
}

#[test]
fn confined_pi_retention_crash_before_current_replacement_reclaims_only_unreleased_leaf() {
    let fixture = Fixture::new();
    let current = fixture.leaf(1);
    fixture.remember(&current);
    let allocated = fixture.allocate();
    fixture.cleanup(Some(&current)).recover().unwrap();
    assert!(
        fixture
            .leaves
            .exists(Path::new(current["cgroup"]["path"].as_str().unwrap()))
            .unwrap()
    );
    assert!(!fixture.leaves.exists(&allocated.path).unwrap());
    let audit = fixture
        .audit(&json!({"cgroup":leaf_value(&allocated)}))
        .unwrap();
    assert_eq!(audit["source"], "allocation");
    assert_eq!(fixture.leaves.entries.borrow().len(), 1);
    assert!(!fixture.control.exists(ALLOCATION).unwrap());
}

#[test]
fn confined_pi_retention_crash_after_current_replacement_reclaims_only_predecessor() {
    let fixture = Fixture::new();
    let previous = fixture.leaf(1);
    fixture.remember(&previous);
    let allocated = fixture.allocate();
    let current = json!({"generation":100,"cgroup":leaf_value(&allocated)});
    fixture.control.write("current.json", &current).unwrap();
    fixture.cleanup(Some(&current)).recover().unwrap();
    assert_eq!(fixture.leaves.entries.borrow().len(), 1);
    assert!(fixture.leaves.exists(&allocated.path).unwrap());
    assert_eq!(fixture.audit(&previous).unwrap()["source"], "recovered");
    assert!(fixture.audit(&current).is_none());
}

#[test]
fn confined_pi_retention_allocation_intent_survives_both_sides_of_mkdir() {
    for create_leaf in [false, true] {
        let fixture = Fixture::new();
        let current = fixture.leaf(1);
        let failed = allocate_with(
            &fixture.control,
            &fixture.root,
            &fixture.binding,
            |root, name| {
                if create_leaf {
                    fixture.leaves.create(root, name)?;
                }
                Err(Error::guard(
                    "injected allocation crash before identity persistence",
                ))
            },
        );
        assert!(failed.is_err());
        let intent = fixture.control.read(ALLOCATION).unwrap().unwrap();
        assert!(intent["cgroup"].is_null());
        for _ in 0..3 {
            let result = fixture.cleanup(Some(&current)).recover();
            if create_leaf {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("identity is unknown")
                );
                assert!(
                    allocate_with(
                        &fixture.control,
                        &fixture.root,
                        &fixture.binding,
                        |root, name| fixture.leaves.create(root, name)
                    )
                    .is_err()
                );
                assert_eq!(fixture.leaves.entries.borrow().len(), 2);
            } else {
                result.unwrap();
                assert!(!fixture.control.exists(ALLOCATION).unwrap());
                assert_eq!(fixture.leaves.entries.borrow().len(), 1);
            }
        }
        assert_eq!(fixture.leaves.removals.get(), 0);
    }
}

#[test]
fn confined_pi_retention_crashes_around_removal_reconcile_from_durable_authorization() {
    for fault in [Fault::BeforeRemoval, Fault::AfterRemoval] {
        let fixture = Fixture::new();
        let previous = fixture.leaf(1);
        fixture.remember(&previous);
        let current = fixture.leaf(2);
        fixture.leaves.fault.set(fault);
        fixture.cleanup(Some(&current)).recover().unwrap_err();
        let pending = fixture.control.read(RETIREMENT).unwrap().unwrap();
        assert!(authorized(&pending), "authorization must precede rmdir");
        assert!(fixture.audit(&previous).is_none());
        fixture.cleanup(Some(&current)).recover().unwrap();
        assert!(fixture.audit(&previous).unwrap()["removed"] == true);
        assert!(!fixture.control.exists(RETIREMENT).unwrap());
        assert_eq!(fixture.leaves.entries.borrow().len(), 1);
        assert!(
            fixture
                .leaves
                .exists(Path::new(current["cgroup"]["path"].as_str().unwrap()))
                .unwrap()
        );
    }
}

#[test]
fn confined_pi_retention_unreleased_removal_crash_settles_both_journals() {
    let fixture = Fixture::new();
    let current = fixture.leaf(1);
    let allocated = fixture.allocate();
    fixture.leaves.fault.set(Fault::AfterRemoval);
    fixture.cleanup(Some(&current)).recover().unwrap_err();
    assert!(fixture.control.exists(ALLOCATION).unwrap());
    assert!(fixture.control.exists(RETIREMENT).unwrap());
    fixture.cleanup(Some(&current)).recover().unwrap();
    assert!(!fixture.control.exists(ALLOCATION).unwrap());
    assert!(!fixture.control.exists(RETIREMENT).unwrap());
    assert!(!fixture.leaves.exists(&allocated.path).unwrap());
    assert_eq!(fixture.leaves.entries.borrow().len(), 1);
}

#[test]
fn confined_pi_retention_missing_without_authorization_never_becomes_a_stop_proof() {
    let fixture = Fixture::new();
    let previous = fixture.leaf(1);
    fixture.remember(&previous);
    let current = fixture.leaf(2);
    fixture
        .leaves
        .entries
        .borrow_mut()
        .remove(Path::new(previous["cgroup"]["path"].as_str().unwrap()));
    for _ in 0..2 {
        let error = fixture.cleanup(Some(&current)).recover().unwrap_err();
        assert!(error.to_string().contains("missing"));
        assert!(!authorized(
            &fixture.control.read(RETIREMENT).unwrap().unwrap()
        ));
        assert!(fixture.audit(&previous).is_none());
        assert_eq!(fixture.leaves.removals.get(), 0);
    }
}

#[test]
fn confined_pi_retention_replaced_or_nonempty_leaf_blocks_even_after_authorization() {
    for after_authorization in [false, true] {
        for changed in ["inode", "boot", "device", "nonempty"] {
            let fixture = Fixture::new();
            let previous = fixture.leaf(1);
            fixture.remember(&previous);
            let current = fixture.leaf(2);
            if after_authorization {
                fixture.leaves.fault.set(Fault::BeforeRemoval);
                fixture.cleanup(Some(&current)).recover().unwrap_err();
            }
            let before = fixture.leaves.removals.get();
            let mut entries = fixture.leaves.entries.borrow_mut();
            let entry = entries
                .get_mut(Path::new(previous["cgroup"]["path"].as_str().unwrap()))
                .unwrap();
            match changed {
                "inode" => entry.identity.ino += 1,
                "boot" => entry.identity.boot_id = "other-boot".into(),
                "device" => entry.identity.dev += 1,
                "nonempty" => entry.populated = 1,
                _ => unreachable!(),
            }
            drop(entries);
            let error = fixture.cleanup(Some(&current)).recover().unwrap_err();
            assert!(
                error.to_string().contains(if changed == "nonempty" {
                    "nonempty"
                } else {
                    "mismatched"
                }),
                "{error}"
            );
            assert_eq!(fixture.leaves.removals.get(), before);
            assert_eq!(fixture.leaves.entries.borrow().len(), 2);
            assert!(fixture.audit(&previous).is_none());
        }
    }
}

#[test]
fn confined_pi_retention_refuses_current_leaf_even_with_a_deletion_authorization() {
    let fixture = Fixture::new();
    let current = fixture.leaf(1);
    let intent = json!({"version":1,"identity":identity(&fixture.binding),"generation":1,
        "path":current["cgroup"]["path"],"cgroup":current["cgroup"]});
    fixture.control.write(ALLOCATION, &intent).unwrap();
    let mut retirement =
        fixture
            .cleanup(Some(&current))
            .retirement("allocation", &json!(1), &current["cgroup"]);
    retirement["authorization"] =
        json!({"proven":true,"cgroup":current["cgroup"],"events":"populated 0\n"});
    fixture.control.write(RETIREMENT, &retirement).unwrap();
    assert!(
        fixture
            .cleanup(Some(&current))
            .recover()
            .unwrap_err()
            .to_string()
            .contains("current bridge leaf")
    );
    assert_eq!(fixture.leaves.removals.get(), 0);
}

#[test]
fn confined_pi_retention_drains_legacy_evidence_in_bounded_batches_before_allocation() {
    let fixture = Fixture::new();
    for generation in 1..=CLEANUP_BATCH as u64 + 1 {
        fixture.remember(&fixture.leaf(generation));
    }
    let current = fixture.leaf(50);
    let error = fixture.cleanup(Some(&current)).recover().unwrap_err();
    assert!(error.to_string().contains("cleanup backlog remains"));
    assert_eq!(fixture.leaves.removals.get(), CLEANUP_BATCH);
    assert_eq!(fixture.leaves.entries.borrow().len(), 2);
    fixture.cleanup(Some(&current)).recover().unwrap();
    assert_eq!(fixture.leaves.entries.borrow().len(), 1);
    assert_eq!(fixture.leaves.removals.get(), CLEANUP_BATCH + 1);
    fixture.cleanup(Some(&current)).recover().unwrap();
    assert_eq!(fixture.leaves.removals.get(), CLEANUP_BATCH + 1);
}

#[test]
fn confined_pi_retention_journal_conflicts_and_foreign_roots_fail_closed() {
    for field in ["identity", "generation", "cgroup", "authorization"] {
        let fixture = Fixture::new();
        let previous = fixture.leaf(1);
        fixture.remember(&previous);
        let current = fixture.leaf(2);
        let mut retirement =
            fixture
                .cleanup(Some(&current))
                .retirement("recovered", &json!(1), &previous["cgroup"]);
        match field {
            "identity" => retirement["identity"] = json!(["foreign", "a", "s"]),
            "generation" => retirement["generation"] = json!(101),
            "cgroup" => retirement["cgroup"]["path"] = json!("/foreign/bridge-1"),
            "authorization" => retirement["authorization"] = json!({"proven":true}),
            _ => unreachable!(),
        }
        fixture.control.write(RETIREMENT, &retirement).unwrap();
        assert!(fixture.cleanup(Some(&current)).recover().is_err());
        assert_eq!(fixture.leaves.removals.get(), 0);
    }
}

#[test]
fn confined_pi_retention_completed_receipts_do_not_allow_a_reappeared_leaf() {
    let fixture = Fixture::new();
    let previous = fixture.leaf(1);
    fixture.remember(&previous);
    let current = fixture.leaf(2);
    fixture.cleanup(Some(&current)).recover().unwrap();
    fixture.leaves.create(&fixture.root, "bridge-1").unwrap();
    assert!(
        fixture
            .cleanup(Some(&current))
            .recover()
            .unwrap_err()
            .to_string()
            .contains("reappeared")
    );
    assert_eq!(fixture.leaves.removals.get(), 1);
    assert_eq!(fixture.leaves.entries.borrow().len(), 2);
}
