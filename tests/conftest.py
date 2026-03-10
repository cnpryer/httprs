"""Test server fixtures for httprs integration tests."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import pathlib
import shutil
import ssl
import subprocess
import threading
import time
import typing

import pytest
from uvicorn.config import Config
from uvicorn.server import Server

Message = typing.Dict[str, typing.Any]
Receive = typing.Callable[[], typing.Awaitable[Message]]
Send = typing.Callable[
    [typing.Dict[str, typing.Any]], typing.Coroutine[None, None, None]
]
Scope = typing.Dict[str, typing.Any]


async def app(scope: Scope, receive: Receive, send: Send) -> None:
    assert scope["type"] == "http"
    path = scope["path"]
    if path.startswith("/slow_response"):
        await slow_response(scope, receive, send)
    elif path.startswith("/status/"):
        await status_code_handler(scope, receive, send)
    elif path.startswith("/echo_body"):
        await echo_body(scope, receive, send)
    elif path.startswith("/echo_binary"):
        await echo_binary(scope, receive, send)
    elif path.startswith("/echo_headers"):
        await echo_headers(scope, receive, send)
    elif path.startswith("/redirect_301"):
        await redirect_301(scope, receive, send)
    elif path.startswith("/redirect_to_private"):
        await redirect_to_private(scope, receive, send)
    elif path.startswith("/redirect_to_ipv6_private"):
        await redirect_to_ipv6_private(scope, receive, send)
    elif path.startswith("/redirect_to_unspecified"):
        await redirect_to_unspecified(scope, receive, send)
    elif path.startswith("/redirect_to_ipv4_mapped_loopback"):
        await redirect_to_ipv4_mapped_loopback(scope, receive, send)
    elif path.startswith("/redirect_to_ipv4_mapped_private"):
        await redirect_to_ipv4_mapped_private(scope, receive, send)
    elif path.startswith("/redirect_to_ipv6_unspecified"):
        await redirect_to_ipv6_unspecified(scope, receive, send)
    elif path.startswith("/json"):
        await hello_world_json(scope, receive, send)
    elif path.startswith("/digest_auth"):
        await digest_auth(scope, receive, send)
    else:
        await hello_world(scope, receive, send)


async def hello_world(scope: Scope, receive: Receive, send: Send) -> None:
    await send({
        "type": "http.response.start",
        "status": 200,
        "headers": [[b"content-type", b"text/plain; charset=utf-8"]],
    })
    await send({"type": "http.response.body", "body": b"Hello, world!"})


async def hello_world_json(scope: Scope, receive: Receive, send: Send) -> None:
    await send({
        "type": "http.response.start",
        "status": 200,
        "headers": [[b"content-type", b"application/json"]],
    })
    await send({"type": "http.response.body", "body": b'{"hello": "world"}'})


async def slow_response(scope: Scope, receive: Receive, send: Send) -> None:
    await send({
        "type": "http.response.start",
        "status": 200,
        "headers": [[b"content-type", b"text/plain"]],
    })
    await asyncio.sleep(3.0)  # Long enough to trigger read timeout
    await send({"type": "http.response.body", "body": b"Slow response"})


async def status_code_handler(scope: Scope, receive: Receive, send: Send) -> None:
    code = int(scope["path"].split("/")[-1])
    await send({
        "type": "http.response.start",
        "status": code,
        "headers": [[b"content-type", b"text/plain"]],
    })
    await send({"type": "http.response.body", "body": b"Hello, world!"})


async def echo_body(scope: Scope, receive: Receive, send: Send) -> None:
    body = b""
    more_body = True
    while more_body:
        message = await receive()
        body += message.get("body", b"")
        more_body = message.get("more_body", False)
    await send({
        "type": "http.response.start",
        "status": 200,
        "headers": [[b"content-type", b"text/plain"]],
    })
    await send({"type": "http.response.body", "body": body})


async def echo_binary(scope: Scope, receive: Receive, send: Send) -> None:
    body = b""
    more_body = True
    while more_body:
        message = await receive()
        body += message.get("body", b"")
        more_body = message.get("more_body", False)
    await send({
        "type": "http.response.start",
        "status": 200,
        "headers": [[b"content-type", b"application/octet-stream"]],
    })
    await send({"type": "http.response.body", "body": body})


async def echo_headers(scope: Scope, receive: Receive, send: Send) -> None:
    body = {name.decode(): value.decode() for name, value in scope.get("headers", [])}
    await send({
        "type": "http.response.start",
        "status": 200,
        "headers": [[b"content-type", b"application/json"]],
    })
    await send({"type": "http.response.body", "body": json.dumps(body).encode()})


async def redirect_301(scope: Scope, receive: Receive, send: Send) -> None:
    await send({
        "type": "http.response.start",
        "status": 301,
        "headers": [[b"location", b"/"]],
    })
    await send({"type": "http.response.body"})


async def redirect_to_private(scope: Scope, receive: Receive, send: Send) -> None:
    """Redirect to a private IP to test SSRF protection."""
    await send({
        "type": "http.response.start",
        "status": 301,
        "headers": [[b"location", b"http://10.0.0.1/secret"]],
    })
    await send({"type": "http.response.body"})


async def redirect_to_ipv6_private(scope: Scope, receive: Receive, send: Send) -> None:
    """Redirect to an IPv6 link-local address to test SSRF protection."""
    await send({
        "type": "http.response.start",
        "status": 301,
        "headers": [[b"location", b"http://[fe80::1]/secret"]],
    })
    await send({"type": "http.response.body"})


async def redirect_to_unspecified(scope: Scope, receive: Receive, send: Send) -> None:
    """Redirect to 0.0.0.0 to test SSRF protection bypass via unspecified address."""
    await send({
        "type": "http.response.start",
        "status": 301,
        "headers": [[b"location", b"http://0.0.0.0/secret"]],
    })
    await send({"type": "http.response.body"})


async def redirect_to_ipv4_mapped_loopback(
    scope: Scope, receive: Receive, send: Send
) -> None:
    """Redirect to ::ffff:127.0.0.1 to test IPv4-mapped IPv6 SSRF protection."""
    await send({
        "type": "http.response.start",
        "status": 301,
        "headers": [[b"location", b"http://[::ffff:127.0.0.1]/secret"]],
    })
    await send({"type": "http.response.body"})


async def redirect_to_ipv4_mapped_private(
    scope: Scope, receive: Receive, send: Send
) -> None:
    """Redirect to ::ffff:10.0.0.1 to test IPv4-mapped IPv6 SSRF protection."""
    await send({
        "type": "http.response.start",
        "status": 301,
        "headers": [[b"location", b"http://[::ffff:10.0.0.1]/secret"]],
    })
    await send({"type": "http.response.body"})


async def redirect_to_ipv6_unspecified(
    scope: Scope, receive: Receive, send: Send
) -> None:
    """Redirect to :: (IPv6 unspecified) to test SSRF protection."""
    await send({
        "type": "http.response.start",
        "status": 301,
        "headers": [[b"location", b"http://[::]/secret"]],
    })
    await send({"type": "http.response.body"})


async def digest_auth(scope: Scope, receive: Receive, send: Send) -> None:
    """Return 401 with a Digest challenge for testing DigestAuth."""
    # Check if Authorization header is present
    headers_dict = {k.lower(): v for k, v in scope.get("headers", [])}
    auth_header = headers_dict.get(b"authorization", b"")
    if auth_header.startswith(b"Digest "):
        # Accept any Digest header as valid for testing purposes
        await send({
            "type": "http.response.start",
            "status": 200,
            "headers": [[b"content-type", b"text/plain"]],
        })
        await send({"type": "http.response.body", "body": b"Authenticated!"})
    else:
        # Return 401 with Digest challenge
        challenge = (
            b'Digest realm="test@example.com", '
            b'qop="auth", '
            b'nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", '
            b'opaque="5ccc069c403ebaf9f0171e9517f40e41"'
        )
        await send({
            "type": "http.response.start",
            "status": 401,
            "headers": [
                [b"content-type", b"text/plain"],
                [b"www-authenticate", challenge],
            ],
        })
        await send({"type": "http.response.body", "body": b"Unauthorized"})


class TestServer(Server):
    """Uvicorn server that runs in a thread for testing."""

    def __init__(self, config: Config) -> None:
        super().__init__(config)
        self._port: int = 0

    @property
    def url(self) -> str:
        host = self.config.host
        return f"http://{host}:{self._port}"

    def install_signal_handlers(self) -> None:
        pass  # Disable signal handling in threads

    async def serve(self, sockets=None):
        self.restart_requested = asyncio.Event()
        loop = asyncio.get_event_loop()
        tasks = {
            loop.create_task(super().serve(sockets=sockets)),
        }
        await asyncio.wait(tasks)


def serve_in_thread(server: TestServer) -> typing.Iterator[TestServer]:
    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()
    try:
        while not server.started:
            time.sleep(1e-3)
        # Discover the actual port
        for s in server.servers:
            for sock in s.sockets:
                server._port = sock.getsockname()[1]
                break
            break
        yield server
    finally:
        server.should_exit = True
        thread.join(timeout=5.0)


@pytest.fixture(scope="session")
def server() -> typing.Iterator[TestServer]:
    config = Config(app=app, host="127.0.0.1", port=0, lifespan="off", loop="asyncio")
    test_server = TestServer(config=config)
    yield from serve_in_thread(test_server)


def _run_openssl(args: list[str], cwd: pathlib.Path) -> None:
    try:
        subprocess.run(
            ["openssl", *args],
            cwd=cwd,
            check=True,
            capture_output=True,
            text=True,
        )
    except FileNotFoundError:
        pytest.skip("openssl is required for mTLS integration tests")
    except subprocess.CalledProcessError as exc:
        raise RuntimeError(
            "openssl command failed:\n"
            f"  openssl {' '.join(args)}\n"
            f"  stdout: {exc.stdout.strip()}\n"
            f"  stderr: {exc.stderr.strip()}"
        ) from exc


def _write_x509_extensions(path: pathlib.Path) -> pathlib.Path:
    extfile = path / "x509_extensions.cnf"
    extfile.write_text(
        "\n".join([
            "[v3_server]",
            "basicConstraints = critical,CA:FALSE",
            "keyUsage = critical,digitalSignature,keyEncipherment",
            "extendedKeyUsage = serverAuth",
            "subjectAltName = IP:127.0.0.1,DNS:localhost",
            "subjectKeyIdentifier = hash",
            "authorityKeyIdentifier = keyid,issuer",
            "",
            "[v3_client]",
            "basicConstraints = critical,CA:FALSE",
            "keyUsage = critical,digitalSignature,keyEncipherment",
            "extendedKeyUsage = clientAuth",
            "subjectKeyIdentifier = hash",
            "authorityKeyIdentifier = keyid,issuer",
            "",
        ]),
        encoding="utf-8",
    )
    return extfile


@dataclass
class MtlsAssets:
    ca_cert: pathlib.Path
    server_cert: pathlib.Path
    server_key: pathlib.Path
    client_cert: pathlib.Path
    client_key: pathlib.Path
    client_pem: pathlib.Path
    bad_client_cert: pathlib.Path
    bad_client_key: pathlib.Path
    bad_client_pem: pathlib.Path


def _generate_mtls_assets(base: pathlib.Path) -> MtlsAssets:
    extfile = _write_x509_extensions(base)
    ca_key = base / "ca.key"
    ca_cert = base / "ca.crt"
    _run_openssl(
        [
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-keyout",
            str(ca_key),
            "-out",
            str(ca_cert),
            "-days",
            "1",
            "-nodes",
            "-subj",
            "/CN=httprs-test-ca",
        ],
        base,
    )

    server_key = base / "server.key"
    server_csr = base / "server.csr"
    server_cert = base / "server.crt"
    _run_openssl(
        [
            "req",
            "-newkey",
            "rsa:2048",
            "-keyout",
            str(server_key),
            "-out",
            str(server_csr),
            "-nodes",
            "-subj",
            "/CN=localhost",
        ],
        base,
    )
    _run_openssl(
        [
            "x509",
            "-req",
            "-in",
            str(server_csr),
            "-CA",
            str(ca_cert),
            "-CAkey",
            str(ca_key),
            "-CAcreateserial",
            "-out",
            str(server_cert),
            "-days",
            "1",
            "-sha256",
            "-set_serial",
            "1001",
            "-extfile",
            str(extfile),
            "-extensions",
            "v3_server",
        ],
        base,
    )

    client_key = base / "client.key"
    client_csr = base / "client.csr"
    client_cert = base / "client.crt"
    _run_openssl(
        [
            "req",
            "-newkey",
            "rsa:2048",
            "-keyout",
            str(client_key),
            "-out",
            str(client_csr),
            "-nodes",
            "-subj",
            "/CN=httprs-client",
        ],
        base,
    )
    _run_openssl(
        [
            "x509",
            "-req",
            "-in",
            str(client_csr),
            "-CA",
            str(ca_cert),
            "-CAkey",
            str(ca_key),
            "-CAcreateserial",
            "-out",
            str(client_cert),
            "-days",
            "1",
            "-sha256",
            "-set_serial",
            "1002",
            "-extfile",
            str(extfile),
            "-extensions",
            "v3_client",
        ],
        base,
    )
    client_pem = base / "client.pem"
    client_pem.write_bytes(client_cert.read_bytes() + b"\n" + client_key.read_bytes())

    bad_ca_key = base / "bad_ca.key"
    bad_ca_cert = base / "bad_ca.crt"
    _run_openssl(
        [
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-keyout",
            str(bad_ca_key),
            "-out",
            str(bad_ca_cert),
            "-days",
            "1",
            "-nodes",
            "-subj",
            "/CN=httprs-bad-ca",
        ],
        base,
    )
    bad_client_key = base / "bad_client.key"
    bad_client_csr = base / "bad_client.csr"
    bad_client_cert = base / "bad_client.crt"
    _run_openssl(
        [
            "req",
            "-newkey",
            "rsa:2048",
            "-keyout",
            str(bad_client_key),
            "-out",
            str(bad_client_csr),
            "-nodes",
            "-subj",
            "/CN=httprs-bad-client",
        ],
        base,
    )
    _run_openssl(
        [
            "x509",
            "-req",
            "-in",
            str(bad_client_csr),
            "-CA",
            str(bad_ca_cert),
            "-CAkey",
            str(bad_ca_key),
            "-CAcreateserial",
            "-out",
            str(bad_client_cert),
            "-days",
            "1",
            "-sha256",
            "-set_serial",
            "2001",
            "-extfile",
            str(extfile),
            "-extensions",
            "v3_client",
        ],
        base,
    )
    bad_client_pem = base / "bad_client.pem"
    bad_client_pem.write_bytes(
        bad_client_cert.read_bytes() + b"\n" + bad_client_key.read_bytes()
    )

    return MtlsAssets(
        ca_cert=ca_cert,
        server_cert=server_cert,
        server_key=server_key,
        client_cert=client_cert,
        client_key=client_key,
        client_pem=client_pem,
        bad_client_cert=bad_client_cert,
        bad_client_key=bad_client_key,
        bad_client_pem=bad_client_pem,
    )


@dataclass
class MtlsServer:
    url: str
    client_cert: pathlib.Path
    client_key: pathlib.Path
    client_pem: pathlib.Path
    bad_client_cert: pathlib.Path
    bad_client_key: pathlib.Path
    bad_client_pem: pathlib.Path


@pytest.fixture(scope="session")
def mtls_server(
    tmp_path_factory: pytest.TempPathFactory,
) -> typing.Iterator[MtlsServer]:
    if shutil.which("openssl") is None:
        pytest.skip("openssl is required for mTLS integration tests")
    assets = _generate_mtls_assets(tmp_path_factory.mktemp("mtls"))

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path.startswith("/redirect_301"):
                self.send_response(301)
                self.send_header("location", "/")
                self.send_header("content-length", "0")
                self.end_headers()
                return
            body = b"mtls-ok"
            self.send_response(200)
            self.send_header("content-type", "text/plain")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_args):
            pass

    httpd = HTTPServer(("127.0.0.1", 0), Handler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(str(assets.server_cert), str(assets.server_key))
    context.verify_mode = ssl.CERT_REQUIRED
    context.load_verify_locations(cafile=str(assets.ca_cert))
    httpd.socket = context.wrap_socket(httpd.socket, server_side=True)
    port = httpd.server_address[1]
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    try:
        yield MtlsServer(
            url=f"https://127.0.0.1:{port}",
            client_cert=assets.client_cert,
            client_key=assets.client_key,
            client_pem=assets.client_pem,
            bad_client_cert=assets.bad_client_cert,
            bad_client_key=assets.bad_client_key,
            bad_client_pem=assets.bad_client_pem,
        )
    finally:
        httpd.shutdown()
        thread.join(timeout=5.0)
