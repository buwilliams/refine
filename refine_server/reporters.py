"""Reporters list — SQLite-backed dropdown source.

Source of truth for who-submitted-what remains the round's `reporter` string.
Renaming or deleting a reporter does not touch historical rounds.
"""
from __future__ import annotations

import sqlite3

from .db import transaction
from .gaps import now_iso
from . import project_state


def list_all(conn: sqlite3.Connection) -> list[dict]:
    return [
        {"id": r["id"], "name": r["name"], "created": r["created"]}
        for r in conn.execute("SELECT id, name, created FROM reporters ORDER BY name COLLATE NOCASE")
    ]


def add(conn: sqlite3.Connection, name: str) -> dict:
    name = name.strip()
    if not name:
        raise ValueError("reporter name cannot be empty")
    with transaction(conn):
        try:
            cur = conn.execute(
                "INSERT INTO reporters(name, created) VALUES(?, ?)",
                (name, now_iso()),
            )
            rid = int(cur.lastrowid or 0)
        except sqlite3.IntegrityError:
            row = conn.execute(
                "SELECT id, created FROM reporters WHERE name = ?", (name,)
            ).fetchone()
            return {"id": row["id"], "name": name, "created": row["created"]}
    rep = {"id": rid, "name": name, "created": now_iso()}
    _sync_from_db(conn)
    return rep


def rename(conn: sqlite3.Connection, rid: int, new_name: str) -> None:
    new_name = new_name.strip()
    if not new_name:
        raise ValueError("reporter name cannot be empty")
    with transaction(conn):
        conn.execute("UPDATE reporters SET name = ? WHERE id = ?", (new_name, rid))
    _sync_from_db(conn)


def remove(conn: sqlite3.Connection, rid: int) -> None:
    with transaction(conn):
        conn.execute("DELETE FROM reporters WHERE id = ?", (rid,))
    _sync_from_db(conn)


def exists(conn: sqlite3.Connection, name: str) -> bool:
    row = conn.execute("SELECT 1 FROM reporters WHERE name = ?", (name,)).fetchone()
    return row is not None


def _sync_from_db(conn: sqlite3.Connection) -> None:
    try:
        project_state.write_reporters(list_all(conn))
    except Exception:
        pass
