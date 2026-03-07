fmt:
    @uv run --no-sync ruff format --preview
    @cargo fmt

test:
    @uv run --no-sync pytest -v

bump version:
    @uv run --no-sync bump.py --version {{version}}