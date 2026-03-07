fmt:
    @uv run --no-sync ruff format --preview
    @cargo fmt

test:
    @uv run --no-sync pytest -v

bump version:
    @uv run --no-sync bump.py --version {{version}}

bench:
    @uv run \
      --no-sync \
      --with httpx \
      --with requests \
      benchmarks.py --packages httpx requests