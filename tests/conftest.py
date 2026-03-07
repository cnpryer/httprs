"""Test server fixtures for httprs integration tests."""

from __future__ import annotations

import asyncio
import json
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
