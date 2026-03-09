"""Integration tests for the synchronous Client."""

from __future__ import annotations

import csv
import json
import pathlib
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


def test_send_uses_custom_transport():
    class StaticTransport:
        def handle_request(self, request):
            return httprs.Response(206, text="from-transport", request=request)

    with httprs.Client(transport=StaticTransport()) as client:
        request = client.build_request("GET", "https://example.com/")
        response = client.send(request)

    assert response.status_code == 206
    assert response.text == "from-transport"


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
