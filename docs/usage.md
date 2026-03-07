# Experimenting with httprs

## Installation

```bash
pip install --pre httprs
```

Requires Python 3.12+. The wheel bundles the Rust runtime — no separate Rust installation needed at runtime.

---

## Module-level functions

The simplest way to make requests. Each call opens a temporary `Client`, makes the request, and closes the client.

```python
import httprs

httprs.get(url, **kwargs)
httprs.post(url, **kwargs)
httprs.put(url, **kwargs)
httprs.patch(url, **kwargs)
httprs.delete(url, **kwargs)
httprs.head(url, **kwargs)
httprs.options(url, **kwargs)
```

All functions accept the same keyword arguments as `Client.request`.

---

## Client

`Client` is a synchronous HTTP client backed by `reqwest::blocking`. It is safe to reuse across multiple requests and supports use as a context manager.

```python
client = httprs.Client(
    base_url=None,  # Prepended to relative URLs
    headers=None,  # Default headers (dict or list of tuples)
    timeout=None,  # Default timeout (float seconds or Timeout object); defaults to 5.0s
    auth=None,  # Default auth (BasicAuth, DigestAuth, or (user, pass) tuple)
    follow_redirects=True,
    http2=False,  # Reserved, not yet active
)
```

### Making requests

```python
with httprs.Client(base_url="https://api.example.com") as client:
    # GET
    r = client.get("/users")

    # POST with JSON body
    r = client.post("/users", json={"name": "Alice"})

    # POST with form data
    r = client.post("/login", data={"user": "alice", "pass": "secret"})

    # POST with raw bytes
    r = client.post("/upload", content=b"\x89PNG...")

    # Per-request headers
    r = client.get("/private", headers={"X-Token": "abc"})

    # Per-request timeout (seconds)
    r = client.get("/slow", timeout=30.0)

    # Per-request auth
    r = client.get("/secure", auth=httprs.BasicAuth("user", "pass"))
```

### Low-level request/send

```python
req = client.build_request("GET", "/data", headers={"Accept": "application/json"})
req.set_header("X-Extra", "value")
response = client.send(req)
```

### Streaming

```python
with client.stream("GET", "/large-file") as response:
    data = response.read()  # read full body
    # or iterate:
    for chunk in response.iter_bytes():
        process(chunk)
    for text in response.iter_text():
        print(text)
```

Module-level shortcut:

```python
with httprs.stream("GET", url) as response:
    body = response.read()
```

---

## AsyncClient

`AsyncClient` is an asynchronous HTTP client backed by `reqwest` (async) via `pyo3-async-runtimes/tokio`. It must be used with `async`/`await`.

```python
async with httprs.AsyncClient(
    base_url=None,
    headers=None,
    timeout=None,
    follow_redirects=True,
    http2=False,
) as client:
    r = await client.get("/users")
    r = await client.post("/users", json={"name": "Bob"})
```

Available methods: `get`, `post`, `put`, `patch`, `delete`, `head`, `options`, `request`.

The async client currently accepts `content`, `json`, `headers`, `auth` (Basic only), and `timeout` (float seconds). `data` (form encoding) is not yet supported on `AsyncClient`.

---

## Response

All request methods return a `Response` object.

```python
r = httprs.get("https://example.com")

r.status_code  # int, e.g. 200
r.reason_phrase  # str, e.g. "OK"
r.http_version  # str, e.g. "HTTP/1.1"
r.headers  # Headers object
r.content  # bytes
r.text  # str (UTF-8 decoded)
r.encoding  # Optional[str], charset from Content-Type
r.url  # URL object
r.elapsed  # datetime.timedelta
r.is_redirect  # bool
r.request  # Optional[Request] that produced this response

r.json()  # parse body as JSON, returns Python object
r.raise_for_status()  # raises HTTPStatusError for 4xx/5xx

# Streaming responses only
r.read()  # bytes — reads and buffers the full body
r.iter_bytes()  # list[bytes] — 64 KB chunks
r.iter_text()  # list[str] — 64 KB chunks decoded as UTF-8
```

---

## Headers

`Headers` stores header name/value pairs. Names are normalized to lowercase.

```python
h = httprs.Headers({"Content-Type": "application/json", "X-Custom": "value"})
# Also accepts list of tuples: [("content-type", "application/json")]

h.get("content-type")  # "application/json"
h.get("missing", "default")  # "default"
h["content-type"]  # "application/json" (raises KeyError if missing)
"content-type" in h  # True
len(h)  # 2
h.keys()  # ["content-type", "x-custom"]
h.values()  # ["application/json", "value"]
h.items()  # [("content-type", "application/json"), ...]
h.update({"X-New": "added"})
list(h)  # iterate as (name, value) tuples
```

---

## URL

```python
u = httprs.URL("https://example.com:8080/path?q=1#frag")

u.scheme  # "https"
u.host  # "example.com"
u.port  # 8080
u.path  # "/path"
u.query  # "q=1"
u.fragment  # "frag"
u.netloc  # "example.com:8080"
str(u)  # full URL string

# Non-destructive mutation
u2 = u.copy_with(path="/other", query=None)
```

---

## Request

```python
req = httprs.Request(
    "POST",
    "https://example.com/api",
    headers={"Content-Type": "application/json"},
    content=b'{"x":1}',
)

req.method  # "POST"
req.url  # URL object
req.headers  # Headers object
req.content  # bytes

req.set_header("Authorization", "Bearer token")
```

---

## Authentication

### BasicAuth

```python
auth = httprs.BasicAuth("alice", "s3cret")
r = httprs.get(url, auth=auth)

# Shorthand tuple (converted to BasicAuth automatically)
r = httprs.get(url, auth=("alice", "s3cret"))
```

### DigestAuth

```python
auth = httprs.DigestAuth("alice", "s3cret")
r = httprs.get(url, auth=auth)
```

Digest auth performs a two-pass exchange: the first request is sent without credentials; when a 401 is returned the `WWW-Authenticate` challenge is parsed and the authenticated request is retried automatically. Supports MD5 and SHA-256 algorithms with `qop=auth`.

---

## Timeout

```python
# Single value applies to connect and read
t = httprs.Timeout(5.0)

# Fine-grained
t = httprs.Timeout(connect=3.0, read=10.0, write=5.0, pool=1.0)

# Per-request override
r = httprs.get(url, timeout=httprs.Timeout(read=30.0))
r = httprs.get(url, timeout=60.0)  # float shorthand
```

---

## Limits

```python
limits = httprs.Limits(
    max_connections=100,
    max_keepalive_connections=20,
    keepalive_expiry=5.0,
)
```

`Limits` is available as a configuration type. It is not yet wired into the client builder.

---

## Exceptions

```
HTTPError
  RequestError
    TransportError
      TimeoutException
        ConnectTimeout
        ReadTimeout
        WriteTimeout
      NetworkError
        ConnectError
        ReadError
      UnsupportedProtocol
    TooManyRedirects
  HTTPStatusError
```

```python
import httprs

try:
    r = httprs.get(url)
    r.raise_for_status()
except httprs.ConnectTimeout:
    print("Connection timed out")
except httprs.HTTPStatusError as e:
    print(f"HTTP {e}")
except httprs.HTTPError as e:
    print(f"Request failed: {e}")
```

---

## Status codes

```python
httprs.codes.OK  # 200
httprs.codes.NOT_FOUND  # 404
httprs.codes.is_success(200)  # True
httprs.codes.is_client_error(404)  # True
```
