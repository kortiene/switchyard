//! SIGKILL during a model stream, before any tool. A mission with a model
//! endpoint admits `unsettled_turn` on reopen and does not replay the request.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use switchyard_core::{CommandBody, Host, ModelEndpoint, Phase};

use super::{World, admit, bridge, linux};

struct SlowStub {
    url: String,
    count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SlowStub {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let connections = Arc::clone(&count);
        let flag = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        connections.fetch_add(1, Ordering::SeqCst);
                        thread::spawn(move || {
                            let _ = stream.write_all(
                                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n",
                            );
                            let _ = stream.flush();
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                            let mut buffer = [0u8; 64];
                            loop {
                                match stream.read(&mut buffer) {
                                    Ok(0) | Err(_) => break,
                                    Ok(_) => {}
                                }
                            }
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            url,
            count,
            stop,
            thread: Some(thread),
        }
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

impl Drop for SlowStub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn open_slow(world: &World, endpoint: &str) -> Host {
    let mut config = bridge(&world.dir.path().join("bridge"));
    config.model_endpoint = Some(ModelEndpoint::parse(endpoint).unwrap());
    Host::open_confined_test_fixture(&world.db(), config, linux(None), "slow-stream").unwrap()
}

#[test]
fn slow_stream_worker() {
    let Ok(root) = std::env::var("SW_SLOW_ROOT") else {
        return;
    };
    let endpoint = std::env::var("SW_SLOW_ENDPOINT").unwrap();
    let mission = std::env::var("SW_SLOW_MISSION").unwrap();
    let mut config = bridge(&std::path::PathBuf::from(&root).join("bridge"));
    config.model_endpoint = Some(ModelEndpoint::parse(&endpoint).unwrap());
    let mut host = Host::open_confined_test_fixture(
        &std::path::PathBuf::from(&root).join("mission.sqlite"),
        config,
        linux(None),
        "slow-stream",
    )
    .unwrap();
    host.dispatch_pending(&mission).unwrap();
    panic!("slow-stream dispatch returned");
}

/// A model stream with no tool entered is `unsettled_turn` on reopen. Replay
/// must not open another stub connection. Cancel then proves the cgroup.
#[test]
fn sigkill_mid_model_stream_is_unsettled_and_does_not_reconnect() {
    let stub = SlowStub::new();
    let endpoint = stub.url.clone();
    let (world, host) = World::configured("slow-stream", |_, config| {
        config.model_endpoint = Some(ModelEndpoint::parse(&endpoint).unwrap());
    });
    drop(host);
    let mut worker = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "slow_stream::slow_stream_worker", "--nocapture"])
        .env("SW_SLOW_ROOT", world.dir.path())
        .env("SW_SLOW_MISSION", &world.mission)
        .env("SW_SLOW_ENDPOINT", &endpoint)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while stub.count() == 0 {
        assert!(
            Instant::now() < deadline,
            "model stream did not reach the stub"
        );
        if let Some(status) = worker.try_wait().unwrap() {
            panic!("worker exited before the stream: {status}");
        }
        thread::sleep(Duration::from_millis(20));
    }
    worker.kill().unwrap();
    let killed = worker.wait().unwrap();
    assert_eq!(killed.signal(), Some(9));
    let before = stub.count();
    assert!(before >= 1, "the first stream made no connection");
    let mut host = open_slow(&world, &endpoint);
    host.recover(&world.mission).unwrap();
    let recovered = host.snapshot(&world.mission).unwrap();
    assert_eq!(recovered.phase, Phase::NeedsAttention);
    assert_eq!(
        recovered.to_json()["attentionReason"],
        "unsettled_turn",
        "{recovered:?}"
    );
    thread::sleep(Duration::from_millis(400));
    assert_eq!(stub.count(), before, "reopen replayed the model request");
    admit(&mut host, &world.mission, CommandBody::Cancel);
    host.dispatch_pending(&world.mission).unwrap();
    let cancelled = host.snapshot(&world.mission).unwrap();
    assert_eq!(cancelled.phase, Phase::Cancelled);
    let export = Host::export_document(&world.db(), &world.mission, "s2-2").unwrap();
    let receipt = export["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["kind"] == "confirm_cancelled")
        .expect("confirm_cancelled")["receipt"]
        .clone();
    assert_eq!(receipt["proven"], true, "{receipt}");
}
