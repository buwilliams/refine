"""SQLite-backed full-text search documents for Gaps and activity."""
from __future__ import annotations

import re
import sqlite3
from typing import Any, Iterable

_TOKEN_RE = re.compile(r"[A-Za-z0-9_]+")


def fts_query(raw: str | None) -> str | None:
    """Return a safe FTS5 prefix query for user-entered search text."""
    tokens = _TOKEN_RE.findall((raw or "").lower())
    if not tokens:
        return None
    return " AND ".join(f"{token}*" for token in tokens)


def rebuild_gap_docs(
    conn: sqlite3.Connection,
    gap_rows: Iterable[tuple[dict[str, Any], str]],
) -> int:
    """Replace the Gap search docs from canonical gap.json rows."""
    conn.execute("DELETE FROM gap_search_docs")
    count = 0
    for gap, _rel_path in gap_rows:
        upsert_gap(conn, gap)
        count += 1
    rebuild_fts(conn, "gap_search_fts")
    return count


def upsert_gap(conn: sqlite3.Connection, gap: dict[str, Any]) -> None:
    gid = str(gap.get("id") or "")
    if not gid:
        return
    conn.execute(
        "INSERT INTO gap_search_docs "
        "(gap_id, name, reporter, round_content, notes_content, updated) "
        "VALUES (?, ?, ?, ?, ?, ?) "
        "ON CONFLICT(gap_id) DO UPDATE SET "
        "name = excluded.name, "
        "reporter = excluded.reporter, "
        "round_content = excluded.round_content, "
        "notes_content = excluded.notes_content, "
        "updated = excluded.updated",
        (
            gid,
            str(gap.get("name") or "Untitled Gap"),
            _latest_reporter(gap),
            _round_content(gap),
            _notes_content(gap),
            str(gap.get("updated") or gap.get("created") or ""),
        ),
    )


def delete_gap(conn: sqlite3.Connection, gap_id: str) -> None:
    conn.execute("DELETE FROM gap_search_docs WHERE gap_id = ?", (gap_id,))


def rebuild_fts(conn: sqlite3.Connection, table: str) -> None:
    conn.execute(f"INSERT INTO {table}({table}) VALUES('rebuild')")


def _latest_reporter(gap: dict[str, Any]) -> str:
    rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
    if not rounds:
        return ""
    return str(rounds[-1].get("reporter") or "")


def _round_content(gap: dict[str, Any]) -> str:
    parts: list[str] = []
    for round_obj in gap.get("rounds") or []:
        if not isinstance(round_obj, dict):
            continue
        parts.extend([
            str(round_obj.get("reporter") or ""),
            str(round_obj.get("actual") or ""),
            str(round_obj.get("target") or ""),
        ])
    return "\n".join(p for p in parts if p)


def _notes_content(gap: dict[str, Any]) -> str:
    parts: list[str] = []
    for note in gap.get("notes") or []:
        if not isinstance(note, dict):
            continue
        parts.extend([
            str(note.get("author") or ""),
            str(note.get("body") or ""),
        ])
    return "\n".join(p for p in parts if p)
