"""Activity feed — writes structured entries to SQLite `activity` table.

Per spec: activity entries share the shape used by round logs[].
"""
from __future__ import annotations

import json
import sqlite3
from typing import Any

from . import search_index
from .db import transaction
from .gaps import now_iso


_SORT_EXPRESSIONS: dict[str, str] = {
    "id": "id",
    "datetime": "datetime",
    "severity": "severity COLLATE NOCASE",
    "category": "category COLLATE NOCASE",
    "actor": "actor COLLATE NOCASE",
    "gap_id": "gap_id COLLATE NOCASE",
    "message": "message COLLATE NOCASE",
}
_DEFAULT_DIR: dict[str, str] = {
    "id": "DESC",
    "datetime": "DESC",
    "severity": "ASC",
    "category": "ASC",
    "actor": "ASC",
    "gap_id": "ASC",
    "message": "ASC",
}


def append(
    conn: sqlite3.Connection,
    *,
    message: str,
    severity: str = "info",
    category: str = "state",
    gap_id: str | None = None,
    actor: str | None = None,
    details: str | None = None,
    actions: list[dict] | None = None,
) -> int:
    with transaction(conn):
        cur = conn.execute(
            "INSERT INTO activity (datetime, severity, category, gap_id, actor, "
            "                      message, details, actions_json) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (
                now_iso(),
                severity,
                category,
                gap_id,
                actor,
                message,
                details,
                json.dumps(actions) if actions else None,
            ),
        )
        return int(cur.lastrowid or 0)


def recent(
    conn: sqlite3.Connection,
    *,
    limit: int = 50,
    offset: int = 0,
    gap_id: str | None = None,
    since_id: int | None = None,
    severity: str | None = None,
    category: str | None = None,
    actor: str | None = None,
    q: str | None = None,
    sort: str | None = None,
    direction: str | None = None,
) -> list[dict[str, Any]]:
    sql = [
        "SELECT id, datetime, severity, category, gap_id, actor, message, "
        "       details, actions_json FROM activity"
    ]
    where, args = _where_args(
        gap_id=gap_id, since_id=since_id, severity=severity,
        category=category, actor=actor, q=q,
    )
    if where is None:
        return []
    if where:
        sql.append("WHERE " + " AND ".join(where))
    sql.append("ORDER BY " + _order_clause(sort, direction))
    sql.append("LIMIT ? OFFSET ?")
    args.extend([limit, max(0, int(offset))])
    out = []
    for r in conn.execute(" ".join(sql), args):
        out.append(_row_to_entry(r))
    return out


def count(
    conn: sqlite3.Connection,
    *,
    gap_id: str | None = None,
    since_id: int | None = None,
    severity: str | None = None,
    category: str | None = None,
    actor: str | None = None,
    q: str | None = None,
) -> int:
    sql = ["SELECT COUNT(*) FROM activity"]
    where, args = _where_args(
        gap_id=gap_id, since_id=since_id, severity=severity,
        category=category, actor=actor, q=q,
    )
    if where is None:
        return 0
    if where:
        sql.append("WHERE " + " AND ".join(where))
    return int(conn.execute(" ".join(sql), args).fetchone()[0])


def _where_args(
    *,
    gap_id: str | None,
    since_id: int | None,
    severity: str | None,
    category: str | None,
    actor: str | None,
    q: str | None,
) -> tuple[list[str], list[Any]] | tuple[None, list[Any]]:
    args: list[Any] = []
    where: list[str] = []
    if gap_id:
        where.append("gap_id = ?")
        args.append(gap_id)
    if since_id is not None:
        where.append("id > ?")
        args.append(since_id)
    if severity:
        where.append("severity = ?")
        args.append(severity)
    if category:
        where.append("category = ?")
        args.append(category)
    if actor:
        where.append("actor = ?")
        args.append(actor)
    if q:
        match = search_index.fts_query(q)
        if match is None:
            return None, []
        where.append(
            "id IN (SELECT rowid FROM activity_search_fts "
            "WHERE activity_search_fts MATCH ?)"
        )
        args.append(match)
    return where, args


def _order_clause(sort: str | None, direction: str | None) -> str:
    key = (sort or "datetime").lower()
    if key not in _SORT_EXPRESSIONS:
        key = "id"
    expr = _SORT_EXPRESSIONS[key]
    d = (direction or "").upper()
    if d not in ("ASC", "DESC"):
        d = _DEFAULT_DIR[key]
    tiebreaker = "" if key == "id" else ", id DESC"
    return f"{expr} {d}{tiebreaker}"


def distinct_categories(conn: sqlite3.Connection) -> list[str]:
    return [r[0] for r in conn.execute(
        "SELECT DISTINCT category FROM activity "
        "WHERE category IS NOT NULL AND category != '' "
        "ORDER BY category"
    )]


def distinct_actors(conn: sqlite3.Connection) -> list[str]:
    return [r[0] for r in conn.execute(
        "SELECT DISTINCT actor FROM activity "
        "WHERE actor IS NOT NULL AND actor != '' "
        "ORDER BY actor"
    )]


def _row_to_entry(r: sqlite3.Row) -> dict[str, Any]:
    entry: dict[str, Any] = {
        "id": r["id"],
        "datetime": r["datetime"],
        "severity": r["severity"],
        "category": r["category"],
        "message": r["message"],
    }
    if r["gap_id"]:
        entry["gap_id"] = r["gap_id"]
    if r["actor"]:
        entry["actor"] = r["actor"]
    if r["details"]:
        entry["details"] = r["details"]
    if r["actions_json"]:
        try:
            entry["actions"] = json.loads(r["actions_json"])
        except (TypeError, ValueError):
            pass
    return entry
