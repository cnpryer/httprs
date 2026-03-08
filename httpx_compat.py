"""httpx compatibility shim for httprs.

Drop-in replacement for ``import httpx`` in test suites.  Re-exports httprs
names under their httpx-expected names and fills in API gaps (params=,
history, module-level functions, exception aliases, transport stubs).

Usage (normally injected by ecosystem_conftest.py):
    import sys
    sys.modules['httpx'] = <this module>
"""

from __future__ import annotations

import sys
import types
import urllib.parse

import httprs
import httprs._httprs as _httprs_ext  # noqa: F401 — available for isinstance checks

_httprs_compat = True

# Capture real httpx base classes *before* sys.modules["httpx"] is replaced with
# this shim.  At exec_module time, sys.modules["httpx"] still holds the real httpx
# (if installed), so isinstance(our_Client(), real_httpx.Client) will return True
# even when user code imported _base_client before the shim was injected.
try:
    _real_httpx = __import__("httpx")
    if getattr(_real_httpx, "_httprs_compat", False):
        raise ImportError("already shim — avoid circular base class")
    _HttpxClientBase: type = _real_httpx.Client
    _HttpxAsyncClientBase: type = _real_httpx.AsyncClient
except (ImportError, AttributeError):
    _HttpxClientBase = object
    _HttpxAsyncClientBase = object


def _append_query_params(url: str, params) -> str:
    """Append params (dict, list of pairs, or str) to URL query string."""
    if not params:
        return url
    if isinstance(params, str):
        new_qs = params
    else:
        if hasattr(params, "items"):
            pairs = list(params.items())
        else:
            pairs = list(params)
        new_qs = urllib.parse.urlencode([(str(k), str(v)) for k, v in pairs])
    if not new_qs:
        return url
    parsed = urllib.parse.urlparse(url)
    combined = f"{parsed.query}&{new_qs}" if parsed.query else new_qs
    return urllib.parse.urlunparse(parsed._replace(query=combined))


URL = httprs.URL
Headers = httprs.Headers
Request = httprs.Request
StreamContext = httprs.StreamContext
Timeout = httprs.Timeout
Limits = httprs.Limits
BasicAuth = httprs.BasicAuth
DigestAuth = httprs.DigestAuth
codes = httprs.codes

# httprs names                  httpx aliases
HTTPError = httprs.HTTPError
RequestError = httprs.RequestError
TransportError = httprs.TransportError
TimeoutException = httprs.TimeoutException
ConnectTimeout = httprs.ConnectTimeout
ReadTimeout = httprs.ReadTimeout
WriteTimeout = httprs.WriteTimeout
# PoolTimeout = httprs.TimeoutException  # httpx-specific alias
NetworkError = httprs.NetworkError
ConnectError = httprs.ConnectError
ReadError = httprs.ReadError
# WriteError = httprs.NetworkError  # httpx-specific alias
# CloseError = httprs.NetworkError  # httpx-specific alias
# ProxyError = httprs.ConnectError  # httpx-specific alias
UnsupportedProtocol = httprs.UnsupportedProtocol
# DecodingError = httprs.ReadError  # httpx-specific alias
# InvalidURL = httprs.UnsupportedProtocol  # httpx-specific alias
TooManyRedirects = httprs.TooManyRedirects
HTTPStatusError = httprs.HTTPStatusError
# Less-common httpx exceptions stubbed as nearest equivalent
# CookieConflict = httprs.HTTPError
# StreamConsumed = httprs.HTTPError
# StreamNotRead = httprs.HTTPError
# StreamClosed = httprs.HTTPError
# ResponseNotRead = httprs.HTTPError
# RequestNotRead = httprs.HTTPError
# LocalProtocolError = httprs.HTTPError
# RemoteProtocolError = httprs.HTTPError


class Response:
    """Proxy around httprs.Response that adds httpx-compatible attributes.

    Accepts either an existing httprs.Response (from Client.request()) or
    the same constructor args as httpx.Response for direct construction.
    httprs.Response only supports content=bytes and headers=dict, so we
    normalize text=, json=, and content=str here.
    """

    def __init__(self, inner_or_status=None, *_positional, **kwargs):
        if isinstance(inner_or_status, httprs.Response):
            object.__setattr__(self, "_inner", inner_or_status)
        else:
            import json as _json

            status_code = 200 if inner_or_status is None else int(inner_or_status)
            headers = kwargs.get("headers", {})
            if "json" in kwargs:
                content = _json.dumps(kwargs["json"]).encode()
                if isinstance(headers, dict):
                    headers = {**headers, "content-type": "application/json"}
            elif "text" in kwargs:
                content = kwargs["text"].encode("utf-8")
            elif "content" in kwargs:
                c = kwargs["content"]
                content = c.encode("utf-8") if isinstance(c, str) else c
            else:
                content = b""
            object.__setattr__(
                self,
                "_inner",
                httprs.Response(status_code, content=content, headers=headers),
            )
        object.__setattr__(self, "history", [])

    async def aread(self) -> bytes:
        """httpx async compat: httprs responses are always fully buffered."""
        return self._inner.content

    async def aiter_bytes(self, chunk_size=None):
        yield self._inner.content

    async def aiter_text(self, chunk_size=None):
        yield self._inner.text

    async def aclose(self) -> None:
        pass

    def __getattr__(self, name: str):
        return getattr(self._inner, name)

    def __setattr__(self, name: str, value):
        if name in ("history", "_inner"):
            object.__setattr__(self, name, value)
        else:
            setattr(self._inner, name, value)

    def __repr__(self) -> str:
        return repr(self._inner)

    def __bool__(self) -> bool:
        return 200 <= self._inner.status_code < 300


# httprs.Client / AsyncClient are pyo3 classes not marked `subclassable`,
# so we wrap them via composition and forward attributes.

_CLIENT_INIT_KWARGS = frozenset({
    "base_url",
    "headers",
    "timeout",
    "auth",
    "follow_redirects",
    "http2",
    "block_private_redirects",
})
_CLIENT_REQUEST_KWARGS = frozenset({
    "content",
    "json",
    "data",
    "headers",
    "auth",
    "timeout",
    "follow_redirects",
})


class Client(_HttpxClientBase):
    """httpx.Client shim: adds params= support, wraps responses, ignores
    unknown httpx-specific constructor kwargs (verify=, cert=, limits=, …).

    Inherits from real httpx.Client (when available) so isinstance checks pass
    in third-party libraries that validate http_client= arguments.
    """

    def __init__(self, *args, **kwargs):
        # Do NOT call super().__init__() — we manage state via httprs.Client.
        # Capture timeout before filtering so callers can read it back (e.g.
        # the OpenAI SDK checks `http_client.timeout` at class-body level).
        self.timeout = kwargs.get("timeout")
        filtered = {k: v for k, v in kwargs.items() if k in _CLIENT_INIT_KWARGS}
        self._client = httprs.Client(*args, **filtered)

    def request(self, method, url, *, params=None, **kwargs):
        if params:
            url = _append_query_params(url, params)
        filtered = {k: v for k, v in kwargs.items() if k in _CLIENT_REQUEST_KWARGS}
        return Response(self._client.request(method, url, **filtered))

    def get(self, url, *, params=None, **kwargs):
        return self.request("GET", url, params=params, **kwargs)

    def post(self, url, *, params=None, **kwargs):
        return self.request("POST", url, params=params, **kwargs)

    def put(self, url, *, params=None, **kwargs):
        return self.request("PUT", url, params=params, **kwargs)

    def patch(self, url, *, params=None, **kwargs):
        return self.request("PATCH", url, params=params, **kwargs)

    def delete(self, url, *, params=None, **kwargs):
        return self.request("DELETE", url, params=params, **kwargs)

    def head(self, url, *, params=None, **kwargs):
        return self.request("HEAD", url, params=params, **kwargs)

    def options(self, url, *, params=None, **kwargs):
        return self.request("OPTIONS", url, params=params, **kwargs)

    def send(self, request, **_kwargs):
        return Response(self._client.send(request))

    def close(self):
        self._client.close()

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        self.close()
        return False

    def __getattr__(self, name: str):
        return getattr(self._client, name)


_ASYNC_CLIENT_INIT_KWARGS = frozenset({
    "base_url",
    "headers",
    "timeout",
    "follow_redirects",
    "http2",
    "block_private_redirects",
})
_ASYNC_CLIENT_REQUEST_KWARGS = frozenset({
    "content",
    "json",
    "headers",
    "auth",
    "timeout",
})


class AsyncClient(_HttpxAsyncClientBase):
    """httpx.AsyncClient shim: adds params= support, wraps responses."""

    def __init__(self, *args, **kwargs):
        # Do NOT call super().__init__() — we manage state via httprs.AsyncClient.
        self.timeout = kwargs.get("timeout")
        filtered = {k: v for k, v in kwargs.items() if k in _ASYNC_CLIENT_INIT_KWARGS}
        self._client = httprs.AsyncClient(*args, **filtered)

    async def request(self, method, url, *, params=None, **kwargs):
        if params:
            url = _append_query_params(url, params)
        filtered = {
            k: v for k, v in kwargs.items() if k in _ASYNC_CLIENT_REQUEST_KWARGS
        }
        inner = await self._client.request(method, url, **filtered)
        return Response(inner)

    async def get(self, url, *, params=None, **kwargs):
        return await self.request("GET", url, params=params, **kwargs)

    async def post(self, url, *, params=None, **kwargs):
        return await self.request("POST", url, params=params, **kwargs)

    async def put(self, url, *, params=None, **kwargs):
        return await self.request("PUT", url, params=params, **kwargs)

    async def patch(self, url, *, params=None, **kwargs):
        return await self.request("PATCH", url, params=params, **kwargs)

    async def delete(self, url, *, params=None, **kwargs):
        return await self.request("DELETE", url, params=params, **kwargs)

    async def head(self, url, *, params=None, **kwargs):
        return await self.request("HEAD", url, params=params, **kwargs)

    async def options(self, url, *, params=None, **kwargs):
        return await self.request("OPTIONS", url, params=params, **kwargs)

    def close(self):
        self._client.close()

    async def aclose(self):
        self.close()

    async def __aenter__(self):
        await self._client.__aenter__()
        return self

    async def __aexit__(self, *args):
        return await self._client.__aexit__(*args)

    def __getattr__(self, name: str):
        return getattr(self._client, name)


# httprs exports these too, but they return bare httprs.Response without
# history=[].  Override to return our shim Response.


def request(method: str, url: str, **kwargs) -> Response:
    with Client() as client:
        return client.request(method, url, **kwargs)


def get(url: str, **kwargs) -> Response:
    return request("GET", url, **kwargs)


def post(url: str, **kwargs) -> Response:
    return request("POST", url, **kwargs)


def put(url: str, **kwargs) -> Response:
    return request("PUT", url, **kwargs)


def patch(url: str, **kwargs) -> Response:
    return request("PATCH", url, **kwargs)


def delete(url: str, **kwargs) -> Response:
    return request("DELETE", url, **kwargs)


def head(url: str, **kwargs) -> Response:
    return request("HEAD", url, **kwargs)


def options(url: str, **kwargs) -> Response:
    return request("OPTIONS", url, **kwargs)


class QueryParams(dict):
    """Stub for httpx.QueryParams."""

    def __init__(self, params=None, **kwargs):
        if isinstance(params, str):
            super().__init__(dict(urllib.parse.parse_qsl(params)), **kwargs)
        elif params is not None:
            super().__init__(params, **kwargs)
        else:
            super().__init__(**kwargs)

    def __str__(self) -> str:
        return urllib.parse.urlencode(list(self.items()))


class Cookies(dict):
    """Stub for httpx.Cookies."""

    def set(self, name: str, value: str, **_kwargs) -> None:
        self[name] = value


class Auth:
    """Base class stub for httpx.Auth."""

    def auth_flow(self, request):
        yield request


class Proxy:
    """Stub for httpx.Proxy."""

    def __init__(self, url, **_kwargs):
        self.url = url


USE_CLIENT_DEFAULT = object()


class BaseTransport:
    """Base class matching httpx.BaseTransport interface."""

    def handle_request(self, request):
        raise NotImplementedError

    def close(self):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        self.close()
        return False


class AsyncBaseTransport:
    """Base class matching httpx.AsyncBaseTransport interface."""

    async def handle_async_request(self, request):
        raise NotImplementedError

    async def aclose(self):
        pass

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_args):
        await self.aclose()
        return False


class _UnsupportedTransport:
    def __new__(cls, *_args, **_kwargs):
        raise NotImplementedError(
            f"{cls.__name__} is not supported by the httprs compat shim"
        )


class ASGITransport(_UnsupportedTransport):
    pass


class WSGITransport(_UnsupportedTransport):
    pass


class MockTransport(_UnsupportedTransport):
    pass


class HTTPTransport(_UnsupportedTransport):
    pass


class AsyncHTTPTransport(_UnsupportedTransport):
    pass


# Register fake httpx.* submodules so `from httpx._models import Response` etc.
# don't raise ImportError.  Populated with this module's public names.


def _register_submodule_stubs(shim_module) -> None:
    _STUB_SUBMODULES = [
        "httpx._models",
        "httpx._client",
        "httpx._exceptions",
        "httpx._types",
        "httpx._config",
        "httpx._transports",
        "httpx._transports.default",
        "httpx._auth",
        "httpx._urls",
        "httpx._content",
        "httpx._multipart",
        "httpx._decoders",
    ]
    for name in _STUB_SUBMODULES:
        if name not in sys.modules:
            stub = types.ModuleType(name)
            for attr in dir(shim_module):
                if not attr.startswith("__"):
                    try:
                        setattr(stub, attr, getattr(shim_module, attr))
                    except Exception:
                        pass
            sys.modules[name] = stub
        # Also set as attribute on the parent module so `httpx._models` works.
        parts = name.split(".")
        parent_name = ".".join(parts[:-1])
        child_attr = parts[-1]
        parent = sys.modules.get(parent_name)
        if parent is not None:
            try:
                setattr(parent, child_attr, sys.modules[name])
            except Exception:
                pass
