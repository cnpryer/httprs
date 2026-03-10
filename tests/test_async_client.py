"""Integration tests for the asynchronous AsyncClient."""

from __future__ import annotations

from datetime import timedelta
import json
import pathlib
import urllib.parse

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
async def test_post_json_compact_utf8_encoding(server):
    payload = {"text": "héllo", "n": 1}
    async with httprs.AsyncClient() as client:
        response = await client.post(server.url + "/echo_body", json=payload)
    assert response.status_code == 200
    assert response.content == b'{"text":"h\xc3\xa9llo","n":1}'


@pytest.mark.anyio
async def test_post_json_rejects_non_finite_float(server):
    async with httprs.AsyncClient() as client:
        with pytest.raises(ValueError, match="not JSON compliant"):
            await client.post(server.url, json={"value": float("nan")})


@pytest.mark.anyio
async def test_post_json_coerces_supported_dict_keys(server):
    async with httprs.AsyncClient() as client:
        response = await client.post(
            server.url + "/echo_body",
            json={False: 1, None: 2, 3: 4, 1.5: 5},
        )
    assert response.status_code == 200
    assert json.loads(response.content) == {"false": 1, "null": 2, "3": 4, "1.5": 5}


@pytest.mark.anyio
async def test_post_json_with_pre_serialized_string_is_encoded_as_json_string(server):
    payload = json.dumps({"text": "Hello"})
    async with httprs.AsyncClient() as client:
        response = await client.post(server.url + "/echo_body", json=payload)
    assert response.status_code == 200
    assert json.loads(response.content) == payload
    assert json.loads(json.loads(response.content)) == {"text": "Hello"}


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
async def test_async_client_default_params_applied_to_request_url(server):
    async with httprs.AsyncClient(params={"client": "1"}) as client:
        response = await client.get(server.url + "/json")
    assert response.status_code == 200
    assert response.url.query == "client=1"


@pytest.mark.anyio
async def test_async_client_default_params_merged_for_build_request(server):
    async with httprs.AsyncClient(params={"client": "1", "shared": "client"}) as client:
        request = client.build_request(
            "GET",
            server.url + "/json?url=1&shared=url",
            params={"request": "1", "shared": "request"},
        )

    query_pairs = urllib.parse.parse_qsl(
        request.url.query or "", keep_blank_values=True
    )
    assert dict(query_pairs) == {
        "client": "1",
        "url": "1",
        "request": "1",
        "shared": "request",
    }


@pytest.mark.anyio
async def test_async_client_default_cookies_applied_to_requests(server):
    async with httprs.AsyncClient(
        cookies={"session": "abc", "theme": "dark"}
    ) as client:
        response = await client.get(server.url + "/echo_headers")

    cookie_header = response.json().get("cookie", "")
    cookie_parts = {part.strip() for part in cookie_header.split(";")}
    assert {"session=abc", "theme=dark"} <= cookie_parts


@pytest.mark.anyio
async def test_async_client_default_cookies_allow_request_header_override(server):
    async with httprs.AsyncClient(cookies={"session": "abc"}) as client:
        response = await client.get(
            server.url + "/echo_headers", headers={"cookie": "manual=1"}
        )

    assert response.json().get("cookie") == "manual=1"


@pytest.mark.anyio
async def test_async_send_applies_client_default_cookies(server):
    async with httprs.AsyncClient(cookies=httprs.Cookies({"session": "abc"})) as client:
        request = httprs.Request("GET", server.url + "/echo_headers")
        response = await client.send(request)

    assert response.json().get("cookie") == "session=abc"


@pytest.mark.anyio
async def test_async_client_default_cookies_are_origin_scoped(server):
    alt_url = server.url.replace("127.0.0.1", "localhost", 1)
    async with httprs.AsyncClient(cookies={"session": "abc"}) as client:
        first = await client.get(server.url + "/echo_headers")
        second = await client.get(alt_url + "/echo_headers")

    assert first.json().get("cookie") == "session=abc"
    assert second.json().get("cookie") is None


@pytest.mark.anyio
async def test_async_client_rejects_cookie_value_with_semicolon():
    with pytest.raises(ValueError, match="cookie value contains invalid characters"):
        httprs.AsyncClient(cookies={"session": "abc;admin=true"})


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
async def test_async_client_invalid_cert_pem_raises():
    with pytest.raises(ValueError, match="invalid client certificate"):
        httprs.AsyncClient(cert=b"not-a-valid-pem")


@pytest.mark.anyio
async def test_async_client_missing_cert_file_raises():
    with pytest.raises(ValueError, match="failed to read client cert file"):
        httprs.AsyncClient(
            cert=str(pathlib.Path(__file__).parent / "input/missing-client-cert.pem")
        )


@pytest.mark.anyio
async def test_async_client_accepts_cert_with_combined_pem_bytes(server, mtls_server):
    async with httprs.AsyncClient(cert=mtls_server.client_pem.read_bytes()) as client:
        response = await client.get(server.url)

    assert response.status_code == 200
    assert response.text == "Hello, world!"


@pytest.mark.anyio
async def test_async_client_accepts_cert_with_pathlike_cert_file(server, mtls_server):
    async with httprs.AsyncClient(cert=mtls_server.client_pem) as client:
        response = await client.get(server.url)

    assert response.status_code == 200
    assert response.text == "Hello, world!"


@pytest.mark.anyio
async def test_async_client_accepts_cert_with_cert_key_tuple(server, mtls_server):
    async with httprs.AsyncClient(
        cert=(mtls_server.client_cert, mtls_server.client_key)
    ) as client:
        response = await client.get(server.url)

    assert response.status_code == 200
    assert response.text == "Hello, world!"


@pytest.mark.anyio
async def test_async_client_accepts_cert_with_cert_key_list(server, mtls_server):
    async with httprs.AsyncClient(
        cert=[mtls_server.client_cert, mtls_server.client_key]
    ) as client:
        response = await client.get(server.url)

    assert response.status_code == 200
    assert response.text == "Hello, world!"


@pytest.mark.anyio
async def test_async_client_rejects_cert_with_verify_disabled_bytes(mtls_server):
    with pytest.raises(ValueError, match="cert cannot be used when verify=False"):
        httprs.AsyncClient(cert=mtls_server.client_pem.read_bytes(), verify=False)


@pytest.mark.anyio
async def test_async_client_rejects_cert_with_verify_disabled_pathlike(mtls_server):
    with pytest.raises(ValueError, match="cert cannot be used when verify=False"):
        httprs.AsyncClient(cert=mtls_server.client_pem, verify=False)


@pytest.mark.anyio
async def test_async_client_rejects_cert_with_verify_disabled_tuple(mtls_server):
    with pytest.raises(ValueError, match="cert cannot be used when verify=False"):
        httprs.AsyncClient(
            cert=(mtls_server.client_cert, mtls_server.client_key), verify=False
        )


@pytest.mark.anyio
async def test_async_client_rejects_cert_with_verify_disabled_list(mtls_server):
    with pytest.raises(ValueError, match="cert cannot be used when verify=False"):
        httprs.AsyncClient(
            cert=[mtls_server.client_cert, mtls_server.client_key], verify=False
        )


@pytest.mark.anyio
async def test_async_client_mtls_without_cert_fails_connect(mtls_server):
    async with httprs.AsyncClient(verify=False) as client:
        with pytest.raises(httprs.RequestError):
            await client.get(mtls_server.url)


@pytest.mark.anyio
async def test_async_client_cert_does_not_bypass_server_verification(mtls_server):
    async with httprs.AsyncClient(cert=mtls_server.client_pem) as client:
        with pytest.raises(httprs.RequestError):
            await client.get(mtls_server.url)


@pytest.mark.anyio
async def test_async_client_send_follow_redirect_override_preserves_cert(
    server, mtls_server
):
    async with httprs.AsyncClient(
        cert=mtls_server.client_pem, follow_redirects=False
    ) as client:
        request = client.build_request("GET", server.url + "/redirect_301")
        response = await client.send(request, follow_redirects=True)

    assert response.status_code == 200
    assert response.text == "Hello, world!"


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
