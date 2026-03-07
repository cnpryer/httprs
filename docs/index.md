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

> Disclaimer: Robust tests have not been conducted. See `benchmark.py`.

Benchmarks on a MacBook Pro M1 Max show httprs ~2.3x faster than httpx and ~3.7x faster than requests.

These loopback tests measure overhead only. Real networks are latency bound. Experimental, not production ready.

```
get  (n=10000)
  package      mean   median    stdev      min      max  (ms)    ratio
  --------------------------------------------------------------------
  httprs      0.131    0.129    0.057    0.092    5.667
  httpx       0.309    0.283    0.199    0.248   12.549    2.37x slower
  requests    0.500    0.495    0.320    0.391   16.877    3.83x slower

post_bytes  (n=10000)
  package      mean   median    stdev      min      max  (ms)    ratio
  --------------------------------------------------------------------
  httprs      0.134    0.133    0.082    0.107    8.245
  httpx       0.325    0.305    0.172    0.279    9.664    2.42x slower
  requests    0.505    0.495    0.353    0.462   16.414    3.76x slower

post_json  (n=10000)
  package      mean   median    stdev      min      max  (ms)    ratio
  --------------------------------------------------------------------
  httprs      0.144    0.141    0.209    0.097   15.847
  httpx       0.323    0.309    0.180    0.288    9.279    2.24x slower
  requests    0.513    0.502    0.345    0.466   16.376    3.57x slower

post_form  (n=10000)
  package      mean   median    stdev      min      max  (ms)    ratio
  --------------------------------------------------------------------
  httprs      0.138    0.137    0.164    0.086   16.439
  httpx       0.322    0.309    0.157    0.287    9.191    2.33x slower
  requests    0.516    0.505    0.348    0.462   16.228    3.73x slower
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
