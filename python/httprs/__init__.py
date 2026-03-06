from __future__ import annotations

from contextlib import contextmanager
from enum import IntEnum

from httprs._httprs import (  # noqa: F401
    AsyncClient,
    BasicAuth,
    Client,
    ConnectError,
    ConnectTimeout,
    DigestAuth,
    Headers,
    HTTPError,
    HTTPStatusError,
    Limits,
    NetworkError,
    ReadError,
    ReadTimeout,
    Request,
    RequestError,
    Response,
    StreamContext,
    Timeout,
    TimeoutException,
    TooManyRedirects,
    TransportError,
    UnsupportedProtocol,
    URL,
    WriteTimeout,
)
from httprs._httprs import BasicAuth as _BasicAuth
from httprs._httprs import DigestAuth as _DigestAuth


class BasicAuth(_BasicAuth):  # noqa: F811
    """HTTP Basic Authentication."""

    def sync_auth_flow(self, request: Request):
        """Single-yield generator that injects Basic Authorization header."""
        request.set_header("Authorization", self.authorization_header())
        yield request

    async def async_auth_flow(self, request: Request):
        """Async single-yield generator that injects Basic Authorization header."""
        request.set_header("Authorization", self.authorization_header())
        yield request


class DigestAuth(_DigestAuth):  # noqa: F811
    """HTTP Digest Authentication."""

    def sync_auth_flow(self, request: Request):
        """Two-step generator for Digest auth: first yields unauthenticated, then authenticated."""
        response = yield request
        if response is not None and response.status_code == 401:
            www_auth = response.headers.get("www-authenticate") or ""
            uri = request.url.path
            if request.url.query:
                uri = f"{uri}?{request.url.query}"
            auth_header = self.compute_header(request.method, uri, www_auth)
            request.set_header("Authorization", auth_header)
            yield request

    async def async_auth_flow(self, request: Request):
        """Async two-step generator for Digest auth."""
        response = yield request
        if response is not None and response.status_code == 401:
            www_auth = response.headers.get("www-authenticate") or ""
            uri = request.url.path
            if request.url.query:
                uri = f"{uri}?{request.url.query}"
            auth_header = self.compute_header(request.method, uri, www_auth)
            request.set_header("Authorization", auth_header)
            yield request


class codes(IntEnum):
    # 1xx Informational
    CONTINUE = 100
    SWITCHING_PROTOCOLS = 101
    PROCESSING = 102
    EARLY_HINTS = 103

    # 2xx Success
    OK = 200
    CREATED = 201
    ACCEPTED = 202
    NON_AUTHORITATIVE_INFORMATION = 203
    NO_CONTENT = 204
    RESET_CONTENT = 205
    PARTIAL_CONTENT = 206
    MULTI_STATUS = 207
    ALREADY_REPORTED = 208
    IM_USED = 226

    # 3xx Redirection
    MULTIPLE_CHOICES = 300
    MOVED_PERMANENTLY = 301
    FOUND = 302
    SEE_OTHER = 303
    NOT_MODIFIED = 304
    USE_PROXY = 305
    TEMPORARY_REDIRECT = 307
    PERMANENT_REDIRECT = 308

    # 4xx Client Error
    BAD_REQUEST = 400
    UNAUTHORIZED = 401
    PAYMENT_REQUIRED = 402
    FORBIDDEN = 403
    NOT_FOUND = 404
    METHOD_NOT_ALLOWED = 405
    NOT_ACCEPTABLE = 406
    PROXY_AUTHENTICATION_REQUIRED = 407
    REQUEST_TIMEOUT = 408
    CONFLICT = 409
    GONE = 410
    LENGTH_REQUIRED = 411
    PRECONDITION_FAILED = 412
    REQUEST_ENTITY_TOO_LARGE = 413
    REQUEST_URI_TOO_LONG = 414
    UNSUPPORTED_MEDIA_TYPE = 415
    REQUESTED_RANGE_NOT_SATISFIABLE = 416
    EXPECTATION_FAILED = 417
    IM_A_TEAPOT = 418
    MISDIRECTED_REQUEST = 421
    UNPROCESSABLE_ENTITY = 422
    LOCKED = 423
    FAILED_DEPENDENCY = 424
    TOO_EARLY = 425
    UPGRADE_REQUIRED = 426
    PRECONDITION_REQUIRED = 428
    TOO_MANY_REQUESTS = 429
    REQUEST_HEADER_FIELDS_TOO_LARGE = 431
    UNAVAILABLE_FOR_LEGAL_REASONS = 451

    # 5xx Server Error
    INTERNAL_SERVER_ERROR = 500
    NOT_IMPLEMENTED = 501
    BAD_GATEWAY = 502
    SERVICE_UNAVAILABLE = 503
    GATEWAY_TIMEOUT = 504
    HTTP_VERSION_NOT_SUPPORTED = 505
    VARIANT_ALSO_NEGOTIATES = 506
    INSUFFICIENT_STORAGE = 507
    LOOP_DETECTED = 508
    NOT_EXTENDED = 510
    NETWORK_AUTHENTICATION_REQUIRED = 511

    @classmethod
    def is_informational(cls, value: int) -> bool:
        return 100 <= value <= 199

    @classmethod
    def is_success(cls, value: int) -> bool:
        return 200 <= value <= 299

    @classmethod
    def is_redirect(cls, value: int) -> bool:
        return 300 <= value <= 399

    @classmethod
    def is_client_error(cls, value: int) -> bool:
        return 400 <= value <= 499

    @classmethod
    def is_server_error(cls, value: int) -> bool:
        return 500 <= value <= 599


def request(method: str, url: str, **kwargs) -> Response:
    """Make an HTTP request using a temporary Client."""
    with Client() as client:
        return client.request(method, url, **kwargs)


def get(url: str, **kwargs) -> Response:
    """Send a GET request."""
    return request("GET", url, **kwargs)


def post(url: str, **kwargs) -> Response:
    """Send a POST request."""
    return request("POST", url, **kwargs)


def put(url: str, **kwargs) -> Response:
    """Send a PUT request."""
    return request("PUT", url, **kwargs)


def patch(url: str, **kwargs) -> Response:
    """Send a PATCH request."""
    return request("PATCH", url, **kwargs)


def delete(url: str, **kwargs) -> Response:
    """Send a DELETE request."""
    return request("DELETE", url, **kwargs)


def options(url: str, **kwargs) -> Response:
    """Send an OPTIONS request."""
    return request("OPTIONS", url, **kwargs)


def head(url: str, **kwargs) -> Response:
    """Send a HEAD request."""
    return request("HEAD", url, **kwargs)


@contextmanager
def stream(method: str, url: str, **kwargs):
    """Send a request and stream the response body."""
    with Client() as client:
        with client.stream(method, url, **kwargs) as response:
            yield response


__all__ = [
    # Core classes
    "URL",
    "Headers",
    "Request",
    "Response",
    "Client",
    "AsyncClient",
    "StreamContext",
    # Config
    "Timeout",
    "Limits",
    # Auth
    "BasicAuth",
    "DigestAuth",
    # Status codes
    "codes",
    # Exceptions
    "HTTPError",
    "RequestError",
    "TransportError",
    "TimeoutException",
    "ConnectTimeout",
    "ReadTimeout",
    "WriteTimeout",
    "NetworkError",
    "ConnectError",
    "ReadError",
    "UnsupportedProtocol",
    "TooManyRedirects",
    "HTTPStatusError",
    # Functions
    "request",
    "get",
    "post",
    "put",
    "patch",
    "delete",
    "options",
    "head",
    "stream",
]
