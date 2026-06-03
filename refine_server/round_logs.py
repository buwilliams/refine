"""Append-only per-Gap round log storage."""
from __future__ import annotations

import json
import os
from collections import defaultdict
from typing import Any

from .paths import gap_json_path, gap_logs_path

_EMBEDDED_LOGS_STASH = "_refine_embedded_round_logs"


def append_log(gap_id: str, round_idx: int, entry: dict[str, Any]) -> None:
    """Append one round-log entry without rewriting gap.json."""
    from . import perf_metrics

    start = perf_metrics.now()
    fsync_ms = 0.0
    path = gap_logs_path(gap_id)
    path.parent.mkdir(parents=True, exist_ok=True)
    record = _stored_entry(round_idx, entry)
    data = (json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8")
    try:
        existed = path.exists()
        with open(path, "ab") as f:
            f.write(data)
            f.flush()
            fsync_start = perf_metrics.now()
            os.fsync(f.fileno())
            fsync_ms = perf_metrics.elapsed_ms(fsync_start)
        if not existed:
            _fsync_dir(path.parent)
        perf_metrics.record(
            "round_log_append",
            gap_id=gap_id,
            elapsed_ms=perf_metrics.elapsed_ms(start),
            bytes_out=len(data),
            details={
                "round_idx": round_idx,
                "fsync_ms": round(fsync_ms, 2),
            },
        )
    except Exception:
        perf_metrics.record(
            "round_log_append",
            gap_id=gap_id,
            elapsed_ms=perf_metrics.elapsed_ms(start),
            success=False,
            bytes_out=len(data),
            details={"round_idx": round_idx, "fsync_ms": round(fsync_ms, 2)},
        )
        raise


def externalize_embedded_logs(gap: dict[str, Any]) -> int:
    """Move any embedded rounds[].logs entries into the append-only file.

    Returns the number of entries newly appended to the sidecar log file.
    The input gap is mutated so callers can serialize it without embedded logs.
    """
    gap_id = str(gap.get("id") or "")
    if not gap_id:
        return 0
    records: list[dict[str, Any]] = []
    stashed = gap.pop(_EMBEDDED_LOGS_STASH, None)
    if isinstance(stashed, dict):
        for raw_idx, logs in stashed.items():
            try:
                idx = int(raw_idx)
            except (TypeError, ValueError):
                continue
            if not isinstance(logs, list):
                continue
            for entry in logs:
                if isinstance(entry, dict):
                    records.append(_stored_entry(idx, entry))
    for idx, round_obj in enumerate(gap.get("rounds") or []):
        if not isinstance(round_obj, dict):
            continue
        logs = round_obj.pop("logs", None)
        if not isinstance(logs, list):
            continue
        for entry in logs:
            if isinstance(entry, dict):
                records.append(_stored_entry(idx, entry))
    if not records:
        return 0

    existing = _existing_keys(gap_id)
    path = gap_logs_path(gap_id)
    path.parent.mkdir(parents=True, exist_ok=True)
    appended = 0
    with open(path, "ab") as f:
        for record in records:
            key = _entry_key(record)
            if key in existing:
                continue
            data = (json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8")
            f.write(data)
            existing.add(key)
            appended += 1
        if appended:
            f.flush()
            os.fsync(f.fileno())
    if appended:
        _fsync_dir(path.parent)
    return appended


def strip_embedded_logs(gap: dict[str, Any]) -> None:
    stashed: dict[str, list[dict[str, Any]]] = {}
    for idx, round_obj in enumerate(gap.get("rounds") or []):
        if isinstance(round_obj, dict):
            logs = round_obj.pop("logs", None)
            if isinstance(logs, list):
                entries = [dict(entry) for entry in logs if isinstance(entry, dict)]
                if entries:
                    stashed[str(idx)] = entries
    if stashed:
        gap[_EMBEDDED_LOGS_STASH] = stashed


def count_by_round(gap_id: str, round_count: int) -> dict[int, int]:
    counts: dict[int, int] = {idx: 0 for idx in range(round_count)}
    seen: set[tuple[Any, ...]] = set()
    for record in _iter_stored_entries(gap_id):
        idx = _round_idx(record)
        if idx is not None and 0 <= idx < round_count:
            key = _entry_key(record)
            if key in seen:
                continue
            seen.add(key)
            counts[idx] = counts.get(idx, 0) + 1
    for idx, logs in _embedded_logs_by_round(gap_id).items():
        if 0 <= idx < round_count:
            for entry in logs:
                key = _entry_key(_stored_entry(idx, entry))
                if key in seen:
                    continue
                seen.add(key)
                counts[idx] = counts.get(idx, 0) + 1
    return counts


def page_round_logs(
    gap_id: str,
    round_idx: int,
    *,
    limit: int,
    offset: int = 0,
) -> tuple[list[dict[str, Any]], bool]:
    entries: list[dict[str, Any]] = []
    stop = offset + limit + 1
    seen = 0
    for entry in _round_entries(gap_id, round_idx):
        if seen >= offset and len(entries) < limit + 1:
            entries.append(entry)
        seen += 1
        if seen >= stop:
            break
    has_more = len(entries) > limit
    return entries[:limit], has_more


def latest_for_round(
    gap_id: str,
    round_idx: int,
) -> tuple[dict[str, Any] | None, dict[str, Any] | None]:
    latest: dict[str, Any] | None = None
    latest_error: dict[str, Any] | None = None
    for entry in _round_entries(gap_id, round_idx):
        latest = entry
        if entry.get("severity") == "error":
            latest_error = entry
    return latest, latest_error


def latest_workflow_for_round(
    gap_id: str,
    round_idx: int,
) -> dict[str, Any] | None:
    latest: dict[str, Any] | None = None
    for entry in _round_entries(gap_id, round_idx):
        if (
            entry.get("category") == "state"
            and str(entry.get("message") or "").startswith(
                "Workflow status changed:",
            )
        ):
            latest = entry
    return latest


def latest_state_for_round(
    gap_id: str,
    round_idx: int,
) -> dict[str, Any] | None:
    latest: dict[str, Any] | None = None
    for entry in _round_entries(gap_id, round_idx):
        if entry.get("category") == "state":
            latest = entry
    return latest


def hydrate_round_logs(gap: dict[str, Any]) -> None:
    rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
    if not rounds:
        return
    by_round: dict[int, list[dict[str, Any]]] = defaultdict(list)
    gap_id = str(gap.get("id") or "")
    if gap_id:
        seen: set[tuple[Any, ...]] = set()
        for record in _iter_stored_entries(gap_id):
            idx = _round_idx(record)
            if idx is not None and 0 <= idx < len(rounds):
                seen.add(_entry_key(record))
                by_round[idx].append(_public_entry(record))
        for idx, logs in _embedded_logs_by_round(gap_id).items():
            if 0 <= idx < len(rounds):
                for entry in logs:
                    key = _entry_key(_stored_entry(idx, entry))
                    if key in seen:
                        continue
                    seen.add(key)
                    by_round[idx].append(entry)
    for idx, round_obj in enumerate(rounds):
        logs = by_round.get(idx, [])
        logs.sort(key=lambda item: item.get("datetime") or "")
        round_obj["logs"] = logs


def _round_entries(gap_id: str, round_idx: int):
    entries: list[dict[str, Any]] = []
    seen: set[tuple[Any, ...]] = set()
    for record in _iter_stored_entries(gap_id):
        if _round_idx(record) == round_idx:
            seen.add(_entry_key(record))
            entries.append(_public_entry(record))
    for entry in _embedded_logs_by_round(gap_id).get(round_idx, []):
        key = _entry_key(_stored_entry(round_idx, entry))
        if key in seen:
            continue
        seen.add(key)
        entries.append(entry)
    entries.sort(key=lambda item: item.get("datetime") or "")
    yield from entries


def _iter_stored_entries(gap_id: str):
    path = gap_logs_path(gap_id)
    if not path.exists():
        return
    with open(path, "rb") as f:
        for raw in f:
            line = raw.strip()
            if not line:
                continue
            try:
                record = json.loads(line.decode("utf-8"))
            except json.JSONDecodeError:
                continue
            if isinstance(record, dict):
                yield record


def _embedded_logs_by_round(gap_id: str) -> dict[int, list[dict[str, Any]]]:
    path = gap_json_path(gap_id)
    if not path.exists():
        return {}
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    by_round: dict[int, list[dict[str, Any]]] = defaultdict(list)
    for idx, round_obj in enumerate(raw.get("rounds") or []):
        if not isinstance(round_obj, dict):
            continue
        logs = round_obj.get("logs")
        if not isinstance(logs, list):
            continue
        for entry in logs:
            if isinstance(entry, dict):
                by_round[idx].append(dict(entry))
    return by_round


def _stored_entry(round_idx: int, entry: dict[str, Any]) -> dict[str, Any]:
    record = dict(entry)
    record["round_idx"] = int(round_idx)
    return record


def _public_entry(record: dict[str, Any]) -> dict[str, Any]:
    entry = dict(record)
    entry.pop("round_idx", None)
    return entry


def _round_idx(record: dict[str, Any]) -> int | None:
    try:
        return int(record.get("round_idx"))
    except (TypeError, ValueError):
        return None


def _existing_keys(gap_id: str) -> set[tuple[Any, ...]]:
    return {_entry_key(record) for record in _iter_stored_entries(gap_id)}


def _entry_key(record: dict[str, Any]) -> tuple[Any, ...]:
    return (
        record.get("round_idx"),
        record.get("datetime"),
        record.get("severity"),
        record.get("category"),
        record.get("actor"),
        record.get("message"),
        record.get("details"),
        json.dumps(record.get("actions") or [], sort_keys=True, separators=(",", ":")),
    )


def _fsync_dir(path) -> None:
    try:
        dir_fd = os.open(str(path), os.O_RDONLY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    except OSError:
        pass
