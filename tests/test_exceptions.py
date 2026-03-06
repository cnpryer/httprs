"""Tests for the exception hierarchy."""

import pytest
import httprs


def test_http_error_base():
    assert issubclass(httprs.HTTPError, Exception)


def test_request_error_hierarchy():
    assert issubclass(httprs.RequestError, httprs.HTTPError)


def test_transport_error_hierarchy():
    assert issubclass(httprs.TransportError, httprs.RequestError)
    assert issubclass(httprs.TransportError, httprs.HTTPError)


def test_timeout_hierarchy():
    assert issubclass(httprs.TimeoutException, httprs.TransportError)
    assert issubclass(httprs.TimeoutException, httprs.RequestError)
    assert issubclass(httprs.TimeoutException, httprs.HTTPError)
    assert issubclass(httprs.ConnectTimeout, httprs.TimeoutException)
    assert issubclass(httprs.ReadTimeout, httprs.TimeoutException)
    assert issubclass(httprs.WriteTimeout, httprs.TimeoutException)


def test_network_error_hierarchy():
    assert issubclass(httprs.NetworkError, httprs.TransportError)
    assert issubclass(httprs.ConnectError, httprs.NetworkError)
    assert issubclass(httprs.ConnectError, httprs.TransportError)
    assert issubclass(httprs.ConnectError, httprs.RequestError)
    assert issubclass(httprs.ConnectError, httprs.HTTPError)
    assert issubclass(httprs.ReadError, httprs.NetworkError)


def test_unsupported_protocol_hierarchy():
    assert issubclass(httprs.UnsupportedProtocol, httprs.TransportError)


def test_too_many_redirects_hierarchy():
    assert issubclass(httprs.TooManyRedirects, httprs.RequestError)
    assert issubclass(httprs.TooManyRedirects, httprs.HTTPError)


def test_http_status_error_hierarchy():
    assert issubclass(httprs.HTTPStatusError, httprs.HTTPError)
    # HTTPStatusError is NOT a RequestError
    assert not issubclass(httprs.HTTPStatusError, httprs.RequestError)
    assert not issubclass(httprs.HTTPStatusError, httprs.TransportError)


def test_can_raise_and_catch_connect_error():
    with pytest.raises(httprs.NetworkError):
        raise httprs.ConnectError("connection refused")


def test_can_raise_and_catch_as_http_error():
    with pytest.raises(httprs.HTTPError):
        raise httprs.ConnectError("connection refused")


def test_can_raise_and_catch_timeout():
    with pytest.raises(httprs.TimeoutException):
        raise httprs.ReadTimeout("read timed out")


def test_can_raise_and_catch_http_status_error():
    with pytest.raises(httprs.HTTPStatusError):
        raise httprs.HTTPStatusError("404 Not Found")


def test_exception_message():
    try:
        raise httprs.ConnectError("test message")
    except httprs.ConnectError as e:
        assert "test message" in str(e)


def test_exception_not_confused_across_branches():
    # HTTPStatusError should not be catchable as RequestError
    with pytest.raises(Exception):
        try:
            raise httprs.HTTPStatusError("404")
        except httprs.RequestError:
            pass  # Should NOT reach here
