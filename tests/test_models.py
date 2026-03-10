"""Unit tests for URL, Headers, Request, and Response models."""

from __future__ import annotations

from datetime import timedelta
import json

import pytest
import httprs


@pytest.fixture
def anyio_backend():
    return "asyncio"


class TestURL:
    def test_parse_full_url(self):
        url = httprs.URL("https://example.com:8080/path?q=1&r=2#frag")
        assert url.scheme == "https"
        assert url.host == "example.com"
        assert url.port == 8080
        assert url.path == "/path"
        assert url.query == "q=1&r=2"
        assert url.fragment == "frag"

    def test_parse_simple_url(self):
        url = httprs.URL("http://example.com/")
        assert url.scheme == "http"
        assert url.host == "example.com"
        assert url.path == "/"

    def test_default_port_http(self):
        url = httprs.URL("http://example.com/path")
        # Port 80 is the default for http
        assert url.port == 80

    def test_default_port_https(self):
        url = httprs.URL("https://example.com/path")
        # Port 443 is the default for https
        assert url.port == 443

    def test_str_conversion(self):
        url_str = "https://example.com/path?q=1"
        url = httprs.URL(url_str)
        assert str(url) == url_str

    def test_repr(self):
        url = httprs.URL("https://example.com/")
        assert "https://example.com/" in repr(url)

    def test_equality(self):
        assert httprs.URL("https://example.com/") == httprs.URL("https://example.com/")
        assert httprs.URL("https://example.com/") != httprs.URL("http://example.com/")

    def test_copy_with_path(self):
        url = httprs.URL("https://example.com/old")
        new_url = url.copy_with(path="/new")
        assert new_url.path == "/new"
        assert new_url.host == "example.com"

    def test_copy_with_query(self):
        url = httprs.URL("https://example.com/path")
        new_url = url.copy_with(query="a=1&b=2")
        assert new_url.query == "a=1&b=2"

    def test_no_query(self):
        url = httprs.URL("https://example.com/path")
        assert url.query is None

    def test_invalid_url_raises(self):
        with pytest.raises(Exception):
            httprs.URL("not-a-url")


class TestHeaders:
    def test_empty_headers(self):
        h = httprs.Headers()
        assert len(h) == 0
        assert not h

    def test_from_list_of_tuples(self):
        h = httprs.Headers([("content-type", "text/plain"), ("x-custom", "value")])
        assert h.get("content-type") == "text/plain"
        assert h.get("x-custom") == "value"

    def test_from_dict(self):
        h = httprs.Headers({"Content-Type": "text/html"})
        assert h.get("content-type") == "text/html"

    def test_case_insensitive_get(self):
        h = httprs.Headers([("Content-Type", "text/plain")])
        assert h.get("content-type") == "text/plain"
        assert h.get("CONTENT-TYPE") == "text/plain"
        assert h.get("Content-Type") == "text/plain"

    def test_get_missing_returns_none(self):
        h = httprs.Headers()
        assert h.get("x-missing") is None

    def test_getitem(self):
        h = httprs.Headers([("content-type", "text/plain")])
        assert h["content-type"] == "text/plain"

    def test_getitem_missing_raises(self):
        h = httprs.Headers()
        with pytest.raises(KeyError):
            _ = h["x-missing"]

    def test_contains(self):
        h = httprs.Headers([("content-type", "text/plain")])
        assert "content-type" in h
        assert "Content-Type" in h
        assert "x-missing" not in h

    def test_len(self):
        h = httprs.Headers([("a", "1"), ("b", "2")])
        assert len(h) == 2

    def test_bool_nonempty(self):
        h = httprs.Headers([("a", "1")])
        assert bool(h)

    def test_bool_empty(self):
        h = httprs.Headers()
        assert not bool(h)

    def test_update(self):
        h = httprs.Headers([("a", "1"), ("b", "2")])
        h.update([("b", "updated"), ("c", "3")])
        assert h["b"] == "updated"
        assert h["c"] == "3"
        assert h["a"] == "1"

    def test_items(self):
        h = httprs.Headers([("a", "1"), ("b", "2")])
        items = h.items()
        assert ("a", "1") in items
        assert ("b", "2") in items

    def test_repr(self):
        h = httprs.Headers([("a", "1")])
        assert "Headers" in repr(h)


class TestRequest:
    def test_basic_request(self):
        r = httprs.Request("GET", "https://example.com/")
        assert r.method == "GET"
        assert str(r.url) == "https://example.com/"
        assert r.content == b""

    def test_method_uppercased(self):
        r = httprs.Request("get", "https://example.com/")
        assert r.method == "GET"

    def test_with_content(self):
        r = httprs.Request("POST", "https://example.com/", content=b"hello")
        assert r.content == b"hello"

    def test_with_headers(self):
        r = httprs.Request("GET", "https://example.com/", headers={"x-custom": "value"})
        assert r.headers.get("x-custom") == "value"

    def test_set_header(self):
        r = httprs.Request("GET", "https://example.com/")
        r.set_header("Authorization", "Bearer token")
        assert r.headers.get("authorization") == "Bearer token"

    def test_set_header_replaces(self):
        r = httprs.Request(
            "GET", "https://example.com/", headers={"authorization": "old"}
        )
        r.set_header("Authorization", "new")
        assert r.headers.get("authorization") == "new"

    def test_repr(self):
        r = httprs.Request("GET", "https://example.com/")
        assert "GET" in repr(r)


class TestResponse:
    def test_basic_response(self):
        r = httprs.Response(200, content=b"hello")
        assert r.status_code == 200
        assert r.reason_phrase == "OK"
        assert r.content == b"hello"

    def test_404_response(self):
        r = httprs.Response(404)
        assert r.status_code == 404
        assert r.reason_phrase == "Not Found"

    def test_text(self):
        r = httprs.Response(200, content=b"Hello, world!")
        assert r.text == "Hello, world!"

    def test_text_uses_default_encoding_when_charset_missing(self):
        r = httprs.Response(
            200,
            content=b"\xe9",
            headers={"content-type": "text/plain"},
            default_encoding="latin-1",
        )
        assert r.text == "é"

    def test_text_falls_back_to_default_encoding_on_decode_error(self):
        r = httprs.Response(
            200,
            content=b"\xe9",
            headers={"content-type": "text/plain; charset=utf-8"},
            default_encoding="latin-1",
        )
        assert r.text == "é"

    def test_text_uses_callable_default_encoding(self):
        r = httprs.Response(
            200,
            content=b"\xe9",
            headers={"content-type": "text/plain"},
            default_encoding=lambda _content: "latin-1",
        )
        assert r.text == "é"

    def test_default_encoding_rejects_invalid_type(self):
        with pytest.raises(TypeError, match="default_encoding must be a string"):
            httprs.Response(200, content=b"hello", default_encoding=123)

    def test_json(self):
        r = httprs.Response(200, content=b'{"key": "value", "num": 42}')
        data = r.json()
        assert data["key"] == "value"
        assert data["num"] == 42

    def test_json_array(self):
        r = httprs.Response(200, content=b"[1, 2, 3]")
        data = r.json()
        assert data == [1, 2, 3]

    def test_json_bool(self):
        r = httprs.Response(200, content=b'{"flag": true, "other": false}')
        data = r.json()
        assert data["flag"] is True
        assert data["other"] is False

    def test_json_null(self):
        r = httprs.Response(200, content=b'{"val": null}')
        data = r.json()
        assert data["val"] is None

    def test_json_argument_compact_utf8_encoding(self):
        r = httprs.Response(200, json={"text": "héllo", "n": 1})
        assert r.content == b'{"text":"h\xc3\xa9llo","n":1}'

    def test_json_argument_rejects_non_finite_float(self):
        with pytest.raises(ValueError, match="not JSON compliant"):
            httprs.Response(200, json={"value": float("nan")})

    def test_json_argument_with_pre_serialized_string_is_json_string(self):
        payload = json.dumps({"text": "Hello"})
        r = httprs.Response(200, json=payload)
        assert r.json() == payload
        assert json.loads(r.json()) == {"text": "Hello"}

    def test_raise_for_status_ok(self):
        r = httprs.Response(200, content=b"OK")
        result = r.raise_for_status()
        assert result is r

    def test_raise_for_status_4xx(self):
        r = httprs.Response(404)
        with pytest.raises(httprs.HTTPStatusError):
            r.raise_for_status()

    def test_raise_for_status_5xx(self):
        r = httprs.Response(500)
        with pytest.raises(httprs.HTTPStatusError):
            r.raise_for_status()

    def test_raise_for_status_3xx_ok(self):
        r = httprs.Response(301)
        # 3xx is not an error
        result = r.raise_for_status()
        assert result is r

    def test_is_redirect_true(self):
        assert httprs.Response(301).is_redirect
        assert httprs.Response(302).is_redirect
        assert httprs.Response(307).is_redirect

    def test_is_redirect_false(self):
        assert not httprs.Response(200).is_redirect
        assert not httprs.Response(404).is_redirect
        assert not httprs.Response(500).is_redirect

    def test_repr(self):
        r = httprs.Response(200)
        assert repr(r) == "<Response [200 OK]>"

    def test_repr_404(self):
        r = httprs.Response(404)
        assert repr(r) == "<Response [404 Not Found]>"

    def test_headers(self):
        r = httprs.Response(200, headers={"content-type": "text/plain"})
        assert r.headers.get("content-type") == "text/plain"

    def test_headers_from_dict(self):
        r = httprs.Response(200, headers={"X-Custom": "value"})
        assert r.headers.get("x-custom") == "value"

    def test_content_from_iterable(self):
        r = httprs.Response(200, content=[b"hello", b"-", b"world"])
        assert r.content == b"hello-world"
        assert r.headers.get("content-length") == "11"

    def test_with_request(self):
        req = httprs.Request("GET", "https://example.com/")
        r = httprs.Response(200, request=req)
        assert r.request is not None
        assert r.request.method == "GET"

    def test_http_version_default(self):
        r = httprs.Response(200)
        assert r.http_version == "HTTP/1.1"

    def test_elapsed_is_timedelta(self):
        r = httprs.Response(200)
        elapsed = r.elapsed
        assert elapsed >= timedelta(0)

    @pytest.mark.anyio
    async def test_aread_returns_content(self):
        r = httprs.Response(200, content=b"abc")
        data = await r.aread()
        assert data == b"abc"

    @pytest.mark.anyio
    async def test_aclose_closes_async_stream_response(self):
        r = httprs.Response(200, stream=httprs.AsyncByteStream(b"abc"))
        assert r.is_closed is False
        await r.aclose()
        assert r.is_closed is True
