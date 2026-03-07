# httprs

An experimental HTTP library for Python built with Rust.

> **Status:** Pre-alpha. Not ready for production use.

## What it is

httprs is a Python package distributed as a compiled wheel. The HTTP logic is written in Rust using [reqwest](https://docs.rs/reqwest) and [tokio](https://tokio.rs/), and exposed to Python via [PyO3](https://pyo3.rs/). The Python layer (`python/httprs/__init__.py`) re-exports the Rust types and adds thin convenience wrappers.

## Goals

- Simple — familiar API inspired by httpx/requests
- Fast — Rust core, GIL released during I/O
- Open — MIT licensed, no required dependencies at runtime

## Plan

1. \>99% tested and measured parity with httpx and requests on core API usage.
2. Continuously benchmark against popular HTTP libraries with full transparency.
3. Minimize complexity and create an intuitive and inviting experience for developers.

## Quick start

```python
import httprs

# One-liner
response = httprs.get("https://example.com")
print(response.status_code, response.text)

# Reusable client
with httprs.Client() as client:
    r = client.post("/api/data", json={"key": "value"})
    r.raise_for_status()

# Async
async with httprs.AsyncClient(base_url="https://api.example.com") as client:
    r = await client.get("/users")
```

## Documentation

| Document | Description |
|---|---|
| [usage.md](usage.md) | Full Python API with examples |
| [architecture.md](architecture.md) | Codebase layout and Rust/Python interop |
| [development.md](development.md) | Setup, build, test, and contribution guide |

## Repository layout

```
httprs/
  src/               # Rust source
    lib.rs           # PyO3 module entry, exceptions, runtime helpers
    client.rs        # Client, AsyncClient, StreamContext
    models.rs        # URL, Headers, Request, Response
    auth.rs          # BasicAuth, DigestAuth
    config.rs        # Timeout, Limits
  python/
    httprs/
      __init__.py    # Python wrapper layer, convenience functions
      _httprs.abi3.so  # Compiled extension (generated)
  tests/             # pytest integration tests
  Cargo.toml         # Rust manifest
  pyproject.toml     # Python manifest (maturin build backend)
```
