"""Agent CLI pre-flight check — `<cli> --version` or equivalent no-op.

Runs against the operator-configured `agent_cli` setting (claude /
codex / gemini). Result lives in SQLite (`preflight` table) so the
webapp can read it without re-running the check, and a failure
also lands in the activity feed.
"""
from __future__ import annotations

import sqlite3
import subprocess

from refine_shared import activity, db
from refine_shared.gaps import now_iso

from . import agent_cli


def _spec_for(conn: sqlite3.Connection) -> agent_cli.CliSpec:
    return agent_cli.get_spec(db.get_setting(conn, "agent_cli"))


def check(conn: sqlite3.Connection, *, actor: str = "runner") -> tuple[bool, str | None]:
    """Run a fast no-op invocation. Returns (ok, message)."""
    # Use the chat env so we hit the same PATH the user's interactive
    # shell sees — otherwise systemd-user's stripped PATH might miss
    # ~/.local/bin/<binary> and we'd preflight a different installation
    # than the agent runs against.
    from .chat_mgr import _chat_env
    env = _chat_env()
    spec = _spec_for(conn)
    bin_path = agent_cli.resolve_binary(spec, env)
    try:
        out = subprocess.run(
            spec.preflight_args(bin_path),
            capture_output=True,
            text=True,
            env=env,
            timeout=10,
        )
        if out.returncode == 0:
            _store(conn, ok=True, message=None)
            return True, None
        msg = (out.stderr.strip() or out.stdout.strip() or
               f"{spec.binary} --version exited {out.returncode}")
        _store(conn, ok=False, message=msg)
        activity.append(
            conn,
            message=f"Refine cannot reach {spec.display_name} — "
                    f"run `{spec.binary} login` (or its equivalent) on the host",
            severity="error", category="auth", actor=actor, details=msg,
        )
        return False, msg
    except FileNotFoundError as e:
        msg = f"`{spec.binary}` CLI not found on PATH: {e}"
        _store(conn, ok=False, message=msg)
        activity.append(
            conn,
            message=f"`{spec.binary}` CLI not installed on the host (PATH miss)",
            severity="error", category="auth", actor=actor, details=msg,
        )
        return False, msg
    except subprocess.TimeoutExpired:
        msg = f"{spec.binary} --version timed out (10s)"
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
