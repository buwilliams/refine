"""gap.json read/write.

The runner is the sole writer (atomic temp+rename); the webapp reads.
Convention is enforced by which package imports `write_gap_json` — the webapp
does not.
"""
from __future__ import annotations

import json
import os
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from .paths import gap_dir, gap_json_path


def now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def new_log_entry(
    message: str,
    *,
    severity: str = "info",
    category: str = "cli",
    details: str | None = None,
    actions: list[dict] | None = None,
    actor: str | None = None,
) -> dict[str, Any]:
    entry: dict[str, Any] = {
        "datetime": now_iso(),
        "severity": severity,
        "category": category,
        "message": message,
    }
    if details is not None:
        entry["details"] = details
    if actions:
        entry["actions"] = actions
    if actor is not None:
        entry["actor"] = actor
    return entry


def empty_gap(gap_id: str, name: str) -> dict[str, Any]:
    now = now_iso()
    return {
        "id": gap_id,
        "name": name,
        "created": now,
        "updated": now,
        "rounds": [],
    }


def new_round(reporter: str, actual: str, target: str) -> dict[str, Any]:
    now = now_iso()
    return {
        "reporter": reporter,
        "actual": actual,
        "target": target,
        "created": now,
        "updated": now,
        "logs": [],
    }


def read_gap_json(gap_id: str, root: Path | None = None) -> dict[str, Any] | None:
    p = gap_json_path(gap_id, root)
    if not p.exists():
        return None
    with open(p, "rb") as f:
        return json.loads(f.read().decode("utf-8"))


def write_gap_json(gap: dict[str, Any], root: Path | None = None) -> None:
    """Atomic write: temp file in same directory + rename + fsync directory.

    RUNNER ONLY. The webapp must route writes through IPC.
    """
    gid = gap["id"]
    d = gap_dir(gid, root)
    d.mkdir(parents=True, exist_ok=True)
    p = gap_json_path(gid, root)
    data = json.dumps(gap, ensure_ascii=False, indent=2).encode("utf-8")
    fd, tmp = tempfile.mkstemp(prefix=".gap.", suffix=".tmp", dir=str(d))
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, p)
    except Exception:
        try:
            os.unlink(tmp)
        except FileNotFoundError:
            pass
        raise
    # fsync the directory to make the rename durable
    try:
        dir_fd = os.open(str(d), os.O_RDONLY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    except OSError:
        pass  # not all filesystems support directory fsync
