"""Integration tests for module-level API functions."""

from __future__ import annotations

import pytest
import httprs


def test_get(server):
    response = httprs.get(server.url)
    assert response.status_code == 200
    assert response.reason_phrase == "OK"
    assert response.text == "Hello, world!"
    assert response.http_version == "HTTP/1.1"


def test_post(server):
    response = httprs.post(server.url + "/echo_body", content=b"Hello, world!")
    assert response.status_code == 200
    assert response.content == b"Hello, world!"


def test_put(server):
    response = httprs.put(server.url + "/echo_body", content=b"data")
    assert response.status_code == 200


def test_patch(server):
    response = httprs.patch(server.url + "/echo_body", content=b"patch data")
    assert response.status_code == 200


def test_delete(server):
    response = httprs.delete(server.url)
    assert response.status_code == 200


def test_head(server):
    response = httprs.head(server.url)
    assert response.status_code == 200


def test_options(server):
    response = httprs.options(server.url)
    assert response.status_code == 200


def test_json_endpoint(server):
    response = httprs.get(server.url + "/json")
    assert response.status_code == 200
    data = response.json()
    assert data == {"hello": "world"}


def test_stream(server):
    with httprs.stream("GET", server.url) as response:
        content = response.read()
    assert response.status_code == 200
    assert response.reason_phrase == "OK"
    assert content == b"Hello, world!"


def test_stream_iter_bytes(server):
    body = b""
    with httprs.stream("GET", server.url) as response:
        for chunk in response.iter_bytes():
            body += chunk
    assert response.status_code == 200
    assert body == b"Hello, world!"


def test_get_invalid_url():
    with pytest.raises(httprs.UnsupportedProtocol):
        httprs.get("invalid://example.org")


def test_post_json(server):
    response = httprs.post(server.url, json={"text": "Hello, world!"})
    assert response.status_code == 200


def test_status_codes(server):
    for code in [200, 404, 500]:
        response = httprs.get(server.url + f"/status/{code}")
        assert response.status_code == code
