"""Agent CLI pre-flight check — send a tiny prompt and verify the answer.

Runs against the operator-configured `agent_cli` setting (claude /
codex / gemini). Result lives in SQLite (`preflight` table) so the
webapp can read it without re-running the check, and a failure
also lands in the activity feed.
"""
from __future__ import annotations

import json
import re
import sqlite3
import subprocess
import tempfile
from pathlib import Path

from refine_server import activity, db
from refine_server.gaps import now_iso

from . import agent_cli, git_ops


_AUTH_PROMPT = "Say exactly the single word hello and nothing else."
_AUTH_TIMEOUT_SECONDS = 60


def _spec_for(conn: sqlite3.Connection) -> agent_cli.CliSpec:
    return agent_cli.get_spec(db.get_setting(conn, "agent_cli"))


def check(conn: sqlite3.Connection, *, actor: str = "runner") -> tuple[bool, str | None]:
    """Run a tiny authenticated prompt. Returns (ok, message)."""
    # Use the chat env so we hit the same PATH the user's interactive
    # shell sees — otherwise systemd's stripped PATH might miss
    # ~/.local/bin/<binary> and we'd preflight a different installation
    # than the agent runs against.
    from .chat_mgr import _chat_env
    env = _chat_env()
    spec = _spec_for(conn)
    bin_path = agent_cli.resolve_binary(spec, env)
    tmp: tempfile.TemporaryDirectory | None = None
    output_last_message: Path | None = None
    if spec.name == "codex":
        tmp = tempfile.TemporaryDirectory(prefix="refine-preflight-")
        output_last_message = Path(tmp.name) / "last_message.txt"
    cwd = git_ops.client_repo_path()
    args = spec.auth_check_args(
        bin_path,
        _AUTH_PROMPT,
        cwd=cwd,
        output_last_message=output_last_message,
    )
    try:
        out = subprocess.run(
            args,
            capture_output=True,
            text=True,
            env=env,
            cwd=str(cwd),
            timeout=_AUTH_TIMEOUT_SECONDS,
        )
        raw = ""
        if output_last_message is not None and output_last_message.exists():
            raw = output_last_message.read_text(encoding="utf-8", errors="replace")
        if not raw:
            raw = _extract_response_text(out.stdout or "")
        if out.returncode == 0 and _is_hello_response(raw):
            _store(conn, ok=True, message=None)
            return True, None
        if out.returncode == 0:
            msg = (
                f"{spec.display_name} auth check returned "
                f"{_preview(raw)!r}; expected exactly `hello`"
            )
        else:
            msg = (out.stderr.strip() or out.stdout.strip() or
                   f"{spec.binary} auth check exited {out.returncode}")
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
        msg = f"{spec.binary} auth check timed out ({_AUTH_TIMEOUT_SECONDS}s)"
        _store(conn, ok=False, message=msg)
        return False, msg
    except Exception as e:  # last resort
        msg = f"pre-flight error: {e!r}"
        _store(conn, ok=False, message=msg)
        return False, msg
    finally:
        if tmp is not None:
            tmp.cleanup()


def _extract_response_text(stdout: str) -> str:
    last = ""
    for line in stdout.splitlines():
        try:
            evt = json.loads(line)
        except json.JSONDecodeError:
            continue
        item = evt.get("item") if isinstance(evt.get("item"), dict) else {}
        text = item.get("text") or evt.get("text")
        item_type = item.get("type")
        if text and item_type in ("agent_message", "assistant_message"):
            last = str(text)
            continue
        if evt.get("type") == "assistant":
            message = evt.get("message") or {}
            text = _text_from_content(message.get("content") or [])
            if text:
                last = text
    return last or stdout


def _text_from_content(content: object) -> str:
    if not isinstance(content, list):
        return ""
    parts: list[str] = []
    for block in content:
        if isinstance(block, dict) and block.get("type") == "text":
            text = block.get("text")
            if text:
                parts.append(str(text))
    return "\n".join(parts).strip()


def _is_hello_response(text: str) -> bool:
    words = re.findall(r"[A-Za-z]+", text.strip().lower())
    return words == ["hello"]


def _preview(text: str, limit: int = 160) -> str:
    compact = " ".join((text or "").strip().split())
    return compact[:limit]


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
