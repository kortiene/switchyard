use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

#[derive(Clone, Debug)]
pub(crate) struct CgroupIdentity {
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
    pub boot_id: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Quiescence {
    pub proven: bool,
    pub reason: String,
    pub populated: Option<u64>,
    pub events: String,
    pub pids_max: u64,
    pub oom_kill: u64,
}

// 64 MiB is enough to prove memory.oom.group and stays inside the CI job's
// service budget. A 256 MiB spike on the hosted runner was killed with SIGTERM.
const MEMORY_MAX: &str = "67108864";
const PIDS_MAX: &str = "64";
const CPU_MAX: &str = "50000 100000";

pub(crate) fn validate_delegation(root: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(root).map_err(|error| {
        Error::guard(format!(
            "cgroup delegation root is unavailable ({error}); refusing an unconfined or rlimit-only fallback"
        ))
    })?;
    if !root.join("cgroup.controllers").is_file() && !root.join("cgroup.subtree_control").is_file()
    {
        return Err(Error::guard(
            "cgroup delegation root is not a cgroup v2 directory",
        ));
    }
    // Same-process callers (parallel tests) must not share a leaf name.
    static PROBE_SEQ: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        "probe-{}-{}-{}",
        std::process::id(),
        crate::domain::now_ms(),
        PROBE_SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let leaf = create_leaf(&root, &name)?;
    let proved = kill_and_prove(&leaf, Duration::from_secs(2))?;
    let _ = fs::remove_dir(&leaf.path);
    if !proved.proven {
        return Err(Error::guard(format!(
            "cgroup delegation probe could not prove populated 0 ({})",
            proved.reason
        )));
    }
    Ok(root)
}

pub(crate) fn create_leaf(root: &Path, name: &str) -> Result<CgroupIdentity> {
    create_limited_leaf(root, name, MEMORY_MAX)
}

/// Node needs a separate reviewed memory allowance; execution limits stay unchanged.
#[cfg(feature = "pi-contract")]
pub(crate) fn create_bridge_leaf(root: &Path, name: &str) -> Result<CgroupIdentity> {
    create_limited_leaf(root, name, "268435456")
}

fn create_limited_leaf(root: &Path, name: &str, memory_max: &str) -> Result<CgroupIdentity> {
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(Error::guard(
            "cgroup leaf name is not a single path segment",
        ));
    }
    let path = root.join(name);
    if path.exists() {
        return Err(Error::guard(
            "cgroup leaf already exists; recorded leaves are not reused",
        ));
    }
    fs::create_dir(&path).map_err(|error| {
        Error::guard(format!(
            "cannot create a delegated cgroup leaf ({error}); refusing an unconfined fallback"
        ))
    })?;
    if let Err(error) = apply_limits(&path, memory_max) {
        let _ = fs::remove_dir(&path);
        return Err(error);
    }
    identity_of(&path)
}

pub(crate) fn leaf_name(operation_id: &str) -> Result<String> {
    let Some(hash) = operation_id.strip_prefix("op-v1:") else {
        return Err(Error::guard(
            "cgroup leaf name requires an op-v1 operation id",
        ));
    };
    if hash.len() < 32 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::guard(
            "cgroup leaf name requires 32 hex digits after op-v1:",
        ));
    }
    Ok(format!("sy-{}", &hash[..32]))
}

pub(crate) fn observe(expected: &CgroupIdentity) -> Result<Quiescence> {
    read_state(expected, false)
}

/// Delete an obsolete leaf only after a fresh identity and emptiness check.
/// The caller must durably authorize deletion before this call and must never
/// supply the leaf referenced by its current recovery record.
#[cfg(feature = "pi-contract")]
pub(crate) fn remove_empty_leaf(expected: &CgroupIdentity) -> Result<Quiescence> {
    let proof = observe(expected)?;
    if !proof.proven {
        return Err(Error::guard(format!(
            "cgroup removal refused: {}",
            proof.reason
        )));
    }
    fs::remove_dir(&expected.path)
        .map_err(|error| Error::guard(format!("cannot remove empty obsolete cgroup: {error}")))?;
    Ok(proof)
}

/// Kill the recorded leaf even when it already reads empty, then require
/// `populated 0`. A missing or replaced leaf is returned without a write.
pub(crate) fn rekill_and_prove(expected: &CgroupIdentity, timeout: Duration) -> Result<Quiescence> {
    let state = read_state(expected, true)?;
    if state.reason == "mismatched" || state.reason == "missing" {
        return Ok(state);
    }
    fs::write(expected.path.join("cgroup.kill"), "1\n").map_err(|error| {
        Error::guard(format!(
            "cgroup.kill failed ({error}); termination is unproven"
        ))
    })?;
    let start = Instant::now();
    loop {
        let mut state = read_state(expected, true)?;
        if state.proven || state.reason == "mismatched" || state.reason == "missing" {
            return Ok(state);
        }
        if start.elapsed() > timeout {
            state.proven = false;
            state.reason = "nonempty".into();
            return Ok(state);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub(crate) fn kill_and_prove(expected: &CgroupIdentity, timeout: Duration) -> Result<Quiescence> {
    let mut state = read_state(expected, true)?;
    if state.reason == "mismatched" || state.reason == "missing" {
        return Ok(state);
    }
    if state.proven {
        return Ok(state);
    }
    fs::write(expected.path.join("cgroup.kill"), "1\n").map_err(|error| {
        Error::guard(format!(
            "cgroup.kill failed ({error}); termination is unproven"
        ))
    })?;
    let start = Instant::now();
    loop {
        state = read_state(expected, true)?;
        if state.proven || state.reason == "mismatched" || state.reason == "missing" {
            return Ok(state);
        }
        if start.elapsed() > timeout {
            state.proven = false;
            state.reason = "nonempty".into();
            return Ok(state);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub(crate) fn git_file_is_regular(workspace: &Path) -> Result<()> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags};
    let directory = rustix::fs::open(
        workspace,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| Error::guard(format!("cannot open the worktree directory: {error}")))?;
    let stat = rustix::fs::statat(&directory, ".git", AtFlags::SYMLINK_NOFOLLOW).map_err(|error| {
        Error::guard(format!(
            "workspace .git cannot be inspected without following a symlink ({error}); refusing to mount"
        ))
    })?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(Error::guard(
            "workspace .git is not a regular file; refusing to let a symlink redirect the mount",
        ));
    }
    Ok(())
}

fn apply_limits(path: &Path, memory_max: &str) -> Result<()> {
    write_file(path, "memory.max", memory_max)?;
    write_file(path, "memory.swap.max", "0")?;
    write_file(path, "memory.oom.group", "1")?;
    write_file(path, "pids.max", PIDS_MAX)?;
    write_file(path, "cpu.max", CPU_MAX)?;
    for required in [
        "memory.max",
        "memory.swap.max",
        "pids.max",
        "cpu.max",
        "cgroup.kill",
    ] {
        if !path.join(required).exists() {
            return Err(Error::guard(format!(
                "delegated cgroup is missing {required}; refusing an rlimit-only fallback"
            )));
        }
    }
    Ok(())
}

fn write_file(dir: &Path, name: &str, value: &str) -> Result<()> {
    fs::write(dir.join(name), value).map_err(|error| {
        Error::guard(format!(
            "cannot set cgroup {name} ({error}); refusing an rlimit-only fallback"
        ))
    })
}

fn identity_of(path: &Path) -> Result<CgroupIdentity> {
    use std::os::unix::fs::MetadataExt;
    let meta =
        fs::metadata(path).map_err(|error| Error::guard(format!("cannot stat cgroup: {error}")))?;
    Ok(CgroupIdentity {
        path: fs::canonicalize(path).map_err(Error::storage)?,
        dev: meta.dev(),
        ino: meta.ino(),
        boot_id: boot_id()?,
    })
}

fn boot_id() -> Result<String> {
    let text = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|error| Error::guard(format!("cannot read boot_id: {error}")))?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(Error::guard("boot_id is empty"));
    }
    Ok(text)
}

fn read_state(expected: &CgroupIdentity, _killing: bool) -> Result<Quiescence> {
    use std::os::unix::fs::MetadataExt;
    let mut state = Quiescence {
        proven: false,
        reason: "unproven".into(),
        populated: None,
        events: String::new(),
        pids_max: 0,
        oom_kill: 0,
    };
    let meta = match fs::symlink_metadata(&expected.path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            state.reason = "missing".into();
            return Ok(state);
        }
        Err(error) => {
            return Err(Error::guard(format!(
                "cannot stat recorded cgroup: {error}"
            )));
        }
    };
    let boot = boot_id()?;
    if !meta.is_dir()
        || meta.dev() != expected.dev
        || meta.ino() != expected.ino
        || boot != expected.boot_id
    {
        state.reason = "mismatched".into();
        return Ok(state);
    }
    let events = fs::read_to_string(expected.path.join("cgroup.events"))
        .map_err(|error| Error::guard(format!("cannot read cgroup.events: {error}")))?;
    state.events = events.clone();
    state.populated = events.lines().find_map(|line| {
        let (key, value) = line.split_once(' ')?;
        (key == "populated").then(|| value.parse().ok()).flatten()
    });
    state.pids_max = event_counter(&expected.path.join("pids.events"), "max");
    let memory = expected.path.join("memory.events");
    state.oom_kill = event_counter(&memory, "oom_kill")
        + event_counter(&memory, "oom_group_kill")
        + event_counter(&memory, "oom");
    if state.populated == Some(0) {
        state.proven = true;
        state.reason = "populated 0".into();
    } else {
        state.reason = "nonempty".into();
    }
    Ok(state)
}

fn event_counter(path: &Path, key: &str) -> u64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let (name, value) = line.split_once(' ')?;
                (name == key).then(|| value.parse().ok()).flatten()
            })
        })
        .unwrap_or(0)
}

pub(crate) fn within_root(root: &Path, leaf: &Path) -> bool {
    let Ok(root) = fs::canonicalize(root) else {
        return false;
    };
    let Ok(leaf) = fs::canonicalize(leaf) else {
        return false;
    };
    leaf.starts_with(&root) && leaf != root
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "pi-contract")]
    #[test]
    fn confined_pi_cgroup_removal_refuses_nonempty_changed_and_symlinked_leaves() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let leaf = temporary.path().join("bridge-owned");
        std::fs::create_dir(&leaf).unwrap();
        std::fs::write(leaf.join("cgroup.events"), "populated 1\nfrozen 0\n").unwrap();
        std::fs::write(leaf.join("sentinel"), "unchanged").unwrap();
        let expected = super::identity_of(&leaf).unwrap();
        let error = super::remove_empty_leaf(&expected).unwrap_err();
        assert!(error.to_string().contains("nonempty"));
        std::fs::write(leaf.join("cgroup.events"), "populated 0\nfrozen 0\n").unwrap();
        for field in ["inode", "device", "boot"] {
            let mut changed = expected.clone();
            match field {
                "inode" => changed.ino += 1,
                "device" => changed.dev += 1,
                "boot" => changed.boot_id = "another-boot".into(),
                _ => unreachable!(),
            }
            assert!(
                super::remove_empty_leaf(&changed)
                    .unwrap_err()
                    .to_string()
                    .contains("mismatched")
            );
        }
        let alias = temporary.path().join("bridge-alias");
        symlink(&leaf, &alias).unwrap();
        let redirected = super::CgroupIdentity {
            path: alias,
            ..expected
        };
        assert!(
            super::remove_empty_leaf(&redirected)
                .unwrap_err()
                .to_string()
                .contains("mismatched")
        );
        assert_eq!(
            std::fs::read_to_string(leaf.join("sentinel")).unwrap(),
            "unchanged"
        );
    }

    #[test]
    fn kill_and_prove_checks_identity_before_writing_cgroup_kill() {
        let temporary = tempfile::tempdir().unwrap();
        let leaf = temporary.path().join("reused");
        std::fs::create_dir(&leaf).unwrap();
        std::fs::write(leaf.join("cgroup.events"), "populated 1\nfrozen 0\n").unwrap();
        std::fs::write(leaf.join("cgroup.kill"), "untouched\n").unwrap();
        let expected = super::identity_of(&leaf).unwrap();
        let mut cases = Vec::new();
        let mut inode = expected.clone();
        inode.ino = inode.ino.wrapping_add(1);
        cases.push(("inode", inode));
        let mut device = expected.clone();
        device.dev = device.dev.wrapping_add(1);
        cases.push(("device", device));
        let mut boot = expected.clone();
        boot.boot_id = "another-boot".into();
        cases.push(("boot", boot));
        let mut missing = expected.clone();
        missing.path = temporary.path().join("gone");
        cases.push(("missing", missing));
        for (label, changed) in cases {
            let state =
                super::kill_and_prove(&changed, std::time::Duration::from_millis(20)).unwrap();
            assert!(!state.proven, "{label}");
            assert!(
                state.reason == "mismatched" || state.reason == "missing",
                "{label}: {}",
                state.reason
            );
            assert_eq!(
                std::fs::read_to_string(leaf.join("cgroup.kill")).unwrap(),
                "untouched\n",
                "{label} wrote cgroup.kill"
            );
        }
    }

    #[test]
    fn leaf_name_uses_the_op_v1_hash_only() {
        let name = super::leaf_name("op-v1:0123456789abcdef0123456789abcdefaaaaaaaa").unwrap();
        assert_eq!(name, "sy-0123456789abcdef0123456789abcdef");
        assert!(super::leaf_name("op-v2:0123456789abcdef0123456789abcdef").is_err());
        assert!(super::leaf_name("0123456789abcdef0123456789abcdef").is_err());
        assert!(super::leaf_name("op-v1:short").is_err());
        assert!(super::leaf_name("op-v1:").is_err());
    }
}
