"""Shared diagnostics operations."""
from __future__ import annotations

from collections.abc import Callable
from typing import Any

from .backend_protocol import M_DIAGNOSTICS


RunnerCall = Callable[[str, dict[str, object], float], dict[str, Any]]


def backend_diagnostics(
    runner_call: RunnerCall,
    *,
    backend: dict[str, Any],
) -> dict[str, Any]:
    result = runner_call(M_DIAGNOSTICS, {}, 5.0)
    result["reachable"] = True
    result["backend"] = backend
    return result


def unreachable(
    *,
    backend: dict[str, Any],
    message: str,
    code: str | None = None,
) -> dict[str, Any]:
    error: dict[str, Any] = {"message": message}
    if code is not None:
        error["code"] = code
    return {
        "reachable": False,
        "backend": backend,
        "error": error,
    }
