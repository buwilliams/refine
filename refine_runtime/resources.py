"""Central resource settings and backend selection."""
from __future__ import annotations

import os
import platform
import shutil
from dataclasses import dataclass
from typing import Mapping


CPU_PRIORITIES = {"normal", "low", "very_low"}
ISOLATION_MODES = {"auto", "enforced", "best_effort"}


@dataclass(frozen=True)
class ResourceSettings:
    worker_memory_limit_mb: int = 2000
    ui_memory_limit_mb: int = 2000
    worker_cpu_priority: str = "low"
    resource_isolation_mode: str = "auto"

    @classmethod
    def from_settings(cls, settings: Mapping[str, str]) -> "ResourceSettings":
        return cls(
            worker_memory_limit_mb=_memory_value(
                settings.get("worker_memory_limit_mb", "2000"),
                "worker_memory_limit_mb",
            ),
            ui_memory_limit_mb=_memory_value(
                settings.get("ui_memory_limit_mb", "2000"),
                "ui_memory_limit_mb",
            ),
            worker_cpu_priority=_choice(
                settings.get("worker_cpu_priority", "low"),
                CPU_PRIORITIES,
                "worker_cpu_priority",
            ),
            resource_isolation_mode=_choice(
                settings.get("resource_isolation_mode", "auto"),
                ISOLATION_MODES,
                "resource_isolation_mode",
            ),
        )


@dataclass(frozen=True)
class BackendCapabilities:
    name: str
    isolation: str
    enforced: bool
    details: str = ""


def validate_setting(key: str, value: object) -> str:
    if key in {"worker_memory_limit_mb", "ui_memory_limit_mb"}:
        return str(_memory_value(value, key))
    if key == "worker_cpu_priority":
        return _choice(value, CPU_PRIORITIES, key)
    if key == "resource_isolation_mode":
        return _choice(value, ISOLATION_MODES, key)
    raise KeyError(key)


def detect_capabilities(mode: str = "auto") -> BackendCapabilities:
    mode = _choice(mode, ISOLATION_MODES, "resource_isolation_mode")
    system = platform.system().lower()
    if mode == "best_effort":
        return BackendCapabilities(
            name="posix",
            isolation="best_effort",
            enforced=False,
            details="best-effort mode requested",
        )
    if system == "linux":
        if shutil.which("systemd-run"):
            return BackendCapabilities(
                name="systemd",
                isolation="enforced",
                enforced=True,
                details="systemd-run available",
            )
        if mode == "enforced":
            return BackendCapabilities(
                name="none",
                isolation="unavailable",
                enforced=False,
                details="systemd-run is required for enforced mode",
            )
    if system == "darwin":
        return BackendCapabilities(
            name="launchd",
            isolation="best_effort",
            enforced=False,
            details="macOS resource isolation is best effort",
        )
    return BackendCapabilities(
        name="posix",
        isolation="best_effort",
        enforced=False,
        details=f"{os.name}/{system} uses best-effort process controls",
    )


def priority_to_nice(priority: str) -> int:
    priority = _choice(priority, CPU_PRIORITIES, "worker_cpu_priority")
    return {"normal": 0, "low": 10, "very_low": 19}[priority]


def priority_to_cpu_weight(priority: str) -> int:
    priority = _choice(priority, CPU_PRIORITIES, "worker_cpu_priority")
    return {"normal": 100, "low": 50, "very_low": 10}[priority]


def cpu_weight(settings: ResourceSettings, kind: str) -> int:
    if kind == "ui":
        return 100
    return priority_to_cpu_weight(settings.worker_cpu_priority)


def memory_limit_mb(settings: ResourceSettings, kind: str) -> int:
    if kind == "ui":
        return settings.ui_memory_limit_mb
    return settings.worker_memory_limit_mb


def _memory_value(value: object, key: str) -> int:
    try:
        n = int(value or 0)
    except (TypeError, ValueError) as e:
        raise ValueError(f"{key} must be an integer") from e
    if n == 0:
        return 0
    if n < 256 or n > 262144:
        raise ValueError(f"{key} must be 0 or between 256 and 262144")
    return n


def _choice(value: object, choices: set[str], key: str) -> str:
    raw = str(value or "").strip().lower()
    if raw not in choices:
        allowed = ", ".join(sorted(choices))
        raise ValueError(f"{key} must be one of {allowed}")
    return raw
