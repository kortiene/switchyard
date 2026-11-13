//! S3-B6: the real confined session can reach one host-configured endpoint and
//! no other destination. Full denial remains the default when no endpoint is set.
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::json;
use switchyard_core::{ConfinedPiConfig, ConfinedPiSession, ModelEndpoint, RuntimeBinding};

const O_NOFOLLOW: i32 = 0o400000;

fn env_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} is required; no skip"))
}

struct Stub {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    count: Arc<AtomicUsize>,
    request: mpsc::Receiver<Vec<u8>>,
}

impl Stub {
    fn serve(listener: TcpListener, token: String) -> Self {
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let count = Arc::new(AtomicUsize::new(0));
        let (sent, request) = mpsc::channel();
        let flag = Arc::clone(&stop);
        let hits = Arc::clone(&count);
        let thread = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let hit = hits.fetch_add(1, Ordering::Relaxed) + 1;
                        if hit == 1 {
                            let request = read_headers(&mut stream);
                            let body = token.as_bytes();
                            let header = format!(
                                "HTTP/1.0 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = stream.write_all(header.as_bytes());
                            let _ = stream.write_all(body);
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                            let _ = sent.send(request);
                        }
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            || error.kind() == std::io::ErrorKind::Interrupted =>
                    {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
            count,
            request,
        }
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_headers(stream: &mut TcpStream) -> Vec<u8> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    while !buf.windows(4).any(|window| window == b"\r\n\r\n") && buf.len() < 8192 {
        match stream.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    buf
}

fn read_nofollow(path: &Path) -> Vec<u8> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(path)
        .unwrap_or_else(|error| {
            panic!(
                "host read of {} must not follow a link: {error}",
                path.display()
            )
        });
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn model_endpoint_proxy_reaches_only_the_configured_stub() {
    let root_dir = tempfile::tempdir().unwrap();
    let root = root_dir.path();
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    let protected = root.join("host-authority");
    fs::write(&protected, "host authority sentinel").unwrap();
    let stub_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let stub_port = stub_listener.local_addr().unwrap().port();
    let decoy = TcpListener::bind("127.0.0.1:0").unwrap();
    decoy.set_nonblocking(true).unwrap();
    let decoy_port = decoy.local_addr().unwrap().port();
    // Positive control: both listeners exist outside the sandbox.
    let _decoy_probe = TcpStream::connect(decoy.local_addr().unwrap()).unwrap();
    assert!(decoy.accept().is_ok());
    let socket = root.join("host.sock");
    let _unix = UnixListener::bind(&socket).unwrap();
    let _unix_probe = UnixStream::connect(&socket).unwrap();
    let mut token_bytes = [0u8; 16];
    fs::File::open("/dev/urandom")
        .unwrap()
        .read_exact(&mut token_bytes)
        .unwrap();
    let token = token_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let stub = Stub::serve(stub_listener, token.clone());
    let endpoint = ModelEndpoint::parse(&format!("http://127.0.0.1:{stub_port}/v1")).unwrap();
    let hostile = "http://169.254.169.254/latest";
    let probe = include_str!("../bridge/probe.mjs")
        .lines()
        .filter(|line| !line.starts_with("denied('owner lock"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(root.join("probe.mjs"), probe).unwrap();
    fs::write(
        root.join("case.json"),
        json!({
            "egress": "model-endpoint",
            "endpoint": hostile,
            "protected": [protected],
            "port": decoy_port,
            "stubPort": stub_port,
            "socket": socket,
        })
        .to_string(),
    )
    .unwrap();
    let original = env_path("SWITCHYARD_BRIDGE_LAUNCHER");
    for path in [root, &original] {
        assert!(!path.to_str().unwrap().contains('\''));
    }
    let wrapper = root.join("probe-launcher");
    fs::write(
        &wrapper,
        format!(
            r#"#!/bin/bash
set -euo pipefail
args=()
previous=''
for arg in "$@"; do
  if [[ "$arg" == --unshare-user ]]; then
    args+=(--ro-bind '{root}/probe.mjs' /probe.mjs --ro-bind '{root}/case.json' /case.json)
  fi
  if [[ "$arg" == /runtime/test/bridge-child.ts && "$previous" == --experimental-strip-types ]]; then
    args+=(/probe.mjs)
  else
    args+=("$arg")
  fi
  previous="$arg"
done
exec '{launcher}' "${{args[@]}}"
"#,
            root = root.display(),
            launcher = original.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let config = ConfinedPiConfig {
        root: root.into(),
        cgroup_root: env_path("SWITCHYARD_CGROUP_ROOT"),
        runtime: env_path("SWITCHYARD_PI_RUNTIME"),
        node: env_path("SWITCHYARD_PI_NODE"),
        bubblewrap: env_path("SWITCHYARD_BWRAP"),
        launcher: wrapper,
        policy: env_path("SWITCHYARD_BRIDGE_POLICY"),
        model_endpoint: Some(endpoint.clone()),
    };
    let session = ConfinedPiSession::open(
        &config,
        RuntimeBinding {
            mission_id: "egress-m".into(),
            attempt_id: "egress-a".into(),
            session_id: "egress-s".into(),
            generation: 1,
        },
        "answer",
    )
    .unwrap_or_else(|error| panic!("confined endpoint session failed to open: {error}"));
    let life = session.lifecycle();
    assert_eq!(life["configuration"]["egress"]["mode"], "model-endpoint");
    assert_eq!(
        life["configuration"]["egress"]["url"],
        format!("http://127.0.0.1:{stub_port}/v1")
    );
    assert_eq!(
        life["configuration"]["egress"]["pinned"],
        format!("127.0.0.1:{stub_port}")
    );
    let state = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("session-")
        })
        .unwrap()
        .join("state");
    let report: Vec<String> =
        serde_json::from_slice(&read_nofollow(&state.join("probe.json"))).unwrap();
    let expected = vec![
        format!("read {}", protected.display()),
        format!("write {}", protected.display()),
        "symlink escape".into(),
        "runtime write /runtime/pins.json".into(),
        "runtime write /node".into(),
        "runtime write /guard.cjs".into(),
        "runtime write /lockdown.so".into(),
        "existing Node executable cannot spawn".into(),
        "credential and configuration environment absent".into(),
        "host TCP denied".into(),
        "host Unix socket denied".into(),
        "loopback listener denied".into(),
        "worker works and inherits subprocess denial".into(),
        "model endpoint URL is not bridge configuration".into(),
        "metadata TCP denied".into(),
        "metadata IPv6 denied".into(),
        "public TCP denied".into(),
        "dns TCP denied".into(),
        "stub TCP denied".into(),
        "other Unix socket denied".into(),
        "model proxy unlink".into(),
        "dns lookup denied".into(),
        "configured model endpoint reachable".into(),
        "worker inherits model-endpoint denial".into(),
    ];
    assert_eq!(report, expected, "in-bridge endpoint probes must all run");
    let body = String::from_utf8(read_nofollow(&state.join("endpoint-body.txt"))).unwrap();
    assert_eq!(
        body, token,
        "the stub token is only available through the proxy"
    );
    let request = stub.request.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        request
            .windows(15)
            .any(|window| window == b"169.254.169.254"),
        "request bytes reached the pinned stub: {request:?}"
    );
    assert_eq!(stub.count.load(Ordering::Relaxed), 1);
    assert!(
        matches!(decoy.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "the decoy local service must receive nothing"
    );
    assert_eq!(
        fs::read_to_string(&protected).unwrap(),
        "host authority sentinel"
    );
    if let Some(directory) = std::env::var_os("SWITCHYARD_CONFINEMENT_EVIDENCE") {
        fs::write(
            Path::new(&directory).join("bridge-egress.json"),
            serde_json::to_vec_pretty(&json!({
                "mode": "model-endpoint",
                "pinned": format!("127.0.0.1:{stub_port}"),
                "decoyPort": decoy_port,
                "stubHits": 1,
                "decoyHits": 0,
                "probes": report,
            }))
            .unwrap(),
        )
        .unwrap();
    }
    drop(session);
}
