"""Main runner orchestrator — ties IPC server, dispatcher, subprocess manager
together, and routes IPC methods to the right handler.

The runner is the sole writer of gap.json.
"""
from __future__ import annotations

import sqlite3
import threading
from pathlib import Path
from typing import Any

from refine_shared import activity, db, gaps as shared_gaps, reporters
from refine_shared.gaps import now_iso
from refine_shared.ipc_protocol import (
    M_APPEND_ROUND, M_CANCEL, M_CHAT_INPUT, M_CHAT_READ, M_CHAT_START,
    M_CHAT_STOP, M_CREATE_GAP, M_DELETE_GAP, M_DIAGNOSTICS, M_EDIT_ROUND,
    M_LAUNCH, M_LOG_APPEND, M_PING, M_PREFLIGHT, M_RUNNING, M_VERIFY,
    default_socket_path,
)

from . import dispatcher as _dispatcher
from . import gap_writer, git_ops, preflight, recovery, state_committer, subprocess_mgr, verify_op
from .chat_mgr import ChatManager
from .ipc_server import IpcServer


class Runner:
    def __init__(self) -> None:
        self._conn_lock = threading.Lock()
        # Use a single shared connection — sqlite3 connections are not strictly
        # thread-safe by default, but with check_same_thread=False and our own
        # lock around transactions, it's fine for our usage pattern.
        from refine_shared.paths import sqlite_path
        self._conn = sqlite3.connect(str(sqlite_path()), check_same_thread=False,
                                     isolation_level=None, timeout=5.0)
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode = WAL")
        self._conn.execute("PRAGMA synchronous = NORMAL")
        self._conn.execute("PRAGMA foreign_keys = ON")

        self.sub_mgr = subprocess_mgr.SubprocessManager(self._get_conn)
        self.dispatcher = _dispatcher.Dispatcher(
            get_conn=self._get_conn, sub_mgr=self.sub_mgr,
        )
        self.ipc = IpcServer(default_socket_path(), self._dispatch_method)
        self.chat = ChatManager(
            get_standalone_idle_timeout=lambda: db.get_setting_int(
                self._conn, "chat_idle_timeout_seconds", 300,
            ),
        )
        self.state_committer = state_committer.StateCommitter(
            get_conn=self._get_conn,
        )

    def _get_conn(self) -> sqlite3.Connection:
        return self._conn

    # ---- lifecycle -----------------------------------------------------------

    def start(self) -> None:
        db.init_db()
        recovery.reconcile_on_start(self._conn)
        preflight.check(self._conn)
        self.dispatcher.start()
        self.state_committer.start()
        self.ipc.start()
        activity.append(
            self._conn, message="refine-runner started",
            severity="info", category="state", actor="runner",
        )

    def shutdown(self) -> None:
        self.ipc.stop()
        self.state_committer.stop()
        self.dispatcher.stop()
        activity.append(
            self._conn, message="refine-runner stopping",
            severity="info", category="state", actor="runner",
        )

    # ---- IPC routing ---------------------------------------------------------

    def _dispatch_method(self, method: str, params: dict) -> dict:
        handlers = {
            M_PING: self._h_ping,
            M_PREFLIGHT: self._h_preflight,
            M_RUNNING: self._h_running,
            M_DIAGNOSTICS: self._h_diagnostics,
            M_LAUNCH: self._h_launch,
            M_CANCEL: self._h_cancel,
            M_VERIFY: self._h_verify,
            M_CREATE_GAP: self._h_create_gap,
            M_APPEND_ROUND: self._h_append_round,
            M_EDIT_ROUND: self._h_edit_round,
            M_LOG_APPEND: self._h_log_append,
            M_DELETE_GAP: self._h_delete_gap,
            M_CHAT_START: self._h_chat_start,
            M_CHAT_INPUT: self._h_chat_input,
            M_CHAT_READ: self._h_chat_read,
            M_CHAT_STOP: self._h_chat_stop,
        }
        h = handlers.get(method)
        if h is None:
            raise KeyError(method)
        return h(params)

    # ---- handlers ------------------------------------------------------------

    def _h_ping(self, _: dict) -> dict:
        return {"pong": True, "at": now_iso()}

    def _h_preflight(self, _: dict) -> dict:
        ok, msg = preflight.check(self._conn)
        return {"ok": ok, "message": msg}

    def _h_running(self, _: dict) -> dict:
        return {"running": self.sub_mgr.running_snapshot()}

    def _h_diagnostics(self, _: dict) -> dict:
        return self.ipc.diagnostics()

    def _h_launch(self, params: dict) -> dict:
        # The dispatcher launches automatically; this method is a no-op that
        # exists mostly so the webapp can nudge the loop after a status change.
        # Status changes (failed→todo via Retry, etc.) are written by the webapp;
        # the dispatcher picks them up on its next tick.
        return {"queued": True}

    def _h_cancel(self, params: dict) -> dict:
        gap_id = params["gap_id"]
        killed = self.sub_mgr.cancel(gap_id)
        # Move to cancelled (terminal). Clean up worktree + branch.
        row = self._conn.execute(
            "SELECT status, branch_name FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
        if row:
            with db.transaction(self._conn):
                self._conn.execute(
                    "UPDATE gaps_index SET status = 'cancelled', updated = ? WHERE id = ?",
                    (now_iso(), gap_id),
                )
            git_ops.remove_worktree(gap_id)
            if row["branch_name"]:
                git_ops.delete_branch(row["branch_name"])
            activity.append(
                self._conn, message="Gap cancelled",
                severity="info", category="state",
                gap_id=gap_id, actor="refine",
            )
        return {"killed_subprocess": killed}

    def _h_verify(self, params: dict) -> dict:
        return verify_op.perform_verify(self._conn, params["gap_id"])

    def _h_create_gap(self, params: dict) -> dict:
        gap_id = params["gap_id"]
        name = params.get("name", "Untitled Gap")
        round_obj = shared_gaps.new_round(
            reporter=params["reporter"],
            actual=params.get("actual", ""),
            target=params.get("target", ""),
        )
        gap = gap_writer.create_gap(gap_id=gap_id, name=name, initial_round=round_obj)

        from refine_shared.paths import relative_gap_path
        with db.transaction(self._conn):
            self._conn.execute(
                "INSERT INTO gaps_index (id, name, status, created, updated, json_path) "
                "VALUES (?, ?, 'todo', ?, ?, ?)",
                (gap_id, name, gap["created"], gap["updated"],
                 relative_gap_path(gap_id)),
            )
        # ensure reporter exists in dropdown list
        try:
            reporters.add(self._conn, params["reporter"])
        except Exception:
            pass
        activity.append(
            self._conn, message=f"Gap created: {name}",
            severity="info", category="state",
            gap_id=gap_id, actor=params["reporter"],
        )
        return {"gap": gap}

    def _h_append_round(self, params: dict) -> dict:
        gap_id = params["gap_id"]
        round_obj = shared_gaps.new_round(
            reporter=params["reporter"],
            actual=params.get("actual", ""),
            target=params.get("target", ""),
        )
        gap = gap_writer.append_round(gap_id, round_obj)
        # review → todo (or todo if currently failed/review/done; webapp guards this)
        with db.transaction(self._conn):
            self._conn.execute(
                "UPDATE gaps_index SET status = 'todo', updated = ? WHERE id = ?",
                (now_iso(), gap_id),
            )
        try:
            reporters.add(self._conn, params["reporter"])
        except Exception:
            pass
        activity.append(
            self._conn, message="New round submitted",
            severity="info", category="state",
            gap_id=gap_id, actor=params["reporter"],
        )
        return {"gap": gap}

    def _h_edit_round(self, params: dict) -> dict:
        gap = gap_writer.edit_latest_round(
            params["gap_id"],
            actual=params.get("actual"),
            target=params.get("target"),
            reporter=params.get("reporter"),
        )
        return {"gap": gap}

    def _h_log_append(self, params: dict) -> dict:
        gap_writer.append_round_log(
            gap_id=params["gap_id"],
            round_idx=int(params["round_idx"]),
            message=params["message"],
            severity=params.get("severity", "info"),
            category=params.get("category", "user"),
            details=params.get("details"),
            actor=params.get("actor"),
            actions=params.get("actions"),
        )
        return {"ok": True}

    def _h_delete_gap(self, params: dict) -> dict:
        gap_id = params["gap_id"]
        # If running, cancel first.
        if self.sub_mgr.is_running(gap_id):
            self.sub_mgr.cancel(gap_id)
        row = self._conn.execute(
            "SELECT status, branch_name FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
        if not row:
            return {"deleted": False}
        # Clean up worktree + branch for non-done statuses; for done, the merged
        # commits stay in the client repo and only the Gap record is removed.
        if row["status"] != "done":
            git_ops.remove_worktree(gap_id)
            if row["branch_name"]:
                git_ops.delete_branch(row["branch_name"])
            gap_writer.delete_gap_file(gap_id)
        else:
            gap_writer.delete_gap_file(gap_id)
        with db.transaction(self._conn):
            self._conn.execute("DELETE FROM gaps_index WHERE id = ?", (gap_id,))
            self._conn.execute("DELETE FROM runs WHERE gap_id = ?", (gap_id,))
        activity.append(
            self._conn, message="Gap deleted",
            severity="info", category="state",
            gap_id=gap_id, actor="refine",
        )
        return {"deleted": True}

    # ---- chat ----------------------------------------------------------------

    def _h_chat_start(self, params: dict) -> dict:
        gap_id = params.get("gap_id")
        priming_prompt: str | None = None
        priming_intro: str | None = None
        if gap_id:
            cwd = git_ops.gap_worktree_path(gap_id)
            if not cwd.exists():
                raise ValueError(f"Gap {gap_id} has no worktree")
            is_standalone = False
            priming_prompt, priming_intro = _build_gap_chat_preamble(
                self._conn, gap_id,
            )
        else:
            cwd = git_ops.client_repo_path()
            is_standalone = True
        sid = self.chat.start(
            cwd, is_standalone=is_standalone,
            priming_prompt=priming_prompt,
            priming_intro=priming_intro,
        )
        return {"session_id": sid}

    def _h_chat_input(self, params: dict) -> dict:
        ok = self.chat.send(params["session_id"], params["text"])
        return {"sent": ok}

    def _h_chat_read(self, params: dict) -> dict:
        max_lines = int(params.get("max_lines", 200))
        return self.chat.read(params["session_id"], max_lines=max_lines)

    def _h_chat_stop(self, params: dict) -> dict:
        ok = self.chat.stop(params["session_id"])
        return {"stopped": ok}


# ---- chat priming -----------------------------------------------------------

def _build_gap_chat_preamble(conn: sqlite3.Connection, gap_id: str,
                              ) -> tuple[str | None, str | None]:
    """Build a context preamble for an attached-chat session.

    Returns (priming_prompt, user_intro_line). The priming prompt is sent to
    claude with output discarded so it lives in claude's session memory
    silently; the intro line is appended to the user-visible chat buffer so
    the user knows context was loaded.
    """
    row = conn.execute(
        "SELECT name, status, branch_name FROM gaps_index WHERE id = ?",
        (gap_id,),
    ).fetchone()
    if not row:
        return None, None
    gap_json = shared_gaps.read_gap_json(gap_id) or {}
    rounds = gap_json.get("rounds") or []
    latest = rounds[-1] if rounds else {}
    recent_activity = activity.recent(conn, limit=10, gap_id=gap_id)
    # Activity rows from `recent` are ordered DESC; flip for chronological.
    recent_activity = list(reversed(recent_activity))

    parts: list[str] = [
        "You're in a refine chat session attached to an in-progress Gap (a",
        "behavior change the team is tracking). You're running inside the",
        "Gap's git worktree (your cwd), so you can read code, run `git log`,",
        "`git status`, `git diff`, and other tools to investigate the agent's",
        "progress.",
        "",
        "Below is the context the user has on their end. When they ask about",
        "the Gap's progress, status, or what's happening, treat their question",
        "as being about THIS GAP — not as small talk. Use the context and the",
        "live worktree state to give a specific, grounded answer. If they ask",
        "an open-ended question (e.g. \"how is it going?\"), summarize the",
        "current state: status, what the latest round asks for, what commits",
        "the agent has made on this branch, and any recent errors.",
        "",
        "## Gap context",
        f"- Name: {row['name']}",
        f"- ID: {gap_id}",
        f"- Status: {row['status']}",
    ]
    if row["branch_name"]:
        parts.append(f"- Branch: {row['branch_name']}")
    if latest:
        parts.append("")
        parts.append(f"## Latest round ({len(rounds)} of {len(rounds)})")
        if latest.get("reporter"):
            parts.append(f"- Reporter: {latest['reporter']}")
        if latest.get("actual"):
            parts.append(f"- Actual (current behavior): {latest['actual']}")
        if latest.get("target"):
            parts.append(f"- Target (desired behavior): {latest['target']}")
    if recent_activity:
        parts.append("")
        parts.append("## Recent activity (oldest first)")
        for entry in recent_activity:
            ts = entry.get("datetime", "")
            msg = entry.get("message", "")
            sev = entry.get("severity", "info")
            parts.append(f"- [{ts}] ({sev}) {msg}")
    parts += [
        "",
        "Don't commit anything to git unless I explicitly ask. Don't repeat",
        "this context block back to me verbatim — synthesize it in your",
        "answer. The user's first message follows.",
    ]
    priming_prompt = "\n".join(parts)
    intro = (
        f"[refine] Loaded Gap context for {row['name']} "
        f"({gap_id[:10]}…, status={row['status']}). You can start chatting "
        f"as soon as the indicator clears."
    )
    return priming_prompt, intro
