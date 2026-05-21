"""SQLite schema + connection helpers.

SQLite is a disposable per-application cache. Canonical project state lives in
JSON under .refine/ and is projected into SQLite on startup/app switch. The
`activity`, `runs`, `preflight`, and `target_app_operations` tables are runtime
history/observability only; losing index.sqlite loses that history but not
canonical Gap/settings/instance state.

WAL mode + busy retry to allow concurrent webapp and runner writers.
"""
from __future__ import annotations

import json
import random
import sqlite3
import threading
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
    reporter    TEXT NOT NULL DEFAULT '',
    created     TEXT NOT NULL,
    updated     TEXT NOT NULL,
    branch_name TEXT,
    instance_id TEXT NOT NULL DEFAULT 'default',
    json_path   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_gaps_status   ON gaps_index(status);
CREATE INDEX IF NOT EXISTS idx_gaps_updated  ON gaps_index(updated);
-- idx_gaps_priority + idx_gaps_reporter are created in _migrate() after
-- their respective ALTER TABLE steps so older databases pick them up.

CREATE TABLE IF NOT EXISTS gap_cache_meta (
    json_path TEXT PRIMARY KEY,
    gap_id    TEXT NOT NULL DEFAULT '',
    mtime_ns  INTEGER NOT NULL,
    size      INTEGER NOT NULL,
    sha256    TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_gap_cache_meta_gap_id
    ON gap_cache_meta(gap_id);

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

CREATE TABLE IF NOT EXISTS gap_search_docs (
    rowid         INTEGER PRIMARY KEY AUTOINCREMENT,
    gap_id        TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL,
    reporter      TEXT NOT NULL DEFAULT '',
    round_content TEXT NOT NULL DEFAULT '',
    notes_content TEXT NOT NULL DEFAULT '',
    updated       TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_gap_search_docs_gap
    ON gap_search_docs(gap_id);

CREATE VIRTUAL TABLE IF NOT EXISTS gap_search_fts USING fts5(
    gap_id,
    name,
    reporter,
    round_content,
    notes_content,
    content='gap_search_docs',
    content_rowid='rowid'
);

CREATE VIRTUAL TABLE IF NOT EXISTS activity_search_fts USING fts5(
    message,
    details,
    content='activity',
    content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS gap_search_docs_ai AFTER INSERT ON gap_search_docs BEGIN
    INSERT INTO gap_search_fts(rowid, gap_id, name, reporter, round_content, notes_content)
    VALUES (new.rowid, new.gap_id, new.name, new.reporter, new.round_content, new.notes_content);
END;
CREATE TRIGGER IF NOT EXISTS gap_search_docs_ad AFTER DELETE ON gap_search_docs BEGIN
    INSERT INTO gap_search_fts(gap_search_fts, rowid, gap_id, name, reporter, round_content, notes_content)
    VALUES ('delete', old.rowid, old.gap_id, old.name, old.reporter, old.round_content, old.notes_content);
END;
CREATE TRIGGER IF NOT EXISTS gap_search_docs_au AFTER UPDATE ON gap_search_docs BEGIN
    INSERT INTO gap_search_fts(gap_search_fts, rowid, gap_id, name, reporter, round_content, notes_content)
    VALUES ('delete', old.rowid, old.gap_id, old.name, old.reporter, old.round_content, old.notes_content);
    INSERT INTO gap_search_fts(rowid, gap_id, name, reporter, round_content, notes_content)
    VALUES (new.rowid, new.gap_id, new.name, new.reporter, new.round_content, new.notes_content);
END;

CREATE TRIGGER IF NOT EXISTS activity_search_ai AFTER INSERT ON activity BEGIN
    INSERT INTO activity_search_fts(rowid, message, details)
    VALUES (new.id, new.message, new.details);
END;
CREATE TRIGGER IF NOT EXISTS activity_search_ad AFTER DELETE ON activity BEGIN
    INSERT INTO activity_search_fts(activity_search_fts, rowid, message, details)
    VALUES ('delete', old.id, old.message, old.details);
END;
CREATE TRIGGER IF NOT EXISTS activity_search_au AFTER UPDATE ON activity BEGIN
    INSERT INTO activity_search_fts(activity_search_fts, rowid, message, details)
    VALUES ('delete', old.id, old.message, old.details);
    INSERT INTO activity_search_fts(rowid, message, details)
    VALUES (new.id, new.message, new.details);
END;

CREATE TABLE IF NOT EXISTS performance_events (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    occurred_at   TEXT NOT NULL,
    operation     TEXT NOT NULL,
    elapsed_ms    REAL NOT NULL DEFAULT 0,
    success       INTEGER NOT NULL DEFAULT 1,
    gap_id        TEXT,
    provider      TEXT,
    query_mode    TEXT,
    rows_scanned  INTEGER,
    rows_returned INTEGER,
    bytes_in      INTEGER,
    bytes_out     INTEGER,
    details_json  TEXT
);
CREATE INDEX IF NOT EXISTS idx_performance_operation
    ON performance_events(operation, occurred_at DESC);
CREATE INDEX IF NOT EXISTS idx_performance_occurred
    ON performance_events(occurred_at DESC);
CREATE INDEX IF NOT EXISTS idx_performance_gap
    ON performance_events(gap_id);
CREATE INDEX IF NOT EXISTS idx_performance_success
    ON performance_events(success);

CREATE TABLE IF NOT EXISTS preflight (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    ok           INTEGER NOT NULL,
    checked_at   TEXT NOT NULL,
    message      TEXT
);

CREATE TABLE IF NOT EXISTS target_app_operations (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    kind          TEXT NOT NULL,
    state         TEXT NOT NULL,
    started_at    TEXT NOT NULL,
    finished_at   TEXT NOT NULL,
    command       TEXT,
    cwd           TEXT,
    exit_code     INTEGER,
    message       TEXT,
    stdout_tail   TEXT,
    stderr_tail   TEXT,
    checks_json   TEXT
);
CREATE INDEX IF NOT EXISTS idx_target_app_operations_started
    ON target_app_operations(started_at DESC);

CREATE TABLE IF NOT EXISTS background_jobs (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    label         TEXT NOT NULL,
    status        TEXT NOT NULL,
    started_at    TEXT NOT NULL,
    finished_at   TEXT NOT NULL DEFAULT '',
    result_json   TEXT,
    error_json    TEXT,
    progress_json TEXT
);
CREATE INDEX IF NOT EXISTS idx_background_jobs_kind_status
    ON background_jobs(kind, status);
CREATE INDEX IF NOT EXISTS idx_background_jobs_started
    ON background_jobs(started_at DESC);

CREATE TABLE IF NOT EXISTS refine_merges (
    commit_sha TEXT PRIMARY KEY,
    branch     TEXT NOT NULL,
    committed  TEXT NOT NULL,
    subject    TEXT NOT NULL,
    gap_id     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_refine_merges_branch_committed
    ON refine_merges(branch, committed DESC, commit_sha DESC);
CREATE INDEX IF NOT EXISTS idx_refine_merges_branch_gap
    ON refine_merges(branch, gap_id);
"""

DEFAULT_SETTINGS = {
    "parallel_run_cap": "3",
    "branch_name_pattern": "refine/{gap_id}",
    "agent_idle_timeout_seconds": "900",   # 15 min
    "agent_hard_cap_seconds": "86400",     # 24 h
    "chat_idle_timeout_seconds": "300",    # 5 min — auto-close idle chats
    # Pause new agent launches after rate-limit or token-limit failures.
    "agent_limit_pause_seconds": "60",
    # How long a Gap can sit in 'backlog' before the dispatcher auto-promotes
    # it to 'todo'. 3600 = 1 h default. Sentinel -1 = never (disabled).
    # 0 = instant (promote on next tick).
    "backlog_promote_after_seconds": "3600",
    # How often this instance checks the target repo for local HEAD changes or
    # upstream commits and refreshes projected state. -1 = never.
    "project_update_pulse_interval_seconds": "60",
    "paused": "0",
    # Which agent CLI refine drives for Gap runs, conflict resolution,
    # chat, import extraction, target-app actions, and pre-flight.
    # One of: claude | codex | gemini.
    "agent_cli": "claude",
    # Subdirectory of the client repo (and per-Gap worktree) used as the cwd
    # for agent + chat subprocesses. Lets a monorepo host agent
    # operations focused on one sub-project while git plumbing — worktree
    # create, fetch, merge, push — still happens at the base repo root.
    # Empty = use the worktree / client repo root (default).
    "agent_subpath": "",
    # The branch all Gap worktrees are based on and all Merge-agent work
    # lands on. Empty = follow the host's currently-checked-out branch.
    # When set, the Merge agent will switch the host's HEAD to this
    # branch (auto-stashing any WIP) and restore the host's original
    # branch afterward.
    "merge_target_branch": "",
    # Gap Governance. Product + Constitution together enable governance
    # review before Gap agent dispatch. Rules are stored as JSON array
    # objects: {id, text, created, updated, source}.
    "governance_product": "",
    "governance_constitution": "",
    "governance_rules_json": "[]",
    # Target-application management. New installs use structured one-line
    # shell commands and checks. The legacy prose settings stay present so
    # old databases can display and convert existing configuration.
    "target_app_start_instructions": "",
    "target_app_stop_instructions": "",
    "target_app_health_url": "",
    "target_app_start_command": "",
    "target_app_stop_command": "",
    "target_app_rebuild_command": "",
    "target_app_status_command": "",
    "target_app_cwd": "",
    "target_app_env_json": "{}",
    "target_app_start_timeout_seconds": "120",
    "target_app_stop_timeout_seconds": "60",
    "target_app_rebuild_timeout_seconds": "300",
    "target_app_status_timeout_seconds": "10",
    "target_app_log_path": "",
    "target_app_http_check_url": "",
    "target_app_tcp_check_host": "",
    "target_app_tcp_check_port": "",
    "target_app_process_check_command": "",
    "target_app_auto_rebuild": "never",
    "target_app_auto_rebuild_last_started_at": "",
    "target_app_auto_rebuild_last_finished_at": "",
    "target_app_auto_rebuild_last_ok": "0",
    "target_app_auto_rebuild_last_message": "",
    # Latest known status. "unknown" | "starting" | "running" |
    # "degraded" | "stopping" | "stopped" | "failed".
    "target_app_state": "unknown",
    "target_app_last_check_at": "",
    "target_app_last_check_ok": "0",
    "target_app_last_check_message": "",
    # Back-compat names surfaced to older clients.
    "target_app_last_health_at": "",
    "target_app_last_health_ok": "0",
    "target_app_last_health_message": "",
    "target_app_last_operation_id": "",
    "target_app_last_error": "",
}

_TRANSACTION_LOCKS: dict[int, threading.RLock] = {}
_TRANSACTION_LOCKS_GUARD = threading.Lock()
_SAVEPOINT_SEQ = 0
_REBUILDING_CACHE = False


def _transaction_lock(conn: sqlite3.Connection) -> threading.RLock:
    ident = id(conn)
    with _TRANSACTION_LOCKS_GUARD:
        lock = _TRANSACTION_LOCKS.get(ident)
        if lock is None:
            lock = threading.RLock()
            _TRANSACTION_LOCKS[ident] = lock
        return lock


def _next_savepoint_name() -> str:
    global _SAVEPOINT_SEQ
    with _TRANSACTION_LOCKS_GUARD:
        _SAVEPOINT_SEQ += 1
        return f"refine_tx_{_SAVEPOINT_SEQ}"


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
        if path is None:
            from . import project_state

            global _REBUILDING_CACHE
            _REBUILDING_CACHE = True
            try:
                project_state.rebuild_sqlite_cache(conn)
            finally:
                _REBUILDING_CACHE = False
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
    if "reporter" not in cols:
        conn.execute(
            "ALTER TABLE gaps_index ADD COLUMN reporter TEXT NOT NULL DEFAULT ''"
        )
        _backfill_reporter(conn)
    if "instance_id" not in cols:
        conn.execute(
            "ALTER TABLE gaps_index ADD COLUMN instance_id TEXT NOT NULL DEFAULT 'default'"
        )
    # Always (re-)assert indexes. CREATE INDEX IF NOT EXISTS is a no-op on
    # fresh databases (just after the executescript built the table) and on
    # already-migrated ones — so this is safe to run unconditionally.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_gaps_priority ON gaps_index(priority)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_gaps_reporter ON gaps_index(reporter)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_gaps_instance ON gaps_index(instance_id)"
    )
    _ensure_search_schema(conn)
    _rebuild_activity_search(conn)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS gap_cache_meta ("
        "json_path TEXT PRIMARY KEY, "
        "gap_id TEXT NOT NULL DEFAULT '', "
        "mtime_ns INTEGER NOT NULL, "
        "size INTEGER NOT NULL, "
        "sha256 TEXT NOT NULL DEFAULT '', "
        "updated_at TEXT NOT NULL)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_gap_cache_meta_gap_id "
        "ON gap_cache_meta(gap_id)"
    )
    conn.execute(
        "CREATE TABLE IF NOT EXISTS performance_events ("
        "id INTEGER PRIMARY KEY AUTOINCREMENT, "
        "occurred_at TEXT NOT NULL, "
        "operation TEXT NOT NULL, "
        "elapsed_ms REAL NOT NULL DEFAULT 0, "
        "success INTEGER NOT NULL DEFAULT 1, "
        "gap_id TEXT, "
        "provider TEXT, "
        "query_mode TEXT, "
        "rows_scanned INTEGER, "
        "rows_returned INTEGER, "
        "bytes_in INTEGER, "
        "bytes_out INTEGER, "
        "details_json TEXT)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_performance_operation "
        "ON performance_events(operation, occurred_at DESC)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_performance_occurred "
        "ON performance_events(occurred_at DESC)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_performance_gap "
        "ON performance_events(gap_id)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_performance_success "
        "ON performance_events(success)"
    )
    conn.execute(
        "CREATE TABLE IF NOT EXISTS refine_merges ("
        "commit_sha TEXT PRIMARY KEY, "
        "branch TEXT NOT NULL, "
        "committed TEXT NOT NULL, "
        "subject TEXT NOT NULL, "
        "gap_id TEXT NOT NULL)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_refine_merges_branch_committed "
        "ON refine_merges(branch, committed DESC, commit_sha DESC)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_refine_merges_branch_gap "
        "ON refine_merges(branch, gap_id)"
    )
    try:
        from . import perf_metrics

        perf_metrics.prune(conn)
    except Exception:
        pass
    conn.execute(
        "CREATE TABLE IF NOT EXISTS background_jobs ("
        "id TEXT PRIMARY KEY, "
        "kind TEXT NOT NULL, "
        "label TEXT NOT NULL, "
        "status TEXT NOT NULL, "
        "started_at TEXT NOT NULL, "
        "finished_at TEXT NOT NULL DEFAULT '', "
        "result_json TEXT, "
        "error_json TEXT, "
        "progress_json TEXT)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_background_jobs_kind_status "
        "ON background_jobs(kind, status)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_background_jobs_started "
        "ON background_jobs(started_at DESC)"
    )


def _ensure_search_schema(conn: sqlite3.Connection) -> None:
    conn.executescript(
        """
        CREATE TABLE IF NOT EXISTS gap_search_docs (
            rowid         INTEGER PRIMARY KEY AUTOINCREMENT,
            gap_id        TEXT NOT NULL UNIQUE,
            name          TEXT NOT NULL,
            reporter      TEXT NOT NULL DEFAULT '',
            round_content TEXT NOT NULL DEFAULT '',
            notes_content TEXT NOT NULL DEFAULT '',
            updated       TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_gap_search_docs_gap
            ON gap_search_docs(gap_id);

        CREATE VIRTUAL TABLE IF NOT EXISTS gap_search_fts USING fts5(
            gap_id,
            name,
            reporter,
            round_content,
            notes_content,
            content='gap_search_docs',
            content_rowid='rowid'
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS activity_search_fts USING fts5(
            message,
            details,
            content='activity',
            content_rowid='id'
        );

        CREATE TRIGGER IF NOT EXISTS gap_search_docs_ai AFTER INSERT ON gap_search_docs BEGIN
            INSERT INTO gap_search_fts(rowid, gap_id, name, reporter, round_content, notes_content)
            VALUES (new.rowid, new.gap_id, new.name, new.reporter, new.round_content, new.notes_content);
        END;
        CREATE TRIGGER IF NOT EXISTS gap_search_docs_ad AFTER DELETE ON gap_search_docs BEGIN
            INSERT INTO gap_search_fts(gap_search_fts, rowid, gap_id, name, reporter, round_content, notes_content)
            VALUES ('delete', old.rowid, old.gap_id, old.name, old.reporter, old.round_content, old.notes_content);
        END;
        CREATE TRIGGER IF NOT EXISTS gap_search_docs_au AFTER UPDATE ON gap_search_docs BEGIN
            INSERT INTO gap_search_fts(gap_search_fts, rowid, gap_id, name, reporter, round_content, notes_content)
            VALUES ('delete', old.rowid, old.gap_id, old.name, old.reporter, old.round_content, old.notes_content);
            INSERT INTO gap_search_fts(rowid, gap_id, name, reporter, round_content, notes_content)
            VALUES (new.rowid, new.gap_id, new.name, new.reporter, new.round_content, new.notes_content);
        END;

        CREATE TRIGGER IF NOT EXISTS activity_search_ai AFTER INSERT ON activity BEGIN
            INSERT INTO activity_search_fts(rowid, message, details)
            VALUES (new.id, new.message, new.details);
        END;
        CREATE TRIGGER IF NOT EXISTS activity_search_ad AFTER DELETE ON activity BEGIN
            INSERT INTO activity_search_fts(activity_search_fts, rowid, message, details)
            VALUES ('delete', old.id, old.message, old.details);
        END;
        CREATE TRIGGER IF NOT EXISTS activity_search_au AFTER UPDATE ON activity BEGIN
            INSERT INTO activity_search_fts(activity_search_fts, rowid, message, details)
            VALUES ('delete', old.id, old.message, old.details);
            INSERT INTO activity_search_fts(rowid, message, details)
            VALUES (new.id, new.message, new.details);
        END;
        """
    )


def _rebuild_activity_search(conn: sqlite3.Connection) -> None:
    conn.execute("INSERT INTO activity_search_fts(activity_search_fts) VALUES('rebuild')")


def _backfill_reporter(conn: sqlite3.Connection) -> None:
    """One-time backfill: read each Gap's gap.json and copy the latest
    round's reporter into the new index column. Runs inside `_migrate`
    immediately after the column is added, so the column is empty on
    every existing row at the time of call."""
    # Import lazily to avoid a circular import: gaps -> db (paths/sqlite_path).
    from . import gaps as shared_gaps
    rows = conn.execute("SELECT id FROM gaps_index").fetchall()
    for row in rows:
        gap_id = row["id"]
        gap = shared_gaps.read_gap_json(gap_id, include_logs=False)
        if not gap:
            continue
        rounds = gap.get("rounds") or []
        if not rounds:
            continue
        rep = (rounds[-1].get("reporter") or "").strip()
        if rep:
            conn.execute(
                "UPDATE gaps_index SET reporter = ? WHERE id = ?",
                (rep, gap_id),
            )


@contextmanager
def transaction(conn: sqlite3.Connection) -> Iterator[None]:
    """Short transaction with bounded busy-retry."""
    with _transaction_lock(conn):
        if conn.in_transaction:
            savepoint = _next_savepoint_name()
            conn.execute(f"SAVEPOINT {savepoint}")
            try:
                yield
                conn.execute(f"RELEASE SAVEPOINT {savepoint}")
                return
            except Exception:
                try:
                    conn.execute(f"ROLLBACK TO SAVEPOINT {savepoint}")
                finally:
                    try:
                        conn.execute(f"RELEASE SAVEPOINT {savepoint}")
                    except sqlite3.Error:
                        pass
                raise

        delays = [0.01, 0.05, 0.25, 0.5]
        last_err: Exception | None = None
        for delay in [0.0, *delays]:
            if delay:
                time.sleep(delay * (0.5 + random.random()))
            try:
                conn.execute("BEGIN IMMEDIATE")
            except sqlite3.OperationalError as e:
                if "locked" in str(e) or "busy" in str(e):
                    last_err = e
                    continue
                raise
            try:
                yield
            except Exception:
                try:
                    if conn.in_transaction:
                        conn.execute("ROLLBACK")
                except sqlite3.Error:
                    pass
                raise
            try:
                conn.execute("COMMIT")
            except Exception:
                try:
                    if conn.in_transaction:
                        conn.execute("ROLLBACK")
                except sqlite3.Error:
                    pass
                raise
            return
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
    if not _REBUILDING_CACHE:
        try:
            from . import project_state

            project_state.set_setting(key, str(value))
        except Exception:
            pass


def list_settings(conn: sqlite3.Connection) -> dict[str, str]:
    return {
        r["key"]: r["value"]
        for r in conn.execute(
            "SELECT key, value FROM settings WHERE key NOT LIKE '__refine_%'"
        )
    }
