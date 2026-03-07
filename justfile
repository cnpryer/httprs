sync:
    @uv sync --all-groups --dev

fmt:
    @uv run --no-sync ruff format --preview
    @cargo fmt

fix:
    @uv run --no-sync ruff check --fix

test:
    @uv run --no-sync pytest -v

bump version:
    @uv run --no-sync bump.py --version {{version}}

bench:
    @uvx maturin develop --release
    @uv run --no-sync --with httpx --with requests benchmarks.py --packages httpx requests -n 200
