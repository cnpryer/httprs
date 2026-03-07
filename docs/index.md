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

### Preliminary results

> Disclaimer: Robust tests have not been conducted. See `benchmarks.py`.

On a MacBook Pro M1 Max, preliminary benchmarks show httprs is ~2.2x faster than httpx and ~3.7x faster than requests on a variety of common request types.

No conclusions should be drawn from these results until robust testing is completed. **This project is experimental and not ready for production use.**

```
get  (n=10000)
  package      mean   median    stdev      min      max  (ms)    ratio
  --------------------------------------------------------------------
  httprs      0.130    0.128    0.098    0.093    9.534
  httpx       0.291    0.275    0.207    0.263   12.659    2.23x slower
  requests    0.489    0.494    0.467    0.407   34.744    3.75x slower

post_bytes  (n=10000)
  package      mean   median    stdev      min      max  (ms)    ratio
  --------------------------------------------------------------------
  httprs      0.134    0.133    0.114    0.101   11.293
  httpx       0.306    0.293    0.183    0.278   10.336    2.28x slower
  requests    0.504    0.493    0.419    0.414   26.584    3.77x slower

post_json  (n=10000)
  package      mean   median    stdev      min      max  (ms)    ratio
  --------------------------------------------------------------------
  httprs      0.142    0.140    0.178    0.110   17.876
  httpx       0.319    0.308    0.181    0.288   10.038    2.25x slower
  requests    0.514    0.500    0.352    0.461   16.916    3.62x slower

post_form  (n=10000)
  package      mean   median    stdev      min      max  (ms)    ratio
  --------------------------------------------------------------------
  httprs      0.142    0.137    0.261    0.108   19.032
  httpx       0.322    0.309    0.202    0.281   12.841    2.27x slower
  requests    0.513    0.503    0.309    0.467   16.525    3.62x slower
```

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
