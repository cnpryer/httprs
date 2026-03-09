dbuild:
    @uvx maturin develop

sync:
    @uv sync --all-groups --dev

fmt:
    @uv run --no-sync ruff format --preview
    @cargo fmt

fix:
    @uv run --no-sync ruff check --fix

test: dbuild
    @uv run --no-sync pytest -v

bump version:
    @uv run --no-sync bump.py --version {{version}}
    @uv lock

bench:
    @uvx maturin develop --release
    @uv run --no-sync --with httpx --with requests benchmark.py --packages httpx requests -n 5000

ecosystem *args: dbuild
    @uv run --no-sync python check_ecosystem.py {{args}}
