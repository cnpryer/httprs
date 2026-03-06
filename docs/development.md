# Development

## Prerequisites

- Rust stable toolchain (`rustup`)
- [uv](https://github.com/astral-sh/uv) (Python package manager)
- Python 3.12+

## Setup

```bash
gh repo clone cnpryer/httprs
cd httprs

# Install Python dev dependencies (no project install yet)
uv sync --all-groups --dev --no-install-project

# Compile the Rust extension and install it in editable mode
uvx maturin develop
```

After `maturin develop`, `import httprs` works from the repo root using the compiled `.so` in `python/httprs/`.

---

## Build

### Development build (fast, no optimizations)

```bash
uvx maturin develop
```

### Release build (LTO + single codegen unit)

```bash
uvx maturin develop --release
# or build a wheel:
uvx maturin build --release
```

The release profile in `Cargo.toml` enables `lto = true` and `codegen-units = 1` for maximum performance.

---

## Running tests

```bash
uv run --no-sync pytest ./tests -v
```

Tests spin up a local ASGI server (uvicorn + a hand-written ASGI app in `tests/conftest.py`) on an ephemeral port. The server fixture is session-scoped, so it starts once per test run.

Run a specific test file:

```bash
uv run --no-sync pytest tests/test_client.py -v
```

Run with extra verbosity (shows captured output):

```bash
uv run --no-sync pytest ./tests -vv
```

---

## Linting and formatting

```bash
# Python
uv run --no-sync ruff format --preview        # format
uv run --no-sync ruff check         # lint
uv run --no-sync ruff check --fix   # lint + autofix

# Rust
cargo fmt                           # format
cargo clippy                        # lint
```

CI enforces `ruff format --preview --check` and `cargo fmt --check` on every PR.

---

## CI

Defined in `.github/workflows/ci.yml`. Three jobs:

| Job | What it does |
|---|---|
| `python-test` | Builds the extension with maturin, runs pytest on Ubuntu and Windows with Python 3.12 |
| `python-check` | Runs `ruff check` and `ruff format --preview --check` on Python 3.14 |
| `rust-check` | Runs `cargo fmt --check` |

CI triggers on push to `main` and on all pull requests.

---

## Making changes

### Adding a new Python-visible method to an existing class

1. Add the method to the relevant `impl` block in the appropriate `src/*.rs` file
2. Annotate it with `#[pyo3(signature = (...))]` to define the Python calling convention
3. Run `uvx maturin develop` to recompile
4. Add tests in `tests/`

Example — adding `Client.trace()`:

```rust
// src/client.rs, inside #[pymethods] impl PyClient
#[pyo3(signature = (url, *, content = None, json = None, data = None, headers = None, auth = None, timeout = None, follow_redirects = None))]
pub fn trace(&self, py: Python<'_>, url: &str, ...) -> PyResult<PyResponse> {
    self.request(py, "TRACE", url, ...)
}
```

If the method also needs to be accessible as a top-level function, add it to `python/httprs/__init__.py`.

### Adding a new exception

1. Add `create_exception!(httprs, MyError, ParentError, "doc string.");` in `src/lib.rs`
2. Register it in `_httprs()`: `m.add("MyError", m.py().get_type::<MyError>())?;`
3. Re-export from `python/httprs/__init__.py`
4. Add to `map_reqwest_error` if it should be raised automatically from reqwest errors

### Adding a new top-level class

1. Create a `#[pyclass]` struct in the appropriate `src/*.rs` file
2. Implement `#[pymethods]` for it
3. Add `m.add_class::<YourClass>()?;` in `_httprs()` in `src/lib.rs`
4. Re-export from `python/httprs/__init__.py` and add to `__all__`

### Changing the Python package structure

The Python source root is `python/` (configured via `[tool.maturin] python-source = "python"`). The extension module is placed at `httprs._httprs` (`module-name = "httprs._httprs"`). Do not move these without updating both `pyproject.toml` and `python/httprs/__init__.py`.

---

## Dependency management

### Rust

Add dependencies to `Cargo.toml`. Prefer features on existing crates over new crates. After adding, run `cargo build` to update `Cargo.lock`.

### Python

Dev dependencies are in `[dependency-groups] dev` in `pyproject.toml`. httprs has no runtime Python dependencies (`dependencies = []`).

To add a dev dependency:

```bash
uv add --dev <package>
```

---

## Publishing

### PyPI (Python wheel)

```bash
uvx maturin publish
```

maturin builds wheels for the target platform and uploads them to PyPI. For multi-platform releases, use maturin's GitHub Actions integration (`PyO3/maturin-action`) to build on each target OS.

The wheel targets `cp312-abi3` (stable ABI), so a single wheel built on a given OS runs on Python 3.12 through 3.x without recompiling.

### crates.io (Rust crate)

```bash
cargo publish
```

The crate is `cdylib`-only and not useful as a Rust library dependency, but publishing keeps the crate name reserved.

---

## Key invariants to maintain

- **GIL must be released during all blocking I/O.** Any `builder.send()`, `resp.bytes()`, or similar call must be wrapped in `crate::without_gil(|| ...)`. Failing to do this will deadlock when a local Python server (e.g., the test server) needs to run on the same thread.

- **`without_gil` must only be called while holding the GIL.** It is not safe to call from a spawned thread or from inside another `without_gil` closure.

- **`ResponseStream` uses `unsafe impl Sync`.** This is sound because the `Mutex` ensures exclusive access. Do not share `ResponseStream` without the mutex.

- **`run_blocking` spawns on `SYNC_RUNTIME`.** Do not use it from an async context (deadlock risk). It is only for driving async futures from synchronous `#[pymethods]`.

- **Header names are lowercased at ingestion.** `PyHeaders::from_pyobject`, `from_reqwest`, and `from_vec` all lowercase keys. Methods that accept a key (`get`, `__getitem__`, `__contains__`) also lowercase before comparison. Maintain this invariant for any new header code.

- **The Python `BasicAuth`/`DigestAuth` subclasses shadow the Rust classes in `__init__.py`.** The Rust base classes are imported as `_BasicAuth`/`_DigestAuth` and the Python subclasses replace them. If a new auth type is added to Rust, follow the same pattern.
