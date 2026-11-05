//! Host-owned model route. The bridge never resolves names and never chooses
//! the destination: it can open only the mounted proxy socket, and this proxy
//! splices bytes to the single address pinned when the operator configuration
//! was parsed.
use std::collections::HashMap;
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};

use crate::{Error, Result};

/// Fixed mount and env token. `lockdown.c` allows `AF_UNIX` stream sockets
/// only when the token is exactly `model-socket`; any other value aborts.
pub(super) const MOUNT_POINT: &str = "/model-proxy";
pub(super) const SOCKET_NAME: &str = "endpoint.sock";
pub(super) const EGRESS_ENV: &str = "SWITCHYARD_BRIDGE_EGRESS";
pub(super) const EGRESS_VALUE: &str = "model-socket";
const MAX_SPLICE: usize = 8;

/// One operator-selected model URL and the only address the proxy may dial.
///
/// Loopback is allowed so a host can pin a local model or a fixture stub.
/// Link-local and cloud-metadata addresses are rejected here, including their
/// IPv4-mapped IPv6 forms. The bridge still has no IP stack of its own.
///
/// `https` only selects port 443 when the URL omits a port and is recorded
/// for the operator. [`ModelProxy`] copies raw TCP bytes. It does not perform
/// a TLS handshake, send SNI, or check a certificate. This is not a finished
/// HTTPS route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelEndpoint {
    url: String,
    pinned: SocketAddr,
}

impl ModelEndpoint {
    pub fn parse(input: &str) -> Result<Self> {
        if input.is_empty()
            || input.len() > 2048
            || input.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(endpoint_syntax());
        }
        let (scheme, rest) = input.split_once("://").ok_or_else(endpoint_syntax)?;
        let default_port = match scheme {
            "http" => 80u16,
            "https" => 443u16,
            _ => return Err(endpoint_syntax()),
        };
        if rest.contains(['@', '\\', '#', '?', '%']) {
            return Err(endpoint_syntax());
        }
        let (authority, path) = match rest.split_once('/') {
            Some((authority, path)) => (authority, format!("/{path}")),
            None => (rest, "/".to_string()),
        };
        if authority.is_empty() {
            return Err(endpoint_syntax());
        }
        let (host, port) = split_host(authority, default_port)?;
        if metadata_name(&host) {
            return Err(excluded_destination());
        }
        let mut addrs = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|_| Error::guard("model endpoint resolution is not a single address"))?;
        let Some(pinned) = addrs.next() else {
            return Err(Error::guard(
                "model endpoint resolution is not a single address",
            ));
        };
        if addrs.next().is_some() {
            return Err(Error::guard(
                "model endpoint resolution is not a single address",
            ));
        }
        if excluded(pinned.ip()) {
            return Err(excluded_destination());
        }
        let display = match pinned {
            SocketAddr::V4(_) => host,
            SocketAddr::V6(_) => format!("[{host}]"),
        };
        Ok(Self {
            url: format!("{scheme}://{display}:{port}{path}"),
            pinned,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn pinned(&self) -> SocketAddr {
        self.pinned
    }
}

pub(super) fn record(endpoint: Option<&ModelEndpoint>) -> Value {
    match endpoint {
        None => json!({"mode": "denied"}),
        Some(endpoint) => json!({
            "mode": "model-endpoint",
            "url": endpoint.url(),
            "pinned": endpoint.pinned().to_string(),
        }),
    }
}

/// Bubblewrap arguments that expose the proxy and select the seccomp mode.
/// The upstream URL is intentionally absent: bridge text cannot retarget it.
pub(super) fn mount_args(directory: &Path) -> Result<Vec<String>> {
    let socket = directory.join(SOCKET_NAME);
    let meta = fs::symlink_metadata(&socket).map_err(Error::storage)?;
    if !directory.is_absolute()
        || directory.to_str().is_none()
        || meta.file_type().is_symlink()
        || !meta.file_type().is_socket()
    {
        return Err(Error::guard(
            "model proxy socket is not an absolute host-owned socket",
        ));
    }
    Ok([
        "--ro-bind",
        directory.to_str().expect("checked UTF-8"),
        MOUNT_POINT,
        "--setenv",
        EGRESS_ENV,
        EGRESS_VALUE,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect())
}

/// Accepts connections on a short `/tmp` socket and splices each one to
/// `endpoint.pinned()`. Request bytes are not interpreted, so a URL inside
/// the request cannot change the TCP destination. The socket cannot live
/// under the session directory: that path is longer than `sockaddr_un`.
pub(super) struct ModelProxy {
    directory: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    splices: Arc<SpliceSet>,
}

/// In-flight splices only. A finished worker drops its sockets before it
/// returns the permit, and the accept thread joins that handle. [`Drop`]
/// still shuts down and joins whatever is left.
struct SpliceSet {
    next_id: AtomicU64,
    workers: Mutex<Vec<(u64, JoinHandle<()>)>>,
    sockets: Mutex<HashMap<u64, (UnixStream, TcpStream)>>,
}

impl ModelProxy {
    pub(super) fn start(endpoint: &ModelEndpoint) -> Result<Self> {
        let directory = create_proxy_dir()?;
        let socket_path = directory.join(SOCKET_NAME);
        // Linux `sun_path` is 108 bytes including the trailing NUL.
        if socket_path.as_os_str().len() >= 108 {
            let _ = fs::remove_dir(&directory);
            return Err(Error::guard(
                "model proxy socket path exceeds the Unix socket limit",
            ));
        }
        let listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = fs::remove_dir(&directory);
                return Err(Error::storage(error));
            }
        };
        if let Err(error) = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)) {
            let _ = fs::remove_file(&socket_path);
            let _ = fs::remove_dir(&directory);
            return Err(Error::storage(error));
        }
        if let Err(error) = listener.set_nonblocking(true) {
            let _ = fs::remove_file(&socket_path);
            let _ = fs::remove_dir(&directory);
            return Err(Error::storage(error));
        }
        let pinned = endpoint.pinned();
        if excluded(pinned.ip()) {
            let _ = fs::remove_file(&socket_path);
            let _ = fs::remove_dir(&directory);
            return Err(excluded_destination());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let inflight = Arc::new(AtomicUsize::new(0));
        let splices = Arc::new(SpliceSet {
            next_id: AtomicU64::new(1),
            workers: Mutex::new(Vec::new()),
            sockets: Mutex::new(HashMap::new()),
        });
        let splices_for_accept = Arc::clone(&splices);
        let thread = match thread::Builder::new()
            .name("model-endpoint-proxy".into())
            .spawn(move || accept_loop(listener, pinned, flag, inflight, splices_for_accept))
        {
            Ok(thread) => thread,
            Err(error) => {
                let _ = fs::remove_file(&socket_path);
                let _ = fs::remove_dir(&directory);
                return Err(Error::storage(error));
            }
        };
        Ok(Self {
            directory,
            stop,
            thread: Some(thread),
            splices,
        })
    }

    pub(super) fn directory(&self) -> &Path {
        &self.directory
    }
}

impl Drop for ModelProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // The accept thread registers every splice before it returns, so the
        // sets below are complete once this join finishes.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let sockets = std::mem::take(&mut *lock_mutex(&self.splices.sockets));
        for (_id, (client, upstream)) in sockets {
            let _ = client.shutdown(Shutdown::Both);
            let _ = upstream.shutdown(Shutdown::Both);
        }
        let workers = std::mem::take(&mut *lock_mutex(&self.splices.workers));
        for (_id, worker) in workers {
            let _ = worker.join();
        }
        let socket = self.directory.join(SOCKET_NAME);
        let _ = fs::remove_file(&socket);
        if proxy_dir_is_ours(&self.directory) {
            let _ = fs::remove_dir(&self.directory);
        }
    }
}

fn lock_mutex<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

/// Private `/tmp/sy-<32 hex>` directory. Session paths include a hash and do
/// not fit in `sockaddr_un`.
fn create_proxy_dir() -> Result<PathBuf> {
    let parent = Path::new("/tmp");
    if !parent.is_dir() {
        return Err(Error::guard("model proxy requires /tmp"));
    }
    for _ in 0..8 {
        let name = format!("sy-{}", uuid::Uuid::new_v4().simple());
        let directory = parent.join(&name);
        if !proxy_dir_is_ours(&directory) {
            return Err(Error::guard(
                "model proxy socket path exceeds the Unix socket limit",
            ));
        }
        match fs::create_dir(&directory) {
            Ok(()) => {
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                    .map_err(Error::storage)?;
                return Ok(directory);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(Error::storage(error)),
        }
    }
    Err(Error::guard("model proxy directory could not be created"))
}

fn proxy_dir_is_ours(directory: &Path) -> bool {
    directory.parent() == Some(Path::new("/tmp"))
        && directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                let Some(rest) = name.strip_prefix("sy-") else {
                    return false;
                };
                rest.len() == 32 && rest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        && directory.join(SOCKET_NAME).as_os_str().len() < 108
}

fn accept_loop(
    listener: UnixListener,
    pinned: SocketAddr,
    stop: Arc<AtomicBool>,
    inflight: Arc<AtomicUsize>,
    splices: Arc<SpliceSet>,
) {
    while !stop.load(Ordering::Relaxed) {
        reap_finished(&splices);
        match listener.accept() {
            Ok((client, _)) => {
                if inflight.fetch_add(1, Ordering::Relaxed) >= MAX_SPLICE {
                    inflight.fetch_sub(1, Ordering::Relaxed);
                    continue;
                }
                if !start_splice(client, pinned, &inflight, &splices) {
                    inflight.fetch_sub(1, Ordering::Relaxed);
                }
            }
            // EMFILE/ENFILE used to break this loop and retire the proxy.
            // Every accept error is retried until stop; finished splices are
            // reaped above so a full descriptor table can drain.
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }
    reap_finished(&splices);
}

const SPLICE_IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Connect on this thread, register both sockets, then spawn the copy. [`Drop`]
/// shuts the registered sockets down and joins the copy, so a hung upstream
/// cannot outlive the proxy.
fn start_splice(
    client: UnixStream,
    pinned: SocketAddr,
    inflight: &Arc<AtomicUsize>,
    splices: &Arc<SpliceSet>,
) -> bool {
    if excluded(pinned.ip()) {
        return false;
    }
    // Dial the pinned address only. This is not an HTTP proxy and not a TLS
    // endpoint: request targets, Host headers, redirects, SNI, and certificates
    // are not interpreted.
    let Ok(upstream) = TcpStream::connect_timeout(&pinned, Duration::from_secs(2)) else {
        return false;
    };
    if client.set_read_timeout(Some(SPLICE_IO_TIMEOUT)).is_err()
        || client.set_write_timeout(Some(SPLICE_IO_TIMEOUT)).is_err()
        || upstream.set_read_timeout(Some(SPLICE_IO_TIMEOUT)).is_err()
        || upstream.set_write_timeout(Some(SPLICE_IO_TIMEOUT)).is_err()
    {
        return false;
    }
    let (Ok(mut client_read), Ok(mut client_write)) = (client.try_clone(), client.try_clone())
    else {
        return false;
    };
    let (Ok(mut upstream_read), Ok(mut upstream_write)) =
        (upstream.try_clone(), upstream.try_clone())
    else {
        return false;
    };
    let id = splices.next_id.fetch_add(1, Ordering::Relaxed);
    lock_mutex(&splices.sockets).insert(id, (client, upstream));
    let permit = Arc::clone(inflight);
    let splices_for_worker = Arc::clone(splices);
    let spawned = thread::Builder::new()
        .name("model-endpoint-splice".into())
        .spawn(move || {
            let forward = thread::spawn(move || {
                let _ = io::copy(&mut client_read, &mut upstream_write);
                let _ = upstream_write.shutdown(Shutdown::Write);
            });
            let _ = io::copy(&mut upstream_read, &mut client_write);
            let _ = client_write.shutdown(Shutdown::Write);
            let _ = forward.join();
            // Close the shutdown pair before the permit returns so a finished
            // exchange cannot pin host descriptors until Drop.
            drop(lock_mutex(&splices_for_worker.sockets).remove(&id));
            permit.fetch_sub(1, Ordering::Relaxed);
        });
    match spawned {
        Ok(worker) => {
            lock_mutex(&splices.workers).push((id, worker));
            true
        }
        Err(_) => {
            if let Some((client, upstream)) = lock_mutex(&splices.sockets).remove(&id) {
                let _ = client.shutdown(Shutdown::Both);
                let _ = upstream.shutdown(Shutdown::Both);
            }
            false
        }
    }
}

/// Join workers that have already dropped their sockets. Handles are removed
/// under the lock so [`Drop`] cannot join the same thread.
fn reap_finished(splices: &SpliceSet) {
    let finished = {
        let mut workers = lock_mutex(&splices.workers);
        let mut done = Vec::new();
        let mut index = 0;
        while index < workers.len() {
            if workers[index].1.is_finished() {
                done.push(workers.swap_remove(index).1);
            } else {
                index += 1;
            }
        }
        done
    };
    for worker in finished {
        let _ = worker.join();
    }
}

fn endpoint_syntax() -> Error {
    Error::guard("model endpoint must be one http(s) URL without credentials")
}

fn excluded_destination() -> Error {
    Error::guard("model endpoint must not be a link-local or metadata destination")
}

fn metadata_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    matches!(host.as_str(), "metadata.google.internal" | "metadata.goog")
}

fn excluded(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => excluded_v4(ip),
        IpAddr::V6(ip) => {
            // `::ffff:169.254.169.254` is not an IPv6 link-local address.
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return excluded_v4(mapped);
            }
            ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_unicast_link_local()
                || ip
                    == "fd00:ec2::254"
                        .parse::<Ipv6Addr>()
                        .expect("static metadata address")
        }
    }
}

fn excluded_v4(ip: Ipv4Addr) -> bool {
    ip.is_unspecified() || ip.is_broadcast() || ip.is_link_local() || ip.is_multicast()
}

fn split_host(authority: &str, default_port: u16) -> Result<(String, u16)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or_else(endpoint_syntax)?;
        if host.is_empty()
            || !host
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        {
            return Err(endpoint_syntax());
        }
        let port = if after.is_empty() {
            default_port
        } else {
            parse_port(after.strip_prefix(':').ok_or_else(endpoint_syntax)?)?
        };
        return Ok((host.to_string(), port));
    }
    if authority.is_empty()
        || !authority
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
        || authority.matches(':').count() > 1
    {
        return Err(endpoint_syntax());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, parse_port(port)?),
        None => (authority, default_port),
    };
    if host.is_empty()
        || host.starts_with(['-', '.'])
        || host.ends_with(['-', '.'])
        || host.contains("..")
    {
        return Err(endpoint_syntax());
    }
    Ok((host.to_string(), port))
}

fn parse_port(value: &str) -> Result<u16> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(endpoint_syntax());
    }
    let port = value.parse::<u16>().map_err(|_| endpoint_syntax())?;
    if port == 0 {
        return Err(endpoint_syntax());
    }
    Ok(port)
}

/// Read helper used by the proxy tests and kept here so the splice contract
/// stays next to the writer. Not used on the session path.
#[cfg(test)]
fn read_headers(stream: &mut TcpStream) -> Vec<u8> {
    use std::io::Read;
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;

    use super::*;

    #[test]
    fn confined_pi_model_endpoint_rejects_credentials_metadata_and_ambiguous_syntax() {
        let ok = ModelEndpoint::parse("http://127.0.0.1:9/v1").unwrap();
        assert_eq!(ok.url(), "http://127.0.0.1:9/v1");
        assert_eq!(ok.pinned().to_string(), "127.0.0.1:9");
        assert_eq!(
            ModelEndpoint::parse("http://127.0.0.1/v1")
                .unwrap()
                .pinned()
                .port(),
            80
        );
        assert_eq!(
            ModelEndpoint::parse("https://127.0.0.1/v1")
                .unwrap()
                .pinned()
                .port(),
            443
        );
        assert_eq!(
            ModelEndpoint::parse("http://[::1]:9/")
                .unwrap()
                .pinned()
                .to_string(),
            "[::1]:9"
        );
        assert_eq!(
            ModelEndpoint::parse("http://[::ffff:127.0.0.1]:9/")
                .unwrap()
                .pinned()
                .port(),
            9
        );
        for input in [
            "",
            "http://user:secret@127.0.0.1:9/",
            "http://127.0.0.1:9/a?x=1",
            "http://127.0.0.1:9/a#b",
            "file:///etc/passwd",
            "ftp://127.0.0.1/",
            "http://127.0.0.1:0/",
            "http://127.0.0.1:09/",
            "http:///v1",
        ] {
            let error = ModelEndpoint::parse(input).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("one http(s) URL without credentials"),
                "{input}: {error}"
            );
        }
        for input in [
            "http://169.254.169.254/latest",
            "http://169.254.170.2/",
            "http://[::ffff:169.254.169.254]/",
            "http://[::ffff:a9fe:a9fe]/",
            "http://[fd00:ec2::254]/",
            "http://metadata.google.internal/computeMetadata/v1/",
            "http://metadata.goog/",
            "http://0.0.0.0:9/",
        ] {
            let error = ModelEndpoint::parse(input).unwrap_err();
            assert!(
                error.to_string().contains("link-local or metadata"),
                "{input}: {error}"
            );
        }
        let error = ModelEndpoint::parse("http://switchyard-endpoint-does-not-exist.invalid:9/")
            .unwrap_err();
        assert!(
            error.to_string().contains("not a single address"),
            "{error}"
        );
        assert_eq!(record(None)["mode"], "denied");
        assert_eq!(record(Some(&ok))["pinned"], "127.0.0.1:9");
    }

    #[test]
    fn confined_pi_model_proxy_splices_only_the_pinned_endpoint() {
        let stub = TcpListener::bind("127.0.0.1:0").unwrap();
        let decoy = TcpListener::bind("127.0.0.1:0").unwrap();
        decoy.set_nonblocking(true).unwrap();
        let port = stub.local_addr().unwrap().port();
        let endpoint = ModelEndpoint::parse(&format!("http://127.0.0.1:{port}/v1")).unwrap();
        let proxy = ModelProxy::start(&endpoint).unwrap();
        let socket_dir = proxy.directory().to_path_buf();
        assert!(super::proxy_dir_is_ours(&socket_dir));
        let args = mount_args(proxy.directory()).unwrap();
        assert_eq!(
            args,
            vec![
                "--ro-bind",
                socket_dir.to_str().unwrap(),
                "/model-proxy",
                "--setenv",
                "SWITCHYARD_BRIDGE_EGRESS",
                "model-socket",
            ]
        );
        assert!(args.iter().all(|arg| !arg.contains("http")));
        let (sent, received) = mpsc::channel();
        let token = "model-endpoint-token".to_string();
        let response_token = token.clone();
        thread::spawn(move || {
            let (mut stream, _) = stub.accept().unwrap();
            let request = read_headers(&mut stream);
            let body = response_token.as_bytes();
            let header = format!(
                "HTTP/1.0 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
            let _ = stream.shutdown(std::net::Shutdown::Both);
            sent.send(request).unwrap();
        });
        let mut client = UnixStream::connect(socket_dir.join(SOCKET_NAME)).unwrap();
        client
            .write_all(
                b"GET http://169.254.169.254/latest HTTP/1.0\r\nHost: 169.254.169.254\r\n\r\n",
            )
            .unwrap();
        let _ = client.shutdown(std::net::Shutdown::Write);
        let mut response = Vec::new();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client.read_to_end(&mut response).unwrap();
        let request = received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            request
                .windows(15)
                .any(|window| window == b"169.254.169.254"),
            "proxy forwarded bytes without retargeting: {request:?}"
        );
        assert!(
            response
                .windows(token.len())
                .any(|window| window == token.as_bytes())
        );
        assert!(
            matches!(decoy.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock),
            "a second local service must receive nothing"
        );
        drop(proxy);
        assert!(!socket_dir.exists());
    }

    #[test]
    fn confined_pi_model_proxy_drop_joins_a_hung_upstream() {
        let stub = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = stub.local_addr().unwrap().port();
        let endpoint = ModelEndpoint::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
        let proxy = ModelProxy::start(&endpoint).unwrap();
        let mut client = UnixStream::connect(proxy.directory().join(SOCKET_NAME)).unwrap();
        let (_stuck, _) = stub.accept().unwrap();
        client.write_all(b"still-open").unwrap();
        let started = std::time::Instant::now();
        drop(proxy);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "drop left the hung splice running for {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn confined_pi_model_proxy_reclaims_completed_splices() {
        let stub = TcpListener::bind("127.0.0.1:0").unwrap();
        stub.set_nonblocking(true).unwrap();
        let port = stub.local_addr().unwrap().port();
        let endpoint = ModelEndpoint::parse(&format!("http://127.0.0.1:{port}/v1")).unwrap();
        let proxy = ModelProxy::start(&endpoint).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let server = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match stub.accept() {
                    Ok((mut stream, _)) => {
                        let _ = read_headers(&mut stream);
                        let body = b"ok";
                        let header = format!(
                            "HTTP/1.0 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(header.as_bytes());
                        let _ = stream.write_all(body);
                        let _ = stream.shutdown(Shutdown::Both);
                    }
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            || error.kind() == io::ErrorKind::Interrupted =>
                    {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        let before = host_fd_count();
        for index in 0..64 {
            let mut client = UnixStream::connect(proxy.directory().join(SOCKET_NAME)).unwrap();
            client
                .write_all(b"GET / HTTP/1.0\r\nHost: stub\r\n\r\n")
                .unwrap();
            client.shutdown(Shutdown::Write).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).unwrap();
            assert!(
                response.windows(2).any(|window| window == b"ok"),
                "exchange {index} failed: {response:?}"
            );
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                let sockets = lock_mutex(&proxy.splices.sockets).len();
                let workers = lock_mutex(&proxy.splices.workers).len();
                if sockets <= MAX_SPLICE && workers <= MAX_SPLICE {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "exchange {index} retained sockets={sockets} workers={workers}"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let (sockets, workers, fds) = loop {
            let sockets = lock_mutex(&proxy.splices.sockets).len();
            let workers = lock_mutex(&proxy.splices.workers).len();
            let fds = host_fd_count();
            if sockets == 0 && workers == 0 {
                break (sockets, workers, fds);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "finished splices still registered sockets={sockets} workers={workers} fds={fds} before={before}"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(sockets, 0);
        assert_eq!(workers, 0);
        assert!(
            fds <= before + 12,
            "host descriptors grew from {before} to {fds}"
        );
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }

    fn host_fd_count() -> usize {
        fs::read_dir("/proc/self/fd")
            .map(|entries| entries.count())
            .unwrap_or(0)
    }
}
