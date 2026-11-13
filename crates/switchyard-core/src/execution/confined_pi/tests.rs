use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::Command;

use crate::canonical::canonicalize;
use crate::domain::{RepoIdentity, SourceIssue};
use crate::persistence::{CreatedMission, Store};

#[test]
fn confined_pi_storage_refuses_symlinks_hardlinks_and_shared_roots() {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = Directory::root(tmp.path()).unwrap();
    let sentinel = tmp.path().join("sentinel");
    fs::write(&sentinel, "unchanged").unwrap();
    // Owner-only, so the hard link below is refused by nlink rather than mode.
    fs::set_permissions(&sentinel, fs::Permissions::from_mode(0o600)).unwrap();
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
    assert!(root.remove("current.json").is_err());
    assert!(tmp.path().join("current.json").is_symlink());
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
    root.write("journal.json", &json!({"settled":true}))
        .unwrap();
    root.remove("journal.json").unwrap();
    assert!(!root.exists("journal.json").unwrap());
    assert!(root.remove("../outside").is_err());
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
    assert!(validate(&binding, "s3-acceptance").is_ok());
    assert!(validate(&binding, "busy-once").is_ok());
    assert!(validate(&binding, "http://169.254.169.254/latest").is_err());
    assert!(validate(&binding, "http://127.0.0.1:9/v1").is_err());
}

#[test]
fn confined_pi_proven_unopened_stop_does_not_require_existing_database() {
    let row = |phase: &str, requires_existing: bool, proven: Option<bool>| {
        let mut record = json!({"phase": phase, "requiresExisting": requires_existing});
        if let Some(proven) = proven {
            record["termination"] = json!({"proven": proven});
        }
        record
    };
    assert!(!require_existing_database(&row("prepared", false, None)));
    assert!(require_existing_database(&row("prepared", true, None)));
    assert!(require_existing_database(&row("released", false, None)));
    assert!(require_existing_database(&row("opened", true, None)));
    assert!(require_existing_database(&row(
        "uncertain",
        false,
        Some(false)
    )));
    assert!(require_existing_database(&row(
        "stopped",
        false,
        Some(false)
    )));
    assert!(require_existing_database(&row("stopped", true, Some(true))));
    let unopened = row("stopped", false, Some(true));
    assert!(!require_existing_database(&unopened));
    assert!(proven_unopened_stop(&unopened));
    assert!(!proven_unopened_stop(&row("stopped", true, Some(true))));
    assert!(!proven_unopened_stop(&row("released", false, None)));
    assert!(!proven_unopened_stop(&row("prepared", false, None)));
}

#[test]
fn confined_pi_configuration_swap_requires_an_empty_unopened_stop() {
    let record =
        |phase: &str, requires_existing: bool, proven: Option<bool>, configuration: &str| {
            let mut previous = json!({
                "phase": phase,
                "requiresExisting": requires_existing,
                "configuration": configuration,
            });
            if let Some(proven) = proven {
                previous["termination"] = json!({"proven": proven});
            }
            previous
        };
    let unopened = record("stopped", false, Some(true), "rejected-policy");
    let corrected = json!("corrected-policy");
    let same = json!("rejected-policy");
    assert!(allows_configuration(&unopened, &corrected, true));
    assert!(!allows_configuration(&unopened, &corrected, false));
    assert!(allows_configuration(&unopened, &same, false));
    assert!(!allows_configuration(
        &record("stopped", true, Some(true), "rejected-policy"),
        &corrected,
        true
    ));
    assert!(!allows_configuration(
        &record("stopped", false, Some(false), "rejected-policy"),
        &corrected,
        true
    ));
    assert!(!allows_configuration(
        &record("released", false, None, "rejected-policy"),
        &corrected,
        true
    ));
}

fn evidence_digest(root: &Path) -> String {
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/runtime-root-digest.mjs");
    let output = Command::new("node")
        .arg(&script)
        .arg(root)
        .output()
        .unwrap_or_else(|error| panic!("node digest script: {error}"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn confined_pi_runtime_root_must_be_private_absolute_and_complete() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fixture_runtime(tmp.path(), "complete");
    let install = policy::inspect_runtime(&root).unwrap();
    assert_eq!(install.root, root);
    assert_eq!(install.digest, evidence_digest(&root));
    assert!(install.digest.starts_with("sha256:"));
    assert!(policy::inspect_runtime(Path::new("relative/runtime")).is_err());
    let escaped = tmp
        .path()
        .join("pi-runtime-complete/../pi-runtime-complete");
    let error = policy::inspect_runtime(&escaped).unwrap_err().to_string();
    assert!(
        error.contains("non-normal"),
        "a non-canonical runtime path must be refused: {error}"
    );
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    let error = policy::inspect_runtime(&root).unwrap_err().to_string();
    assert!(error.contains("owner-private"), "{error}");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_file(root.join(".artifacts/provenance.json")).unwrap();
    let error = policy::inspect_runtime(&root).unwrap_err().to_string();
    assert!(error.contains("missing"), "{error}");
}

#[test]
fn confined_pi_runtime_symlinked_or_writable_part_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fixture_runtime(tmp.path(), "links");
    let linked = tmp.path().join("linked-root");
    symlink(&root, &linked).unwrap();
    let error = policy::inspect_runtime(&linked).unwrap_err().to_string();
    assert!(error.contains("symlink"), "{error}");

    let pins = fs::read(root.join("pins.json")).unwrap();
    fs::remove_file(root.join("pins.json")).unwrap();
    fs::write(tmp.path().join("pins.json"), &pins).unwrap();
    symlink(tmp.path().join("pins.json"), root.join("pins.json")).unwrap();
    let error = policy::inspect_runtime(&root).unwrap_err().to_string();
    assert!(error.contains("symlink"), "{error}");
    fs::remove_file(root.join("pins.json")).unwrap();
    fs::write(root.join("pins.json"), pins).unwrap();
    fs::set_permissions(root.join("pins.json"), fs::Permissions::from_mode(0o644)).unwrap();

    // npm's node_modules/.bin links stay inside the tree. They are recorded,
    // not followed, and do not make the required directory a symlink.
    symlink("keep.txt", root.join("node_modules/.bin-link")).unwrap();
    let install = policy::inspect_runtime(&root).unwrap();
    assert_eq!(install.digest, evidence_digest(&root));

    fs::set_permissions(root.join("src"), fs::Permissions::from_mode(0o775)).unwrap();
    let error = policy::inspect_runtime(&root).unwrap_err().to_string();
    assert!(error.contains("writable"), "{error}");
    fs::set_permissions(root.join("src"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(
        root.join("confinement/guard.cjs"),
        fs::Permissions::from_mode(0o4755),
    )
    .unwrap();
    let error = policy::inspect_runtime(&root).unwrap_err().to_string();
    assert!(error.contains("setid"), "{error}");
}

#[test]
fn confined_pi_runtime_tampered_guard_fails_before_launch() {
    assert_tampered_file_fails_before_launch("confinement/guard.cjs", b"tampered guard\n");
}

#[test]
fn confined_pi_runtime_tampered_lockfile_fails_before_launch() {
    assert_tampered_file_fails_before_launch("package-lock.json", b"{}\n");
}

fn assert_tampered_file_fails_before_launch(rel: &str, bytes: &[u8]) {
    let tmp = tempfile::tempdir().unwrap();
    let root = fixture_runtime(tmp.path(), "tamper");
    fs::write(root.join(rel), bytes).unwrap();
    fs::set_permissions(root.join(rel), fs::Permissions::from_mode(0o644)).unwrap();
    let config = ConfinedPiConfig {
        root: tmp.path().into(),
        cgroup_root: tmp.path().into(),
        runtime: root,
        node: PathBuf::from("/no/such/node"),
        bubblewrap: PathBuf::from("/no/such/bwrap"),
        launcher: PathBuf::from("/no/such/launcher"),
        policy: PathBuf::from("/no/such/policy"),
        model_endpoint: None,
    };
    let error = policy::check(&config).unwrap_err().to_string();
    assert!(
        error.contains(rel) && error.contains("reviewed bytes"),
        "{error}"
    );
    assert!(
        !error.contains("untrusted bridge runtime path"),
        "reviewed bytes must fail before a runtime binary is opened: {error}"
    );
}

#[test]
fn confined_pi_mismatched_runtime_digest_is_refused_before_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = fixture_runtime(tmp.path(), "digest");
    let binding = RuntimeBinding {
        mission_id: "digest-m".into(),
        attempt_id: "digest-a".into(),
        session_id: "digest-s".into(),
        generation: 2,
    };
    let key = payload_hash(&json!([
        &binding.mission_id,
        &binding.attempt_id,
        &binding.session_id
    ]))
    .unwrap();
    let session = tmp.path().join(format!("session-{key}"));
    fs::create_dir_all(&session).unwrap();
    fs::set_permissions(&session, fs::Permissions::from_mode(0o700)).unwrap();
    let current = br#"{"version":1,"phase":"opened","generation":1}"#;
    let journal = b"untouched-retirement";
    fs::write(session.join("current.json"), current).unwrap();
    fs::write(session.join("leaf-retirement.json"), journal).unwrap();
    fs::set_permissions(
        session.join("current.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fs::set_permissions(
        session.join("leaf-retirement.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let tool = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .unwrap_or_else(|| panic!("{name} is required; no fallback"))
    };
    let config = ConfinedPiConfig {
        root: tmp.path().into(),
        cgroup_root: tool("SWITCHYARD_CGROUP_ROOT"),
        runtime,
        node: tool("SWITCHYARD_PI_NODE"),
        bubblewrap: tool("SWITCHYARD_BWRAP"),
        launcher: tool("SWITCHYARD_BRIDGE_LAUNCHER"),
        policy: tool("SWITCHYARD_BRIDGE_POLICY"),
        model_endpoint: None,
    };
    let error = match ConfinedPiSession::open_required(
        &config,
        binding,
        "answer",
        false,
        Some("sha256:not-the-installed-runtime"),
    ) {
        Ok(_) => panic!("a mismatched runtime digest must be refused"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("host ledger pin"), "{error}");
    assert!(
        session.join("owner.lock").is_file(),
        "open did not reach the planted session directory"
    );
    assert_eq!(fs::read(session.join("current.json")).unwrap(), current);
    assert_eq!(
        fs::read(session.join("leaf-retirement.json")).unwrap(),
        journal
    );
    for entry in fs::read_dir(&session).unwrap() {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        assert!(
            !name.starts_with("recovered-") && !name.starts_with("abandoned-"),
            "recovery wrote {name}"
        );
    }
}

#[test]
fn confined_pi_runtime_provenance_mismatch_fails_before_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fixture_runtime(tmp.path(), "provenance");
    let mut provenance: Value =
        serde_json::from_slice(&fs::read(root.join(".artifacts/provenance.json")).unwrap())
            .unwrap();
    provenance["sourceCommit"] = json!("0000000000000000000000000000000000000000");
    fs::write(
        root.join(".artifacts/provenance.json"),
        serde_json::to_vec(&provenance).unwrap(),
    )
    .unwrap();
    fs::set_permissions(
        root.join(".artifacts/provenance.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let config = ConfinedPiConfig {
        root: tmp.path().into(),
        cgroup_root: tmp.path().into(),
        runtime: root.clone(),
        node: PathBuf::from("/no/such/node"),
        bubblewrap: PathBuf::from("/no/such/bwrap"),
        launcher: PathBuf::from("/no/such/launcher"),
        policy: PathBuf::from("/no/such/policy"),
        model_endpoint: None,
    };
    let error = policy::check(&config).unwrap_err().to_string();
    assert!(error.contains("provenance"), "{error}");
    assert!(
        !error.contains("untrusted bridge runtime path"),
        "provenance must fail before a runtime binary is opened: {error}"
    );

    let pins_root = fixture_runtime(tmp.path(), "pins");
    let mut pins: Value =
        serde_json::from_slice(&fs::read(pins_root.join("pins.json")).unwrap()).unwrap();
    pins["commit"] = json!("1111111111111111111111111111111111111111");
    fs::write(
        pins_root.join("pins.json"),
        serde_json::to_vec(&pins).unwrap(),
    )
    .unwrap();
    fs::set_permissions(
        pins_root.join("pins.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let error = policy::inspect_runtime(&pins_root).unwrap_err().to_string();
    assert!(error.contains("reviewed pins"), "{error}");
    assert!(!error.contains("provenance"), "{error}");
}

#[test]
fn confined_pi_changed_runtime_root_on_existing_ledger_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let first = fixture_runtime(tmp.path(), "alpha");
    let second = fixture_runtime(tmp.path(), "beta");
    let left = policy::inspect_runtime(&first).unwrap();
    let right = policy::inspect_runtime(&second).unwrap();
    assert_ne!(left.digest, right.digest);
    let previous = json!({
        "phase": "opened",
        "requiresExisting": true,
        "configuration": {"runtimeRoot": left.record()},
    });
    assert!(!allows_configuration(
        &previous,
        &json!({"runtimeRoot": right.record()}),
        false
    ));
    assert!(allows_configuration(
        &previous,
        &json!({"runtimeRoot": left.record()}),
        false
    ));

    let host_config = |runtime: PathBuf| ConfinedPiConfig {
        root: tmp.path().into(),
        cgroup_root: tmp.path().into(),
        runtime,
        node: PathBuf::from("/uninstalled"),
        bubblewrap: PathBuf::from("/uninstalled"),
        launcher: PathBuf::from("/uninstalled"),
        policy: PathBuf::from("/uninstalled"),
        model_endpoint: None,
    };
    let saved =
        super::super::confined_runtime::ConfinedRuntime::new(host_config(first.clone()), "answer")
            .unwrap()
            .configuration();
    let swapped =
        super::super::confined_runtime::ConfinedRuntime::new(host_config(second), "answer")
            .unwrap()
            .configuration();
    assert_eq!(saved["runtimeRoot"], left.digest);
    assert_ne!(saved["runtime"], swapped["runtime"]);
    assert_ne!(saved["runtimeRoot"], swapped["runtimeRoot"]);
    let db = tmp.path().join("mission.sqlite");
    let mut store = Store::open_writer(&db).unwrap();
    store.migrate().unwrap();
    store
        .create_mission(CreatedMission {
            runtime_config: canonicalize(&saved).unwrap(),
            id: "m-runtime".into(),
            task_id: "t-runtime".into(),
            attempt_id: "a-runtime".into(),
            session_id: "s-runtime".into(),
            objective: "one confined runtime".into(),
            acceptance: "original install".into(),
            content_digest: "digest".into(),
            repo: RepoIdentity {
                toplevel: "/repo".into(),
                head_sha: "abc".into(),
                branch: "main".into(),
                porcelain_hash: "sha256:00".into(),
                origin_url: None,
            },
            issue: SourceIssue {
                provider: "github".into(),
                number: 97,
                title: "runtime install".into(),
                snapshot_at: 0,
            },
            actor: "operator".into(),
            max_repairs: 1,
            max_plan_revisions: 1,
            successor_of_mission_id: None,
            successor_of_attempt_id: None,
        })
        .unwrap();
    store.require_runtime_config(&saved).unwrap();
    let error = store
        .require_runtime_config(&swapped)
        .unwrap_err()
        .to_string();
    assert!(error.contains("no adapter replacement"), "{error}");
    assert!(error.contains("runtimeRoot"), "{error}");
}
