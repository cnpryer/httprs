"""Tests for transport and byte-stream API surfaces."""

from __future__ import annotations

import pytest
import httprs


@pytest.fixture
def anyio_backend():
    return "asyncio"


def test_base_transport_handle_request_not_implemented():
    transport = httprs.BaseTransport()
    request = httprs.Request("GET", "https://example.com/")
    with pytest.raises(NotImplementedError, match="must be implemented"):
        transport.handle_request(request)


def test_async_base_transport_handle_async_request_not_implemented():
    transport = httprs.AsyncBaseTransport()
    request = httprs.Request("GET", "https://example.com/")
    with pytest.raises(NotImplementedError, match="must be implemented"):
        transport.handle_async_request(request)


def test_wsgi_transport_not_implemented():
    transport = httprs.WSGITransport(lambda environ, start_response: [])
    request = httprs.Request("GET", "https://example.com/")
    with pytest.raises(NotImplementedError, match="not implemented"):
        transport.handle_request(request)


def test_asgi_transport_not_implemented():
    transport = httprs.ASGITransport(lambda scope, receive, send: None)
    request = httprs.Request("GET", "https://example.com/")
    with pytest.raises(NotImplementedError, match="not implemented"):
        transport.handle_async_request(request)


def test_http_transport_roundtrip(server):
    transport = httprs.HTTPTransport()
    request = httprs.Request("GET", server.url)
    response = transport.handle_request(request)

    assert response.status_code == 200
    assert response.text == "Hello, world!"

    transport.close()
    assert transport.is_closed is True
    with pytest.raises(RuntimeError, match="closed"):
        transport.handle_request(request)


@pytest.mark.anyio
async def test_async_http_transport_roundtrip(server):
    transport = httprs.AsyncHTTPTransport()
    request = httprs.Request("GET", server.url)
    response = await transport.handle_async_request(request)

    assert response.status_code == 200
    assert response.text == "Hello, world!"

    transport.aclose()
    assert transport.is_closed is True
    with pytest.raises(RuntimeError, match="closed"):
        await transport.handle_async_request(request)


def test_sync_byte_stream_iterates_once():
    stream = httprs.SyncByteStream(b"abc")
    assert list(stream) == [b"abc"]
    assert list(stream) == []


def test_byte_stream_iterates_once_sync():
    stream = httprs.ByteStream(b"abc")
    assert list(stream) == [b"abc"]
    assert list(stream) == []


@pytest.mark.anyio
async def test_async_byte_stream_iterates_once():
    stream = httprs.AsyncByteStream(b"abc")
    chunks = [chunk async for chunk in stream]
    assert chunks == [b"abc"]
    chunks = [chunk async for chunk in stream]
    assert chunks == []


@pytest.mark.anyio
async def test_byte_stream_iterates_once_async():
    stream = httprs.ByteStream(b"abc")
    chunks = [chunk async for chunk in stream]
    assert chunks == [b"abc"]
    chunks = [chunk async for chunk in stream]
    assert chunks == []


@pytest.mark.anyio
async def test_mock_transport_handle_async_request_wraps_sync_result():
    def handler(request):
        return httprs.Response(299, text="mock-sync", request=request)

    transport = httprs.MockTransport(handler)
    request = httprs.Request("GET", "https://example.com/")
    response = await transport.handle_async_request(request)

    assert response.status_code == 299
    assert response.text == "mock-sync"


@pytest.mark.anyio
async def test_mock_transport_handle_async_request_accepts_async_result():
    async def handler(request):
        return httprs.Response(298, text="mock-async", request=request)

    transport = httprs.MockTransport(handler)
    request = httprs.Request("GET", "https://example.com/")
    response = await transport.handle_async_request(request)

    assert response.status_code == 298
    assert response.text == "mock-async"
