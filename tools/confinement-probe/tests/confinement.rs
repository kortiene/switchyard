#![forbid(unsafe_code)]
use std::fs;
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::symlink;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};
use switchyard_confinement_probe::probe::*;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_confinement-fixture"))
}
fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

struct World {
    temp: tempfile::TempDir,
    tcp: TcpListener,
    _socket: UnixListener,
    index: Vec<u8>,
    head: Vec<u8>,
    marker: Vec<u8>,
    protected: Vec<PathBuf>,
}
impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("original")).unwrap();
        let repo = root.join("original");
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["config", "user.email", "probe@example.com"]);
        git(&repo, &["config", "user.name", "Confinement probe"]);
        fs::write(repo.join("source.txt"), "PROTECTED\n").unwrap();
        git(&repo, &["add", "source.txt"]);
        git(&repo, &["commit", "-m", "fixture"]);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                root.join("workspace").to_str().unwrap(),
                "HEAD",
            ],
        );
        let mut protected = vec![
            repo.join("source.txt"),
            repo.join(".git/HEAD"),
            repo.join(".git/index"),
        ];
        for name in [
            "control/mission.sqlite",
            "control/bridge.json",
            "home/.ssh/key",
            "home/.config/gh/hosts.yml",
        ] {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "PROTECTED\n").unwrap();
            protected.push(path);
        }
        symlink(repo.join("source.txt"), root.join("workspace/escape")).unwrap();
        symlink(
            "../original/source.txt",
            root.join("workspace/relative-escape"),
        )
        .unwrap();
        symlink(
            format!("/proc/1/root{}", repo.join("source.txt").display()),
            root.join("workspace/proc-escape"),
        )
        .unwrap();
        fs::write(
            root.join("masked-git"),
            "Git administration is unavailable in this probe\n",
        )
        .unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = UnixListener::bind(root.join("host.sock")).unwrap();
        // Prove the negative targets actually exist and are accessible outside.
        TcpStream::connect(tcp.local_addr().unwrap()).unwrap();
        UnixStream::connect(root.join("host.sock")).unwrap();
        for p in &protected {
            assert!(!fs::read(p).unwrap().is_empty());
        }
        let mut denied = protected.clone();
        denied.push(repo.join("new-file"));
        denied.push(root.join("control/new-file"));
        fs::write(
            root.join("case.json"),
            serde_json::to_vec(&serde_json::json!({
                "deniedPaths":denied, "hostTcp":tcp.local_addr().unwrap().to_string(),
                "hostSocket":root.join("host.sock")
            }))
            .unwrap(),
        )
        .unwrap();
        let index = fs::read(repo.join(".git/index")).unwrap();
        let head = git(&repo, &["rev-parse", "HEAD"]);
        let marker = fs::read(root.join("workspace/.git")).unwrap();
        Self {
            temp,
            tcp,
            _socket: socket,
            index,
            head,
            marker,
            protected,
        }
    }
    fn root(&self) -> &Path {
        self.temp.path()
    }
    fn assert_unchanged(&self) {
        let repo = self.root().join("original");
        assert_eq!(fs::read(repo.join(".git/index")).unwrap(), self.index);
        assert_eq!(git(&repo, &["rev-parse", "HEAD"]), self.head);
        assert!(git(&repo, &["status", "--porcelain"]).is_empty());
        assert_eq!(
            fs::read(self.root().join("workspace/.git")).unwrap(),
            self.marker
        );
        for p in &self.protected {
            if p.ends_with("HEAD") || p.ends_with("index") {
                continue;
            }
            assert_eq!(fs::read(p).unwrap(), b"PROTECTED\n");
        }
        assert!(!self.root().join("control/new-file").exists());
        TcpStream::connect(self.tcp.local_addr().unwrap()).unwrap();
    }
}

impl Drop for World {
    fn drop(&mut self) {
        if let Some(destination) = std::env::var_os("SWITCHYARD_CONFINEMENT_EVIDENCE") {
            let target = PathBuf::from(destination).join(self.root().file_name().unwrap());
            fs::create_dir_all(&target).expect("preserve confinement evidence");
            for name in ["stdout.log", "stderr.log", "supervisor.log", "case.json"] {
                let source = self.root().join(name);
                if source.exists() {
                    fs::copy(source, target.join(name)).expect("save diagnostic");
                }
            }
        }
    }
}

fn wait_ready(root: &Path) {
    let start = Instant::now();
    while !["tree", "branch", "leaf"]
        .iter()
        .all(|r| root.join(format!("workspace/{r}.ready")).exists())
    {
        assert!(
            start.elapsed() < LIMIT,
            "no live fixture: {}",
            fs::read_to_string(root.join("stderr.log")).unwrap_or_default()
        );
        sleep(Duration::from_millis(10));
    }
}
fn observe(root: &Path) -> (PathBuf, Vec<ProcessIdentity>) {
    wait_ready(root);
    let ns = namespace_from_status(root).unwrap();
    assert_ne!(
        ns,
        fs::read_link("/proc/self/ns/pid").unwrap(),
        "PID namespace was not isolated"
    );
    let members = namespace_members(&ns).unwrap();
    // Prove each named writer is among the observed host PIDs, including the
    // setsid child and its grandchild (not just the direct process handle).
    for role in ["tree", "branch", "leaf"] {
        let local_pid = fs::read_to_string(root.join(format!("workspace/{role}.ready"))).unwrap();
        assert!(
            members.iter().any(|p| {
                fs::read_to_string(format!("/proc/{}/status", p.pid))
                    .unwrap()
                    .lines()
                    .any(|line| {
                        line.starts_with("NSpid:")
                            && line.split_whitespace().last() == Some(local_pid.as_str())
                    })
            }),
            "unobserved {role}"
        );
    }
    assert!(members.len() >= 3);
    println!("observed namespace={ns:?} processes={members:?}");
    (ns, members)
}
fn verify_stop_and_reuse(world: &World, ns: &Path, observed: &[ProcessIdentity]) {
    require_quiescent(ns, observed)
        .expect("workspace remains blocked until no live writers remain");
    let heartbeats: Vec<_> = ["tree", "branch", "leaf"]
        .map(|r| fs::read(world.root().join(format!("workspace/{r}.heartbeat"))).unwrap())
        .into();
    sleep(Duration::from_millis(200));
    for (r, before) in ["tree", "branch", "leaf"].iter().zip(heartbeats) {
        assert_eq!(
            fs::read(world.root().join(format!("workspace/{r}.heartbeat"))).unwrap(),
            before,
            "late writer after stop"
        );
    }
    let mut replacement = Sandbox::spawn(world.root(), &fixture(), "check").unwrap();
    let status = replacement.wait(LIMIT).unwrap();
    assert!(
        status.success(),
        "replacement failed: {}",
        replacement.diagnostics()
    );
    world.assert_unchanged();
}

#[test]
fn filesystem_network_credentials_and_git_authority_are_confined() {
    let world = World::new();
    let mut cmd = command(world.root(), &fixture(), "check").unwrap();
    // Deliberately put sentinel secrets in the launcher's environment; the
    // inner clearenv must remove them. Never use real credentials in fixtures.
    cmd.env("GH_TOKEN", "SENTINEL_NOT_A_REAL_TOKEN")
        .env("ANTHROPIC_API_KEY", "SENTINEL_NOT_A_REAL_KEY");
    let mut sandbox = Sandbox {
        child: cmd.spawn().unwrap(),
        root: world.root().to_path_buf(),
    };
    let status = sandbox.wait(LIMIT).unwrap();
    assert!(
        status.success(),
        "confinement prerequisites or boundary failed: {}",
        sandbox.diagnostics()
    );
    assert!(world.root().join("workspace/check.passed").exists());
    assert_eq!(
        fs::read(world.root().join("workspace/allowed.txt")).unwrap(),
        b"allowed workspace write\n"
    );
    world.assert_unchanged();
}

#[test]
fn explicit_stop_and_deadline_terminate_detached_descendants_before_reuse() {
    for deadline in [false, true] {
        let world = World::new();
        let mut sandbox = Sandbox::spawn(world.root(), &fixture(), "tree").unwrap();
        let (ns, observed) = observe(world.root());
        if deadline {
            assert_eq!(
                sandbox.wait(Duration::from_millis(100)).unwrap_err().kind(),
                std::io::ErrorKind::TimedOut
            );
        } else {
            sandbox.stop().unwrap();
        }
        verify_stop_and_reuse(&world, &ns, &observed);
    }
}

#[test]
fn abrupt_supervisor_death_terminates_detached_descendants_before_reuse() {
    let world = World::new();
    let child = Command::new(fixture())
        .arg("supervisor")
        .arg(world.root())
        .env_clear()
        .env("SWITCHYARD_BWRAP", backend())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(world.root().join("supervisor.log")).unwrap())
        .spawn()
        .unwrap();
    let mut supervisor = Sandbox {
        child,
        root: world.root().to_path_buf(),
    };
    let (ns, observed) = observe(world.root());
    supervisor.child.kill().unwrap();
    supervisor.child.wait().unwrap();
    verify_stop_and_reuse(&world, &ns, &observed);
}

#[test]
fn output_is_bounded_and_missing_backend_fails_closed() {
    let world = World::new();
    let mut sandbox = Sandbox::spawn(world.root(), &fixture(), "flood").unwrap();
    assert!(!sandbox.wait(LIMIT).unwrap().success());
    // Do not accept backend startup failure as evidence of the output limit.
    assert_eq!(
        fs::metadata(world.root().join("stderr.log")).unwrap().len(),
        1048576
    );
    let status = Command::new(fixture())
        .arg("supervisor")
        .arg(world.root())
        .env_clear()
        .env("SWITCHYARD_BWRAP", world.root().join("missing-backend"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
    assert!(!world.root().join("workspace/tree.ready").exists());
    world.assert_unchanged();
}
