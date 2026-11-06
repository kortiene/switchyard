//! Credential-free composed S3 evidence. Pi responses remain faux; the HTTP
//! stub proves startup connectivity/policy, not a working live model client.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};

use switchyard_core::{ModelEndpoint, RuntimeBinding};

use super::*;

const FIXTURE: &str = "s3-acceptance";
const SENTINEL: &str = "must not repeat";

struct Stub {
    address: String,
    token: String,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl Stub {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let mut bytes = [0u8; 16];
        fs::File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut bytes)
            .unwrap();
        let token = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let flag = Arc::clone(&stop);
        let recorded = Arc::clone(&requests);
        let body = token.clone();
        let thread = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut request = Vec::new();
                        let mut buffer = [0u8; 1024];
                        while !request.windows(4).any(|window| window == b"\r\n\r\n")
                            && request.len() < 8192
                        {
                            match stream.read(&mut buffer) {
                                Ok(0) | Err(_) => break,
                                Ok(size) => request.extend_from_slice(&buffer[..size]),
                            }
                        }
                        recorded
                            .lock()
                            .unwrap()
                            .push(String::from_utf8(request).unwrap());
                        write!(
                            stream,
                            "HTTP/1.0 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        )
                        .unwrap();
                        stream.shutdown(Shutdown::Both).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("stub listener failed: {error}"),
                }
            }
        });
        Self {
            address,
            token,
            stop,
            requests,
            thread: Some(thread),
        }
    }

    fn url(&self) -> String {
        format!("http://{}/v1", self.address)
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn configuration(root: &Path, endpoint: &str) -> ConfinedPiConfig {
    let mut config = bridge(&root.join("bridge"));
    config.launcher = root.join("s3-launcher");
    config.model_endpoint = Some(ModelEndpoint::parse(endpoint).unwrap());
    config
}

fn install_probe(root: &Path, endpoint: &Stub, decoy_port: u16) {
    let probe = include_str!("../../bridge/probe.mjs")
        .lines()
        .filter(|line| !line.starts_with("denied('owner lock"))
        .collect::<Vec<_>>()
        .join("\n");
    let entry = "await import('/runtime/test/bridge-child.ts');";
    assert_eq!(probe.matches(entry).count(), 1);
    // Keep each generation's actual probe observations; a later reopen must
    // not overwrite the only policy evidence from the crashed generation.
    let probe = format!(
        "import {{ appendFileSync }} from 'node:fs';\n{}",
        probe.replace(
            entry,
            r#"
appendFileSync('/state/s3-policy.ndjson', JSON.stringify({
  generation: JSON.parse(process.argv[3]).generation,
  probes: results, token: readFileSync('/state/endpoint-body.txt', 'utf8'),
}) + '\n');
await import('/runtime/test/bridge-child.ts');
"#
        )
    );
    fs::write(root.join("s3-probe.mjs"), probe).unwrap();
    fs::write(root.join("s3-case.json"), json!({
        "egress":"model-endpoint", "endpoint":"http://169.254.169.254/latest",
        "protected":[root.join("mission.sqlite"),root.join("mission.sqlite.owner.lock"),root.join("repo/README.md"),root.join("repo/.git/index")],
        "port":decoy_port, "stubPort":endpoint.address.rsplit_once(':').unwrap().1.parse::<u16>().unwrap(),
        "socket":root.join("host.sock"),
    }).to_string()).unwrap();
    let original = env_path("SWITCHYARD_BRIDGE_LAUNCHER");
    for path in [root, &original] {
        assert!(!path.to_str().unwrap().contains('\''));
    }
    fs::write(root.join("s3-launcher"), format!(r#"#!/bin/bash
set -euo pipefail
args=()
previous=''
for arg in "$@"; do
  if [[ "$arg" == --unshare-user ]]; then
    args+=(--ro-bind '{root}/s3-probe.mjs' /probe.mjs --ro-bind '{root}/s3-case.json' /case.json)
  fi
  if [[ "$arg" == /runtime/test/bridge-child.ts && "$previous" == --experimental-strip-types ]]; then
    args+=(/probe.mjs)
  else
    args+=("$arg")
  fi
  previous="$arg"
done
exec '{launcher}' "${{args[@]}}"
"#, root=root.display(), launcher=original.display())).unwrap();
    fs::set_permissions(root.join("s3-launcher"), fs::Permissions::from_mode(0o755)).unwrap();
}

fn observation(repo: &Path) -> Value {
    let command = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    };
    json!({"branch":command(&["symbolic-ref","HEAD"]),"head":command(&["rev-parse","HEAD"]),
        "status":command(&["status","--porcelain"]),"diff":command(&["diff","HEAD","--binary"]),
        "index":payload_hash(&json!(fs::read(repo.join(".git/index")).unwrap())).unwrap(),
        "readme":payload_hash(&json!(fs::read(repo.join("README.md")).unwrap())).unwrap()})
}

fn reopen(world: &World, endpoint: &str) -> Host {
    Host::open_confined_pi(
        &world.db(),
        configuration(world.dir.path(), endpoint),
        linux(None),
        FIXTURE,
    )
    .unwrap()
}

fn session_directory(world: &World) -> PathBuf {
    fs::read_dir(world.dir.path().join("bridge"))
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

fn read_nofollow(path: &Path) -> String {
    let mut text = String::new();
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(0o400000)
        .open(path)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

fn retirement(world: &World, lifecycle: &Value) -> Value {
    assert!(!Path::new(lifecycle["cgroup"]["path"].as_str().unwrap()).exists());
    let audit = fs::read_dir(session_directory(world))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("retired-")
        })
        .map(|path| serde_json::from_str::<Value>(&read_nofollow(&path)).unwrap())
        .find(|audit| audit["cgroup"] == lifecycle["cgroup"])
        .expect("old generation needs its exact retirement receipt");
    assert_eq!(audit["removed"], true);
    assert_eq!(audit["authorization"]["proven"], true);
    assert_eq!(audit["authorization"]["cgroup"], lifecycle["cgroup"]);
    audit
}

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn s3_composed_acceptance_recovers_stops_and_enforces_egress() {
    let stub = Stub::new();
    let endpoint = stub.url();
    let decoy = TcpListener::bind("127.0.0.1:0").unwrap();
    decoy.set_nonblocking(true).unwrap();
    let _control = std::net::TcpStream::connect(decoy.local_addr().unwrap()).unwrap();
    decoy.accept().unwrap();
    let mut socket = None;
    let mut original = Value::Null;
    let (world, host) = World::configured(FIXTURE, |root, config| {
        original = observation(&root.join("repo"));
        socket = Some(UnixListener::bind(root.join("host.sock")).unwrap());
        let _positive = UnixStream::connect(root.join("host.sock")).unwrap();
        install_probe(root, &stub, decoy.local_addr().unwrap().port());
        *config = configuration(root, &endpoint);
    });
    let first = host.snapshot(&world.mission).unwrap().to_json();
    drop(host);
    let mut worker = Worker(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "composed::s3_acceptance_crash_worker",
                "--nocapture",
            ])
            .env("SW_S3_ROOT", world.dir.path())
            .env("SW_S3_MISSION", &world.mission)
            .env("SW_S3_ENDPOINT", &endpoint)
            .env("SWITCHYARD_CREDENTIAL_SENTINEL", "host-only-fixture")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let stdout = worker.0.stdout.take().unwrap();
    let (send, wait) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let line = wait
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("missing committed-receipt crash barrier");
        if line.contains("AT-receipt") {
            break;
        }
    }
    worker.0.kill().unwrap();
    assert_eq!(worker.0.wait().unwrap().signal(), Some(9));
    let crashed = Host::inspect(&world.db(), Some(&world.mission)).unwrap()[0].to_json();
    let original_receipts = world.receipts();
    assert_eq!(original_receipts.len(), 1);
    assert!(original_receipts[0].1.is_some());
    assert_eq!(output_text(&world).as_deref(), Some("once"));
    assert!(!world.workspace.join("process.marker").exists());
    fs::write(world.workspace.join("output.txt"), SENTINEL).unwrap();
    let old_generation = crashed["piHost"]["lifecycle"]["generation"]
        .as_u64()
        .unwrap();
    let stale = RuntimeBinding {
        mission_id: world.mission.clone(),
        attempt_id: first["attemptId"].as_str().unwrap().into(),
        session_id: first["sessionId"].as_str().unwrap().into(),
        generation: old_generation,
    };
    let mut host = reopen(&world, &endpoint);
    host.recover(&world.mission).unwrap();
    assert!(host.accept_runtime_binding(&stale, &world.mission).is_err());
    let recovered = until(&mut host, &world.mission, |value| {
        value["piHost"]["observation"]["runtimeIdle"] == true
            && value["linuxExecutionEffects"]
                .as_array()
                .is_some_and(|effects| effects.len() == 3)
            && world.workspace.join("process.heartbeat").exists()
    });
    assert_eq!(recovered["phase"], "implementing");
    for field in ["missionId", "attemptId", "sessionId"] {
        assert_eq!(recovered[field], first[field]);
    }
    let approval = |value: &Value| {
        value["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["kind"] == "approve_plan")
            .unwrap()["id"]
            .clone()
    };
    assert_eq!(approval(&crashed), approval(&recovered));
    assert_eq!(output_text(&world).as_deref(), Some(SENTINEL));
    assert_process_not_relaunched(&world, "composed recovery");
    let receipts = world.receipts();
    assert!(receipts.contains(&original_receipts[0]));
    assert!(receipts.iter().all(|(_, receipt)| receipt.is_some()));
    let effects = recovered["linuxExecutionEffects"].as_array().unwrap();
    for method in ["write_file", "exec_start", "exec_status"] {
        assert_eq!(
            effects
                .iter()
                .filter(|effect| effect["method"] == method)
                .count(),
            1
        );
    }
    assert!(effects.iter().all(|effect| effect["simulated"] == false));
    let old_retirement = retirement(&world, &crashed["piHost"]["lifecycle"]);
    // A claimed idle model cannot establish workspace quiescence: a detached
    // writer is still running until the operator's actual stop below.
    assert!(
        effects
            .iter()
            .any(|effect| effect["method"] == "exec_start" && effect["state"] == "running")
    );
    let heartbeat = fs::read(world.workspace.join("process.heartbeat")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while fs::read(world.workspace.join("process.heartbeat")).unwrap() == heartbeat {
        assert!(Instant::now() < deadline, "detached writer did not advance");
        thread::sleep(Duration::from_millis(20));
    }
    // Exercise the same blocked-workspace stop path with egress enabled.
    rusqlite::Connection::open(world.db())
        .unwrap()
        .execute("UPDATE workspaces SET verified_generation=NULL", [])
        .unwrap();
    admit(&mut host, &world.mission, CommandBody::Pause);
    host.dispatch_stops(&world.mission).unwrap();
    let paused = host.snapshot(&world.mission).unwrap().to_json();
    assert_eq!(paused["phase"], "paused");
    assert_eq!(paused["executionBlocked"], false);
    assert_eq!(paused["piHost"]["lifecycle"]["termination"]["proven"], true);
    assert_eq!(
        command_origin(&host, &world.mission, "confirm_stopped"),
        "host_supervisor"
    );
    let stopped_heartbeat = fs::read(world.workspace.join("process.heartbeat")).unwrap();
    thread::sleep(Duration::from_millis(150));
    assert_eq!(
        fs::read(world.workspace.join("process.heartbeat")).unwrap(),
        stopped_heartbeat
    );
    assert_eq!(
        paused["linuxExecutionEffects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|effect| effect["method"] == "exec_start")
            .unwrap()["quiescence"],
        "populated 0"
    );
    let stopped_ids = effect_ids(&paused);
    drop(host);
    let mut host = reopen(&world, &endpoint);
    host.recover(&world.mission).unwrap();
    let inert = host.snapshot(&world.mission).unwrap().to_json();
    assert_eq!(inert["phase"], "paused");
    assert_eq!(effect_ids(&inert), stopped_ids);
    assert_eq!(output_text(&world).as_deref(), Some(SENTINEL));
    assert_process_not_relaunched(&world, "inert reopen");
    admit(&mut host, &world.mission, CommandBody::Cancel);
    host.dispatch_stops(&world.mission).unwrap();
    let cancelled = host.snapshot(&world.mission).unwrap().to_json();
    assert_eq!(cancelled["phase"], "cancelled");
    assert_eq!(cancelled["executionBlocked"], false);
    assert_eq!(
        cancelled["piHost"]["lifecycle"]["termination"]["proven"],
        true
    );
    assert_eq!(effect_ids(&cancelled), stopped_ids);
    assert_eq!(
        command_origin(&host, &world.mission, "confirm_cancelled"),
        "host_supervisor"
    );
    let final_receipts = world.receipts();
    assert!(final_receipts.contains(&original_receipts[0]));
    assert_eq!(output_text(&world).as_deref(), Some(SENTINEL));
    assert_process_not_relaunched(&world, "terminal cancellation");
    assert_eq!(
        fs::read(world.workspace.join("process.heartbeat")).unwrap(),
        stopped_heartbeat
    );
    let original_after = observation(&world.dir.path().join("repo"));
    assert_eq!(original_after, original);
    let history = read_nofollow(&session_directory(&world).join("state/s3-policy.ndjson"));
    let policy = history
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    for observed in &policy {
        assert_eq!(observed["token"], stub.token);
        let probes = observed["probes"].as_array().unwrap();
        for required in [
            "host TCP denied",
            "host Unix socket denied",
            "metadata TCP denied",
            "metadata IPv6 denied",
            "public TCP denied",
            "dns TCP denied",
            "stub TCP denied",
            "other Unix socket denied",
            "dns lookup denied",
            "loopback listener denied",
            "existing Node executable cannot spawn",
            "credential and configuration environment absent",
            "configured model endpoint reachable",
            "worker inherits model-endpoint denial",
        ] {
            assert!(
                probes.iter().any(|probe| probe == required),
                "missing {required}"
            );
        }
        assert!(
            probes
                .iter()
                .filter(|probe| probe.as_str().unwrap().starts_with("read "))
                .count()
                >= 4
        );
        assert!(
            probes
                .iter()
                .filter(|probe| probe.as_str().unwrap().starts_with("write "))
                .count()
                >= 4
        );
    }
    let generations = policy
        .iter()
        .map(|entry| entry["generation"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert!(generations.contains(&old_generation));
    assert!(
        generations.contains(
            &recovered["piHost"]["lifecycle"]["generation"]
                .as_u64()
                .unwrap()
        )
    );
    assert!(generations.windows(2).all(|pair| pair[0] < pair[1]));
    let requests = stub.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), generations.len());
    assert!(
        requests
            .iter()
            .all(|request| request.contains("169.254.169.254"))
    );
    assert!(matches!(decoy.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
    let code = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let revision = String::from_utf8(git(&code, &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_string();
    let tree = String::from_utf8(git(&code, &["rev-parse", "HEAD^{tree}"]))
        .unwrap()
        .trim()
        .to_string();
    let pins: Value =
        serde_json::from_str(include_str!("../../../../runtime/pi-source/pins.json")).unwrap();
    let report = json!({"schema":1,"scenario":"host-pi-linux-egress-recovery-stop-v1","decision":"PENDING_REVIEW",
        "revision":revision,"tree":tree,"pins":pins,"platform":"linux-x86_64","environmentEvidence":"bridge-environment.txt",
        "scope":{"seededPlan":true,"model":"faux","endpoint":"local startup connectivity probe","liveProvider":false,"productEnabled":false},
        "missionId":world.mission,"attemptId":first["attemptId"],"sessionId":first["sessionId"],"approvalCommandId":approval(&recovered),
        "crash":{"signal":9,"barrier":"receipt","generation":old_generation,"receipt":original_receipts[0]},
        "recovery":{"generation":recovered["piHost"]["lifecycle"]["generation"],"stableFileReceipt":true,"fileNotRepeated":true,
            "processNotRelaunched":true,"staleBindingRejected":true,"effects":recovered["linuxExecutionEffects"],"oldRetirement":old_retirement},
        "pause":{"phase":paused["phase"],"bridgeTermination":paused["piHost"]["lifecycle"]["termination"],"executionBlocked":false,
            "heartbeatStopped":true,"origin":"host_supervisor","effects":paused["linuxExecutionEffects"]},
        "inertReopen":{"phase":inert["phase"],"noNewEffects":true},
        "cancel":{"phase":cancelled["phase"],"bridgeTermination":cancelled["piHost"]["lifecycle"]["termination"],"executionBlocked":false,"origin":"host_supervisor"},
        "egress":{"endpoint":endpoint,"generations":generations,"probes":policy,"stubRequests":requests.len(),"decoyHits":0},
        "receipts":{"beforeCrash":payload_hash(&json!(original_receipts)).unwrap(),"recovered":payload_hash(&json!(receipts)).unwrap(),
            "final":payload_hash(&json!(final_receipts)).unwrap(),
            "recoveredFile":receipts.iter().find(|(id, _)| *id == original_receipts[0].0).unwrap(),
            "finalFile":final_receipts.iter().find(|(id, _)| *id == original_receipts[0].0).unwrap()},
        "originalBefore":original,"originalAfter":original_after});
    fs::write(
        env_path("SWITCHYARD_CONFINEMENT_EVIDENCE").join("s3-composed-acceptance.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}

#[test]
fn s3_acceptance_crash_worker() {
    let Some(root) = std::env::var_os("SW_S3_ROOT").map(PathBuf::from) else {
        return;
    };
    let endpoint = std::env::var("SW_S3_ENDPOINT").unwrap();
    let mission = std::env::var("SW_S3_MISSION").unwrap();
    let mut host = Host::open_confined_pi(
        &root.join("mission.sqlite"),
        configuration(&root, &endpoint),
        linux(Some("receipt")),
        FIXTURE,
    )
    .unwrap();
    host.dispatch_pending(&mission).unwrap();
    until(&mut host, &mission, |_| false);
    panic!("composed Host passed mandatory receipt barrier");
}
