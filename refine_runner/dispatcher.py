"""Background dispatcher: scans for `todo` Gaps and launches subprocesses
up to the parallel-run cap.

Reasons we may not launch even when work is available:
- Pool is paused (settings.paused = 1)
- Cap is already at the limit
- Pre-flight failed and Gap's last failure was auth-related (Retry pre-flight rule)
"""
from __future__ import annotations

import sqlite3
import threading
import time
from dataclasses import dataclass

from refine_shared import activity, db
from refine_shared.gaps import now_iso, read_gap_json

from . import gap_writer, git_ops, preflight, subprocess_mgr
from .friendly_outcome import classify_outcome


@dataclass
class Dispatcher:
    """Polls SQLite for ready Gaps and launches subprocesses."""

    get_conn: callable  # type: ignore[type-arg]
    sub_mgr: subprocess_mgr.SubprocessManager
    poll_interval: float = 2.0

    _stop: threading.Event = None  # type: ignore[assignment]
    _thread: threading.Thread = None  # type: ignore[assignment]

    def start(self) -> None:
        self._stop = threading.Event()
        self._thread = threading.Thread(
            target=self._loop, name="refine-dispatcher", daemon=True,
        )
        self._thread.start()

    def stop(self) -> None:
        if self._stop:
            self._stop.set()

    def _loop(self) -> None:
        while not self._stop.is_set():
            try:
                self._tick()
            except Exception as e:
                conn = self.get_conn()
                activity.append(
                    conn,
                    message=f"Dispatcher error: {e!r}",
                    severity="error", category="cli", actor="runner",
                )
            self._stop.wait(self.poll_interval)

    def _tick(self) -> None:
        conn = self.get_conn()
        paused = db.get_setting_int(conn, "paused", 0)
        if paused:
            return
        cap = db.get_setting_int(conn, "parallel_run_cap", 3)
        running = len(self.sub_mgr.running_snapshot())
        if running >= cap:
            return
        rows = conn.execute(
            "SELECT id, name, branch_name FROM gaps_index "
            "WHERE status = 'todo' "
            "ORDER BY updated ASC LIMIT ?",
            (cap - running,),
        ).fetchall()
        for row in rows:
            gid = row["id"]
            # already running? (race)
            if self.sub_mgr.is_running(gid):
                continue
            self._launch_one(conn, gid, row["branch_name"])

    def _launch_one(self, conn: sqlite3.Connection, gap_id: str,
                    existing_branch: str | None) -> None:
        # Retry pre-flight: if last failure was auth, re-check first. This is
        # a "soft" abort — leave the Gap in todo so a successful re-check
        # picks it up automatically.
        last = conn.execute(
            "SELECT failure_category FROM runs WHERE gap_id = ? "
            "ORDER BY id DESC LIMIT 1",
            (gap_id,),
        ).fetchone()
        if last and last["failure_category"] == "auth":
            ok, msg = preflight.check(conn)
            if not ok:
                activity.append(
                    conn,
                    message="Retry blocked — auth pre-flight still failing",
                    severity="warn", category="auth",
                    gap_id=gap_id, actor="runner", details=msg or "",
                )
                return

        # Pre-checks: branch + upstream
        current = git_ops.current_branch()
        if current is None:
            self._abort_to_failed(
                conn, gap_id,
                "Client repo is in detached-HEAD state — pickup aborted",
                category="git",
            )
            return
        upstream = git_ops.upstream_branch(current)
        if upstream is None:
            self._abort_to_failed(
                conn, gap_id,
                f"Branch `{current}` has no upstream — run `git push -u origin {current}` on the host",
                category="git",
            )
            return

        # Fetch fresh.
        r = git_ops.fetch()
        if not r.ok:
            self._abort_to_failed(
                conn, gap_id, "git fetch failed",
                category="git", details=r.stderr[:2000],
            )
            return

        # Compute the branch name + worktree.
        pattern = db.get_setting(conn, "branch_name_pattern", "refine/{gap_id}") or "refine/{gap_id}"
        branch_name = existing_branch or pattern.format(gap_id=gap_id)
        base_ref = f"origin/{current}"

        wt = git_ops.create_worktree(gap_id, base_ref, branch_name)
        if not wt.ok:
            self._abort_to_failed(
                conn, gap_id, "git worktree create failed",
                category="git", details=wt.stderr[:2000],
            )
            return

        # Read the Gap and compute the prompt from the latest round.
        gap = read_gap_json(gap_id)
        if not gap or not gap.get("rounds"):
            self._abort_to_failed(
                conn, gap_id, "Gap has no rounds — cannot launch",
                category="state",
            )
            return
        round_idx = len(gap["rounds"]) - 1
        latest = gap["rounds"][round_idx]
        prompt = _format_prompt(latest)

        # Capture base for "no commits produced" detection.
        rev = git_ops._run(
            ["rev-parse", "HEAD"], cwd=git_ops.gap_worktree_path(gap_id),
        )
        base_commit = rev.stdout.strip() if rev.ok else base_ref

        # Transition: todo → in-progress
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'in-progress', updated = ?, branch_name = ? "
                "WHERE id = ? AND status = 'todo'",
                (now_iso(), branch_name, gap_id),
            )
        activity.append(
            conn,
            message="Agent run started",
            severity="info", category="cli",
            gap_id=gap_id, actor="runner",
        )

        idle = db.get_setting_int(conn, "agent_idle_timeout_seconds", 900)
        cap = db.get_setting_int(conn, "agent_hard_cap_seconds", 86400)

        self.sub_mgr.launch(
            gap_id=gap_id,
            round_idx=round_idx,
            prompt=prompt,
            cwd=git_ops.gap_worktree_path(gap_id),
            base_ref=base_commit,
            idle_window=idle,
            hard_cap=cap,
            on_finished=lambda gid, code, reason: self._on_finished(
                gid, round_idx, code, reason, base_commit,
            ),
        )

    def _abort_to_failed(self, conn: sqlite3.Connection, gap_id: str,
                         message: str, *, category: str,
                         details: str | None = None) -> None:
        """Log a pre-launch failure and move the Gap to `failed` so the
        dispatcher stops re-attempting it every tick. The user can Reopen the
        Gap once the underlying environment issue is resolved."""
        activity.append(
            conn, message=message,
            severity="error", category=category,
            gap_id=gap_id, actor="runner", details=details or "",
        )
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'failed', updated = ? "
                "WHERE id = ? AND status = 'todo'",
                (now_iso(), gap_id),
            )

    def _on_finished(self, gap_id: str, round_idx: int, exit_code: int,
                     killed_reason: str | None, base_commit: str) -> None:
        conn = self.get_conn()
        cwd = git_ops.gap_worktree_path(gap_id)
        new_commits = git_ops.commits_on_branch_since(base_commit, cwd)
        no_new_commits = new_commits == 0

        outcome = classify_outcome(
            exit_code=exit_code,
            killed_reason=killed_reason,
            no_new_commits=no_new_commits,
        )

        success = outcome.kind == "success"
        next_status = "review" if success else "failed"

        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = ?, updated = ? WHERE id = ?",
                (next_status, now_iso(), gap_id),
            )

        try:
            gap_writer.append_round_log(
                gap_id=gap_id,
                round_idx=round_idx,
                severity="info" if success else outcome.severity,
                category=outcome.category,
                message=outcome.message,
                details=outcome.details,
                actor="runner",
            )
        except Exception:
            pass

        activity.append(
            conn,
            message=outcome.message,
            severity="info" if success else outcome.severity,
            category=outcome.category,
            gap_id=gap_id, actor="runner",
            details=outcome.details,
        )


def _format_prompt(round_obj: dict) -> str:
    return (
        f"You are working on a software change.\n\n"
        f"Current behavior (actual):\n{round_obj.get('actual','').strip()}\n\n"
        f"Desired behavior (target):\n{round_obj.get('target','').strip()}\n\n"
        f"Make the necessary code changes in this worktree. Commit your changes "
        f"with clear messages. Run any relevant tests. When you're satisfied, exit."
    )
