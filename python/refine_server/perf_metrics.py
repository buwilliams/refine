"""Best-effort local performance telemetry.

Metrics are runtime observability only. They live in SQLite, are excluded from
canonical .refine JSON, and must never fail the caller being measured.
"""
from __future__ import annotations

import json
import sqlite3
import time
from collections import defaultdict
from datetime import datetime, timedelta, timezone
from typing import Any, Iterable


RETENTION_DAYS = 30
RECENT_LIMIT = 100
MAX_DETAILS_CHARS = 8000


def now() -> float:
    return time.perf_counter()


def elapsed_ms(start: float) -> float:
    return max(0.0, (time.perf_counter() - start) * 1000.0)


def record(
    operation: str,
    *,
    conn: sqlite3.Connection | None = None,
    elapsed_ms: float = 0.0,
    success: bool = True,
    gap_id: str | None = None,
    provider: str | None = None,
    query_mode: str | None = None,
    rows_scanned: int | None = None,
    rows_returned: int | None = None,
    bytes_in: int | None = None,
    bytes_out: int | None = None,
    details: dict[str, Any] | None = None,
) -> None:
    """Insert one event. All errors are swallowed by design."""
    if not operation:
        return
    own_conn = None
    try:
        if conn is None:
            from . import db

            own_conn = db.connect()
            conn = own_conn
        payload = _details_json(details)
        conn.execute(
            "INSERT INTO performance_events "
            "(occurred_at, operation, elapsed_ms, success, gap_id, provider, "
            " query_mode, rows_scanned, rows_returned, bytes_in, bytes_out, details_json) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                _now_iso(),
                str(operation),
                float(elapsed_ms or 0.0),
                1 if success else 0,
                _clean_optional(gap_id),
                _clean_optional(provider),
                _clean_optional(query_mode),
                _clean_int(rows_scanned),
                _clean_int(rows_returned),
                _clean_int(bytes_in),
                _clean_int(bytes_out),
                payload,
            ),
        )
        if own_conn is not None:
            own_conn.commit()
    except Exception:
        pass
    finally:
        if own_conn is not None:
            try:
                own_conn.close()
            except Exception:
                pass


def prune(conn: sqlite3.Connection, *, days: int = RETENTION_DAYS) -> int:
    cutoff = _cutoff(days)
    try:
        cur = conn.execute(
            "DELETE FROM performance_events WHERE occurred_at < ?",
            (cutoff,),
        )
        conn.commit()
        return int(cur.rowcount or 0)
    except Exception:
        return 0


def clear(conn: sqlite3.Connection) -> int:
    try:
        cur = conn.execute("DELETE FROM performance_events")
        conn.commit()
        return int(cur.rowcount or 0)
    except Exception:
        return 0


def snapshot(
    conn: sqlite3.Connection,
    *,
    days: int = RETENTION_DAYS,
    limit: int = RECENT_LIMIT,
    offset: int = 0,
    operation: str | None = None,
    success: bool | None = None,
) -> dict[str, Any]:
    prune(conn, days=days)
    cutoff = _cutoff(days)
    where = ["occurred_at >= ?"]
    args: list[Any] = [cutoff]
    if operation:
        where.append("operation = ?")
        args.append(operation)
    if success is not None:
        where.append("success = ?")
        args.append(1 if success else 0)
    where_sql = " AND ".join(where)
    rows = conn.execute(
        "SELECT id, occurred_at, operation, elapsed_ms, success, gap_id, provider, "
        "query_mode, rows_scanned, rows_returned, bytes_in, bytes_out, details_json "
        f"FROM performance_events WHERE {where_sql} "
        "ORDER BY occurred_at DESC LIMIT ? OFFSET ?",
        (*args, max(1, int(limit)) + 1, max(0, int(offset))),
    ).fetchall()
    has_more = len(rows) > max(1, int(limit))
    page_rows = rows[:max(1, int(limit))]
    summary_rows = conn.execute(
        "SELECT operation, elapsed_ms, success, occurred_at "
        "FROM performance_events WHERE occurred_at >= ?",
        (cutoff,),
    ).fetchall()
    event_count = conn.execute(
        "SELECT COUNT(*) AS n FROM performance_events WHERE occurred_at >= ?",
        (cutoff,),
    ).fetchone()["n"]
    total_count = conn.execute(
        "SELECT COUNT(*) AS n FROM performance_events",
    ).fetchone()["n"]
    filtered_count = conn.execute(
        f"SELECT COUNT(*) AS n FROM performance_events WHERE {where_sql}",
        args,
    ).fetchone()["n"]
    page_limit = max(1, int(limit))
    page_offset = max(0, int(offset))
    return {
        "retention_days": days,
        "event_count": int(event_count or 0),
        "total_event_count": int(total_count or 0),
        "filtered_event_count": int(filtered_count or 0),
        "summary": _summary(summary_rows),
        "recent": [_row_to_event(row) for row in page_rows],
        "operations": sorted({str(row["operation"]) for row in summary_rows}),
        "page": {
            "limit": page_limit,
            "offset": page_offset,
            "has_more": has_more,
            "total": int(filtered_count or 0),
        },
    }


def _summary(rows: Iterable[sqlite3.Row]) -> list[dict[str, Any]]:
    by_operation: dict[str, list[sqlite3.Row]] = defaultdict(list)
    for row in rows:
        by_operation[str(row["operation"])].append(row)
    out: list[dict[str, Any]] = []
    for operation, items in by_operation.items():
        elapsed = sorted(float(row["elapsed_ms"] or 0.0) for row in items)
        count = len(elapsed)
        failures = sum(1 for row in items if not int(row["success"] or 0))
        latest = max(str(row["occurred_at"] or "") for row in items)
        out.append({
            "operation": operation,
            "count": count,
            "failures": failures,
            "avg_ms": round(sum(elapsed) / count, 2) if count else 0.0,
            "p50_ms": round(_percentile(elapsed, 0.50), 2),
            "p95_ms": round(_percentile(elapsed, 0.95), 2),
            "max_ms": round(elapsed[-1], 2) if elapsed else 0.0,
            "last_seen": latest,
        })
    out.sort(key=lambda row: (-float(row["p95_ms"]), row["operation"]))
    return out


def _percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    idx = min(len(values) - 1, max(0, int(round((len(values) - 1) * pct))))
    return values[idx]


def _row_to_event(row: sqlite3.Row) -> dict[str, Any]:
    event = {
        "id": row["id"],
        "occurred_at": row["occurred_at"],
        "operation": row["operation"],
        "elapsed_ms": round(float(row["elapsed_ms"] or 0.0), 2),
        "success": bool(row["success"]),
    }
    for key in (
        "gap_id", "provider", "query_mode", "rows_scanned", "rows_returned",
        "bytes_in", "bytes_out",
    ):
        if row[key] is not None:
            event[key] = row[key]
    if row["details_json"]:
        try:
            event["details"] = json.loads(row["details_json"])
        except Exception:
            event["details"] = {"raw": row["details_json"]}
    return event


def _details_json(details: dict[str, Any] | None) -> str | None:
    if not details:
        return None
    try:
        raw = json.dumps(details, ensure_ascii=False, sort_keys=True)
    except Exception:
        raw = json.dumps({"repr": repr(details)}, ensure_ascii=False)
    if len(raw) > MAX_DETAILS_CHARS:
        raw = raw[:MAX_DETAILS_CHARS - 20] + "...[truncated]"
    return raw


def _clean_optional(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text or None


def _clean_int(value: Any) -> int | None:
    if value is None:
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _cutoff(days: int) -> str:
    return (
        datetime.now(timezone.utc) - timedelta(days=max(0, int(days)))
    ).strftime("%Y-%m-%dT%H:%M:%SZ")


def _now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
