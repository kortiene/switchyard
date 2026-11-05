//! Host-owned model route. The bridge never resolves names and never chooses
//! the destination: it can open only the mounted proxy socket, and this proxy
//! splices bytes to the single address pinned when the operator configuration
//! was parsed.
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
/// Link-local and cloud-metadata addresses are rejected here, and the bridge
/// still has no IP stack of its own.
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

/// Accepts connections on `directory/endpoint.sock` and splices each one to
/// `endpoint.pinned()`. Request bytes are not interpreted, so a URL inside
/// the request cannot change the TCP destination.
pub(super) struct ModelProxy {
    directory: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ModelProxy {
    pub(super) fn start(directory: &Path, endpoint: &ModelEndpoint) -> Result<Self> {
        let meta = fs::symlink_metadata(directory).map_err(Error::storage)?;
        if !directory.is_absolute() || meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(Error::guard(
                "model proxy directory is not a real directory",
            ));
        }
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(Error::storage)?;
        let socket_path = directory.join(SOCKET_NAME);
        match fs::symlink_metadata(&socket_path) {
            Ok(meta) if meta.file_type().is_socket() => {
                fs::remove_file(&socket_path).map_err(Error::storage)?;
            }
            Ok(_) => {
                return Err(Error::guard("model proxy path exists and is not a socket"));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::storage(error)),
        }
        let listener = UnixListener::bind(&socket_path).map_err(Error::storage)?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .map_err(Error::storage)?;
        listener.set_nonblocking(true).map_err(Error::storage)?;
        let pinned = endpoint.pinned();
        if excluded(pinned.ip()) {
            return Err(excluded_destination());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let inflight = Arc::new(AtomicUsize::new(0));
        let thread = thread::Builder::new()
            .name("model-endpoint-proxy".into())
            .spawn(move || accept_loop(listener, pinned, flag, inflight))
            .map_err(Error::storage)?;
        Ok(Self {
            directory: directory.to_path_buf(),
            stop,
            thread: Some(thread),
        })
    }

    pub(super) fn directory(&self) -> &Path {
        &self.directory
    }
}

impl Drop for ModelProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(self.directory.join(SOCKET_NAME));
    }
}

fn accept_loop(
    listener: UnixListener,
    pinned: SocketAddr,
    stop: Arc<AtomicBool>,
    inflight: Arc<AtomicUsize>,
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((client, _)) => {
                if inflight.fetch_add(1, Ordering::Relaxed) >= MAX_SPLICE {
                    inflight.fetch_sub(1, Ordering::Relaxed);
                    continue;
                }
                let inflight = Arc::clone(&inflight);
                let permit = Arc::clone(&inflight);
                if thread::Builder::new()
                    .name("model-endpoint-splice".into())
                    .spawn(move || {
                        splice(client, pinned);
                        permit.fetch_sub(1, Ordering::Relaxed);
                    })
                    .is_err()
                {
                    inflight.fetch_sub(1, Ordering::Relaxed);
                }
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::Interrupted =>
            {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
}

fn splice(client: UnixStream, pinned: SocketAddr) {
    if excluded(pinned.ip()) {
        return;
    }
    // Dial the pinned address only. This is not an HTTP proxy: the request
    // target, Host header, and redirects are opaque bytes.
    let Ok(upstream) = TcpStream::connect_timeout(&pinned, Duration::from_secs(2)) else {
        return;
    };
    let Ok(mut client_read) = client.try_clone() else {
        return;
    };
    let Ok(mut upstream_read) = upstream.try_clone() else {
        return;
    };
    let _ = client_read.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = upstream_read.set_read_timeout(Some(Duration::from_secs(30)));
    let mut upstream_write = upstream;
    let mut client_write = client;
    let forward = thread::spawn(move || {
        let _ = io::copy(&mut client_read, &mut upstream_write);
        let _ = upstream_write.shutdown(std::net::Shutdown::Write);
    });
    let _ = io::copy(&mut upstream_read, &mut client_write);
    let _ = client_write.shutdown(std::net::Shutdown::Write);
    let _ = forward.join();
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
        IpAddr::V4(ip) => {
            ip.is_unspecified() || ip.is_broadcast() || ip.is_link_local() || ip.is_multicast()
        }
        IpAddr::V6(ip) => {
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

fn split_host(authority: &str, default_port: u16) -> Result<(String, u16)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or_else(endpoint_syntax)?;
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_hexdigit() || c == ':') {
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc;

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
        let directory = tempfile::tempdir().unwrap();
        let proxy = ModelProxy::start(directory.path(), &endpoint).unwrap();
        let args = mount_args(proxy.directory()).unwrap();
        assert_eq!(
            args,
            vec![
                "--ro-bind",
                directory.path().to_str().unwrap(),
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
        let mut client = UnixStream::connect(directory.path().join(SOCKET_NAME)).unwrap();
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
    }
}
