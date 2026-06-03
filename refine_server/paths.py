"""Filesystem layout helpers.

Resolves paths inside the volume root (the directory containing refine.toml).
Spec:
    <volume-root>/
      refine.toml
      index.sqlite
      gaps/<first 2 chars>/<remaining ULID>/gap.json
      features/<first 2 chars>/<remaining ULID>/feature.json
"""
from __future__ import annotations

from pathlib import Path

from . import config


def volume_root() -> Path:
    return config.get().volume_root


def sqlite_path() -> Path:
    return config.get().sqlite_path


def gap_dir(gap_id: str) -> Path:
    gid = gap_id.upper()
    if len(gid) < 3:
        raise ValueError(f"gap_id too short: {gap_id!r}")
    return volume_root() / "gaps" / gid[:2] / gid[2:]


def gap_json_path(gap_id: str) -> Path:
    return gap_dir(gap_id) / "gap.json"


def gap_logs_path(gap_id: str) -> Path:
    return gap_dir(gap_id) / "logs.jsonl"


def relative_gap_path(gap_id: str) -> str:
    """Path stored in SQLite's gaps_index.json_path column (relative to volume root)."""
    gid = gap_id.upper()
    return f"gaps/{gid[:2]}/{gid[2:]}/gap.json"


def feature_dir(feature_id: str) -> Path:
    fid = feature_id.upper()
    if len(fid) < 3:
        raise ValueError(f"feature_id too short: {feature_id!r}")
    return volume_root() / "features" / fid[:2] / fid[2:]


def feature_json_path(feature_id: str) -> Path:
    return feature_dir(feature_id) / "feature.json"


def relative_feature_path(feature_id: str) -> str:
    """Path stored in SQLite's features_index.json_path column."""
    fid = feature_id.upper()
    return f"features/{fid[:2]}/{fid[2:]}/feature.json"
