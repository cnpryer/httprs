"""Integration tests for the synchronous Client."""

from __future__ import annotations

import csv
import json
import pathlib
import urllib.parse
from datetime import timedelta

import pytest
import httprs

_INPUT_DIR = pathlib.Path(__file__).parent / "input"


def test_context_manager(server):
    with httprs.Client() as client:
        response = client.get(server.url)
    assert response.status_code == 200


def test_get(server):
    url = server.url
    with httprs.Client() as client:
        response = client.get(url)
    assert response.status_code == 200
    assert response.reason_phrase == "OK"
    assert response.content == b"Hello, world!"
    assert response.text == "Hello, world!"
    assert response.http_version == "HTTP/1.1"
    assert response.headers
    assert response.is_redirect is False
    assert repr(response) == "<Response [200 OK]>"
    assert response.elapsed >= timedelta(0)


def test_post(server):
    with httprs.Client() as client:
        response = client.post(server.url + "/echo_body", content=b"Hello, world!")
    assert response.status_code == 200
    assert response.content == b"Hello, world!"


def test_post_json(server):
    with httprs.Client() as client:
        response = client.post(server.url, json={"text": "Hello, world!"})
    assert response.status_code == 200
    assert response.reason_phrase == "OK"


def test_post_json_compact_utf8_encoding(server):
    payload = {"text": "héllo", "n": 1}
    with httprs.Client() as client:
        response = client.post(server.url + "/echo_body", json=payload)
    assert response.status_code == 200
    assert response.content == b'{"text":"h\xc3\xa9llo","n":1}'


def test_post_json_rejects_non_finite_float(server):
    with httprs.Client() as client:
        with pytest.raises(ValueError, match="not JSON compliant"):
            client.post(server.url, json={"value": float("nan")})


def test_post_json_coerces_supported_dict_keys(server):
    with httprs.Client() as client:
        response = client.post(
            server.url + "/echo_body",
            json={False: 1, None: 2, 3: 4, 1.5: 5},
        )
    assert response.status_code == 200
    assert json.loads(response.content) == {"false": 1, "null": 2, "3": 4, "1.5": 5}


def test_post_json_with_pre_serialized_string_is_encoded_as_json_string(server):
    payload = json.dumps({"text": "Hello"})
    with httprs.Client() as client:
        response = client.post(server.url + "/echo_body", json=payload)
    assert response.status_code == 200
    assert json.loads(response.content) == payload
    assert json.loads(json.loads(response.content)) == {"text": "Hello"}


def test_put(server):
    with httprs.Client() as client:
        response = client.put(server.url + "/echo_body", content=b"data")
    assert response.status_code == 200


def test_patch(server):
    with httprs.Client() as client:
        response = client.patch(server.url + "/echo_body", content=b"patch")
    assert response.status_code == 200


def test_delete(server):
    with httprs.Client() as client:
        response = client.delete(server.url)
    assert response.status_code == 200


def test_head(server):
    with httprs.Client() as client:
        response = client.head(server.url)
    assert response.status_code == 200


def test_options(server):
    with httprs.Client() as client:
        response = client.options(server.url)
    assert response.status_code == 200


def test_build_request_and_send(server):
    url_with_echo = server.url + "/echo_headers"
    with httprs.Client() as client:
        request = client.build_request("GET", url_with_echo)
        request.set_header("x-custom-header", "my-value")
        response = client.send(request)
    assert response.status_code == 200
    data = response.json()
    assert data.get("x-custom-header") == "my-value"


def test_client_default_params_applied_to_request_url(server):
    with httprs.Client(params={"client": "1"}) as client:
        response = client.get(server.url + "/json")
    assert response.status_code == 200
    assert response.url.query == "client=1"


def test_client_default_params_merged_for_build_request(server):
    with httprs.Client(params={"client": "1", "shared": "client"}) as client:
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


def test_stream_uses_client_default_params(server):
    with httprs.Client(params={"stream": "yes"}) as client:
        with client.stream("GET", server.url + "/json") as response:
            _ = response.read()

    assert response.url.query == "stream=yes"


def test_client_default_cookies_applied_to_requests(server):
    with httprs.Client(cookies={"session": "abc", "theme": "dark"}) as client:
        response = client.get(server.url + "/echo_headers")

    cookie_header = response.json().get("cookie", "")
    cookie_parts = {part.strip() for part in cookie_header.split(";")}
    assert {"session=abc", "theme=dark"} <= cookie_parts


def test_client_default_cookies_allow_request_header_override(server):
    with httprs.Client(cookies={"session": "abc"}) as client:
        response = client.get(
            server.url + "/echo_headers", headers={"cookie": "manual=1"}
        )

    assert response.json().get("cookie") == "manual=1"


def test_send_applies_client_default_cookies(server):
    with httprs.Client(cookies=httprs.Cookies({"session": "abc"})) as client:
        request = httprs.Request("GET", server.url + "/echo_headers")
        response = client.send(request)

    assert response.json().get("cookie") == "session=abc"


def test_client_default_cookies_are_origin_scoped(server):
    alt_url = server.url.replace("127.0.0.1", "localhost", 1)
    with httprs.Client(cookies={"session": "abc"}) as client:
        first = client.get(server.url + "/echo_headers")
        second = client.get(alt_url + "/echo_headers")

    assert first.json().get("cookie") == "session=abc"
    assert second.json().get("cookie") is None


def test_client_rejects_cookie_value_with_semicolon():
    with pytest.raises(ValueError, match="cookie value contains invalid characters"):
        httprs.Client(cookies={"session": "abc;admin=true"})


def test_send_uses_custom_transport():
    class StaticTransport:
        def handle_request(self, request):
            return httprs.Response(206, text="from-transport", request=request)

    with httprs.Client(transport=StaticTransport()) as client:
        request = client.build_request("GET", "https://example.com/")
        response = client.send(request)

    assert response.status_code == 206
    assert response.text == "from-transport"


def test_send_mount_uses_matching_transport():
    class FallbackTransport:
        def handle_request(self, request):
            return httprs.Response(207, text="fallback", request=request)

    class ApiTransport:
        def handle_request(self, request):
            return httprs.Response(208, text="api", request=request)

    with httprs.Client(
        transport=FallbackTransport(),
        mounts={"https://example.com/api/": ApiTransport()},
    ) as client:
        mounted_req = client.build_request("GET", "https://example.com/api/items")
        fallback_req = client.build_request("GET", "https://example.org/api/items")
        mounted_resp = client.send(mounted_req)
        fallback_resp = client.send(fallback_req)

    assert mounted_resp.status_code == 208
    assert mounted_resp.text == "api"
    assert fallback_resp.status_code == 207
    assert fallback_resp.text == "fallback"


def test_send_mount_uses_longest_matching_prefix():
    class RootTransport:
        def handle_request(self, request):
            return httprs.Response(209, text="root", request=request)

    class ApiTransport:
        def handle_request(self, request):
            return httprs.Response(210, text="api", request=request)

    with httprs.Client(
        mounts={
            "https://example.com/": RootTransport(),
            "https://example.com/api/": ApiTransport(),
        }
    ) as client:
        api_req = client.build_request("GET", "https://example.com/api/items")
        root_req = client.build_request("GET", "https://example.com/other")
        api_resp = client.send(api_req)
        root_resp = client.send(root_req)

    assert api_resp.status_code == 210
    assert api_resp.text == "api"
    assert root_resp.status_code == 209
    assert root_resp.text == "root"


def test_send_mount_host_prefix_requires_boundary():
    class HostTransport:
        def handle_request(self, request):
            return httprs.Response(211, text="host", request=request)

    class FallbackTransport:
        def handle_request(self, request):
            return httprs.Response(212, text="fallback", request=request)

    with httprs.Client(
        transport=FallbackTransport(),
        mounts={"https://example.com": HostTransport()},
    ) as client:
        request = client.build_request("GET", "https://example.com.evil/resource")
        response = client.send(request)

    assert response.status_code == 212
    assert response.text == "fallback"


def test_client_rejects_mount_with_none_transport():
    with pytest.raises(TypeError, match="mount transport cannot be None"):
        httprs.Client(mounts={"https://example.com/": None})


def test_client_rejects_mount_with_non_string_key():
    with pytest.raises(TypeError, match="mount keys must be strings"):
        httprs.Client(mounts={1: object()})


def test_send_auth_argument_basic(server):
    with httprs.Client() as client:
        request = client.build_request("GET", server.url + "/echo_headers")
        response = client.send(request, auth=httprs.BasicAuth("user", "pass"))
    assert response.status_code == 200
    data = response.json()
    assert data.get("authorization", "").startswith("Basic ")


def test_send_auth_argument_overrides_existing_authorization_header(server):
    with httprs.Client() as client:
        request = client.build_request(
            "GET",
            server.url + "/echo_headers",
            headers={"authorization": "Basic stale-token"},
        )
        response = client.send(request, auth=httprs.BasicAuth("user", "pass"))
    assert response.status_code == 200
    data = response.json()
    assert data.get("authorization", "").startswith("Basic ")
    assert data.get("authorization") != "Basic stale-token"


def test_send_uses_client_default_auth(server):
    with httprs.Client(auth=httprs.BasicAuth("user", "pass")) as client:
        request = client.build_request("GET", server.url + "/echo_headers")
        response = client.send(request)
    assert response.status_code == 200
    data = response.json()
    assert data.get("authorization", "").startswith("Basic ")


def test_custom_headers_per_request(server):
    with httprs.Client() as client:
        response = client.get(
            server.url + "/echo_headers",
            headers={"x-my-header": "hello"},
        )
    assert response.status_code == 200
    data = response.json()
    assert data.get("x-my-header") == "hello"


def test_raise_for_status(server):
    with httprs.Client() as client:
        for status_code in (200, 400, 404, 500, 505):
            response = client.get(server.url + f"/status/{status_code}")
            if 400 <= status_code < 600:
                with pytest.raises(httprs.HTTPStatusError):
                    response.raise_for_status()
            else:
                assert response.raise_for_status() is response


def test_follow_redirects_true(server):
    with httprs.Client(follow_redirects=True) as client:
        response = client.get(server.url + "/redirect_301")
    # After following redirect, should get the final response
    assert response.status_code == 200


def test_follow_redirects_false(server):
    with httprs.Client(follow_redirects=False) as client:
        response = client.get(server.url + "/redirect_301")
    assert response.status_code == 301
    assert response.is_redirect


def test_basic_auth_via_client(server):
    with httprs.Client(auth=httprs.BasicAuth("user", "pass")) as client:
        response = client.get(server.url + "/echo_headers")
    assert response.status_code == 200
    data = response.json()
    auth_header = data.get("authorization", "")
    assert auth_header.startswith("Basic ")


def test_basic_auth_tuple(server):
    with httprs.Client(auth=("user", "pass")) as client:
        response = client.get(server.url + "/echo_headers")
    assert response.status_code == 200
    data = response.json()
    assert data.get("authorization", "").startswith("Basic ")


def test_timeout_raises_on_slow_response(server):
    with httprs.Client(timeout=httprs.Timeout(read=0.1)) as client:
        with pytest.raises(httprs.TimeoutException):
            client.get(server.url + "/slow_response")


def test_stream_context_manager(server):
    with httprs.Client() as client:
        with client.stream("GET", server.url) as response:
            content = response.read()
    assert response.status_code == 200
    assert content == b"Hello, world!"


def test_stream_iter_bytes(server):
    body = b""
    with httprs.Client() as client:
        with client.stream("GET", server.url) as response:
            for chunk in response.iter_bytes():
                body += chunk
    assert response.status_code == 200
    assert body == b"Hello, world!"


def test_json_response(server):
    with httprs.Client() as client:
        response = client.get(server.url + "/json")
    assert response.status_code == 200
    data = response.json()
    assert data == {"hello": "world"}


def test_invalid_url_raises(server):
    with httprs.Client() as client:
        with pytest.raises((httprs.UnsupportedProtocol, httprs.RequestError)):
            client.get("invalid://example.org")


def test_http2_flag_ignored(server):
    """http2 parameter should be accepted but not necessarily used."""
    with httprs.Client(http2=True) as client:
        response = client.get(server.url)
    assert response.status_code == 200


def test_client_closed_raises(server):
    client = httprs.Client()
    client.close()
    with pytest.raises(RuntimeError):
        client.get(server.url)


def test_client_invalid_cert_pem_raises():
    with pytest.raises(ValueError, match="invalid client certificate"):
        httprs.Client(cert=b"not-a-valid-pem")


def test_client_missing_cert_file_raises():
    with pytest.raises(ValueError, match="failed to read client cert file"):
        httprs.Client(cert=str(_INPUT_DIR / "missing-client-cert.pem"))


def test_client_accepts_cert_with_combined_pem_bytes(server, mtls_server):
    with httprs.Client(cert=mtls_server.client_pem.read_bytes()) as client:
        response = client.get(server.url)

    assert response.status_code == 200
    assert response.text == "Hello, world!"


def test_client_accepts_cert_with_pathlike_cert_file(server, mtls_server):
    with httprs.Client(cert=mtls_server.client_pem) as client:
        response = client.get(server.url)

    assert response.status_code == 200
    assert response.text == "Hello, world!"


def test_client_accepts_cert_with_cert_key_tuple(server, mtls_server):
    with httprs.Client(
        cert=(mtls_server.client_cert, mtls_server.client_key)
    ) as client:
        response = client.get(server.url)

    assert response.status_code == 200
    assert response.text == "Hello, world!"


def test_client_accepts_cert_with_cert_key_list(server, mtls_server):
    with httprs.Client(
        cert=[mtls_server.client_cert, mtls_server.client_key]
    ) as client:
        response = client.get(server.url)

    assert response.status_code == 200
    assert response.text == "Hello, world!"


def test_client_rejects_cert_with_verify_disabled_bytes(mtls_server):
    with pytest.raises(ValueError, match="cert cannot be used when verify=False"):
        httprs.Client(cert=mtls_server.client_pem.read_bytes(), verify=False)


def test_client_rejects_cert_with_verify_disabled_pathlike(mtls_server):
    with pytest.raises(ValueError, match="cert cannot be used when verify=False"):
        httprs.Client(cert=mtls_server.client_pem, verify=False)


def test_client_rejects_cert_with_verify_disabled_tuple(mtls_server):
    with pytest.raises(ValueError, match="cert cannot be used when verify=False"):
        httprs.Client(
            cert=(mtls_server.client_cert, mtls_server.client_key), verify=False
        )


def test_client_rejects_cert_with_verify_disabled_list(mtls_server):
    with pytest.raises(ValueError, match="cert cannot be used when verify=False"):
        httprs.Client(
            cert=[mtls_server.client_cert, mtls_server.client_key], verify=False
        )


def test_client_mtls_without_cert_fails_connect(mtls_server):
    with httprs.Client(verify=False) as client:
        with pytest.raises(httprs.RequestError):
            client.get(mtls_server.url)


def test_client_cert_does_not_bypass_server_verification(mtls_server):
    with httprs.Client(cert=mtls_server.client_pem) as client:
        with pytest.raises(httprs.RequestError):
            client.get(mtls_server.url)


def test_client_request_follow_redirect_override_preserves_cert(server, mtls_server):
    with httprs.Client(cert=mtls_server.client_pem, follow_redirects=False) as client:
        response = client.get(server.url + "/redirect_301", follow_redirects=True)

    assert response.status_code == 200
    assert response.text == "Hello, world!"


def test_client_send_follow_redirect_override_preserves_cert(server, mtls_server):
    with httprs.Client(cert=mtls_server.client_pem, follow_redirects=False) as client:
        request = client.build_request("GET", server.url + "/redirect_301")
        response = client.send(request, follow_redirects=True)

    assert response.status_code == 200
    assert response.text == "Hello, world!"


def test_subclass_super_init_kwargs(server):
    class WrappedClient(httprs.Client):
        def __init__(self, **kwargs):
            kwargs.setdefault("follow_redirects", True)
            super().__init__(**kwargs)

    with WrappedClient(timeout=1.0) as client:
        response = client.get(server.url)

    assert response.status_code == 200


def test_data_bytes(server):
    """Regression test for https://github.com/cnpryer/httprs/issues/2.

    Passing io.BytesIO.getvalue() (bytes) as data= raised:
      TypeError: 'bytes' object is not an instance of 'dict'
    """
    import io

    body = io.BytesIO(b"hello world").getvalue()
    with httprs.Client() as client:
        response = client.post(server.url + "/echo_body", data=body)
    assert response.status_code == 200
    assert response.content == b"hello world"


def test_data_dict(server):
    """data=dict still form-encodes the body."""
    with httprs.Client() as client:
        response = client.post(server.url + "/echo_body", data={"key": "value"})
    assert response.status_code == 200
    assert response.content == b"key=value"


def test_data_invalid_type_raises(server):
    """data=<invalid type> raises TypeError with a clear message."""
    with httprs.Client() as client:
        with pytest.raises(TypeError, match="data must be a dict, bytes, or list"):
            client.post(server.url, data=[1, 2, 3])


def test_data_list_of_tuples(server):
    """data=list-of-tuples form-encodes the body."""
    with httprs.Client() as client:
        response = client.post(
            server.url + "/echo_body", data=[("key", "value"), ("other", "data")]
        )
    assert response.status_code == 200
    assert response.content == b"key=value&other=data"


def test_data_list_of_tuples_duplicate_keys(server):
    """data=list-of-tuples preserves duplicate keys."""
    with httprs.Client() as client:
        response = client.post(
            server.url + "/echo_body", data=[("tag", "a"), ("tag", "b")]
        )
    assert response.status_code == 200
    assert response.content == b"tag=a&tag=b"


def test_post_small_json_fixture(server):
    payload = json.loads((_INPUT_DIR / "small.json").read_bytes())
    with httprs.Client() as client:
        response = client.post(server.url + "/echo_body", json=payload)
    assert response.status_code == 200


def test_post_small_csv_fixture(server):
    pairs = [
        (row["name"], row["value"])
        for row in csv.DictReader((_INPUT_DIR / "small.csv").read_text().splitlines())
    ]
    with httprs.Client() as client:
        response = client.post(server.url + "/echo_body", data=pairs)
    assert response.status_code == 200
    assert (
        response.content
        == b"username=alice&email=alice%40example.com&role=admin&theme=dark&lang=en-US"
    )
