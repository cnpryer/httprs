# Development

## Prerequisites

- Rust stable toolchain (`rustup`)
- [uv](https://github.com/astral-sh/uv) (Python package manager)
- Python 3.12+

## Setup

```bash
gh repo clone cnpryer/httprs
cd httprs
```

Use `just` to run development tasks:

```
just sync
just fmt
just fix
just test
```

## Tools

- [just](https://github.com/casey/just)
- [uv](https://github.com/astral-sh/uv)
- [rustup](https://rustup.rs/)

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

## Ecosystem tests

Ecosystem tests measure real-world httpx API compatibility by cloning third-party repos, running their test suites twice — once against real httpx (baseline) and once with httprs substituted via a compatibility shim — and reporting which tests regress.

### Scripts

| File | Purpose |
|---|---|
| `check_ecosystem.py` | Main orchestrator: clones repos, manages venvs, runs pytest, prints report |
| `httpx_compat.py` | httpx → httprs compatibility shim (materialized as a temporary `httpx` package on `PYTHONPATH`) |
| `ecosystem_conftest.py` | minimal pytest safety-net plugin that ensures the shim is loaded if real httpx was imported first |

### Target repos

| Key | Repo |
|---|---|
| `anthropic` | `anthropics/anthropic-sdk-python` |
| `openai` | `openai/openai-python` |

### Quick start

```bash
# Build the extension first
uvx maturin develop

# Smoke test: one repo, no baseline, short timeout
just ecosystem --repos openai --no-baseline --timeout 120

# Full baseline vs. experiment comparison
just ecosystem -v

# Skip the build step with a pre-built wheel
just ecosystem --httprs-wheel target/wheels/httprs-*.whl
```

### How it works

1. **Clone** each repo at the pinned ref (reuses existing checkouts unless `--clean`)
2. **Create `.venv-ecosystem`** inside the checkout, install the repo's dev dependencies plus the httprs wheel
3. **Baseline run**: pytest with real httpx (no shim)
4. **Experiment run**:
   - create a temporary `httpx` package (`httpx/__init__.py`) from `httpx_compat.py`
   - prepend that directory to `PYTHONPATH` so all `import httpx` resolve to httprs compat
   - inject `ecosystem_conftest.py` via `-p _httprs_compat` as a fallback safety net
5. **Diff**: `regressions = experiment_failing − baseline_failing`; always exits with code `0` (report only)

### Current exclusions

Only explicit path-based excludes are applied:

- **Live API tests** (`tests/api_resources/`): both SDKs' `api_resources` directories make real network calls; excluded via `--ignore=tests/api_resources`

### Intentional strictness

- No fake `httpx._*` private submodules are registered.
- No respx-specific module rewiring is performed in the harness.
- Regressions are expected to surface as hard failures, then be fixed in Rust bindings/behavior.

### Triage policy (current)

#### Downstream compatibility exceptions

- Ignore `respx`-related ecosystem failures for now.
- Failures are considered `respx`-based if traces mention `respx/plugin.py`, `respx/router.py`, `RESPX: some routes were not called!`, or connection errors to `127.0.0.1:4010`.
- Focus on regressions unrelated to `respx` (e.g., API, headers, cookies, streams, retries).
- Address `respx` compatibility only in a future transport-interception phase.

#### Minimal shim scope

- The shim is module-name routing only (`import httpx` resolves to `httprs`); it does not emulate `httpx` behavior.
- `respx` interception parity is out of scope in the normal ecosystem loop; treat those failures as excluded from day-to-day compatibility work.
- `ASGITransport` and `WSGITransport` are not implemented in `httprs` and currently raise `NotImplementedError`.
- Private `httpx` internals (`httpx._*`) are intentionally not mirrored; compatibility work targets public API/behavior only.

### Development loop

1. Run `just ecosystem --repos <repo> --timeout <seconds> -v`
2. Group regressions by root cause (API surface vs. behavior vs. transport/mocking)
3. Implement fixes in Rust (`src/*.rs`) and keep `python/httprs/__init__.py` as a thin export wrapper
4. Rebuild extension with `uvx maturin develop` and rerun the same ecosystem command
5. Repeat until the regression group is reduced or eliminated

### CLI reference

```
just ecosystem [options]
# or: python check_ecosystem.py [options]

  --checkouts-dir PATH   Directory for repo checkouts (default: /tmp/httprs-ecosystem)
  --repos REPO …         Subset to run: openai, anthropic (default: all)
  --httprs-wheel PATH    Use a pre-built wheel instead of running maturin build
  --no-baseline          Skip the httpx baseline; only report httprs pass/fail counts
  --clean                Wipe existing checkouts before cloning
  --timeout SECONDS      Per-repo pytest timeout (default: 600)
  --concurrency N        Max repos processed in parallel (default: 2)
  -v, --verbose          Print all failing test IDs
```

---

## Linting and formatting

```bash
# Python
uv run --no-sync ruff format --preview   # format
uv run --no-sync ruff check              # lint
uv run --no-sync ruff check --fix        # lint + autofix

# Rust
cargo fmt      # format
cargo clippy   # lint
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

- Follow the [PyO3 guide](https://pyo3.rs/) for binding patterns and signatures.
- Keep behavior in Rust (`src/*.rs`), and keep `python/httprs/__init__.py` as a thin re-export wrapper.
- Rebuild the extension after wheel-impacting changes: `uvx maturin develop`.
- Validate with `uv run --no-sync pytest ./tests`.
- Keep package layout stable: Python source root is `python/`, extension module is `httprs._httprs`.

---

## Dependency management

- Use [uv documentation](https://docs.astral.sh/uv/) as the source of truth for Python environment and dependency workflows.
- Add Rust dependencies in `Cargo.toml`, then run `cargo build` to update `Cargo.lock`.
- Add Python dev dependencies to the `dev` group in `pyproject.toml` (for example: `uv add --group dev <package>`).

---

## Publishing

See [Release](release.md) for the automated release pipeline and PyPI publishing process.

---

## Key invariants to maintain

- Release the GIL around blocking network/body I/O (`crate::without_gil(...)`).
- Only call `without_gil` while already holding the GIL; do not nest it.
- Use `run_blocking` only from synchronous `#[pymethods]` that must drive async work.
- Keep header keys normalized to lowercase for lookup/update behavior.
- Keep Python wrapper logic minimal; implement API behavior in Rust first.
