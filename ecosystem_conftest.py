"""pytest plugin injected into cloned ecosystem repos.

Primary compat injection happens in check_ecosystem.py by placing a temporary
"httpx" package on PYTHONPATH. This plugin is a minimal safety net that only
loads the shim if real httpx was imported first.
"""

from __future__ import annotations

import importlib.util
import os
import sys
from pathlib import Path

# Locate the compat shim: env var (set by check_ecosystem.py) or sibling file.
_SHIM_PATH = os.environ.get(
    "HTTPRS_COMPAT_SHIM",
    str(Path(__file__).parent / "httpx_compat.py"),
)


def _patch_httpx() -> None:
    """Patch sys.modules['httpx'] with the httprs compat shim."""
    existing = sys.modules.get("httpx")
    if existing is not None and getattr(existing, "_httprs_compat", False):
        return

    shim_path = Path(_SHIM_PATH)
    if not shim_path.exists():
        import warnings

        warnings.warn(
            f"httprs compat shim not found at {shim_path}; httpx will not be patched",
            stacklevel=1,
        )
        return

    spec = importlib.util.spec_from_file_location("httpx", shim_path)
    shim = importlib.util.module_from_spec(spec)
    shim.__file__ = str(shim_path)
    shim.__package__ = "httpx"

    try:
        spec.loader.exec_module(shim)
    except Exception as exc:
        import warnings

        warnings.warn(
            f"Failed to load httprs compat shim: {exc}",
            stacklevel=1,
        )
        return

    sys.modules["httpx"] = shim


def pytest_load_initial_conftests(early_config, parser, args) -> None:
    """Patch as early as possible in pytest startup."""
    _patch_httpx()


def pytest_configure(config) -> None:
    """Safety net in case early hooks were bypassed."""
    _patch_httpx()


# Import-time patching is intentional as a fallback if any plugin imports
# real httpx before pytest hooks execute.
_patch_httpx()
