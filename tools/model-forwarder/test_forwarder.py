#!/usr/bin/env python3
"""Stdlib tests for the operator model forwarder. No network, no real key."""

from __future__ import annotations

import contextlib
import hashlib
import io
import ipaddress
import json
import os
import shutil
import socket
import ssl
import stat
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

import forwarder

PATH = "/v1/chat/completions"
KEY = b"sy-test-key-0123456789abcdef"
LOOKUP = "dig +short, 2026-11-19T00:00:00Z, 8.8.8.8"
HERE = Path(__file__).resolve().parent
CERTS: dict[str, Path] = {}


def openssl(*args: str, cwd: Path) -> None:
    subprocess.run(
        ["openssl", *args],
        cwd=cwd,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def setUpModule() -> None:
    directory = Path(tempfile.mkdtemp(prefix="fwd-ca-"))
    CERTS["dir"] = directory
    openssl("ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", "ca.key", cwd=directory)
    openssl(
        "req", "-x509", "-new", "-key", "ca.key", "-out", "ca.crt", "-days", "2",
        "-subj", "/CN=TestCA", "-addext", "basicConstraints=critical,CA:TRUE",
        cwd=directory,
    )
    for name, san in (("good", "DNS:api.x.ai"), ("bad", "DNS:wrong.example")):
        openssl("ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", f"{name}.key", cwd=directory)
        openssl(
            "req", "-new", "-key", f"{name}.key", "-out", f"{name}.csr",
            "-subj", f"/CN={san[4:]}",
            "-addext", f"subjectAltName={san}",
            "-addext", "extendedKeyUsage=serverAuth",
            cwd=directory,
        )
        openssl(
            "x509", "-req", "-in", f"{name}.csr", "-CA", "ca.crt", "-CAkey", "ca.key",
            "-CAcreateserial", "-out", f"{name}.crt", "-days", "2", "-copy_extensions", "copy",
            cwd=directory,
        )


def tearDownModule() -> None:
    directory = CERTS.get("dir")
    if directory is not None:
        shutil.rmtree(directory, ignore_errors=True)


def allow_literal(ip: str) -> str:
    return str(ipaddress.ip_address(ip))


def client_context(ca_path: Path) -> ssl.SSLContext:
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    ctx.load_verify_locations(cafile=str(ca_path))
    ctx.check_hostname = True
    ctx.verify_mode = ssl.CERT_REQUIRED
    return ctx


def chat_body(model: str = "grok-4.7", content: str = "BODYMARKER-do-not-log") -> bytes:
    return json.dumps(
        {"model": model, "stream": True, "messages": [{"role": "user", "content": content}]}
    ).encode()


def http_request(body: bytes, path: str = PATH, method: str = "POST", extra: tuple[str, ...] = ()) -> bytes:
    lines = [
        f"{method} {path} HTTP/1.1",
        "Host: 127.0.0.1",
        "Authorization: Bearer unused",
        "Content-Type: application/json",
        f"Content-Length: {len(body)}",
        "Connection: close",
        *extra,
    ]
    return ("\r\n".join(lines) + "\r\n\r\n").encode() + body


def read_request(tls: ssl.SSLSocket) -> bytes:
    tls.settimeout(3)
    data = b""
    while b"\r\n\r\n" not in data:
        piece = tls.recv(4096)
        if not piece:
            break
        data += piece
    head, _, rest = data.partition(b"\r\n\r\n")
    lower = head.lower()
    marker = b"content-length:"
    if marker in lower:
        line = lower.split(marker, 1)[1].split(b"\r\n", 1)[0]
        need = int(line.strip())
        while len(rest) < need:
            piece = tls.recv(4096)
            if not piece:
                break
            rest += piece
        rest = rest[:need]
    return head + b"\r\n\r\n" + rest


def clean_close(tls: ssl.SSLSocket) -> None:
    try:
        tls.unwrap()
    except OSError:
        pass
    try:
        tls.close()
    except OSError:
        pass


def cut_tls(tls: ssl.SSLSocket) -> None:
    try:
        tls.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
    except OSError:
        pass
    try:
        os.close(tls.fileno())
    except OSError:
        pass
    try:
        tls.close()
    except OSError:
        pass


def chunk(data: bytes) -> bytes:
    return f"{len(data):x}\r\n".encode() + data + b"\r\n"


def decode_chunked(body: bytes) -> tuple[bytes, bool]:
    out = bytearray()
    index = 0
    while True:
        newline = body.find(b"\r\n", index)
        if newline < 0:
            return bytes(out), False
        try:
            size = int(body[index:newline], 16)
        except ValueError:
            return bytes(out), False
        index = newline + 2
        if size == 0:
            return bytes(out), True
        if index + size + 2 > len(body):
            return bytes(out), False
        out += body[index : index + size]
        index += size + 2


def split_http(raw: bytes) -> tuple[bytes, bytes]:
    head, _, body = raw.partition(b"\r\n\r\n")
    return head, body


def read_all(sock: socket.socket, limit: int = 8 * 1024 * 1024) -> bytes:
    data = b""
    while len(data) < limit:
        piece = sock.recv(8192)
        if not piece:
            break
        data += piece
    return data


def exchange_row(seq: int, outcome: str = "ok") -> dict:
    return {
        "kind": "exchange",
        "seq": seq,
        "start": "2026-11-19T00:00:00.000Z",
        "elapsed_ms": 1,
        "method": "POST",
        "path": PATH,
        "status": 200 if outcome == "ok" else 429,
        "outcome": outcome,
        "request_bytes": 1,
        "response_bytes": 1,
        "request_sha256": "ab" * 32,
        "response_sha256": "cd" * 32,
        "upstream_ip": "127.0.0.1",
        "cert_sha256": "ef" * 32,
    }


class DnsPatch:
    def __init__(self, stub_port: int) -> None:
        self.stub_port = stub_port
        self.calls: list[tuple[str, int]] = []

    def __enter__(self) -> "DnsPatch":
        self._create = socket.create_connection
        self._gai = socket.getaddrinfo

        def gai(host, port, *args, **kwargs):
            ipaddress.ip_address(str(host))
            return self._gai(host, port, *args, **kwargs)

        def create(address, *args, **kwargs):
            host, port = address
            if port != 443:
                return self._create(address, *args, **kwargs)
            ipaddress.ip_address(str(host))
            self.calls.append((str(host), port))
            return self._create(("127.0.0.1", self.stub_port), *args, **kwargs)

        socket.getaddrinfo = gai
        socket.create_connection = create
        return self

    def __exit__(self, *exc) -> None:
        socket.getaddrinfo = self._gai
        socket.create_connection = self._create


class TlsStub:
    def __init__(self, cert: Path, key: Path) -> None:
        self.ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        self.ctx.load_cert_chain(str(cert), str(key))
        self.sni: list[str | None] = []

        def sni_cb(sock, name, ctx):
            self.sni.append(name)
            return None

        self.ctx.sni_callback = sni_cb
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(64)
        self.port = self.sock.getsockname()[1]
        self.requests: list[bytes] = []
        self.accepts = 0
        self.lock = threading.Lock()
        self.stop_flag = threading.Event()
        self.handler = None
        self.thread = threading.Thread(target=self._loop, daemon=True)
        self.thread.start()

    def _loop(self) -> None:
        self.sock.settimeout(0.2)
        while not self.stop_flag.is_set():
            try:
                conn, _addr = self.sock.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            with self.lock:
                self.accepts += 1
            threading.Thread(target=self._wrap, args=(conn,), daemon=True).start()

    def _wrap(self, conn: socket.socket) -> None:
        try:
            conn.settimeout(3)
            tls = self.ctx.wrap_socket(conn, server_side=True)
        except ssl.SSLError:
            conn.close()
            return
        try:
            try:
                tls.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            except OSError:
                pass
            handler = self.handler
            if handler is not None:
                handler(tls)
        finally:
            try:
                tls.close()
            except OSError:
                pass

    def stop(self) -> None:
        self.stop_flag.set()
        try:
            self.sock.close()
        except OSError:
            pass


class Harness:
    def __init__(self, cert: str = "good", exchanges: list[dict] | None = None) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.log_path = self.root / "mission.log"
        cert_dir = CERTS["dir"]
        self.stub = TlsStub(cert_dir / f"{cert}.crt", cert_dir / f"{cert}.key")
        self.ctx = client_context(cert_dir / "ca.crt")
        self.settings = forwarder.Settings(
            listen_host="127.0.0.1",
            listen_port=0,
            path=PATH,
            model="grok-4.7",
            key=KEY,
            host_uid=0,
            upstream_ip="127.0.0.1",
            lookup_record=LOOKUP,
        )
        if exchanges is not None:
            header = forwarder._header_record(self.settings)
            blob = "".join(
                json.dumps(row, separators=(",", ":")) + "\n" for row in (header, *exchanges)
            )
            fd = os.open(self.log_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            os.write(fd, blob.encode())
            os.close(fd)
        self.patch = DnsPatch(self.stub.port)
        self.patch.__enter__()
        self.mission = forwarder.open_mission_log(str(self.log_path), self.settings)
        self.httpd = forwarder.build_server(self.settings, self.mission, self.ctx, allow_literal)
        self.thread = threading.Thread(
            target=self.httpd.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True
        )
        self.thread.start()
        self.port = self.httpd.server_address[1]
        self.clients: list[socket.socket] = []

    def stop(self) -> None:
        for sock in self.clients:
            try:
                sock.close()
            except OSError:
                pass
        try:
            self.httpd.close_active()
            self.httpd.shutdown()
            self.httpd.server_close()
        finally:
            self.mission.close()
            self.stub.stop()
            self.patch.__exit__(None, None, None)
            self.tmp.cleanup()

    def connect(self) -> socket.socket:
        sock = socket.create_connection(("127.0.0.1", self.port), timeout=5)
        sock.settimeout(5)
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.clients.append(sock)
        return sock


def log_rows(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text().splitlines() if line]


def run_cli(args: list[str], timeout: float = 5) -> tuple[int, str, str]:
    proc = subprocess.Popen(
        [sys.executable, str(HERE / "forwarder.py"), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        out, err = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        out, err = proc.communicate()
        raise AssertionError(f"forwarder still running\n{out}\n{err}")
    return proc.returncode, out, err


class TestChecks(unittest.TestCase):
    def test_require_global_ip_rejects_private_and_names(self) -> None:
        called = []
        real = socket.getaddrinfo

        def wrapped(host, *args, **kwargs):
            called.append(host)
            return real(host, *args, **kwargs)

        socket.getaddrinfo = wrapped
        try:
            self.assertEqual(forwarder.require_global_ip("8.8.8.8"), "8.8.8.8")
            for bad in ("127.0.0.1", "10.1.2.3", "192.168.0.1", "169.254.1.1", "api.x.ai", "localhost"):
                with self.assertRaises(forwarder.StartupError):
                    forwarder.require_global_ip(bad)
        finally:
            socket.getaddrinfo = real
        self.assertFalse(any(host == "api.x.ai" for host in called))

    def test_identity_injection(self) -> None:
        with self.assertRaises(forwarder.StartupError) as root:
            forwarder.assert_process_identity(host_uid=1, euid=0)
        self.assertIn("uid 0", str(root.exception))
        with self.assertRaises(forwarder.StartupError) as host:
            forwarder.assert_process_identity(host_uid=os.geteuid(), euid=os.geteuid())
        self.assertIn("host uid", str(host.exception))

    def test_key_file_mode_symlink_and_owner(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        try:
            good = tmp / "good"
            good.write_bytes(KEY + b"\n")
            os.chmod(good, 0o600)
            self.assertEqual(forwarder.read_key_file(str(good)), KEY)
            with self.assertRaises(forwarder.StartupError) as owner:
                forwarder.read_key_file(str(good), euid=os.geteuid() + 1)
            self.assertIn("owner", str(owner.exception))
            self.assertNotIn(KEY, str(owner.exception).encode())
            loose = tmp / "loose"
            loose.write_bytes(KEY)
            os.chmod(loose, 0o640)
            with self.assertRaises(forwarder.StartupError) as mode:
                forwarder.read_key_file(str(loose))
            self.assertIn("0600", str(mode.exception))
            link = tmp / "link"
            link.symlink_to(good)
            with self.assertRaises(forwarder.StartupError) as symlink:
                forwarder.read_key_file(str(link))
            self.assertIn("symlink", str(symlink.exception))
            with self.assertRaises(forwarder.StartupError):
                forwarder.read_key_file(str(tmp))
        finally:
            shutil.rmtree(tmp)

    def test_sudo_wrong_owner(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        key_path = tmp / "key"
        key_path.write_bytes(KEY)
        os.chmod(key_path, 0o600)
        script = (
            "import importlib.util, sys\n"
            "spec = importlib.util.spec_from_file_location('forwarder', sys.argv[1])\n"
            "mod = importlib.util.module_from_spec(spec)\n"
            "sys.modules['forwarder'] = mod\n"
            "spec.loader.exec_module(mod)\n"
            "try:\n"
            "    mod.read_key_file(sys.argv[2])\n"
            "except mod.StartupError as exc:\n"
            "    sys.stderr.write(str(exc))\n"
            "    raise SystemExit(1)\n"
        )
        try:
            proc = subprocess.run(
                ["sudo", "-n", sys.executable, "-c", script, str(HERE / "forwarder.py"), str(key_path)],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=15,
            )
            self.assertEqual(proc.returncode, 1, proc.stderr)
            self.assertIn(b"owner", proc.stderr)
            self.assertNotIn(KEY, proc.stderr)
            self.assertNotIn(KEY, proc.stdout)
        finally:
            shutil.rmtree(tmp)

    def test_path_required_and_model_default(self) -> None:
        base = [
            "--listen", "127.0.0.1:9",
            "--key-file", "k",
            "--host-uid", "1",
            "--upstream-ip", "8.8.8.8",
            "--lookup-record", "x",
            "--log", "l",
        ]
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                forwarder.parse_args(base)
            parsed = forwarder.parse_args(["--path", PATH, *base])
            self.assertEqual(parsed.model, "grok-4.7")
            self.assertEqual(parsed.path, PATH)
            with self.assertRaises(SystemExit):
                forwarder.parse_args(["--path", PATH, "--insecure", *base])
            with self.assertRaises(SystemExit):
                forwarder.parse_args(["--path", PATH, "--ca-file", "ca.pem", *base])

    def test_listen_and_source_contract(self) -> None:
        for value in ("localhost:9", "0.0.0.0:9", "::1:9", "127.0.0.2:9", "127.0.0.1"):
            with self.assertRaises(forwarder.StartupError):
                forwarder.parse_listen(value)
        self.assertEqual(forwarder.parse_listen("127.0.0.1:9"), ("127.0.0.1", 9))
        source = (HERE / "forwarder.py").read_text()
        self.assertIn("socket.create_connection((ip, 443)", source)
        self.assertIn('server_hostname="api.x.ai"', source)
        self.assertIn("is_global", source)
        self.assertNotIn("--insecure", source)
        self.assertNotIn("--ca-file", source)
        modules = set()
        for line in source.splitlines():
            if line.startswith("import "):
                modules.add(line.split()[1].split(".")[0].rstrip(","))
            elif line.startswith("from "):
                modules.add(line.split()[1].split(".")[0])
        for name in modules:
            self.assertIn(name, sys.stdlib_module_names, name)
        ctx = forwarder.production_context()
        self.assertEqual(ctx.verify_mode, ssl.CERT_REQUIRED)
        self.assertTrue(ctx.check_hostname)

    def test_cli_refuses_bad_listen_ip_and_key(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        key_path = tmp / "key"
        key_path.write_bytes(KEY)
        os.chmod(key_path, 0o600)
        common = [
            "--path", PATH,
            "--key-file", str(key_path),
            "--host-uid", "0",
            "--upstream-ip", "8.8.8.8",
            "--lookup-record", LOOKUP,
            "--log", str(tmp / "mission.log"),
        ]
        try:
            for listen in ("0.0.0.0:9", "localhost:9", "::1:9"):
                code, out, err = run_cli(["--listen", listen, *common])
                self.assertEqual(code, 1, err)
                self.assertIn("127.0.0.1", err)
                self.assertNotIn(KEY.decode(), out + err)
            code, out, err = run_cli([
                "--listen", "127.0.0.1:9",
                "--path", PATH,
                "--key-file", str(key_path),
                "--host-uid", "0",
                "--upstream-ip", "api.x.ai",
                "--lookup-record", LOOKUP,
                "--log", str(tmp / "mission.log"),
            ])
            self.assertEqual(code, 1, err)
            self.assertIn("IP address", err)
            self.assertNotIn(KEY.decode(), err)
            code, _out, err = run_cli([
                "--listen", "127.0.0.1:9",
                "--path", PATH,
                "--key-file", str(key_path),
                "--host-uid", "0",
                "--upstream-ip", "10.1.2.3",
                "--lookup-record", LOOKUP,
                "--log", str(tmp / "mission.log"),
            ])
            self.assertEqual(code, 1)
            self.assertIn("not a global", err)
            os.chmod(key_path, 0o640)
            code, _out, err = run_cli([
                "--listen", "127.0.0.1:9",
                "--path", PATH,
                "--key-file", str(key_path),
                "--host-uid", "0",
                "--upstream-ip", "8.8.8.8",
                "--lookup-record", LOOKUP,
                "--log", str(tmp / "mission.log"),
            ])
            self.assertEqual(code, 1)
            self.assertIn("0600", err)
            self.assertNotIn(KEY.decode(), err)
            os.chmod(key_path, 0o600)
            code, _out, err = run_cli([
                "--listen", "127.0.0.1:9",
                "--path", PATH,
                "--key-file", str(key_path),
                "--host-uid", str(os.geteuid()),
                "--upstream-ip", "8.8.8.8",
                "--lookup-record", LOOKUP,
                "--log", str(tmp / "mission.log"),
            ])
            self.assertEqual(code, 1)
            self.assertIn("host uid", err)
            self.assertNotIn(KEY.decode(), err)
        finally:
            shutil.rmtree(tmp)

    def test_log_malformed_unreadable_and_symlink(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        settings = forwarder.Settings(
            listen_host="127.0.0.1",
            listen_port=9,
            path=PATH,
            model="grok-4.7",
            key=KEY,
            host_uid=0,
            upstream_ip="8.8.8.8",
            lookup_record=LOOKUP,
        )
        try:
            bad = tmp / "bad"
            bad.write_text("{not-json\n")
            os.chmod(bad, 0o600)
            with self.assertRaises(forwarder.StartupError) as malformed:
                forwarder.open_mission_log(str(bad), settings)
            self.assertIn("malformed", str(malformed.exception))
            hidden = tmp / "hidden"
            hidden.write_bytes(b"")
            os.chmod(hidden, 0)
            try:
                with self.assertRaises(forwarder.StartupError) as unreadable:
                    forwarder.open_mission_log(str(hidden), settings)
                self.assertIn("unreadable", str(unreadable.exception))
            finally:
                os.chmod(hidden, 0o600)
            real = tmp / "real"
            real.write_bytes(b"")
            os.chmod(real, 0o600)
            link = tmp / "link"
            link.symlink_to(real)
            with self.assertRaises(forwarder.StartupError) as symlink:
                forwarder.open_mission_log(str(link), settings)
            self.assertIn("symlink", str(symlink.exception))
        finally:
            shutil.rmtree(tmp)


class TestProxy(unittest.TestCase):
    def setUp(self) -> None:
        self.h = Harness()

    def tearDown(self) -> None:
        self.h.stop()

    def test_strips_client_auth_and_records_exchange(self) -> None:
        body = chat_body()
        upstream_body = b'{"ok":true}'
        captured = []

        def handler(tls):
            captured.append(read_request(tls))
            raw = (
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: "
                + str(len(upstream_body)).encode()
                + b"\r\n\r\n"
                + upstream_body
            )
            tls.sendall(raw)
            clean_close(tls)

        self.h.stub.handler = handler
        extra = (
            "Cookie: session=client-cookie",
            "X-Api-Key: client-x-api-key",
            "Api-Key: client-api-key",
            "Proxy-Authorization: Basic client-proxy",
            "Upgrade: websocket",
        )
        sock = self.h.connect()
        sock.sendall(http_request(body, extra=extra))
        data = read_all(sock)
        sock.close()
        head, resp = split_http(data)
        self.assertIn(b"Content-Length: " + str(len(upstream_body)).encode(), head)
        self.assertNotIn(b"Transfer-Encoding:", head)
        self.assertIn(b"Connection: close", head)
        self.assertEqual(resp, upstream_body)
        raw = captured[0]
        self.assertIn(b"Host: api.x.ai\r\n", raw)
        self.assertIn(b"Authorization: Bearer " + KEY + b"\r\n", raw)
        self.assertNotIn(b"Bearer unused", raw)
        self.assertNotIn(b"127.0.0.1", raw)
        self.assertNotIn(b"client-cookie", raw)
        self.assertNotIn(b"client-x-api-key", raw)
        self.assertNotIn(b"client-api-key", raw)
        self.assertNotIn(b"client-proxy", raw)
        self.assertNotIn(b"websocket", raw)
        self.assertEqual(raw.lower().count(b"authorization:"), 1)
        self.assertIn(body, raw)
        self.assertEqual(self.h.patch.calls, [("127.0.0.1", 443)])
        self.assertIn("api.x.ai", self.h.stub.sni)
        rows = log_rows(self.h.log_path)
        self.assertEqual(rows[0]["label"], forwarder.LABEL)
        self.assertEqual(rows[0]["lookup_record"], LOOKUP)
        self.assertEqual(rows[0]["sni"], "api.x.ai")
        self.assertEqual(rows[0]["path"], PATH)
        self.assertEqual(rows[0]["model"], "grok-4.7")
        self.assertEqual(rows[0]["forwarder_sha256"], forwarder.forwarder_sha256())
        self.assertEqual(rows[0]["caps_requests"], 40)
        exchange = rows[1]
        self.assertEqual(exchange["outcome"], "ok")
        self.assertEqual(exchange["request_sha256"], hashlib.sha256(body).hexdigest())
        self.assertEqual(exchange["response_sha256"], hashlib.sha256(resp).hexdigest())
        self.assertEqual(exchange["request_bytes"], len(body))
        self.assertEqual(exchange["response_bytes"], len(resp))
        der = ssl.PEM_cert_to_DER_cert((CERTS["dir"] / "good.crt").read_text())
        self.assertEqual(exchange["cert_sha256"], hashlib.sha256(der).hexdigest())
        log_bytes = self.h.log_path.read_bytes()
        for secret in (KEY, b"Bearer unused", b"BODYMARKER-do-not-log", b"client-cookie"):
            self.assertNotIn(secret, log_bytes)
        self.assertEqual(stat.S_IMODE(self.h.log_path.stat().st_mode), 0o600)

    def test_half_close_still_returns_the_response(self) -> None:
        payload = b"data: after-half-close\n\n"
        framed = chunk(payload) + b"0\r\n\r\n"

        def handler(tls):
            read_request(tls)
            tls.sendall(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n"
                b"Transfer-Encoding: chunked\r\n\r\n" + framed
            )
            clean_close(tls)

        self.h.stub.handler = handler
        sock = self.h.connect()
        sock.sendall(http_request(chat_body()))
        sock.shutdown(socket.SHUT_WR)
        data = read_all(sock)
        _head, body = split_http(data)
        self.assertIn(payload, body)
        decoded, complete = decode_chunked(body)
        self.assertTrue(complete)
        self.assertEqual(decoded, payload)
        self.assertNotIn(b": ping", data)
        self.assertEqual(body, framed)

    def test_close_delimited_upstream_is_chunked(self) -> None:
        payload = b"hello-done"

        def handler(tls):
            read_request(tls)
            tls.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n" + payload)
            clean_close(tls)

        self.h.stub.handler = handler
        sock = self.h.connect()
        sock.sendall(http_request(chat_body()))
        data = read_all(sock)
        head, body = split_http(data)
        self.assertIn(b"Transfer-Encoding: chunked", head)
        self.assertNotIn(b"Content-Length:", head)
        self.assertIn(b"Connection: close", head)
        decoded, complete = decode_chunked(body)
        self.assertTrue(complete)
        self.assertEqual(decoded, payload)

    def test_upstream_cut_is_incomplete_chunked(self) -> None:
        payload = b"hello-partial"
        seen = threading.Event()

        def handler(tls):
            read_request(tls)
            tls.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n" + payload)
            if not seen.wait(3):
                raise TimeoutError("client never saw the partial body")
            cut_tls(tls)

        self.h.stub.handler = handler
        sock = self.h.connect()
        sock.sendall(http_request(chat_body()))
        data = b""
        while payload not in data:
            piece = sock.recv(4096)
            if not piece:
                self.fail(f"eof before payload: {data!r}")
            data += piece
        seen.set()
        sock.settimeout(3)
        data += read_all(sock)
        head, body = split_http(data)
        self.assertIn(b"Transfer-Encoding: chunked", head)
        self.assertIn(payload, body)
        _decoded, complete = decode_chunked(body)
        self.assertFalse(complete)
        self.assertNotIn(b"\r\n0\r\n", body)
        rows = log_rows(self.h.log_path)
        self.assertEqual(rows[-1]["outcome"], "upstream_error")

    def test_local_refusals_do_not_reach_upstream(self) -> None:
        cases = [
            (b"GET " + PATH.encode() + b" HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\n\r\n{}", b"405"),
            (http_request(chat_body(), path="/v1/other"), b"404"),
            (http_request(chat_body(), path="//v1/chat/completions"), b"404"),
            (http_request(chat_body(), path=PATH + "?x=1"), b"404"),
            (b"POST " + PATH.encode() + b" HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n", b"411"),
            (
                b"POST " + PATH.encode() + b" HTTP/1.1\r\nHost: 127.0.0.1\r\nTransfer-Encoding: chunked\r\n\r\n",
                b"411",
            ),
            (http_request(b"{"), b"400"),
            (http_request(b'{"model":"other","stream":true}'), b"400"),
        ]
        for raw, status in cases:
            sock = self.h.connect()
            sock.settimeout(2)
            sock.sendall(raw)
            try:
                data = read_all(sock)
            except socket.timeout:
                self.fail(f"no response for {status}")
            self.assertIn(b"HTTP/1.1 " + status, data)
            self.assertIn(b"Connection: close", data)
        huge = (
            b"POST " + PATH.encode() + b" HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: "
            + str(forwarder.MAX_REQUEST_BYTES + 1).encode()
            + b"\r\nConnection: close\r\n\r\n"
        )
        sock = self.h.connect()
        sock.settimeout(2)
        sock.sendall(huge)
        try:
            data = read_all(sock)
        except socket.timeout:
            self.fail("413 read the body")
        self.assertIn(b"HTTP/1.1 413", data)
        self.assertEqual(self.h.stub.accepts, 0)
        outcomes = [row["outcome"] for row in log_rows(self.h.log_path) if row["kind"] == "exchange"]
        self.assertNotIn("ok", outcomes)

    def test_redirect_is_502_and_not_followed(self) -> None:
        def handler(tls):
            read_request(tls)
            tls.sendall(
                b"HTTP/1.1 302 Found\r\nLocation: https://evil.example/steal\r\nContent-Length: 0\r\n\r\n"
            )
            clean_close(tls)

        self.h.stub.handler = handler
        sock = self.h.connect()
        sock.sendall(http_request(chat_body()))
        data = read_all(sock)
        self.assertIn(b"HTTP/1.1 502", data)
        self.assertNotIn(b"evil.example", data)
        self.assertEqual(len(self.h.patch.calls), 1)
        self.assertNotIn(b"evil.example", self.h.log_path.read_bytes())
        self.assertEqual(log_rows(self.h.log_path)[-1]["outcome"], "redirect_refused")

    def test_content_length_over_cap_is_502(self) -> None:
        marker = b"OVERSIZE-MARKER"

        def handler(tls):
            read_request(tls)
            tls.sendall(
                b"HTTP/1.1 200 OK\r\nContent-Length: "
                + str(forwarder.MAX_RESPONSE_BYTES + 1).encode()
                + b"\r\n\r\n"
                + marker
            )
            clean_close(tls)

        self.h.stub.handler = handler
        sock = self.h.connect()
        sock.sendall(http_request(chat_body()))
        data = read_all(sock)
        self.assertIn(b"HTTP/1.1 502", data)
        self.assertNotIn(marker, data)
        self.assertEqual(log_rows(self.h.log_path)[-1]["outcome"], "response_cap")

    def test_streamed_over_cap_has_no_terminator(self) -> None:
        marker = b"OVERFLOWMARKER"
        payload = b"A" * forwarder.MAX_RESPONSE_BYTES + marker

        def handler(tls):
            read_request(tls)
            tls.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\r\n" + payload)
            clean_close(tls)

        self.h.stub.handler = handler
        sock = self.h.connect()
        sock.settimeout(10)
        sock.sendall(http_request(chat_body()))
        data = read_all(sock)
        _head, body = split_http(data)
        self.assertNotIn(marker, data)
        _decoded, complete = decode_chunked(body)
        self.assertFalse(complete)
        self.assertEqual(log_rows(self.h.log_path)[-1]["outcome"], "response_cap")

    def test_chunks_arrive_incrementally(self) -> None:
        first = b"data: one\n\n" + (b"A" * 2048)
        second = b"data: two\n\n"
        state = {"second": False}

        def handler(tls):
            read_request(tls)
            tls.sendall(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"
                + chunk(first)
            )
            time.sleep(1.0)
            state["second"] = True
            tls.sendall(chunk(second) + b"0\r\n\r\n")
            clean_close(tls)

        self.h.stub.handler = handler
        sock = self.h.connect()
        sock.settimeout(5)
        sock.sendall(http_request(chat_body()))
        data = b""
        while first not in data:
            piece = sock.recv(8192)
            if not piece:
                self.fail(f"eof before first chunk: {data[:80]!r}")
            data += piece
        seen = time.monotonic()
        self.assertFalse(state["second"])
        while second not in data:
            data += sock.recv(8192)
        self.assertGreaterEqual(time.monotonic() - seen, 0.6)
        self.assertNotIn(b": ping", data)
        self.assertNotIn(b": heartbeat", data)

    def test_one_request_per_connection(self) -> None:
        def handler(tls):
            read_request(tls)
            tls.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            clean_close(tls)

        self.h.stub.handler = handler
        body = chat_body()
        first = http_request(body)
        second = http_request(body)
        sock = self.h.connect()
        sock.sendall(first + second)
        data = read_all(sock)
        self.assertEqual(data.count(b"HTTP/1.1 "), 1)
        self.assertIn(b"Connection: close", data)
        self.assertEqual(self.h.stub.accepts, 1)

    def test_wrong_hostname_does_not_send_the_key(self) -> None:
        self.h.stop()
        self.h = Harness(cert="bad")
        seen = []

        def handler(tls):
            tls.settimeout(1)
            try:
                seen.append(read_request(tls))
            except (socket.timeout, ssl.SSLError, OSError):
                seen.append(b"")

        self.h.stub.handler = handler
        sock = self.h.connect()
        sock.sendall(http_request(chat_body()))
        data = read_all(sock)
        self.assertIn(b"HTTP/1.1 502", data)
        blob = b"".join(seen)
        self.assertNotIn(KEY, blob)
        self.assertNotIn(b"Bearer unused", blob)
        self.assertNotIn(KEY, self.h.log_path.read_bytes())
        self.assertEqual(log_rows(self.h.log_path)[-1]["outcome"], "upstream_error")
        self.assertIsNone(log_rows(self.h.log_path)[-1]["cert_sha256"])

    def test_restart_at_40_is_429_without_upstream(self) -> None:
        self.h.stop()
        rows = [exchange_row(i) for i in range(1, 41)]
        self.h = Harness(exchanges=rows)
        sock = self.h.connect()
        sock.sendall(http_request(chat_body()))
        data = read_all(sock)
        self.assertIn(b"HTTP/1.1 429", data)
        self.assertEqual(self.h.stub.accepts, 0)
        self.assertEqual(log_rows(self.h.log_path)[-1]["outcome"], "429")
        self.assertEqual(log_rows(self.h.log_path)[-1]["seq"], 41)

    def test_refusal_does_not_consume_a_logged_slot(self) -> None:
        self.h.stop()
        self.h = Harness(exchanges=[exchange_row(i) for i in range(1, 40)])
        sock = self.h.connect()
        sock.sendall(http_request(b'{"model":"nope"}'))
        data = read_all(sock)
        self.assertIn(b"HTTP/1.1 400", data)
        self.assertEqual(self.h.stub.accepts, 0)

        def handler(tls):
            read_request(tls)
            tls.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            clean_close(tls)

        self.h.stub.handler = handler
        sock = self.h.connect()
        sock.sendall(http_request(chat_body()))
        data = read_all(sock)
        self.assertIn(b"HTTP/1.1 200", data)
        self.assertEqual(self.h.stub.accepts, 1)

    def test_forty_then_forty_first_is_429(self) -> None:
        def handler(tls):
            raw = read_request(tls)
            with self.h.stub.lock:
                self.h.stub.requests.append(raw)
            tls.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            clean_close(tls)

        self.h.stub.handler = handler
        body = chat_body()
        for _ in range(40):
            sock = self.h.connect()
            sock.sendall(http_request(body))
            data = read_all(sock)
            self.assertIn(b"HTTP/1.1 200", data)
            sock.close()
        self.assertEqual(self.h.stub.accepts, 40)
        sock = self.h.connect()
        sock.sendall(http_request(body))
        data = read_all(sock)
        self.assertIn(b"HTTP/1.1 429", data)
        self.assertEqual(self.h.stub.accepts, 40)
        for raw in self.h.stub.requests:
            self.assertNotIn(b"Bearer unused", raw)
            self.assertIn(KEY, raw)


class TestSubprocess(unittest.TestCase):
    def test_streams_and_log_do_not_contain_the_key(self) -> None:
        root = Path(tempfile.mkdtemp(prefix="fwd-child-"))
        key_path = root / "key"
        key_path.write_bytes(KEY)
        os.chmod(key_path, 0o600)
        env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": os.environ.get("HOME", ""),
            "LANG": os.environ.get("LANG", "C.UTF-8"),
            "FORWARDER_TEST_CHILD": "proxy",
            "PYTHONDONTWRITEBYTECODE": "1",
        }
        try:
            proc = subprocess.run(
                [sys.executable, str(HERE / "test_forwarder.py"), str(root), str(CERTS["dir"])],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=env,
                timeout=20,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr.decode(errors="replace"))
            for blob in (proc.stdout, proc.stderr, (root / "mission.log").read_bytes()):
                self.assertNotIn(KEY, blob)
                self.assertNotIn(b"Bearer unused", blob)
                self.assertNotIn(b"BODYMARKER-do-not-log", blob)
            capture = (root / "capture").read_bytes()
            self.assertIn(KEY, capture)
            self.assertNotIn(b"Bearer unused", capture)
            self.assertIn(b"Host: api.x.ai", capture)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    def _hold(self, sig: int) -> tuple[subprocess.Popen, socket.socket]:
        root = Path(tempfile.mkdtemp(prefix="fwd-hold-"))
        key_path = root / "key"
        key_path.write_bytes(KEY)
        os.chmod(key_path, 0o600)
        env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": os.environ.get("HOME", ""),
            "LANG": os.environ.get("LANG", "C.UTF-8"),
            "FORWARDER_TEST_CHILD": "hold",
            "PYTHONDONTWRITEBYTECODE": "1",
        }
        proc = subprocess.Popen(
            [sys.executable, str(HERE / "test_forwarder.py"), str(root), str(CERTS["dir"])],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )
        assert proc.stdout is not None
        line = proc.stdout.readline().decode()
        self.assertTrue(line.startswith("PORT "), line)
        port = int(line.split()[1])
        sock = socket.create_connection(("127.0.0.1", port), timeout=5)
        sock.settimeout(5)
        sock.sendall(http_request(chat_body()))
        data = b""
        while b"partial-chunk" not in data:
            piece = sock.recv(4096)
            if not piece:
                self.fail("eof before partial chunk")
            data += piece
        os.kill(proc.pid, sig)
        return proc, sock

    def test_kill_mid_stream_eof_is_prompt(self) -> None:
        proc, sock = self._hold(signal_kill())
        try:
            sock.settimeout(2)
            started = time.monotonic()
            try:
                while sock.recv(4096):
                    pass
            except OSError:
                pass
            self.assertLess(time.monotonic() - started, 2)
        finally:
            sock.close()
            proc.wait(timeout=3)
            if proc.stdout is not None:
                proc.stdout.close()
            if proc.stderr is not None:
                proc.stderr.close()

    def test_sigterm_shuts_down(self) -> None:
        proc, sock = self._hold(signal_term())
        try:
            sock.settimeout(2)
            started = time.monotonic()
            try:
                while sock.recv(4096):
                    pass
            except OSError:
                pass
            self.assertLess(time.monotonic() - started, 2)
            code = proc.wait(timeout=3)
            self.assertEqual(code, 0)
        finally:
            sock.close()
            if proc.stdout is not None:
                proc.stdout.close()
            if proc.stderr is not None:
                proc.stderr.close()


def signal_kill() -> int:
    import signal

    return signal.SIGKILL


def signal_term() -> int:
    import signal

    return signal.SIGTERM


def _child_boot(root: Path, cert_dir: Path, handler):
    stub = TlsStub(cert_dir / "good.crt", cert_dir / "good.key")
    stub.handler = handler
    ctx = client_context(cert_dir / "ca.crt")
    settings = forwarder.Settings(
        listen_host="127.0.0.1",
        listen_port=0,
        path=PATH,
        model="grok-4.7",
        key=KEY,
        host_uid=0,
        upstream_ip="127.0.0.1",
        lookup_record=LOOKUP,
    )
    patch = DnsPatch(stub.port)
    patch.__enter__()
    mission = forwarder.open_mission_log(str(root / "mission.log"), settings)
    httpd = forwarder.build_server(settings, mission, ctx, allow_literal)
    return stub, patch, mission, httpd


def child_proxy() -> None:
    root = Path(sys.argv[1])
    cert_dir = Path(sys.argv[2])
    key = (root / "key").read_bytes()
    proc_blob = Path("/proc/self/cmdline").read_bytes() + Path("/proc/self/environ").read_bytes()
    if key in proc_blob:
        raise SystemExit(3)
    captured: list[bytes] = []

    def handler(tls):
        captured.append(read_request(tls))
        tls.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
        clean_close(tls)

    stub, _patch, _mission, httpd = _child_boot(root, cert_dir, handler)
    thread = threading.Thread(target=httpd.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
    thread.start()
    port = httpd.server_address[1]
    sock = socket.create_connection(("127.0.0.1", port), timeout=5)
    sock.sendall(http_request(chat_body()))
    read_all(sock)
    sock.close()
    (root / "capture").write_bytes(b"".join(captured))
    httpd.close_active()
    httpd.shutdown()
    httpd.server_close()
    stub.stop()
    print("DONE", flush=True)


def child_hold() -> None:
    root = Path(sys.argv[1])
    cert_dir = Path(sys.argv[2])

    def handler(tls):
        read_request(tls)
        tls.sendall(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n"
            b"Transfer-Encoding: chunked\r\n\r\n" + chunk(b"partial-chunk")
        )
        time.sleep(60)

    _stub, _patch, _mission, httpd = _child_boot(root, cert_dir, handler)
    forwarder.install_shutdown_signals(httpd)
    print(f"PORT {httpd.server_address[1]}", flush=True)
    httpd.serve_forever(poll_interval=0.1)


if __name__ == "__main__":
    kind = os.environ.get("FORWARDER_TEST_CHILD")
    if kind == "proxy":
        child_proxy()
    elif kind == "hold":
        child_hold()
    else:
        unittest.main(verbosity=2)
