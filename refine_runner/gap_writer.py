"""Sole owner of gap.json writes. Serializes per-Gap with a lock map.

All other modules (webapp via IPC, subprocess supervisor, dispatcher) call into
this module to mutate gap.json. Reads happen elsewhere via refine_shared.gaps.
"""
from __future__ import annotations

import threading
from collections import defaultdict
from pathlib import Path
from typing import Any

from refine_shared import gaps as shared_gaps
from refine_shared.gaps import now_iso

_locks: dict[str, threading.Lock] = defaultdict(threading.Lock)
_locks_master = threading.Lock()


def _lock_for(gap_id: str) -> threading.Lock:
    with _locks_master:
        return _locks[gap_id]


def create_gap(*, gap_id: str, name: str, initial_round: dict[str, Any]) -> dict[str, Any]:
    """Initialize gap.json with one round. Returns the new Gap record."""
    with _lock_for(gap_id):
        gap = shared_gaps.empty_gap(gap_id, name)
        gap["rounds"].append(initial_round)
        gap["updated"] = now_iso()
        shared_gaps.write_gap_json(gap)
        return gap


def append_round(gap_id: str, round_obj: dict[str, Any]) -> dict[str, Any]:
    with _lock_for(gap_id):
        gap = shared_gaps.read_gap_json(gap_id)
        if gap is None:
            raise FileNotFoundError(f"gap.json missing for {gap_id}")
        gap["rounds"].append(round_obj)
        gap["updated"] = now_iso()
        shared_gaps.write_gap_json(gap)
        return gap


def edit_latest_round(gap_id: str, *, actual: str | None = None,
                      target: str | None = None, reporter: str | None = None) -> dict[str, Any]:
    with _lock_for(gap_id):
        gap = shared_gaps.read_gap_json(gap_id)
        if gap is None:
            raise FileNotFoundError(f"gap.json missing for {gap_id}")
        if not gap["rounds"]:
            raise ValueError("Gap has no rounds")
        r = gap["rounds"][-1]
        if actual is not None:
            r["actual"] = actual
        if target is not None:
            r["target"] = target
        if reporter is not None:
            r["reporter"] = reporter
        r["updated"] = now_iso()
        gap["updated"] = r["updated"]
        shared_gaps.write_gap_json(gap)
        return gap


def append_round_log(*, gap_id: str, round_idx: int, message: str,
                     severity: str = "info", category: str = "cli",
                     details: str | None = None,
                     actor: str | None = None,
                     actions: list[dict] | None = None) -> None:
    with _lock_for(gap_id):
        gap = shared_gaps.read_gap_json(gap_id)
        if gap is None:
            return
        rounds = gap.get("rounds", [])
        if round_idx < 0 or round_idx >= len(rounds):
            return
        entry = shared_gaps.new_log_entry(
            message,
            severity=severity, category=category,
            details=details, actions=actions, actor=actor,
        )
        rounds[round_idx].setdefault("logs", []).append(entry)
        rounds[round_idx]["updated"] = entry["datetime"]
        gap["updated"] = entry["datetime"]
        shared_gaps.write_gap_json(gap)


def update_name(gap_id: str, name: str) -> None:
    with _lock_for(gap_id):
        gap = shared_gaps.read_gap_json(gap_id)
        if gap is None:
            return
        gap["name"] = name
        gap["updated"] = now_iso()
        shared_gaps.write_gap_json(gap)


def delete_gap_file(gap_id: str) -> None:
    """Remove gap.json and the containing dir. (SQLite cleanup is separate.)"""
    from refine_shared.paths import gap_dir, gap_json_path
    with _lock_for(gap_id):
        p = gap_json_path(gap_id)
        if p.exists():
            p.unlink()
        d = gap_dir(gap_id)
        try:
            d.rmdir()
            d.parent.rmdir()  # shard dir, if empty
        except OSError:
            pass
