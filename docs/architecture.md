# Architecture

## Overview

httprs is a Python extension module. The Rust crate compiles to a shared library (`_httprs.abi3.so`) that Python imports directly. A thin Python layer re-exports symbols and adds Python-specific conveniences.

```
Python caller
     |
     v
httprs/__init__.py          (Python: re-exports, convenience wrappers)
     |
     v
httprs/_httprs.abi3.so      (Rust extension, compiled by maturin)
     |
     v
reqwest + tokio             (Rust HTTP + async runtime)
```

---

## Build system

[maturin](https://www.maturin.rs/) is the build backend. It compiles the Rust crate and packages the resulting `.so` alongside the Python source in a wheel.

Key configuration in `pyproject.toml`:

```toml
[tool.maturin]
python-source = "python"        # Python package root
module-name = "httprs._httprs"  # Import path of the extension
features = ["pyo3/extension-module"]
```

The `abi3-py312` feature in `Cargo.toml` produces a stable ABI wheel (`cp312-abi3`) that runs on Python 3.12+:

```toml
pyo3 = { version = "0.28.2", features = ["extension-module", "abi3-py312"] }
```

---

## Rust module structure

### `src/lib.rs`

Entry point for the PyO3 module. Responsibilities:

- Declares the exception hierarchy using `create_exception!`
- Registers all classes and exceptions in the `_httprs` module via `#[pymodule]`
- Provides three shared utilities used across modules:

| Symbol | Purpose |
|---|---|
| `sync_runtime()` | Returns a lazily-initialized `tokio::runtime::Runtime` (multi-thread) used by `PyClient` to drive async reqwest internally |
| `run_blocking(fut)` | Spawns a future on `SYNC_RUNTIME` and blocks the calling thread via a channel — used for streaming response reads |
| `without_gil(f)` | Releases the Python GIL while executing a blocking closure (`PyEval_SaveThread` / `PyEval_RestoreThread`) |
| `map_reqwest_error(e)` | Maps `reqwest::Error` variants to the appropriate Python exception subclass |

### `src/client.rs`

Contains three pyclass types:

**`PyClient` (exposed as `Client`)**

- Wraps `reqwest::blocking::Client`
- `request()` is the core method; all HTTP verb methods delegate to it
- GIL is released during I/O via `without_gil(|| builder.send())`
- Digest auth uses a two-pass strategy: sends unauthenticated, checks for 401, then retries with computed `Authorization` header
- `stream()` returns a `PyStreamContext`
- Implements `__enter__`/`__exit__` for use as a context manager
- `close()` drops the inner client by setting `inner = None`

**`PyStreamContext` (exposed as `StreamContext`)**

- Created by `Client.stream()`; implements `__enter__`/`__exit__`
- `__enter__` sends the request and returns a `PyResponse` whose body is backed by a live `reqwest::blocking::Response` (wrapped in `Arc<Mutex<Option<ResponseStream>>>`)
- `__exit__` drops the response to close the connection

**`PyAsyncClient` (exposed as `AsyncClient`)**

- Wraps `reqwest::Client` (async)
- Each request method calls `pyo3_async_runtimes::tokio::future_into_py` to bridge the Rust future into a Python awaitable
- Implements `__aenter__`/`__aexit__`
- Does not currently support `data` (form bodies) or `DigestAuth`

### `src/models.rs`

**`PyURL`** — Thin wrapper around `url::Url`. All URL parsing/validation occurs in Rust.

**`PyHeaders`** — Stores `Vec<(String, String)>` with lowercase names. Accepts `dict`, `list[tuple]`, or any Python iterable of 2-tuples. Implements `__getitem__`, `__contains__`, `__iter__`, `__len__`.

**`PyRequest`** — Holds method (uppercased), URL, headers, and raw content bytes. Used by `Client.build_request()` / `Client.send()`.

**`PyResponse`** — Holds all response metadata and either an eager body (`Vec<u8>`) or a lazy stream (`Arc<Mutex<Option<ResponseStream>>>`). `read()`, `iter_bytes()`, and `iter_text()` drain the stream. JSON parsing is done via `serde_json` in Rust, then converted to Python objects recursively by `json_to_python`.

**`ResponseStream`** — Enum wrapping either `reqwest::blocking::Response` or `reqwest::Response`. Shared via `Arc<Mutex<Option<...>>>` between `PyStreamContext` and `PyResponse`. Marked `unsafe impl Sync` because access is serialized through the `Mutex`.

### `src/auth.rs`

**`PyBasicAuth`** — Precomputes the `Authorization: Basic <base64>` header value at construction time.

**`PyDigestAuth`** — Implements RFC 7616 Digest authentication. On `compute_header()`:
1. Parses the `WWW-Authenticate: Digest ...` challenge
2. Computes HA1 = hash(username:realm:password), HA2 = hash(method:uri)
3. Computes response hash per qop mode (`auth` or legacy)
4. Supports MD5 (default) and SHA-256 algorithms
5. Tracks nonce count in `Mutex<DigestState>` for replay protection

Both types are subclassed in Python (`python/httprs/__init__.py`) to add `sync_auth_flow` and `async_auth_flow` generator protocols.

### `src/config.rs`

**`PyTimeout`** — Stores `connect`, `read`, `write`, and `pool` timeouts as `Option<f64>` seconds. A single positional argument sets all four simultaneously.

**`PyLimits`** — Configuration type for connection pool limits. Stored but not yet wired into the client builder.

---

## Python layer (`python/httprs/__init__.py`)

The Python file does four things:

1. **Re-exports** all Rust types from `httprs._httprs` into the `httprs` namespace
2. **Subclasses** `BasicAuth` and `DigestAuth` to add `sync_auth_flow` / `async_auth_flow` generator protocols (for potential middleware-style auth flows)
3. **Defines `codes`** — an `IntEnum` with all standard HTTP status codes and classification helpers
4. **Defines convenience functions** — `get`, `post`, `put`, `patch`, `delete`, `head`, `options`, `request`, `stream` — each creates a temporary `Client` and tears it down

---

## GIL management

Because `reqwest::blocking` uses a dedicated OS thread pool internally, blocking on I/O while holding the GIL would prevent other Python threads from running. Every blocking call releases the GIL:

```rust
// In client.rs — any blocking send
let result = crate::without_gil(|| builder.send());

// In models.rs — reading the response body
let content = crate::without_gil(|| resp.bytes())
```

`without_gil` is a safe wrapper around the unsafe `PyEval_SaveThread` / `PyEval_RestoreThread` FFI pair. It must only be called from a `#[pymethods]` function while the GIL is held.

---

## Async/sync interop

`PyClient` uses `reqwest::blocking` which does its own internal async execution. It does **not** use `SYNC_RUNTIME` for normal requests. `SYNC_RUNTIME` is reserved for cases where an async future must be driven from a synchronous `#[pymethods]` context — currently used only in `Response.read()` when the underlying stream is an async `reqwest::Response`.

`PyAsyncClient` uses `pyo3_async_runtimes::tokio::future_into_py`, which hooks into the tokio runtime managed by `pyo3-async-runtimes`. This is a separate runtime from `SYNC_RUNTIME`.

---

## Error mapping

`map_reqwest_error` in `lib.rs` converts `reqwest::Error` to the Python exception hierarchy:

| reqwest error kind | Python exception |
|---|---|
| `is_timeout()` | `TimeoutException` |
| `is_redirect()` | `TooManyRedirects` |
| `is_connect()` | `ConnectError` |
| `is_builder()` | `UnsupportedProtocol` |
| `is_request()` | `UnsupportedProtocol` |
| `is_body()` / `is_decode()` | `ReadError` |
| other | `RequestError` |
