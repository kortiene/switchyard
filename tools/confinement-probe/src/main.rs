//! Fixed deterministic fixtures. Not a user-facing execution command.
#![forbid(unsafe_code)]
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};
use switchyard_confinement_probe::probe::Sandbox;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("supervisor") => {
            let root = Path::new(args.get(2).expect("fixture root"));
            let mut sandbox = Sandbox::spawn(root, &std::env::current_exe().unwrap(), "tree")
                .expect("sandbox prerequisites");
            // The test kills this process without Drop. Bubblewrap must arrange
            // descendant death independently of cooperative Rust cleanup.
            sandbox
                .wait(Duration::from_secs(30))
                .expect("supervisor fixture deadline");
        }
        Some("check") => check(),
        Some("tree") => {
            let mut descendant = Command::new("/usr/bin/setsid")
                .args(["/probe", "branch"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("detached branch");
            heartbeat("tree");
            let _ = descendant.kill();
            let _ = descendant.wait();
            panic!("test did not terminate the tree before emergency lifetime limit");
        }
        Some("branch") => {
            let mut descendant = Command::new("/probe")
                .arg("leaf")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("grandchild");
            heartbeat("branch");
            let _ = descendant.kill();
            let _ = descendant.wait();
            panic!("test did not terminate the branch before emergency lifetime limit");
        }
        Some("leaf") => heartbeat("leaf"),
        Some("flood") => {
            let chunk = [b'x'; 8192];
            loop {
                std::io::stderr().write_all(&chunk).unwrap();
            }
        }
        _ => panic!("unknown fixture; use the dedicated confinement test harness"),
    }
}

fn heartbeat(role: &str) {
    // Namespace-local process identity lets the outside observer map each role
    // through NSpid, rather than mistaking an unrelated host process for it.
    fs::write(
        format!("/workspace/{role}.ready"),
        std::process::id().to_string(),
    )
    .unwrap();
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("/workspace/{role}.heartbeat"))
        .unwrap();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(25) {
        log.write_all(b"tick\n").unwrap();
        log.flush().unwrap();
        sleep(Duration::from_millis(20));
    }
}

fn check() {
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read("/case.json").unwrap()).unwrap();
    assert_eq!(std::env::current_dir().unwrap(), Path::new("/workspace"));
    fs::write("/workspace/allowed.txt", "allowed workspace write\n").unwrap();
    fs::write("/tmp/allowed", "private tmpfs\n").unwrap();
    for key in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "ANTHROPIC_API_KEY",
        "AWS_SECRET_ACCESS_KEY",
        "SSH_AUTH_SOCK",
        "HOME",
    ] {
        assert!(
            std::env::var_os(key).is_none(),
            "inherited credential/environment key: {key}"
        );
    }
    for path in config["deniedPaths"].as_array().unwrap() {
        let path = path.as_str().unwrap();
        assert!(
            fs::read(path).is_err(),
            "protected read unexpectedly succeeded"
        );
        assert!(
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)
                .is_err(),
            "protected write unexpectedly succeeded"
        );
    }
    for path in [
        "/workspace/escape",
        "/workspace/relative-escape",
        "/workspace/proc-escape",
    ] {
        assert!(fs::read(path).is_err(), "symlink escape read succeeded");
        assert!(
            fs::write(path, "escape").is_err(),
            "symlink escape write succeeded"
        );
    }
    assert!(fs::write("/workspace/.git", "replace Git authority").is_err());
    assert!(fs::remove_file("/workspace/.git").is_err());
    assert!(fs::rename("/workspace/.git", "/workspace/stolen-git").is_err());
    assert!(fs::write("/probe", "replace fixture").is_err());
    assert!(fs::write("/case.json", "replace config").is_err());
    assert!(fs::write("/outside-workspace", "escape").is_err());
    let status = fs::read_to_string("/proc/self/status").unwrap();
    assert!(status.lines().any(|s| s == "NoNewPrivs:\t1"));
    assert!(status.lines().any(|s| s == "CapEff:\t0000000000000000"));
    assert!(
        !Command::new("/usr/bin/unshare")
            .args(["--user", "--map-root-user", "/bin/true"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    let address: SocketAddr = config["hostTcp"].as_str().unwrap().parse().unwrap();
    assert!(
        TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_err(),
        "host network reachable"
    );
    assert!(
        UnixStream::connect(config["hostSocket"].as_str().unwrap()).is_err(),
        "host socket reachable"
    );
    fs::write(
        "/workspace/check.passed",
        "filesystem, environment, capabilities, userns and network denied\n",
    )
    .unwrap();
}
