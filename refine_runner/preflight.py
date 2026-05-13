"""Claude CLI pre-flight check — `claude --version` or equivalent no-op.

Result is stored in SQLite (`preflight` table) so the webapp can read it without
re-running the check. Also written into the activity feed on failure.
"""
from __future__ import annotations

import shutil
import sqlite3
import subprocess

from refine_shared import activity, db
from refine_shared.gaps import now_iso


def claude_cli_path() -> str:
    return shutil.which("claude") or "claude"


def check(conn: sqlite3.Connection, *, actor: str = "runner") -> tuple[bool, str | None]:
    """Run a fast no-op invocation. Returns (ok, message)."""
    try:
        out = subprocess.run(
            [claude_cli_path(), "--version"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        if out.returncode == 0:
            _store(conn, ok=True, message=None)
            return True, None
        msg = (out.stderr.strip() or out.stdout.strip() or
               f"claude --version exited {out.returncode}")
        _store(conn, ok=False, message=msg)
        activity.append(
            conn,
            message="Refine cannot reach Claude — run `claude login` on the host",
            severity="error", category="auth", actor=actor, details=msg,
        )
        return False, msg
    except FileNotFoundError as e:
        msg = f"`claude` CLI not found on PATH: {e}"
        _store(conn, ok=False, message=msg)
        activity.append(
            conn,
            message="`claude` CLI not installed on the host (PATH miss)",
            severity="error", category="auth", actor=actor, details=msg,
        )
        return False, msg
    except subprocess.TimeoutExpired:
        msg = "claude --version timed out (10s)"
        _store(conn, ok=False, message=msg)
        return False, msg
    except Exception as e:  # last resort
        msg = f"pre-flight error: {e!r}"
        _store(conn, ok=False, message=msg)
        return False, msg


def _store(conn: sqlite3.Connection, *, ok: bool, message: str | None) -> None:
    with db.transaction(conn):
        conn.execute(
            "INSERT INTO preflight (id, ok, checked_at, message) VALUES (1, ?, ?, ?) "
            "ON CONFLICT(id) DO UPDATE SET ok = excluded.ok, "
            "  checked_at = excluded.checked_at, message = excluded.message",
            (1 if ok else 0, now_iso(), message),
        )


def read(conn: sqlite3.Connection) -> dict | None:
    row = conn.execute("SELECT ok, checked_at, message FROM preflight WHERE id = 1").fetchone()
    if not row:
        return None
    return {"ok": bool(row["ok"]), "checked_at": row["checked_at"], "message": row["message"]}
