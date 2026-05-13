"""Filesystem layout for the volume root.

Spec: <volume-root>/
        index.sqlite
        gaps/<first 2 chars of ULID>/<remaining ULID>/gap.json
"""
from __future__ import annotations

import os
from pathlib import Path


def volume_root() -> Path:
    """Read REFINE_VOLUME_ROOT env var (set in docker-compose / runner start)."""
    p = os.environ.get("REFINE_VOLUME_ROOT")
    if not p:
        raise RuntimeError("REFINE_VOLUME_ROOT not set")
    return Path(p)


def sqlite_path(root: Path | None = None) -> Path:
    return (root or volume_root()) / "index.sqlite"


def gap_dir(gap_id: str, root: Path | None = None) -> Path:
    gid = gap_id.upper()
    if len(gid) < 3:
        raise ValueError(f"gap_id too short: {gap_id!r}")
    return (root or volume_root()) / "gaps" / gid[:2] / gid[2:]


def gap_json_path(gap_id: str, root: Path | None = None) -> Path:
    return gap_dir(gap_id, root) / "gap.json"


def relative_gap_path(gap_id: str) -> str:
    """Path stored in SQLite's gaps_index.json_path column (relative to volume root)."""
    gid = gap_id.upper()
    return f"gaps/{gid[:2]}/{gid[2:]}/gap.json"
