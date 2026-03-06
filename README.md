# httprs

> **Disclaimer:** This project is under active development and is not ready for use. Do not use this project at this time.

An experimental HTTP library for Python built with Rust.

## Usage

### Install

```bash
pip install httprs
```

### Example

```python
import httprs

httprs.get("https://postman-echo.com/get").json()
```

## Goals

- Simple
- Fast
- Open

## Development

```bash
gh repo clone cnpryer/httprs
cd httprs

# Install just dependencies and build the project in development mode
uv sync --all-groups --dev --no-install-project
uvx maturin develop

# Format and check code
uv run --no-sync ruff format --preview
uv run --no-sync ruff check
cargo fmt
cargo clippy

# Run tests
uv run --no-sync pytest ./tests -v
```
