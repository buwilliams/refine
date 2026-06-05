"""Live system-operation events for the Toolbar System tab."""
from __future__ import annotations

from datetime import datetime, timezone
from typing import Any

from . import sse

SYSTEM_OPERATION_EVENT = "system_operation"


def publish(message: str, *, status: str = "info", category: str = "system", **extra: Any) -> None:
    text = str(message or "").strip()
    if not text:
        return
    payload: dict[str, Any] = {
        "message": text,
        "status": status,
        "category": category,
        "timestamp": datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z"),
    }
    payload.update(extra)
    sse.publish(SYSTEM_OPERATION_EVENT, payload)
