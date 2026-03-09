"""Integration tests for the asynchronous AsyncClient."""

from __future__ import annotations

from datetime import timedelta

import pytest
import httprs

# pyo3_async_runtimes only supports the asyncio event loop, not trio
pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


@pytest.mark.anyio
async def test_get(server):
    async with httprs.AsyncClient() as client:
        response = await client.get(server.url)
    assert response.status_code == 200
    assert response.text == "Hello, world!"
    assert response.http_version == "HTTP/1.1"
    assert response.headers
    assert repr(response) == "<Response [200 OK]>"
    assert response.elapsed >= timedelta(0)


@pytest.mark.anyio
async def test_post(server):
    async with httprs.AsyncClient() as client:
        response = await client.post(
            server.url + "/echo_body", content=b"Hello, world!"
        )
    assert response.status_code == 200
    assert response.content == b"Hello, world!"


@pytest.mark.anyio
async def test_post_json(server):
    async with httprs.AsyncClient() as client:
        response = await client.post(server.url, json={"text": "Hello, world!"})
    assert response.status_code == 200


@pytest.mark.anyio
async def test_put(server):
    async with httprs.AsyncClient() as client:
        response = await client.put(server.url + "/echo_body", content=b"data")
    assert response.status_code == 200


@pytest.mark.anyio
async def test_patch(server):
    async with httprs.AsyncClient() as client:
        response = await client.patch(server.url + "/echo_body", content=b"patch data")
    assert response.status_code == 200


@pytest.mark.anyio
async def test_delete(server):
    async with httprs.AsyncClient() as client:
        response = await client.delete(server.url)
    assert response.status_code == 200


@pytest.mark.anyio
async def test_head(server):
    async with httprs.AsyncClient() as client:
        response = await client.head(server.url)
    assert response.status_code == 200


@pytest.mark.anyio
async def test_options(server):
    async with httprs.AsyncClient() as client:
        response = await client.options(server.url)
    assert response.status_code == 200


@pytest.mark.anyio
async def test_json_response(server):
    async with httprs.AsyncClient() as client:
        response = await client.get(server.url + "/json")
    assert response.status_code == 200
    data = response.json()
    assert data == {"hello": "world"}


@pytest.mark.anyio
async def test_raise_for_status(server):
    async with httprs.AsyncClient() as client:
        for code in [200, 404, 500]:
            response = await client.get(server.url + f"/status/{code}")
            if code >= 400:
                with pytest.raises(httprs.HTTPStatusError):
                    response.raise_for_status()
            else:
                assert response.raise_for_status() is response


@pytest.mark.anyio
async def test_custom_headers(server):
    async with httprs.AsyncClient() as client:
        response = await client.get(
            server.url + "/echo_headers",
            headers={"x-async-header": "async-value"},
        )
    assert response.status_code == 200
    data = response.json()
    assert data.get("x-async-header") == "async-value"


@pytest.mark.anyio
async def test_basic_auth(server):
    async with httprs.AsyncClient() as client:
        response = await client.get(
            server.url + "/echo_headers",
            auth=httprs.BasicAuth("user", "pass"),
        )
    assert response.status_code == 200
    data = response.json()
    assert data.get("authorization", "").startswith("Basic ")


@pytest.mark.anyio
async def test_follow_redirects(server):
    async with httprs.AsyncClient(follow_redirects=True) as client:
        response = await client.get(server.url + "/redirect_301")
    assert response.status_code == 200


@pytest.mark.anyio
async def test_invalid_url_raises(server):
    async with httprs.AsyncClient() as client:
        with pytest.raises((httprs.UnsupportedProtocol, httprs.RequestError)):
            await client.get("invalid://example.org")


@pytest.mark.anyio
async def test_http2_flag_ignored(server):
    async with httprs.AsyncClient(http2=True) as client:
        response = await client.get(server.url)
    assert response.status_code == 200


@pytest.mark.anyio
async def test_context_manager_closes_client():
    client = httprs.AsyncClient()
    async with client as c:
        assert c is client
    # After exiting, further calls should raise
    with pytest.raises(RuntimeError):
        await client.get("http://example.com/")


@pytest.mark.anyio
async def test_aclose_closes_client(server):
    client = httprs.AsyncClient()
    assert client.is_closed is False
    await client.aclose()
    assert client.is_closed is True
    with pytest.raises(RuntimeError):
        await client.get(server.url)


@pytest.mark.anyio
async def test_send_uses_sync_only_transport():
    class SyncOnlyTransport:
        def handle_request(self, request):
            return httprs.Response(209, text="sync-only", request=request)

    async with httprs.AsyncClient(transport=SyncOnlyTransport()) as client:
        request = client.build_request("GET", "https://example.com/")
        response = await client.send(request)

    assert response.status_code == 209
    assert response.text == "sync-only"


@pytest.mark.anyio
async def test_send_prefers_handle_async_request_over_handle_request():
    class DualTransport:
        def handle_request(self, request):
            return httprs.Response(599, text="sync-path", request=request)

        async def handle_async_request(self, request):
            return httprs.Response(210, text="async-path", request=request)

    async with httprs.AsyncClient(transport=DualTransport()) as client:
        request = client.build_request("GET", "https://example.com/")
        response = await client.send(request)

    assert response.status_code == 210
    assert response.text == "async-path"


@pytest.mark.anyio
async def test_send_auth_argument_basic(server):
    async with httprs.AsyncClient() as client:
        request = client.build_request("GET", server.url + "/echo_headers")
        response = await client.send(request, auth=httprs.BasicAuth("user", "pass"))
    assert response.status_code == 200
    data = response.json()
    assert data.get("authorization", "").startswith("Basic ")


@pytest.mark.anyio
async def test_send_auth_argument_overrides_existing_authorization_header(server):
    async with httprs.AsyncClient() as client:
        request = client.build_request(
            "GET",
            server.url + "/echo_headers",
            headers={"authorization": "Basic stale-token"},
        )
        response = await client.send(request, auth=httprs.BasicAuth("user", "pass"))
    assert response.status_code == 200
    data = response.json()
    assert data.get("authorization", "").startswith("Basic ")
    assert data.get("authorization") != "Basic stale-token"


@pytest.mark.anyio
async def test_send_uses_client_default_auth(server):
    async with httprs.AsyncClient(auth=httprs.BasicAuth("user", "pass")) as client:
        request = client.build_request("GET", server.url + "/echo_headers")
        response = await client.send(request)
    assert response.status_code == 200
    data = response.json()
    assert data.get("authorization", "").startswith("Basic ")


@pytest.mark.anyio
async def test_subclass_super_init_kwargs(server):
    class WrappedAsyncClient(httprs.AsyncClient):
        def __init__(self, **kwargs):
            kwargs.setdefault("follow_redirects", True)
            super().__init__(**kwargs)

    async with WrappedAsyncClient(timeout=1.0) as client:
        response = await client.get(server.url)

    assert response.status_code == 200
