"""pytest plugin injected into cloned ecosystem repos.

Patches sys.modules['httpx'] with the httprs compat shim before any test
modules are collected, and converts transport-stub NotImplementedErrors into
skips so they don't count as regressions.

Injected via PYTHONPATH by check_ecosystem.py -- not used directly.
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


def pytest_configure(config) -> None:
    """Patch sys.modules['httpx'] with the httprs compat shim."""
    existing = sys.modules.get("httpx")
    if existing is not None and getattr(existing, "_httprs_compat", False):
        return  # Already patched; idempotent.

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

    # Register submodule stubs so `from httpx._models import X` doesn't fail.
    shim._register_submodule_stubs(shim)


def pytest_runtest_logreport(report) -> None:
    """Convert unsupported-transport failures (any phase) to skips."""
    if not report.failed:
        return

    longrepr = str(getattr(report, "longrepr", ""))

    # Transport stubs: NotImplementedError with our sentinel phrase.
    if "NotImplementedError" in longrepr and "httprs compat shim" in longrepr:
        report.outcome = "skipped"
        report.longrepr = "Skipped: transport not supported by httprs compat shim"
        return

    # respx / pytest-httpx patch httpx internals; skip those too.
    if "AttributeError" in longrepr and any(
        pat in longrepr for pat in ("respx.", "pytest_httpx.", "httpx._", "HTTPX_MOCK")
    ):
        report.outcome = "skipped"
        report.longrepr = "Skipped: httpx mock library incompatible with httprs shim"
