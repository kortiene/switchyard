use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub const LIMIT: Duration = Duration::from_secs(10);

pub fn backend() -> PathBuf {
    std::env::var_os("SWITCHYARD_BWRAP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/bin/bwrap"))
}

/// Fixed launch policy for a trusted fixture, never arbitrary agent code.
pub fn command(root: &Path, fixture: &Path, mode: &str) -> io::Result<Command> {
    use std::os::unix::fs::PermissionsExt;
    if !["check", "tree", "flood"].contains(&mode) {
        return Err(io::Error::other("unknown deterministic fixture mode"));
    }
    let backend = backend();
    if !backend.is_absolute() || fs::metadata(&backend)?.permissions().mode() & 0o6000 != 0 {
        return Err(io::Error::other(
            "require an absolute, non-setuid/non-setgid Bubblewrap",
        ));
    }
    let version = Command::new(&backend)
        .arg("--version")
        .env_clear()
        .output()?;
    if !version.status.success() || version.stdout != b"bubblewrap 0.9.0\n" {
        return Err(io::Error::other("reviewed probe requires Bubblewrap 0.9.0"));
    }
    let root = fs::canonicalize(root)?;
    let fixture = fs::canonicalize(fixture)?;
    // A fixed trusted shell duplicates the status file onto fd 3, then execs
    // prlimit (no extra supervisor). Fixture stdout is disconnected from it.
    // Bubblewrap closes its status descriptor before executing the fixture.
    let mut cmd = Command::new("/bin/sh");
    cmd.args([
        "-c",
        "exec 3>&1; exec 1>/dev/null; exec \"$@\"",
        "confinement-launch",
        "/usr/bin/prlimit",
    ]);
    cmd.env_clear()
        .args([
            "--as=536870912:536870912",
            "--cpu=5:5",
            "--fsize=1048576:1048576",
            "--nofile=64:64",
            "--",
        ])
        .arg(backend)
        .args([
            "--unshare-user",
            "--disable-userns",
            "--assert-userns-disabled",
            "--unshare-pid",
            "--unshare-net",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-cgroup",
            "--cap-drop",
            "ALL",
            "--die-with-parent",
            "--new-session",
            "--json-status-fd",
            "3",
            "--clearenv",
            "--setenv",
            "PATH",
            "/usr/bin:/bin",
            "--setenv",
            "LANG",
            "C",
            "--ro-bind",
            "/usr",
            "/usr",
            "--symlink",
            "usr/bin",
            "/bin",
            "--symlink",
            "usr/lib",
            "/lib",
            "--symlink",
            "usr/lib64",
            "/lib64",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--size",
            "8388608",
            "--tmpfs",
            "/tmp",
            "--bind",
        ])
        .arg(root.join("workspace"))
        .arg("/workspace")
        .arg("--ro-bind")
        .arg(root.join("masked-git"))
        .arg("/workspace/.git")
        .arg("--ro-bind")
        .arg(fixture)
        .arg("/probe")
        .arg("--ro-bind")
        .arg(root.join("case.json"))
        .arg("/case.json")
        .args([
            "--chdir",
            "/workspace",
            "--remount-ro",
            "/",
            "--",
            "/probe",
            mode,
        ])
        .stdin(Stdio::null())
        .stdout(File::create(root.join("status.jsonl"))?)
        .stderr(File::create(root.join("stderr.log"))?);
    Ok(cmd)
}

pub struct Sandbox {
    pub child: Child,
    pub root: PathBuf,
}
impl Sandbox {
    pub fn spawn(root: &Path, fixture: &Path, mode: &str) -> io::Result<Self> {
        Ok(Self {
            child: command(root, fixture, mode)?.spawn()?,
            root: root.to_path_buf(),
        })
    }
    pub fn diagnostics(&self) -> String {
        fs::read_to_string(self.root.join("stderr.log")).unwrap_or_default()
    }
    pub fn wait(&mut self, deadline: Duration) -> io::Result<ExitStatus> {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if start.elapsed() >= deadline {
                self.stop()?;
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "sandbox deadline; supervisor killed",
                ));
            }
            sleep(Duration::from_millis(10));
        }
    }
    pub fn stop(&mut self) -> io::Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        self.child.wait()?;
        Ok(())
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// PID reuse is not evidence about the original process. Z means exited (cannot
/// write); reaping belongs to the kernel's adopting parent after supervisor loss.
#[derive(Clone, Debug)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start: String,
}
fn stat(pid: u32) -> io::Result<(char, String)> {
    let raw = fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| {
        // Linux ESRCH can race a procfs read when the process disappears.
        if e.raw_os_error() == Some(3) {
            io::Error::new(io::ErrorKind::NotFound, e)
        } else {
            e
        }
    })?;
    let (_, rest) = raw
        .rsplit_once(") ")
        .ok_or_else(|| io::Error::other("bad proc stat"))?;
    let fields: Vec<_> = rest.split_whitespace().collect();
    if fields.len() < 20 {
        return Err(io::Error::other("short proc stat"));
    }
    Ok((fields[0].chars().next().unwrap(), fields[19].to_string()))
}
impl ProcessIdentity {
    pub fn live(&self) -> io::Result<bool> {
        match stat(self.pid) {
            Ok((state, start)) => Ok(start == self.start && state != 'Z' && state != 'X'),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
}

pub fn namespace_from_status(root: &Path) -> io::Result<PathBuf> {
    let log = fs::read_to_string(root.join("status.jsonl"))?;
    for line in log.lines() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(pid) = value["child-pid"].as_u64()
        {
            return fs::read_link(format!("/proc/{pid}/ns/pid"));
        }
    }
    Err(io::Error::other("Bubblewrap did not report its child PID"))
}

pub fn namespace_members(namespace: &Path) -> io::Result<Vec<ProcessIdentity>> {
    let mut members = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        // Unrelated users' namespaces may be unreadable. The fixture namespace
        // must have observable members before any termination assertion.
        if fs::read_link(entry.path().join("ns/pid")).ok().as_deref() != Some(namespace) {
            continue;
        }
        match stat(pid) {
            Ok((_, start)) => members.push(ProcessIdentity { pid, start }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
    }
    Ok(members)
}

pub fn require_quiescent(namespace: &Path, observed: &[ProcessIdentity]) -> io::Result<()> {
    let start = Instant::now();
    loop {
        let remaining = namespace_members(namespace)?;
        let mut live = false;
        for p in observed.iter().chain(remaining.iter()) {
            live |= p.live()?;
        }
        if !live {
            return Ok(());
        }
        if start.elapsed() > LIMIT {
            return Err(io::Error::other("writers remain; workspace reuse denied"));
        }
        sleep(Duration::from_millis(10));
    }
}
