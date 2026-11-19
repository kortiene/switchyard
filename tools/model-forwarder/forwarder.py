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
import hashlib
import ipaddress
import json
import os
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
UPSTREAM_TIMEOUT_S = 300
UPSTREAM_SNI = "api.x.ai"
MAX_KEY_BYTES = 4096
MAX_LOG_BYTES = 8 * 1024 * 1024

RESERVED_OUTCOMES = frozenset(
    {"ok", "redirect_refused", "response_cap", "upstream_error", "502"}
)
LOCAL_OUTCOMES = frozenset({"429", "413", "411", "400", "404", "405"})
OUTCOMES = RESERVED_OUTCOMES | LOCAL_OUTCOMES

REQUEST_DROP = frozenset(
    {
        "authorization",
        "proxy-authorization",
        "cookie",
        "x-api-key",
        "api-key",
        "host",
        "content-length",
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    }
)

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
    """Accept only a global IP literal. Never resolves a name."""
    try:
        addr = ipaddress.ip_address(ip)
    except ValueError:
        raise StartupError("refusing to start: upstream-ip must be an IP address") from None
    if not addr.is_global:
        raise StartupError("refusing to start: upstream-ip is not a global address") from None
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
        fd = _open_nofollow(path, os.O_RDONLY)
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


def _validate_loaded_log(records: list[dict], settings: Settings) -> tuple[int, int]:
    headers = [row for row in records if row.get("kind") == "header"]
    exchanges = [row for row in records if row.get("kind") == "exchange"]
    if len(headers) != 1 or records[0] is not headers[0]:
        raise StartupError("refusing to start: log is malformed")
    if len(headers) + len(exchanges) != len(records):
        raise StartupError("refusing to start: log is malformed")
    header = headers[0]
    if header.get("label") != LABEL:
        raise StartupError("refusing to start: log does not match this mission")
    if header.get("path") != settings.path or header.get("model") != settings.model:
        raise StartupError("refusing to start: log does not match this mission")
    if header.get("upstream_ip") != settings.upstream_ip or header.get("sni") != settings.sni:
        raise StartupError("refusing to start: log does not match this mission")
    if not _hex64(header.get("forwarder_sha256")):
        raise StartupError("refusing to start: log is malformed")
    seqs: list[int] = []
    reserved = 0
    for row in exchanges:
        if any(key not in row for key in EXCHANGE_REQUIRED):
            raise StartupError("refusing to start: log is malformed")
        seq = row["seq"]
        outcome = row["outcome"]
        if not isinstance(seq, int) or isinstance(seq, bool) or seq < 1:
            raise StartupError("refusing to start: log is malformed")
        if outcome not in OUTCOMES:
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
        if outcome in RESERVED_OUTCOMES:
            reserved += 1
        seqs.append(seq)
    if sorted(seqs) != list(range(1, len(seqs) + 1)):
        raise StartupError("refusing to start: log is malformed")
    next_seq = (max(seqs) + 1) if seqs else 1
    return reserved, next_seq


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

    def claim(self, upstream: bool) -> tuple[int, bool]:
        """Reserve a sequence. Upstream claims also take one of the 40 slots."""
        with self._lock:
            allowed = True
            if upstream and self.reserved >= MAX_REQUESTS:
                allowed = False
            elif upstream:
                self.reserved += 1
            seq = self.next_seq
            self.next_seq += 1
            return seq, allowed

    def append(self, record: dict) -> None:
        line = json.dumps(record, separators=(",", ":"), ensure_ascii=True).encode("ascii")
        line += b"\n"
        with self._lock:
            os.write(self._fd, line)
            os.fsync(self._fd)


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
        "sni": settings.sni,
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
        reserved, next_seq = _validate_loaded_log(records, settings)
        return MissionLog(fd, reserved, next_seq)
    except Exception:
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
    sock = socket.create_connection((ip, 443), UPSTREAM_TIMEOUT_S)
    try:
        sock.settimeout(UPSTREAM_TIMEOUT_S)
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


def _connection_hops(headers) -> set[str]:
    names: set[str] = set()
    for value in headers.get_all("Connection") or []:
        for part in value.split(","):
            token = part.strip().lower()
            if token and token not in ("close", "keep-alive"):
                names.add(token)
    return names


def build_upstream_request(path: str, headers, body: bytes, key: bytes) -> bytes:
    hops = _connection_hops(headers)
    lines = [
        f"POST {path} HTTP/1.1",
        "Host: api.x.ai",
        "Authorization: Bearer " + key.decode("ascii"),
        "Content-Length: " + str(len(body)),
        "Connection: close",
    ]
    for name, value in headers.items():
        lowered = name.lower()
        if lowered in REQUEST_DROP or lowered in hops:
            continue
        if "\r" in name or "\n" in name or "\r" in value or "\n" in value:
            raise _BadHeader()
        try:
            f"{name}: {value}".encode("latin-1")
        except UnicodeEncodeError:
            raise _BadHeader() from None
        lines.append(f"{name}: {value}")
    return ("\r\n".join(lines) + "\r\n\r\n").encode("latin-1") + body


def _read_head(sock: ssl.SSLSocket) -> tuple[bytes, bytes]:
    buf = b""
    while b"\r\n\r\n" not in buf:
        try:
            piece = sock.recv(4096)
        except (TimeoutError, socket.timeout, OSError, ssl.SSLError) as exc:
            raise _UpstreamCut(type(exc).__name__) from None
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


def _recv_some(sock: ssl.SSLSocket) -> bytes | None:
    """Return bytes, b'' on a clean TLS close, or None on a cut."""
    try:
        return sock.recv(8192)
    except (TimeoutError, socket.timeout, OSError, ssl.SSLError):
        return None


def _wire_reframe(tls: ssl.SSLSocket, wfile, rest: bytes, code: int) -> tuple[int, str, int, str]:
    """Re-frame a close-delimited upstream body as chunked, unbuffered."""
    digest = hashlib.sha256()
    wire = 0
    payload = 0
    pending = rest

    def account(frame: bytes) -> None:
        nonlocal wire
        wfile.write(frame)
        wfile.flush()
        digest.update(frame)
        wire += len(frame)

    while True:
        if not pending:
            nxt = _recv_some(tls)
            if nxt is None:
                return code, "upstream_error", wire, digest.hexdigest()
            if nxt == b"":
                end = b"0\r\n\r\n"
                account(end)
                return code, "ok", wire, digest.hexdigest()
            pending = nxt
        room = MAX_RESPONSE_BYTES - payload
        if room <= 0:
            return code, "response_cap", wire, digest.hexdigest()
        piece = pending[:room]
        frame = f"{len(piece):x}\r\n".encode("ascii") + piece + b"\r\n"
        account(frame)
        payload += len(piece)
        pending = pending[len(piece) :]
        if payload >= MAX_RESPONSE_BYTES and pending:
            return code, "response_cap", wire, digest.hexdigest()


def relay_response(tls: ssl.SSLSocket, wfile) -> tuple[int, str, int, str]:
    """Forward one upstream response. Never close-delimit the body.

    Content-Length from upstream is preserved. A close-delimited body is
    re-framed as chunked. Chunked upstream bytes are copied through.
    A cut or the 1 MiB cap does not write a terminating 0-chunk.
    """
    head, rest = _read_head(tls)
    status_line, pairs = _split_head(head)
    code = _status_code(status_line)
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
        return _wire_reframe(tls, wfile, rest, code)
    digest = hashlib.sha256()
    sent = 0
    if mode == "content-length":
        assert isinstance(length, int)
        pending = rest
        while sent < length:
            if not pending:
                pending = _recv_some(tls)
                if pending is None or pending == b"":
                    return code, "upstream_error", sent, digest.hexdigest()
            take = pending[: length - sent]
            wfile.write(take)
            wfile.flush()
            digest.update(take)
            sent += len(take)
            pending = pending[len(take) :]
        return code, "ok", sent, digest.hexdigest()
    pending = rest
    while True:
        if not pending:
            pending = _recv_some(tls)
            if pending is None:
                return code, "upstream_error", sent, digest.hexdigest()
            if pending == b"":
                return code, "ok", sent, digest.hexdigest()
        if sent + len(pending) > MAX_RESPONSE_BYTES:
            room = MAX_RESPONSE_BYTES - sent
            if room:
                wfile.write(pending[:room])
                wfile.flush()
                digest.update(pending[:room])
                sent += room
            return code, "response_cap", sent, digest.hexdigest()
        wfile.write(pending)
        wfile.flush()
        digest.update(pending)
        sent += len(pending)
        pending = b""


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
        obj = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        _refuse(handler, 400, "400", body, started_m, started, method, path)
        return
    if not isinstance(obj, dict) or not isinstance(obj.get("model"), str) or obj.get("model") != settings.model:
        _refuse(handler, 400, "400", body, started_m, started, method, path)
        return
    try:
        upstream = build_upstream_request(settings.path, handler.headers, body, settings.key)
    except _BadHeader:
        _refuse(handler, 400, "400", body, started_m, started, method, path)
        return
    seq, allowed = handler.server.mission.claim(True)
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
        status, outcome, response_len, response_hash = relay_response(tls, handler.wfile)
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
        # and still reads the response.
        self.close_connection = True
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
    daemon_threads = True
    block_on_close = False
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


def build_server(settings: Settings, mission: MissionLog, context: ssl.SSLContext | None = None, ip_check=None):
    if settings.listen_host != "127.0.0.1":
        raise StartupError("refusing to start: listen address must be 127.0.0.1:<port>")
    check = ip_check if ip_check is not None else require_global_ip
    upstream_ip = check(settings.upstream_ip)
    ctx = context if context is not None else production_context()
    httpd = ForwarderServer((settings.listen_host, settings.listen_port), Handler)
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
            try:
                httpd.server_close()
            except Exception:
                pass
        if mission is not None:
            mission.close()


if __name__ == "__main__":
    sys.exit(main())
