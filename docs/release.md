# Release

## Prerequisites (one-time PyPI setup)

httprs uses [Trusted Publishing](https://docs.pypi.org/trusted-publishers/).

## How releases work

Pushing a tag matching `v*` triggers `.github/workflows/release.yml`, which runs five jobs:

| Job | Runner | What it produces |
|---|---|---|
| `linux` | `ubuntu-latest` | `manylinux` wheels for `x86_64` and `aarch64` |
| `windows` | `windows-latest` | Windows wheels for `x86_64` and `aarch64` |
| `macos` | `macos-latest` | macOS wheels for `x86_64` and `aarch64` |
| `sdist` | `ubuntu-latest` | Source distribution (`.tar.gz`) |
| `publish` | `ubuntu-latest` | Downloads all artifacts, publishes to PyPI via OIDC |

The `publish` job only runs after all build jobs succeed.

## Artifact matrix

| Platform | Architecture | Wheel tag |
|---|---|---|
| Linux (manylinux) | x86_64 | `*-manylinux_*_x86_64.whl` |
| Linux (manylinux) | aarch64 | `*-manylinux_*_aarch64.whl` |
| Windows | x86_64 | `*-win_amd64.whl` |
| Windows | aarch64 | `*-win_arm64.whl` |
| macOS | x86_64 | `*-macosx_*_x86_64.whl` |
| macOS | aarch64 | `*-macosx_*_arm64.whl` |

All wheels use the `abi3-py312` stable ABI tag, so a single wheel per platform runs on Python 3.12 and all future 3.x releases without recompiling.

## Cutting a release

1. Bump the version in both `Cargo.toml` and `pyproject.toml` to the same value (e.g. `0.0.1a0`).
   ```bash
   uv run --no-sync bump.py --version 0.0.1a0
   ```
2. Commit:
   ```bash
   git commit -am "Release v0.0.1a0"
   ```
3. Tag and push:
   ```bash
   git tag v0.0.1a0
   git push origin main v0.0.1a0
   ```
