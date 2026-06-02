"""Shared activity, performance, and background-job operations."""
from __future__ import annotations

import json
import sqlite3
from collections.abc import Callable
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from . import activity, db, gaps as shared_gaps, perf_metrics, project_state, round_logs


JobLookup = Callable[[str], dict[str, Any] | None]
ProgressCallback = Callable[[int, int, str], None]
LifecycleCallback = Callable[[], None]

ACTIVITY_SORT_KEYS = {"id", "datetime", "severity", "category", "actor", "gap_id", "message"}
ACTIVITY_DEFAULT_DIR = {
    "id": "desc",
    "datetime": "desc",
    "severity": "asc",
    "category": "asc",
    "actor": "asc",
    "gap_id": "asc",
    "message": "asc",
}
LOG_RETENTION_OPTIONS = (0, 7, 30, 60, 90, 365)


def page_bounds(limit: int, offset: int = 0) -> tuple[int, int]:
    return max(1, int(limit)), max(0, int(offset))


def empty_activity(
    *,
    limit: int = 50,
    offset: int = 0,
    include_facets: bool = False,
) -> dict[str, Any]:
    page_limit, page_offset = page_bounds(limit, offset)
    body: dict[str, Any] = {
        "activity": [],
        "page": {
            "limit": page_limit,
            "offset": page_offset,
            "has_more": False,
            "total": 0,
        },
        "attached": False,
    }
    if include_facets:
        body["facets"] = {
            "categories": [],
            "actors": [],
            "severities": ["info", "warn", "error"],
        }
    return body


def list_activity(
    conn: sqlite3.Connection,
    *,
    limit: int = 50,
    gap_id: str | None = None,
    since_id: int | None = None,
    severity: str | None = None,
    category: str | None = None,
    actor: str | None = None,
    q: str | None = None,
    offset: int = 0,
    sort: str | None = None,
    direction: str | None = None,
    include_facets: bool = False,
    metric_operation: str = "api.list_activity",
) -> dict[str, Any]:
    metric_start = perf_metrics.now()
    page_limit, page_offset = page_bounds(limit, offset)
    if gap_id and since_id is None:
        body = _list_gap_activity_with_round_logs(
            conn,
            gap_id=gap_id,
            page_limit=page_limit,
            page_offset=page_offset,
            severity=severity,
            category=category,
            actor=actor,
            q=q,
            sort=sort,
            direction=direction,
            include_facets=include_facets,
        )
    else:
        body = _list_activity_table_page(
            conn,
            page_limit=page_limit,
            page_offset=page_offset,
            gap_id=gap_id,
            since_id=since_id,
            severity=severity,
            category=category,
            actor=actor,
            q=q,
            sort=sort,
            direction=direction,
            include_facets=include_facets,
        )
    perf_metrics.record(
        metric_operation,
        conn=conn,
        elapsed_ms=perf_metrics.elapsed_ms(metric_start),
        gap_id=gap_id,
        query_mode="filtered" if any([gap_id, since_id, severity, category, actor, q]) else "recent",
        rows_returned=len(body["activity"]),
        details={
            "limit": page_limit,
            "offset": page_offset,
            "since_id": since_id,
            "severity": severity or "",
            "category": category or "",
            "actor": actor or "",
            "q": bool(q),
            "sort": sort or "",
            "direction": direction or "",
        },
    )
    return body


def record_ui_error(conn: sqlite3.Connection, body: dict[str, Any]) -> dict[str, Any]:
    message = str(body.get("message") or "UI error").strip()[:1000]
    details = body.get("details")
    detail_lines = []
    if details:
        detail_lines.append(str(details)[:4000])
    meta = {
        key: body.get(key)
        for key in ("route", "path", "status", "code", "source")
        if body.get(key) not in (None, "")
    }
    if meta:
        detail_lines.append(json.dumps(meta, sort_keys=True))
    activity.append(
        conn,
        message=message,
        severity="error",
        category="ui",
        actor="browser",
        details="\n\n".join(detail_lines) if detail_lines else None,
    )
    return {"ok": True}


def cleanup_logs(conn: sqlite3.Connection, days: int) -> dict[str, Any]:
    try:
        days = int(days)
    except (TypeError, ValueError) as e:
        raise ValueError("days must be an integer") from e
    if days not in LOG_RETENTION_OPTIONS:
        raise ValueError(f"days must be one of {sorted(LOG_RETENTION_OPTIONS)}")
    if days == 0:
        cur = conn.execute("DELETE FROM activity")
    else:
        cutoff = (
            datetime.now(timezone.utc) - timedelta(days=days)
        ).strftime("%Y-%m-%dT%H:%M:%SZ")
        cur = conn.execute("DELETE FROM activity WHERE datetime < ?", (cutoff,))
    deleted = cur.rowcount or 0
    conn.commit()
    return {"deleted": deleted, "days_kept": days}


def performance_summary(
    conn: sqlite3.Connection,
    *,
    operation: str | None = None,
    success: str | None = None,
    limit: int = 50,
    offset: int = 0,
    backend: dict[str, Any] | None = None,
) -> dict[str, Any]:
    success_filter: bool | None = None
    if success in ("1", "true", "ok", "success"):
        success_filter = True
    elif success in ("0", "false", "failed", "failure"):
        success_filter = False
    snapshot = perf_metrics.snapshot(
        conn,
        days=perf_metrics.RETENTION_DAYS,
        limit=limit,
        offset=offset,
        operation=operation or None,
        success=success_filter,
    )
    if backend is not None:
        snapshot["backend"] = backend
    return snapshot


def performance_cleanup(conn: sqlite3.Connection, *, clear: bool = False) -> dict[str, Any]:
    deleted = (
        perf_metrics.clear(conn)
        if clear
        else perf_metrics.prune(conn, days=perf_metrics.RETENTION_DAYS)
    )
    return {
        "deleted": deleted,
        "retention_days": perf_metrics.RETENTION_DAYS,
    }


def rebuild_sqlite_cache(
    sqlite_file: Path,
    *,
    backend: dict[str, Any] | None = None,
    restart_services: bool = True,
    controls_runner_lifecycle: bool = False,
    progress: ProgressCallback | None = None,
    stop_all: LifecycleCallback | None = None,
    stop_poller: LifecycleCallback | None = None,
    ensure_poller: LifecycleCallback | None = None,
    ensure_runner: LifecycleCallback | None = None,
) -> dict[str, Any]:
    mode = "rebuilt"
    details = ""
    removed: list[str] = []
    backend_payload = backend or {}

    if restart_services and controls_runner_lifecycle and stop_all is not None:
        stop_all()
    elif restart_services and stop_poller is not None:
        stop_poller()
    try:
        try:
            db.init_db(sqlite_file)
            conn = db.connect(sqlite_file)
            try:
                integrity = conn.execute("PRAGMA integrity_check").fetchone()
                if integrity is None or str(integrity[0]).lower() != "ok":
                    detail = str(integrity[0]) if integrity is not None else "no integrity result"
                    raise sqlite3.DatabaseError(f"integrity_check failed: {detail}")
                project_state.rebuild_sqlite_cache(conn, force=True, progress=progress)
            finally:
                conn.close()
        except sqlite3.Error as e:
            mode = "recreated"
            details = str(e)
            if progress is not None:
                progress(0, 0, "Recreating corrupted SQLite cache")
            removed = unlink_sqlite_cache_files(sqlite_file)
            db.init_db(sqlite_file)

        conn = db.connect(sqlite_file)
        try:
            project_state.ensure_sqlite_cache_current(conn)
            counts = sqlite_cache_counts(conn)
            activity.append(
                conn,
                message=(
                    "SQLite cache recreated from canonical JSON"
                    if mode == "recreated"
                    else "SQLite cache rebuilt from canonical JSON"
                ),
                severity="info",
                category="state",
                actor="refine",
                details=details or None,
            )
        finally:
            conn.close()
    finally:
        if restart_services:
            if ensure_poller is not None:
                ensure_poller()
            if controls_runner_lifecycle and ensure_runner is not None:
                ensure_runner()

    return {
        "ok": True,
        "mode": mode,
        "path": str(sqlite_file),
        "removed": removed,
        "backend": backend_payload,
        "runner_restarted": bool(
            restart_services and controls_runner_lifecycle and ensure_runner is not None
        ),
        "poller_restarted": bool(restart_services and ensure_poller is not None),
        **counts,
    }


def sqlite_cache_files(sqlite_file: Path) -> list[Path]:
    return [
        sqlite_file,
        Path(f"{sqlite_file}-wal"),
        Path(f"{sqlite_file}-shm"),
    ]


def unlink_sqlite_cache_files(sqlite_file: Path) -> list[str]:
    removed: list[str] = []
    for path in sqlite_cache_files(sqlite_file):
        try:
            path.unlink()
            removed.append(path.name)
        except FileNotFoundError:
            continue
    return removed


def sqlite_cache_counts(conn: sqlite3.Connection) -> dict[str, int]:
    return {
        "gaps": int(
            conn.execute("SELECT COUNT(*) AS n FROM gaps_index").fetchone()["n"] or 0,
        ),
        "reporters": int(
            conn.execute("SELECT COUNT(*) AS n FROM reporters").fetchone()["n"] or 0,
        ),
    }


def background_job(job_id: str, snapshot: JobLookup) -> dict[str, Any]:
    job = snapshot(job_id)
    if job is None:
        raise LookupError("Background job not found")
    return {"job": job}


def cancel_background_job(job_id: str, cancel: JobLookup) -> dict[str, Any]:
    job = cancel(job_id)
    if job is None:
        raise LookupError("Background job not found")
    return {"job": job}


def _list_gap_activity_with_round_logs(
    conn: sqlite3.Connection,
    *,
    gap_id: str,
    page_limit: int,
    page_offset: int,
    severity: str | None,
    category: str | None,
    actor: str | None,
    q: str | None,
    sort: str | None,
    direction: str | None,
    include_facets: bool,
) -> dict[str, Any]:
    gap = shared_gaps.read_gap_json(gap_id)
    if gap is None:
        return _list_activity_table_page(
            conn,
            page_limit=page_limit,
            page_offset=page_offset,
            gap_id=gap_id,
            since_id=None,
            severity=severity,
            category=category,
            actor=actor,
            q=q,
            sort=sort,
            direction=direction,
            include_facets=include_facets,
        )
    rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
    base_entries = [
        *_mark_log_source(
            activity.recent(
                conn,
                limit=max(1, activity.count(conn, gap_id=gap_id)),
                offset=0,
                gap_id=gap_id,
                sort="datetime",
                direction="asc",
            ),
            "activity",
        ),
        *_gap_round_log_entries(gap_id, len(rounds)),
    ]
    entries = [
        item
        for item in base_entries
        if _activity_entry_matches(
            item,
            severity=severity,
            category=category,
            actor=actor,
            q=q,
        )
    ]
    _sort_activity_entries(entries, sort=sort, direction=direction)
    page = entries[page_offset:page_offset + page_limit]
    body: dict[str, Any] = {
        "activity": page,
        "page": {
            "limit": page_limit,
            "offset": page_offset,
            "has_more": page_offset + len(page) < len(entries),
            "total": len(entries),
        },
    }
    if include_facets:
        body["facets"] = {
            "categories": sorted({
                str(item.get("category") or "")
                for item in base_entries
                if item.get("category")
            }, key=str.lower),
            "actors": sorted({
                str(item.get("actor") or "")
                for item in base_entries
                if item.get("actor")
            }, key=str.lower),
            "severities": ["info", "warn", "error"],
        }
    return body


def _list_activity_table_page(
    conn: sqlite3.Connection,
    *,
    page_limit: int,
    page_offset: int,
    gap_id: str | None,
    since_id: int | None,
    severity: str | None,
    category: str | None,
    actor: str | None,
    q: str | None,
    sort: str | None,
    direction: str | None,
    include_facets: bool,
) -> dict[str, Any]:
    entries = activity.recent(
        conn, limit=page_limit + 1, offset=page_offset,
        gap_id=gap_id, since_id=since_id,
        severity=severity, category=category, actor=actor, q=q,
        sort=sort, direction=direction,
    )
    total = activity.count(
        conn, gap_id=gap_id, since_id=since_id,
        severity=severity, category=category, actor=actor, q=q,
    )
    body: dict[str, Any] = {
        "activity": entries[:page_limit],
        "page": {
            "limit": page_limit,
            "offset": page_offset,
            "has_more": len(entries) > page_limit,
            "total": total,
        },
    }
    if include_facets:
        body["facets"] = {
            "categories": activity.distinct_categories(conn),
            "actors": activity.distinct_actors(conn),
            "severities": ["info", "warn", "error"],
        }
    return body


def _gap_round_log_entries(gap_id: str, round_count: int) -> list[dict[str, Any]]:
    counts = round_logs.count_by_round(gap_id, round_count)
    out: list[dict[str, Any]] = []
    for round_idx in range(round_count):
        count = counts.get(round_idx, 0)
        if count <= 0:
            continue
        entries, _has_more = round_logs.page_round_logs(
            gap_id,
            round_idx,
            limit=count,
            offset=0,
        )
        for idx, log in enumerate(entries):
            item = dict(log)
            item.setdefault("id", f"round:{round_idx}:{idx}")
            item.setdefault("gap_id", gap_id)
            item.setdefault("source", "round")
            item["round_idx"] = round_idx
            out.append(item)
    return out


def _mark_log_source(logs: list[dict[str, Any]], source: str) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for log in logs:
        item = dict(log)
        item.setdefault("source", source)
        out.append(item)
    return out


def _activity_entry_matches(
    item: dict[str, Any],
    *,
    severity: str | None,
    category: str | None,
    actor: str | None,
    q: str | None,
) -> bool:
    if severity and item.get("severity") != severity:
        return False
    if category and item.get("category") != category:
        return False
    if actor and item.get("actor") != actor:
        return False
    if q:
        needle = q.casefold()
        haystack = "\n".join(
            str(item.get(key) or "")
            for key in ("message", "details")
        ).casefold()
        if needle not in haystack:
            return False
    return True


def _sort_activity_entries(
    entries: list[dict[str, Any]],
    *,
    sort: str | None,
    direction: str | None,
) -> None:
    key = (sort or "datetime").lower()
    if key not in ACTIVITY_SORT_KEYS:
        key = "id"
    selected_dir = (direction or "").lower()
    if selected_dir not in ("asc", "desc"):
        selected_dir = ACTIVITY_DEFAULT_DIR[key]
    entries.sort(
        key=lambda item: (
            _activity_sort_value(item, key),
            _activity_sort_value(item, "id"),
        ),
        reverse=selected_dir == "desc",
    )


def _activity_sort_value(item: dict[str, Any], key: str) -> tuple[int, Any]:
    value = item.get(key)
    if key == "id":
        if isinstance(value, int):
            return (0, value)
        try:
            return (0, int(str(value)))
        except (TypeError, ValueError):
            return (1, str(value or "").casefold())
    return (0, str(value or "").casefold())
