#!/usr/bin/env python3
"""Operator-run D1(b) model forwarder.

Plain HTTP on 127.0.0.1 only. TLS goes to one operator-supplied global IP.
Switchyard never sees the provider key. Logs use the label
"egress pinned by an operator forwarder, not by Switchyard".

The pi-ai client half-closes its write side after the request body. This
process keeps reading the upstream response and writing it back. Upstream
bodies are not close-delimited toward the client: Content-Length is kept
when upstream sends it, and a close-delimited upstream body is re-framed
as chunked. A cut does not get a terminating 0-chunk. There is no SSE
heartbeat.
"""

from __future__ import annotations

import argparse
import errno
import fcntl
import hashlib
import ipaddress
import json
import os
import select
import signal
import socket
import ssl
import stat
import sys
import threading
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LABEL = "egress pinned by an operator forwarder, not by Switchyard"
MAX_REQUEST_BYTES = 256 * 1024
MAX_RESPONSE_BYTES = 1024 * 1024
MAX_REQUESTS = 40
CONNECT_TIMEOUT_S = 10
# Idle time between upstream reads. Each read that returns data starts it again.
UPSTREAM_TIMEOUT_S = 300
# Client keepalive. A pi-ai half-close leaves the socket in CLOSE_WAIT, and a
# later full close sends no packet. Probes are what notice a dead peer.
# With these values a dead client showed up at about 66 s in review, not 1 s.
# A close within about 1 s happens only while response bytes are flowing, or on an RST.
KEEPALIVE_IDLE = 5
KEEPALIVE_INTVL = 5
KEEPALIVE_CNT = 2
UPSTREAM_SNI = "api.x.ai"
MAX_KEY_BYTES = 4096
MAX_LOG_BYTES = 8 * 1024 * 1024

RESERVED_OUTCOMES = frozenset(
    {"ok", "redirect_refused", "response_cap", "upstream_error", "502"}
)
LOCAL_OUTCOMES = frozenset({"429", "413", "411", "400", "404", "405"})
OUTCOMES = RESERVED_OUTCOMES | LOCAL_OUTCOMES

# Client headers that may be copied upstream. Host and Authorization are
# replaced, not copied. Anything else, including proxy and API-key headers,
# is dropped.
ALLOWED_REQUEST_HEADERS = frozenset({"content-type", "accept", "user-agent"})

EXCHANGE_REQUIRED = (
    "kind",
    "seq",
    "start",
    "elapsed_ms",
    "method",
    "path",
    "status",
    "outcome",
    "request_bytes",
    "response_bytes",
    "request_sha256",
    "response_sha256",
    "upstream_ip",
    "cert_sha256",
)


class StartupError(Exception):
    """Refusing to start. The message never contains the key."""


class _BadHeader(Exception):
    pass


class _UpstreamCut(Exception):
    """Upstream ended without a clean close_notify, or the read failed."""


class _ClientGone(Exception):
    """The client disconnected while upstream was still open."""


@dataclass
class Settings:
    listen_host: str
    listen_port: int
    path: str
    model: str
    key: bytes = field(repr=False)
    host_uid: int
    upstream_ip: str
    lookup_record: str
    mission_id: str
    sni: str = UPSTREAM_SNI

    def __repr__(self) -> str:
        return "Settings(key=<redacted>)"


def utc_millis() -> str:
    now = datetime.now(timezone.utc)
    return now.strftime("%Y-%m-%dT%H:%M:%S.") + f"{now.microsecond // 1000:03d}Z"


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def forwarder_sha256() -> str:
    digest = hashlib.sha256()
    with open(os.path.abspath(__file__), "rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require_global_ip(ip: str) -> str:
    """Accept one global IPv4 literal. Never resolves a name."""
    try:
        addr = ipaddress.ip_address(ip)
    except ValueError:
        raise StartupError("refusing to start: upstream-ip must be an IP address") from None
    if not isinstance(addr, ipaddress.IPv4Address):
        raise StartupError("refusing to start: upstream-ip must be an IPv4 address")
    if (
        addr.is_multicast
        or not addr.is_global
        or addr in ipaddress.ip_network("192.88.99.0/24")
    ):
        raise StartupError("refusing to start: upstream-ip is not a global address")
    return str(addr)


def assert_process_identity(host_uid: int, euid: int) -> None:
    if euid == 0:
        raise StartupError("refusing to start as uid 0")
    if host_uid < 0:
        raise StartupError("refusing to start: host uid is invalid")
    if euid == host_uid:
        raise StartupError("refusing to start as the host uid")


def parse_listen(value: str) -> tuple[str, int]:
    prefix = "127.0.0.1:"
    if not value.startswith(prefix):
        raise StartupError("refusing to start: listen address must be 127.0.0.1:<port>")
    port_text = value[len(prefix) :]
    if not port_text.isdigit():
        raise StartupError("refusing to start: listen address must be 127.0.0.1:<port>")
    port = int(port_text)
    if not 1 <= port <= 65535:
        raise StartupError("refusing to start: listen port is out of range")
    return "127.0.0.1", port


def validate_path(path: str) -> str:
    if not path.startswith("/") or path.startswith("//"):
        raise StartupError("refusing to start: path must be one absolute path")
    if len(path) > 512 or any(ch in path for ch in " ?#\\\r\n\t"):
        raise StartupError("refusing to start: path must be one absolute path")
    return path


def validate_model(model: str) -> str:
    if not model or any(ch in model for ch in " \r\n\t"):
        raise StartupError("refusing to start: model is empty")
    return model


def validate_mission_id(mission_id: str) -> str:
    if not isinstance(mission_id, str) or not mission_id or len(mission_id) > 200:
        raise StartupError("refusing to start: mission id is empty")
    if any(ch in mission_id for ch in " \r\n\t"):
        raise StartupError("refusing to start: mission id is empty")
    return mission_id


def _open_nofollow(path: str, flags: int, mode: int | None = None) -> int:
    if not hasattr(os, "O_NOFOLLOW"):
        raise StartupError("refusing to start: O_NOFOLLOW is not available")
    open_flags = flags | os.O_NOFOLLOW
    try:
        if mode is None:
            return os.open(path, open_flags)
        return os.open(path, open_flags, mode)
    except OSError as exc:
        if exc.errno == errno.ELOOP:
            raise StartupError("symlink") from None
        if exc.errno == errno.EEXIST:
            raise FileExistsError(path) from None
        raise StartupError("open") from None


def read_key_file(path: str, euid: int | None = None) -> bytes:
    """Read the key once. Refuse unless it is a 0600 regular file we own."""
    if euid is None:
        euid = os.geteuid()
    try:
        fd = _open_nofollow(path, os.O_RDONLY | os.O_NONBLOCK)
    except StartupError as exc:
        if str(exc) == "symlink":
            raise StartupError("refusing to start: key file is a symlink") from None
        raise StartupError("refusing to start: key file cannot be opened") from None
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode):
            raise StartupError("refusing to start: key file is not a regular file")
        if stat.S_IMODE(info.st_mode) != 0o600:
            raise StartupError("refusing to start: key file mode is not 0600")
        if info.st_uid != euid:
            raise StartupError("refusing to start: key file owner is not the forwarder uid")
        data = os.read(fd, MAX_KEY_BYTES + 1)
    finally:
        os.close(fd)
    if data.endswith(b"\r\n"):
        data = data[:-2]
    elif data.endswith(b"\n"):
        data = data[:-1]
    if not data:
        raise StartupError("refusing to start: key file is empty")
    if len(data) > MAX_KEY_BYTES:
        raise StartupError("refusing to start: key file is too large")
    if any(byte in data for byte in (b"\n", b"\r", b"\x00")):
        raise StartupError("refusing to start: key file has a forbidden character")
    try:
        data.decode("ascii")
    except UnicodeDecodeError:
        raise StartupError("refusing to start: key file is not ascii") from None
    return data


def _hex64(value: object) -> bool:
    return isinstance(value, str) and len(value) == 64 and all(
        ch in "0123456789abcdef" for ch in value
    )


def _seq_ok(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 1


def _validate_loaded_log(records: list[dict], settings: Settings) -> tuple[int, int, bool]:
    """Return reserved count, next seq, and whether the pinned IP changed."""
    headers = [row for row in records if row.get("kind") == "header"]
    if len(headers) != 1 or records[0] is not headers[0]:
        raise StartupError("refusing to start: log is malformed")
    header = headers[0]
    if header.get("label") != LABEL or header.get("sni") != settings.sni:
        raise StartupError("refusing to start: log does not match this mission")
    if header.get("path") != settings.path or header.get("model") != settings.model:
        raise StartupError("refusing to start: log does not match this mission")
    if header.get("mission_id") != settings.mission_id:
        raise StartupError("refusing to start: mission id does not match the log")
    if not isinstance(header.get("upstream_ip"), str) or not _hex64(header.get("forwarder_sha256")):
        raise StartupError("refusing to start: log is malformed")
    reserved = 0
    seqs: list[int] = []
    reserve_seqs: list[int] = []
    for row in records[1:]:
        kind = row.get("kind")
        if kind == "reserve":
            if not _seq_ok(row.get("seq")) or not isinstance(row.get("start"), str):
                raise StartupError("refusing to start: log is malformed")
            if not isinstance(row.get("upstream_ip"), str):
                raise StartupError("refusing to start: log is malformed")
            reserve_seqs.append(row["seq"])
            seqs.append(row["seq"])
            reserved += 1
            continue
        if kind == "restart":
            if row.get("mission_id") != settings.mission_id:
                raise StartupError("refusing to start: mission id does not match the log")
            if not isinstance(row.get("upstream_ip"), str) or not isinstance(row.get("start"), str):
                raise StartupError("refusing to start: log is malformed")
            if not isinstance(row.get("lookup_record"), str) or not _hex64(row.get("forwarder_sha256")):
                raise StartupError("refusing to start: log is malformed")
            continue
        if kind != "exchange":
            raise StartupError("refusing to start: log is malformed")
        if any(key not in row for key in EXCHANGE_REQUIRED):
            raise StartupError("refusing to start: log is malformed")
        if not _seq_ok(row["seq"]) or row["outcome"] not in OUTCOMES:
            raise StartupError("refusing to start: log is malformed")
        if not _hex64(row["request_sha256"]) or not _hex64(row["response_sha256"]):
            raise StartupError("refusing to start: log is malformed")
        cert = row["cert_sha256"]
        if cert is not None and not _hex64(cert):
            raise StartupError("refusing to start: log is malformed")
        for key in ("status", "elapsed_ms", "request_bytes", "response_bytes"):
            number = row[key]
            if not isinstance(number, int) or isinstance(number, bool) or number < 0:
                raise StartupError("refusing to start: log is malformed")
        if not isinstance(row["start"], str) or not isinstance(row["method"], str):
            raise StartupError("refusing to start: log is malformed")
        seqs.append(row["seq"])
    if len(reserve_seqs) != len(set(reserve_seqs)):
        raise StartupError("refusing to start: log is malformed")
    next_seq = (max(seqs) + 1) if seqs else 1
    ip_changed = header.get("upstream_ip") != settings.upstream_ip
    return reserved, next_seq, ip_changed


class MissionLog:
    def __init__(self, fd: int, reserved: int, next_seq: int) -> None:
        self._fd = fd
        self.reserved = reserved
        self.next_seq = next_seq
        self._lock = threading.Lock()

    def close(self) -> None:
        fd = self._fd
        self._fd = -1
        if fd >= 0:
            os.close(fd)

    def _write_unlocked(self, record: dict) -> None:
        line = json.dumps(record, separators=(",", ":"), ensure_ascii=True).encode("ascii")
        line += b"\n"
        os.write(self._fd, line)
        os.fsync(self._fd)

    def claim(self, upstream: bool, upstream_ip: str = "") -> tuple[int, bool]:
        """Allocate a seq. An upstream claim fsyncs a reserve row before return."""
        with self._lock:
            if upstream and self.reserved >= MAX_REQUESTS:
                seq = self.next_seq
                self.next_seq += 1
                return seq, False
            seq = self.next_seq
            self.next_seq += 1
            if upstream:
                self.reserved += 1
                self._write_unlocked(
                    {
                        "kind": "reserve",
                        "seq": seq,
                        "start": utc_millis(),
                        "upstream_ip": upstream_ip,
                    }
                )
            return seq, True

    def append(self, record: dict) -> None:
        with self._lock:
            self._write_unlocked(record)


def _header_record(settings: Settings) -> dict:
    return {
        "kind": "header",
        "label": LABEL,
        "forwarder_sha256": forwarder_sha256(),
        "listen": f"{settings.listen_host}:{settings.listen_port}",
        "path": settings.path,
        "model": settings.model,
        "upstream_ip": settings.upstream_ip,
        "lookup_record": settings.lookup_record,
        "mission_id": settings.mission_id,
        "sni": settings.sni,
        "hash_scope": "de-framed-request-and-response-bodies",
        "caps_request_bytes": MAX_REQUEST_BYTES,
        "caps_response_bytes": MAX_RESPONSE_BYTES,
        "caps_requests": MAX_REQUESTS,
    }


def open_mission_log(path: str, settings: Settings) -> MissionLog:
    flags = os.O_RDWR | os.O_APPEND
    created = False
    try:
        fd = _open_nofollow(path, flags | os.O_CREAT | os.O_EXCL, 0o600)
        created = True
    except FileExistsError:
        try:
            fd = _open_nofollow(path, flags)
        except StartupError as exc:
            if str(exc) == "symlink":
                raise StartupError("refusing to start: log is a symlink") from None
            raise StartupError("refusing to start: log is unreadable") from None
    except StartupError as exc:
        if str(exc) == "symlink":
            raise StartupError("refusing to start: log is a symlink") from None
        raise StartupError("refusing to start: log is unreadable") from None
    try:
        if created:
            os.fchmod(fd, 0o600)
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode):
            raise StartupError("refusing to start: log is not a regular file")
        if stat.S_IMODE(info.st_mode) != 0o600:
            raise StartupError("refusing to start: log mode is not 0600")
        if info.st_uid != os.geteuid():
            raise StartupError("refusing to start: log owner is not the forwarder uid")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            raise StartupError("refusing to start: log is locked") from None
        os.lseek(fd, 0, os.SEEK_SET)
        raw = b""
        while True:
            chunk = os.read(fd, 65536)
            if not chunk:
                break
            raw += chunk
            if len(raw) > MAX_LOG_BYTES:
                raise StartupError("refusing to start: log is too large")
        if raw == b"":
            line = json.dumps(
                _header_record(settings), separators=(",", ":"), ensure_ascii=True
            ).encode("ascii") + b"\n"
            os.write(fd, line)
            os.fsync(fd)
            return MissionLog(fd, 0, 1)
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            raise StartupError("refusing to start: log is malformed") from None
        if not text.endswith("\n") or "\n\n" in text:
            raise StartupError("refusing to start: log is malformed")
        records = []
        for line in text.split("\n")[:-1]:
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                raise StartupError("refusing to start: log is malformed") from None
            if not isinstance(obj, dict):
                raise StartupError("refusing to start: log is malformed")
            records.append(obj)
        reserved, next_seq, _ip_changed = _validate_loaded_log(records, settings)
        log = MissionLog(fd, reserved, next_seq)
        fd = -1
        log.append(
            {
                "kind": "restart",
                "start": utc_millis(),
                "mission_id": settings.mission_id,
                "upstream_ip": settings.upstream_ip,
                "lookup_record": settings.lookup_record,
                "forwarder_sha256": forwarder_sha256(),
            }
        )
        return log
    except Exception:
        if fd >= 0:
            os.close(fd)
        raise


def production_context() -> ssl.SSLContext:
    ctx = ssl.create_default_context()
    if ctx.verify_mode != ssl.CERT_REQUIRED or not ctx.check_hostname:
        raise StartupError("refusing to start: TLS verification is not enabled")
    try:
        ctx.set_alpn_protocols(["http/1.1"])
    except NotImplementedError:
        pass
    return ctx


def connect_upstream(ip: str, context: ssl.SSLContext) -> ssl.SSLSocket:
    """Dial the operator IP only. Port 443. No name is resolved here."""
    sock = socket.create_connection((ip, 443), CONNECT_TIMEOUT_S)
    try:
        sock.settimeout(CONNECT_TIMEOUT_S)
        try:
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except OSError:
            pass
        tls = context.wrap_socket(
            sock,
            server_hostname="api.x.ai",
            suppress_ragged_eofs=False,
        )
        tls.settimeout(UPSTREAM_TIMEOUT_S)
        return tls
    except Exception:
        sock.close()
        raise


def cert_fingerprint(tls: ssl.SSLSocket) -> str | None:
    try:
        der = tls.getpeercert(binary_form=True)
    except ssl.SSLError:
        return None
    if not der:
        return None
    return sha256_bytes(der)


def _request_length(headers) -> int | str:
    if headers.get("Transfer-Encoding") is not None:
        return "chunked"
    values = headers.get_all("Content-Length")
    if not values:
        return "missing"
    if len(values) != 1:
        return "invalid"
    text = values[0].strip()
    if not text.isdigit():
        return "invalid"
    return int(text)


def build_upstream_request(path: str, headers, body: bytes, key: bytes) -> bytes:
    lines = [
        f"POST {path} HTTP/1.1",
        "Host: api.x.ai",
        "Authorization: Bearer " + key.decode("ascii"),
        "Content-Length: " + str(len(body)),
        "Connection: close",
    ]
    for name, value in headers.items():
        if name.lower() not in ALLOWED_REQUEST_HEADERS:
            continue
        if "\r" in name or "\n" in name or "\r" in value or "\n" in value:
            raise _BadHeader()
        try:
            f"{name}: {value}".encode("latin-1")
        except UnicodeEncodeError:
            raise _BadHeader() from None
        lines.append(f"{name}: {value}")
    return ("\r\n".join(lines) + "\r\n\r\n").encode("latin-1") + body


def _read_head(
    sock: ssl.SSLSocket,
    client: socket.socket | None,
    half_closed: bool,
    clock: _IdleClock,
) -> tuple[bytes, bytes]:
    buf = b""
    while b"\r\n\r\n" not in buf:
        piece = _recv_some(sock, client, half_closed, clock)
        if not piece:
            raise _UpstreamCut("eof")
        buf += piece
        if len(buf) > 65536:
            raise _UpstreamCut("headers")
    index = buf.find(b"\r\n\r\n")
    return buf[:index], buf[index + 4 :]


def _split_head(head: bytes) -> tuple[str, list[tuple[str, str]]]:
    text = head.decode("iso-8859-1")
    lines = text.split("\r\n")
    if not lines or not lines[0]:
        raise _UpstreamCut("status")
    pairs: list[tuple[str, str]] = []
    for line in lines[1:]:
        if not line:
            continue
        if line[0] in " \t" or ":" not in line:
            raise _UpstreamCut("header")
        name, value = line.split(":", 1)
        pairs.append((name.strip(), value.strip()))
    return lines[0], pairs


def _status_code(status_line: str) -> int:
    parts = status_line.split(" ")
    if len(parts) < 2 or not parts[1].isdigit():
        raise _UpstreamCut("status")
    return int(parts[1])


def _content_length(pairs: list[tuple[str, str]]) -> int | None | str:
    values = [value.strip() for name, value in pairs if name.lower() == "content-length"]
    if not values:
        return None
    numbers = []
    for value in values:
        if not value.isdigit():
            return "bad"
        numbers.append(int(value))
    if any(number != numbers[0] for number in numbers):
        return "bad"
    return numbers[0]


def _is_chunked(pairs: list[tuple[str, str]]) -> bool:
    for name, value in pairs:
        if name.lower() != "transfer-encoding":
            continue
        parts = [part.strip().lower() for part in value.split(",")]
        if "chunked" in parts:
            return True
    return False


def _response_head(status_line: str, pairs: list[tuple[str, str]], mode: str, length: int) -> bytes:
    drop = {
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "upgrade",
        "transfer-encoding",
        "content-length",
    }
    lines = [status_line]
    for name, value in pairs:
        if name.lower() in drop:
            continue
        lines.append(f"{name}: {value}")
    if mode == "content-length":
        lines.append(f"Content-Length: {length}")
    else:
        lines.append("Transfer-Encoding: chunked")
    lines.append("Connection: close")
    return ("\r\n".join(lines) + "\r\n\r\n").encode("iso-8859-1")


def _write_local(wfile, status: int, outcome: str) -> bytes:
    reason = {
        400: "Bad Request",
        404: "Not Found",
        405: "Method Not Allowed",
        411: "Length Required",
        413: "Payload Too Large",
        429: "Too Many Requests",
        502: "Bad Gateway",
    }[status]
    body = json.dumps({"outcome": outcome}, separators=(",", ":")).encode("ascii")
    head = (
        f"HTTP/1.1 {status} {reason}\r\n"
        "Content-Type: application/json\r\n"
        f"Content-Length: {len(body)}\r\n"
        "Connection: close\r\n"
        "\r\n"
    ).encode("ascii")
    wfile.write(head)
    wfile.write(body)
    wfile.flush()
    return body


def _tcp_state(sock: socket.socket) -> int:
    try:
        info = sock.getsockopt(socket.IPPROTO_TCP, socket.TCP_INFO, 1)
    except OSError:
        return 0
    return info[0] if info else 0


class _IdleClock:
    """Per-read idle timeout. A read that returns data starts the window again."""

    def reset(self) -> None:
        self.deadline = time.monotonic() + UPSTREAM_TIMEOUT_S

    def expired(self) -> bool:
        return time.monotonic() >= self.deadline

    def slice(self) -> float:
        return max(0.0, min(0.2, self.deadline - time.monotonic()))


def _poll_mask(sock: socket.socket, timeout: float) -> int:
    """Poll one socket. poll, not select: fds can be 1024 or higher."""
    try:
        poller = select.poll()
        poller.register(sock.fileno(), select.POLLIN | select.POLLERR | select.POLLHUP)
        events = poller.poll(max(0, int(timeout * 1000)))
    except (OSError, ValueError):
        return 0
    if not events:
        return 0
    return events[0][1]


def _write_side_closed(sock: socket.socket) -> bool:
    if _tcp_state(sock) == 8:
        return True
    mask = _poll_mask(sock, 0)
    if not mask:
        return False
    if mask & (select.POLLERR | select.POLLNVAL):
        return True
    if not mask & select.POLLIN:
        return False
    try:
        peeked = sock.recv(1, socket.MSG_PEEK | socket.MSG_DONTWAIT)
    except BlockingIOError:
        return False
    except OSError:
        return True
    return peeked == b""


_probe_at: dict[int, float] = {}


def _close_wait_write_probe(sock: socket.socket) -> bool:
    """Notice a dead peer that already half-closed.

    Linux does not send keepalive segments in CLOSE_WAIT, and a later full
    close sends no packet. A write is what changes the socket. The byte is
    out-of-band so it is not part of the HTTP stream. The schedule is the
    keepalive idle/interval configured on the socket.
    """
    if _tcp_state(sock) != 8:
        return False
    due = _probe_at.get(id(sock), 0.0)
    now = time.monotonic()
    if now < due:
        return False
    try:
        interval = sock.getsockopt(socket.IPPROTO_TCP, socket.TCP_KEEPINTVL)
    except OSError:
        interval = KEEPALIVE_INTVL
    _probe_at[id(sock)] = now + max(1, interval)
    try:
        sock.send(b"\x00", socket.MSG_OOB | socket.MSG_DONTWAIT | socket.MSG_NOSIGNAL)
    except OSError:
        return True
    try:
        err = sock.getsockopt(socket.SOL_SOCKET, socket.SO_ERROR)
    except OSError:
        return True
    if err:
        return True
    return _tcp_state(sock) in (7, 9)


def _client_disconnected(sock: socket.socket, half_closed_at_start: bool) -> bool:
    """True when the peer is dead, not merely write-half-closed.

    CLOSE_WAIT by itself is not a disconnect, whether or not the client had
    already half-closed when the response started. A dead peer shows up as a
    socket error: an RST, keepalive on an established socket, or the
    CLOSE_WAIT write probe.
    """
    del half_closed_at_start
    try:
        err = sock.getsockopt(socket.SOL_SOCKET, socket.SO_ERROR)
    except OSError:
        return True
    if err:
        return True
    state = _tcp_state(sock)
    if state in (7, 9):
        return True
    return state == 8 and _close_wait_write_probe(sock)


def _arm_client_keepalive(sock: socket.socket) -> None:
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_KEEPALIVE, 1)
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_KEEPIDLE, KEEPALIVE_IDLE)
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_KEEPINTVL, KEEPALIVE_INTVL)
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_KEEPCNT, KEEPALIVE_CNT)
    except OSError:
        return
    _probe_at[id(sock)] = time.monotonic() + KEEPALIVE_IDLE


def _recv_some(
    sock: ssl.SSLSocket,
    client: socket.socket | None,
    half_closed: bool,
    clock: _IdleClock,
) -> bytes | None:
    """Return bytes, b'' on a clean TLS close, or None on a cut.

    A non-empty read resets the idle clock. Raises _ClientGone after closing
    upstream when keepalive or an RST shows the client is dead.
    """
    while True:
        if client is not None and _client_disconnected(client, half_closed):
            try:
                sock.close()
            except OSError:
                pass
            raise _ClientGone()
        # A readable SSL socket can still block in recv. Keep that wait
        # inside the idle slice so a silent upstream cannot hide a dead client.
        sock.settimeout(max(clock.slice(), 0.05))
        if sock.pending():
            try:
                data = sock.recv(8192)
            except (TimeoutError, socket.timeout):
                continue
            except (OSError, ssl.SSLError):
                return None
            if data:
                clock.reset()
            return data
        if clock.expired():
            return None
        mask = _poll_mask(sock, clock.slice())
        if not mask:
            continue
        try:
            data = sock.recv(8192)
        except (TimeoutError, socket.timeout):
            continue
        except (OSError, ssl.SSLError):
            return None
        if data:
            clock.reset()
        return data


def _take_chunk_frames(buf: bytes) -> tuple[bytes, bytes, bool, bool, bytes]:
    """Split complete chunk frames out of buf.

    Returns forwardable bytes, the unparsed tail, whether the 0-chunk
    finished the message, whether the framing is invalid, and the de-framed
    payload inside those frames.
    """
    frames: list[bytes] = []
    payload = bytearray()
    index = 0
    while True:
        newline = buf.find(b"\r\n", index)
        if newline < 0:
            break
        size_token = buf[index:newline].split(b";", 1)[0].strip()
        try:
            size = int(size_token, 16)
        except ValueError:
            return b"".join(frames), buf[index:], False, True, bytes(payload)
        data_at = newline + 2
        if size == 0:
            if buf[data_at : data_at + 2] == b"\r\n":
                end = data_at + 2
            else:
                end_rel = buf.find(b"\r\n\r\n", newline)
                if end_rel < 0:
                    break
                end = end_rel + 4
            frames.append(buf[index:end])
            return b"".join(frames), buf[end:], True, False, bytes(payload)
        end = data_at + size + 2
        if len(buf) < end:
            break
        if buf[end - 2 : end] != b"\r\n":
            return b"".join(frames), buf[index:], False, True, bytes(payload)
        frames.append(buf[index:end])
        payload += buf[data_at : data_at + size]
        index = end
    return b"".join(frames), buf[index:], False, False, bytes(payload)


def _wire_reframe(
    tls: ssl.SSLSocket,
    wfile,
    rest: bytes,
    code: int,
    client: socket.socket | None,
    half_closed: bool,
    clock: _IdleClock,
) -> tuple[int, str, int, str]:
    """Re-frame a close-delimited upstream body as chunked, unbuffered.

    The hash and byte count are the de-framed payload, not the chunk framing.
    """
    digest = hashlib.sha256()
    payload = 0
    pending = rest

    def write_out(frame: bytes, piece: bytes) -> None:
        nonlocal payload
        wfile.write(frame)
        wfile.flush()
        if piece:
            digest.update(piece)
            payload += len(piece)

    try:
        while True:
            if not pending:
                nxt = _recv_some(tls, client, half_closed, clock)
                if nxt is None:
                    return code, "upstream_error", payload, digest.hexdigest()
                if nxt == b"":
                    write_out(b"0\r\n\r\n", b"")
                    return code, "ok", payload, digest.hexdigest()
                pending = nxt
            room = MAX_RESPONSE_BYTES - payload
            if room <= 0:
                return code, "response_cap", payload, digest.hexdigest()
            piece = pending[:room]
            frame = f"{len(piece):x}\r\n".encode("ascii") + piece + b"\r\n"
            write_out(frame, piece)
            pending = pending[len(piece) :]
            if payload >= MAX_RESPONSE_BYTES and pending:
                return code, "response_cap", payload, digest.hexdigest()
    except _ClientGone:
        return code, "upstream_error", payload, digest.hexdigest()


def relay_response(
    tls: ssl.SSLSocket, wfile, client: socket.socket | None = None
) -> tuple[int, str, int, str]:
    """Forward one upstream response. Never close-delimit the body.

    Content-Length from upstream is preserved. A close-delimited body is
    re-framed as chunked. A chunked body is parsed, and the terminating
    0-chunk ends the reply even if upstream stays open. A cut does not
    get a terminating 0-chunk. request_sha256 and response_sha256 are the
    de-framed body bytes only, so a chunked body and a Content-Length body
    of the same payload hash the same.
    """
    half_closed = bool(client is not None and _write_side_closed(client))
    clock = _IdleClock()
    clock.reset()
    head, rest = _read_head(tls, client, half_closed, clock)
    status_line, pairs = _split_head(head)
    code = _status_code(status_line)
    if 100 <= code <= 199:
        body = _write_local(wfile, 502, "502")
        return 502, "502", len(body), sha256_bytes(body)
    if 300 <= code <= 399:
        body = _write_local(wfile, 502, "redirect_refused")
        return 502, "redirect_refused", len(body), sha256_bytes(body)
    length = _content_length(pairs)
    if length == "bad":
        body = _write_local(wfile, 502, "502")
        return 502, "502", len(body), sha256_bytes(body)
    if isinstance(length, int) and length > MAX_RESPONSE_BYTES:
        body = _write_local(wfile, 502, "response_cap")
        return 502, "response_cap", len(body), sha256_bytes(body)
    chunked = _is_chunked(pairs)
    if chunked:
        mode = "chunked"
    elif isinstance(length, int):
        mode = "content-length"
    else:
        mode = "close"
    framed = "content-length" if mode == "content-length" else "chunked"
    wfile.write(_response_head(status_line, pairs, framed, length if isinstance(length, int) else 0))
    wfile.flush()
    if mode == "close":
        return _wire_reframe(tls, wfile, rest, code, client, half_closed, clock)
    digest = hashlib.sha256()
    sent = 0
    try:
        if mode == "content-length":
            assert isinstance(length, int)
            pending = rest
            while sent < length:
                if not pending:
                    pending = _recv_some(tls, client, half_closed, clock)
                    if pending is None or pending == b"":
                        return code, "upstream_error", sent, digest.hexdigest()
                take = pending[: length - sent]
                wfile.write(take)
                wfile.flush()
                digest.update(take)
                sent += len(take)
                pending = pending[len(take) :]
            try:
                tls.close()
            except OSError:
                pass
            return code, "ok", sent, digest.hexdigest()
        pending = rest
        payload = 0
        while True:
            frames, pending, finished, invalid, payload_bytes = _take_chunk_frames(pending)
            if invalid:
                return code, "upstream_error", payload, digest.hexdigest()
            if payload_bytes or frames:
                if payload + len(payload_bytes) > MAX_RESPONSE_BYTES:
                    return code, "response_cap", payload, digest.hexdigest()
                wfile.write(frames)
                wfile.flush()
                if payload_bytes:
                    digest.update(payload_bytes)
                    payload += len(payload_bytes)
            if finished:
                try:
                    tls.close()
                except OSError:
                    pass
                return code, "ok", payload, digest.hexdigest()
            nxt = _recv_some(tls, client, half_closed, clock)
            if nxt is None or nxt == b"":
                return code, "upstream_error", payload, digest.hexdigest()
            pending += nxt
    except _ClientGone:
        counted = payload if mode == "chunked" else sent
        return code, "upstream_error", counted, digest.hexdigest()


def _target_path(handler: BaseHTTPRequestHandler) -> str:
    parts = handler.requestline.split(" ")
    if len(parts) >= 2:
        return parts[1]
    return handler.path


def _read_exact(rfile, count: int) -> bytes | None:
    buf = b""
    while len(buf) < count:
        piece = rfile.read(count - len(buf))
        if not piece:
            return None
        buf += piece
    return buf


def _log_exchange(
    mission: MissionLog,
    *,
    seq: int,
    started: str,
    elapsed_ms: int,
    method: str,
    path: str,
    status: int,
    outcome: str,
    request_body: bytes,
    response_bytes: int,
    response_sha256: str,
    upstream_ip: str,
    cert_sha256: str | None,
) -> None:
    mission.append(
        {
            "kind": "exchange",
            "seq": seq,
            "start": started,
            "elapsed_ms": elapsed_ms,
            "method": method,
            "path": path,
            "status": status,
            "outcome": outcome,
            "request_bytes": len(request_body),
            "response_bytes": response_bytes,
            "request_sha256": sha256_bytes(request_body),
            "response_sha256": response_sha256,
            "upstream_ip": upstream_ip,
            "cert_sha256": cert_sha256,
        }
    )


def _refuse(handler, status: int, outcome: str, request_body: bytes, started_m: float, started: str, method: str, path: str) -> None:
    body = _write_local(handler.wfile, status, outcome)
    seq, _allowed = handler.server.mission.claim(False)
    _log_exchange(
        handler.server.mission,
        seq=seq,
        started=started,
        elapsed_ms=int((time.monotonic() - started_m) * 1000),
        method=method,
        path=path,
        status=status,
        outcome=outcome,
        request_body=request_body,
        response_bytes=len(body),
        response_sha256=sha256_bytes(body),
        upstream_ip=handler.server.upstream_ip,
        cert_sha256=None,
    )


def handle_exchange(handler: BaseHTTPRequestHandler) -> None:
    """One request. A client write-side half-close must not stop the response."""
    started_m = time.monotonic()
    started = utc_millis()
    method = handler.command or ""
    path = _target_path(handler)
    settings: Settings = handler.server.settings
    if path != settings.path:
        _refuse(handler, 404, "404", b"", started_m, started, method, path)
        return
    if method != "POST":
        _refuse(handler, 405, "405", b"", started_m, started, method, path)
        return
    length = _request_length(handler.headers)
    if length in ("chunked", "missing"):
        _refuse(handler, 411, "411", b"", started_m, started, method, path)
        return
    if length == "invalid":
        _refuse(handler, 400, "400", b"", started_m, started, method, path)
        return
    if isinstance(length, int) and length > MAX_REQUEST_BYTES:
        # Do not read the body. The client may already have half-closed.
        _refuse(handler, 413, "413", b"", started_m, started, method, path)
        return
    body = _read_exact(handler.rfile, int(length))
    if body is None:
        _refuse(handler, 400, "400", b"", started_m, started, method, path)
        return
    try:
        obj = json.loads(body.decode("utf-8"), object_pairs_hook=_json_object)
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError):
        _refuse(handler, 400, "400", body, started_m, started, method, path)
        return
    if not isinstance(obj, dict) or not isinstance(obj.get("model"), str) or obj.get("model") != settings.model:
        _refuse(handler, 400, "400", body, started_m, started, method, path)
        return
    if obj.get("stream") is not True:
        _refuse(handler, 400, "400", body, started_m, started, method, path)
        return
    try:
        upstream = build_upstream_request(settings.path, handler.headers, body, settings.key)
    except _BadHeader:
        _refuse(handler, 400, "400", body, started_m, started, method, path)
        return
    seq, allowed = handler.server.mission.claim(True, handler.server.upstream_ip)
    if not allowed:
        response = _write_local(handler.wfile, 429, "429")
        _log_exchange(
            handler.server.mission,
            seq=seq,
            started=started,
            elapsed_ms=int((time.monotonic() - started_m) * 1000),
            method=method,
            path=path,
            status=429,
            outcome="429",
            request_body=body,
            response_bytes=len(response),
            response_sha256=sha256_bytes(response),
            upstream_ip=handler.server.upstream_ip,
            cert_sha256=None,
        )
        return
    tls = None
    cert = None
    status = 502
    outcome = "upstream_error"
    response_len = 0
    response_hash = sha256_bytes(b"")
    try:
        tls = connect_upstream(handler.server.upstream_ip, handler.server.ssl_context)
        handler.server.register_socket(tls)
        cert = cert_fingerprint(tls)
        tls.sendall(upstream)
        status, outcome, response_len, response_hash = relay_response(
            tls, handler.wfile, handler.connection
        )
    except Exception:
        if response_len == 0 and outcome == "upstream_error":
            try:
                response = _write_local(handler.wfile, 502, "upstream_error")
                response_len = len(response)
                response_hash = sha256_bytes(response)
                status = 502
            except Exception:
                pass
    finally:
        if tls is not None:
            handler.server.unregister_socket(tls)
            try:
                tls.close()
            except OSError:
                pass
        _log_exchange(
            handler.server.mission,
            seq=seq,
            started=started,
            elapsed_ms=int((time.monotonic() - started_m) * 1000),
            method=method,
            path=path,
            status=status,
            outcome=outcome,
            request_body=body,
            response_bytes=response_len,
            response_sha256=response_hash,
            upstream_ip=handler.server.upstream_ip,
            cert_sha256=cert,
        )


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    disable_nagle_algorithm = True
    # Upstream timeout is 300s. The inbound side uses the same idle bound.
    timeout = UPSTREAM_TIMEOUT_S

    def handle(self) -> None:
        # Exactly one request. Do not treat a client write-side half-close
        # as the end of the exchange: pi-ai shuts down writes after the body
        # and still reads the response. Keepalive is what later notices a
        # full close, because that close sends no further packet.
        self.close_connection = True
        _arm_client_keepalive(self.connection)
        self.server.register_socket(self.connection)
        try:
            self.raw_requestline = self.rfile.readline(65537)
            if not self.raw_requestline or len(self.raw_requestline) > 65536:
                self.close_connection = True
                return
            if not self.parse_request():
                self.close_connection = True
                return
            self.close_connection = True
            handle_exchange(self)
        except Exception:
            self.close_connection = True
        finally:
            self.close_connection = True
            self.server.unregister_socket(self.connection)

    def handle_expect_100(self) -> bool:
        # The body is already on the wire. Do not delay it with 100 Continue.
        return True

    def log_message(self, fmt: str, *args) -> None:
        return

    def log_error(self, fmt: str, *args) -> None:
        return


class ForwarderServer(ThreadingHTTPServer):
    daemon_threads = False
    block_on_close = True
    allow_reuse_address = False
    address_family = socket.AF_INET

    def __init__(self, address, handler) -> None:
        super().__init__(address, handler)
        self.active: list[socket.socket] = []
        self.active_lock = threading.Lock()
        self.settings: Settings
        self.mission: MissionLog
        self.ssl_context: ssl.SSLContext
        self.upstream_ip = ""

    def handle_error(self, request, client_address) -> None:
        return

    def register_socket(self, sock) -> None:
        with self.active_lock:
            self.active.append(sock)

    def unregister_socket(self, sock) -> None:
        with self.active_lock:
            try:
                self.active.remove(sock)
            except ValueError:
                pass

    def close_active(self) -> None:
        with self.active_lock:
            socks = list(self.active)
        for sock in socks:
            try:
                sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            try:
                sock.close()
            except OSError:
                pass

    def begin_shutdown(self) -> None:
        self.close_active()
        if self.socket is not None:
            try:
                self.socket.close()
            except OSError:
                pass
        try:
            self.shutdown()
        except Exception:
            pass


def _json_object(pairs: list[tuple[str, object]]) -> dict:
    keys = [key for key, _value in pairs]
    if len(keys) != len(set(keys)):
        raise ValueError("duplicate key")
    return dict(pairs)


def build_server(settings: Settings, mission: MissionLog, context: ssl.SSLContext | None = None, ip_check=None):
    if settings.listen_host != "127.0.0.1":
        raise StartupError("refusing to start: listen address must be 127.0.0.1:<port>")
    check = ip_check if ip_check is not None else require_global_ip
    upstream_ip = check(settings.upstream_ip)
    ctx = context if context is not None else production_context()
    try:
        httpd = ForwarderServer((settings.listen_host, settings.listen_port), Handler)
    except OSError as exc:
        if exc.errno == errno.EADDRINUSE:
            raise StartupError("refusing to start: listen port is in use") from None
        raise
    bound = httpd.socket.getsockname()
    if bound[0] != "127.0.0.1":
        httpd.server_close()
        raise StartupError("refusing to start: refused to bind a non-127.0.0.1 address")
    httpd.settings = settings
    httpd.mission = mission
    httpd.ssl_context = ctx
    httpd.upstream_ip = upstream_ip
    return httpd


def install_shutdown_signals(httpd: ForwarderServer) -> None:
    def _stop(_signum, _frame) -> None:
        threading.Thread(target=httpd.begin_shutdown, daemon=True).start()

    signal.signal(signal.SIGINT, _stop)
    signal.signal(signal.SIGTERM, _stop)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="forwarder.py")
    parser.add_argument("--listen", required=True)
    parser.add_argument("--path", required=True)
    parser.add_argument("--model", default="grok-4.7")
    parser.add_argument("--key-file", required=True)
    parser.add_argument("--host-uid", required=True, type=int)
    parser.add_argument("--upstream-ip", required=True)
    parser.add_argument("--lookup-record", required=True)
    parser.add_argument("--mission-id", required=True)
    parser.add_argument("--log", required=True)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    mission = None
    httpd = None
    try:
        host, port = parse_listen(args.listen)
        path = validate_path(args.path)
        model = validate_model(args.model)
        if not isinstance(args.lookup_record, str) or not args.lookup_record:
            raise StartupError("refusing to start: lookup record is empty")
        if len(args.lookup_record) > 8192:
            raise StartupError("refusing to start: lookup record is too long")
        mission_id = validate_mission_id(args.mission_id)
        assert_process_identity(args.host_uid, os.geteuid())
        upstream_ip = require_global_ip(args.upstream_ip)
        key = read_key_file(args.key_file)
        settings = Settings(
            listen_host=host,
            listen_port=port,
            path=path,
            model=model,
            key=key,
            host_uid=args.host_uid,
            upstream_ip=upstream_ip,
            lookup_record=args.lookup_record,
            mission_id=mission_id,
        )
        mission = open_mission_log(args.log, settings)
        httpd = build_server(settings, mission, production_context(), require_global_ip)
        install_shutdown_signals(httpd)
        sys.stderr.write(f"listening {httpd.server_address[0]}:{httpd.server_address[1]}\n")
        sys.stderr.flush()
        httpd.serve_forever(poll_interval=0.2)
        return 0
    except StartupError as exc:
        sys.stderr.write(str(exc) + "\n")
        return 1
    finally:
        if httpd is not None:
            httpd.close_active()
            try:
                httpd.server_close()
            except Exception:
                pass
        if mission is not None:
            mission.close()


if __name__ == "__main__":
    sys.exit(main())
