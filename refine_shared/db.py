"""SQLite schema + connection helpers.

SQLite holds: status, run state, settings, reporters, activity, ID→path index.
WAL mode + busy retry to allow concurrent webapp and runner writers.
"""
from __future__ import annotations

import json
import random
import sqlite3
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterable, Iterator

from .paths import sqlite_path

SCHEMA = """
CREATE TABLE IF NOT EXISTS gaps_index (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    status      TEXT NOT NULL,
    priority    TEXT NOT NULL DEFAULT 'low',
    created     TEXT NOT NULL,
    updated     TEXT NOT NULL,
    branch_name TEXT,
    json_path   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_gaps_status   ON gaps_index(status);
CREATE INDEX IF NOT EXISTS idx_gaps_updated  ON gaps_index(updated);
CREATE INDEX IF NOT EXISTS idx_gaps_priority ON gaps_index(priority);

CREATE TABLE IF NOT EXISTS runs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    gap_id          TEXT NOT NULL,
    round_idx       INTEGER NOT NULL,
    started_at      TEXT NOT NULL,
    finished_at     TEXT,
    pid             INTEGER,
    status          TEXT NOT NULL,
    last_output_at  TEXT,
    failure_category TEXT
);
CREATE INDEX IF NOT EXISTS idx_runs_gap    ON runs(gap_id);
CREATE INDEX IF NOT EXISTS idx_runs_active ON runs(gap_id, finished_at);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS reporters (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    name    TEXT NOT NULL UNIQUE,
    created TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS activity (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    datetime    TEXT NOT NULL,
    severity    TEXT NOT NULL,
    category    TEXT NOT NULL,
    gap_id      TEXT,
    actor       TEXT,
    message     TEXT NOT NULL,
    details     TEXT,
    actions_json TEXT
);
CREATE INDEX IF NOT EXISTS idx_activity_datetime ON activity(datetime DESC);
CREATE INDEX IF NOT EXISTS idx_activity_gap      ON activity(gap_id);

CREATE TABLE IF NOT EXISTS preflight (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    ok           INTEGER NOT NULL,
    checked_at   TEXT NOT NULL,
    message      TEXT
);
"""

DEFAULT_SETTINGS = {
    "parallel_run_cap": "3",
    "branch_name_pattern": "refine/{gap_id}",
    "agent_idle_timeout_seconds": "900",   # 15 min
    "agent_hard_cap_seconds": "86400",     # 24 h
    "chat_idle_timeout_seconds": "300",    # 5 min — auto-close idle chats
    "paused": "0",
}


def connect(path: Path | None = None) -> sqlite3.Connection:
    p = path or sqlite_path()
    p.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(p), isolation_level=None, timeout=5.0)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA journal_mode = WAL")
    conn.execute("PRAGMA synchronous = NORMAL")
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


def init_db(path: Path | None = None) -> None:
    """Create schema and seed defaults if missing."""
    conn = connect(path)
    try:
        conn.executescript(SCHEMA)
        _migrate(conn)
        for k, v in DEFAULT_SETTINGS.items():
            conn.execute(
                "INSERT OR IGNORE INTO settings(key, value) VALUES (?, ?)", (k, v)
            )
    finally:
        conn.close()


def _migrate(conn: sqlite3.Connection) -> None:
    """Bring existing databases up to current schema. Idempotent — checks
    `PRAGMA table_info` before adding columns."""
    cols = {r["name"] for r in conn.execute("PRAGMA table_info(gaps_index)")}
    if "priority" not in cols:
        conn.execute(
            "ALTER TABLE gaps_index ADD COLUMN priority TEXT NOT NULL DEFAULT 'low'"
        )
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_gaps_priority ON gaps_index(priority)"
        )


@contextmanager
def transaction(conn: sqlite3.Connection) -> Iterator[None]:
    """Short transaction with bounded busy-retry."""
    delays = [0.01, 0.05, 0.25, 0.5]
    last_err: Exception | None = None
    for delay in [0.0, *delays]:
        if delay:
            time.sleep(delay * (0.5 + random.random()))
        try:
            conn.execute("BEGIN IMMEDIATE")
            try:
                yield
                conn.execute("COMMIT")
                return
            except Exception:
                conn.execute("ROLLBACK")
                raise
        except sqlite3.OperationalError as e:
            if "locked" in str(e) or "busy" in str(e):
                last_err = e
                continue
            raise
    raise last_err or sqlite3.OperationalError("transaction busy-retry exhausted")


def get_setting(conn: sqlite3.Connection, key: str, default: str | None = None) -> str | None:
    row = conn.execute("SELECT value FROM settings WHERE key = ?", (key,)).fetchone()
    return row["value"] if row else default


def get_setting_int(conn: sqlite3.Connection, key: str, default: int = 0) -> int:
    v = get_setting(conn, key)
    if v is None:
        return default
    try:
        return int(v)
    except ValueError:
        return default


def set_setting(conn: sqlite3.Connection, key: str, value: str) -> None:
    with transaction(conn):
        conn.execute(
            "INSERT INTO settings(key, value) VALUES(?, ?) "
            "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (key, value),
        )


def list_settings(conn: sqlite3.Connection) -> dict[str, str]:
    return {r["key"]: r["value"] for r in conn.execute("SELECT key, value FROM settings")}
