"""Thin module-name shim for ecosystem tests.

Maps ``import httpx`` to the actual ``httprs`` surface. This shim intentionally
avoids behavior emulation so compatibility work lands in httprs itself.
"""

from __future__ import annotations

import httprs
import httprs._httprs as _httprs_ext  # noqa: F401

_httprs_compat = True

for _name in dir(httprs):
    if _name.startswith("__"):
        continue
    globals()[_name] = getattr(httprs, _name)

__all__ = list(
    getattr(httprs, "__all__", [n for n in globals() if not n.startswith("_")])
)


def __getattr__(name: str):
    return getattr(httprs, name)


def __dir__() -> list[str]:
    return sorted(set(globals()) | set(dir(httprs)))
