use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

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
    if let Err(error) = apply_limits(&path) {
        let _ = fs::remove_dir(&path);
        return Err(error);
    }
    identity_of(&path)
}

pub(crate) fn leaf_name(operation_id: &str) -> String {
    let hex: String = operation_id
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    format!("sy-{}", &hex[..32.min(hex.len())])
}

pub(crate) fn observe(expected: &CgroupIdentity) -> Result<Quiescence> {
    read_state(expected, false)
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

fn apply_limits(path: &Path) -> Result<()> {
    write_file(path, "memory.max", MEMORY_MAX)?;
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
    let meta = match fs::metadata(&expected.path) {
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
    if meta.dev() != expected.dev || meta.ino() != expected.ino || boot != expected.boot_id {
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
