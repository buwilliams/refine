"""Main runner orchestrator for dispatcher, subprocess manager, and backend calls.

The runner is the sole writer of gap.json.
"""
from __future__ import annotations

import json
import os
import sqlite3
import threading
from pathlib import Path
from typing import Any

from refine_server import activity, changes_index, config, db, gaps as shared_gaps, governance, perf_metrics, project_state, quality, reporters, round_logs, search_index
from refine_server.gaps import now_iso
from refine_server.backend_protocol import (
    M_APPEND_ROUND, M_BACKGROUND_PROCESSES_SET, M_CANCEL, M_CANCEL_ALL, M_CHAT_INPUT, M_CHAT_READ,
    M_CHAT_RESET_ALL, M_CHAT_START, M_CHAT_STOP, M_CREATE_GAP, M_DELETE_GAP, M_DIAGNOSTICS, M_EDIT_ROUND,
    M_BULK_DELETE_GAPS, M_BULK_UPDATE_GAPS, M_ENFORCE_SCHEDULING, M_EXTRACT_GAPS, M_LAUNCH, M_LIST_CHANGES, M_LOG_APPEND, M_PING,
    M_GOVERNANCE_GENERATE_RULES, M_GOVERNANCE_GET, M_GOVERNANCE_SAVE,
    M_GOVERNANCE_WAKE, M_MERGE_REPORTER, M_PREFLIGHT, M_RENAME_REPORTER, M_RENAME_REPORTER_STRINGS,
    M_RETRY_MERGE, M_RETRY_QA, M_RUNNING,
    M_HARD_RESET_WORKTREE, M_PROJECT_SYNC, M_REGRESSION_RUN, M_SET_NOTES, M_TARGET_APP_GENERATE, M_TARGET_APP_HEALTH,
    M_TARGET_APP_REBUILD_PENDING, M_TARGET_APP_REBUILD_QUEUE, M_TARGET_APP_RUN, M_UNDO_GAP, M_VERIFY,
)

from . import dispatcher as _dispatcher
from . import gap_writer, git_ops, llm, merger as _merger, mutation_guard, preflight, project_sync, push_ops, recovery, regressions, state_committer, subprocess_mgr, target_app, target_app_rebuilder, verify_op
from .chat_mgr import ChatManager
from .governance_agent import GovernanceAgent


def _combine_step_tail(steps: list[dict[str, Any]], key: str) -> str:
    parts = []
    for step in steps:
        text = (step.get(key) or "").strip()
        if text:
            parts.append(f"{step.get('kind') or 'step'}:\n{text}")
    return "\n\n".join(parts)


def _automatic_rebuild_details(result: dict[str, Any]) -> str | None:
    parts = []
    steps = result.get("steps") or []
    if steps:
        parts.append("steps:\n" + json.dumps(steps, indent=2))
    if result.get("stdout_tail"):
        parts.append("stdout:\n" + str(result["stdout_tail"]))
    if result.get("stderr_tail"):
        parts.append("stderr:\n" + str(result["stderr_tail"]))
    if result.get("checks"):
        parts.append("checks:\n" + json.dumps(result["checks"], indent=2))
    return "\n\n".join(parts) if parts else None


class Runner:
    def __init__(self) -> None:
        self._conn_lock = threading.Lock()
        # Use a single shared connection — sqlite3 connections are not strictly
        # thread-safe by default, but with check_same_thread=False and our own
        # lock around transactions, it's fine for our usage pattern.
        from refine_server.paths import sqlite_path
        self._conn = sqlite3.connect(str(sqlite_path()), check_same_thread=False,
                                     isolation_level=None, timeout=5.0)
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode = WAL")
        self._conn.execute("PRAGMA synchronous = NORMAL")
        self._conn.execute("PRAGMA foreign_keys = ON")
        db.register_shared_connection(self._conn)
        self.sub_mgr = subprocess_mgr.SubprocessManager(self._get_conn)
        self._target_app_lock = threading.Lock()
        self._bulk_update_lock = threading.Lock()
        self.target_app_rebuilder = target_app_rebuilder.TargetAppRebuilder(
            get_conn=self._get_conn,
            run_rebuild=self._run_automatic_target_app_rebuild,
        )
        # The merger owns the host worktree — everything that merges,
        # auto-resolves conflicts, or cleans stale git op state goes
        # through it. See refine_server/merger.py for the rationale.
        self.merger = _merger.Merger(
            get_conn=self._get_conn, sub_mgr=self.sub_mgr,
            on_worktree_merged=self.target_app_rebuilder.queue_for_worktree_merge,
            queue_rebuild_for_pending=(
                self.target_app_rebuilder.queue_pending_awaiting_rebuild
            ),
        )
        self.dispatcher = _dispatcher.Dispatcher(
            get_conn=self._get_conn, sub_mgr=self.sub_mgr,
            on_run_finished=lambda _gid: self.merger.wake(),
            launch_blocked=self._automation_blocked,
            on_post_rebuild_quality_failed=self._handle_post_rebuild_quality_failure,
            target_app_lock=self._target_app_lock,
        )
        self.governance_agent = GovernanceAgent(
            get_conn=self._get_governance_conn,
            on_pass=lambda _gid: self.dispatcher.enforce_now(),
            close_conn=True,
        )
        self.chat = ChatManager(
            get_standalone_idle_timeout=lambda: db.get_setting_int(
                self._conn, "chat_idle_timeout_seconds", 300,
            ),
        )
        self.state_committer = state_committer.StateCommitter(
            get_conn=self._get_conn,
            mutation_blocked=self._automation_blocked,
        )
        self._diag_lock = threading.Lock()
        self._last_call_at: str | None = None
        self._recent_errors: list[str] = []
        self._closed = False

    def _get_conn(self) -> sqlite3.Connection:
        project_state.ensure_sqlite_cache_current(self._conn)
        return self._conn

    def _get_governance_conn(self) -> sqlite3.Connection:
        conn = db.connect()
        project_state.ensure_sqlite_cache_current(conn)
        return conn

    def _automation_blocked(self) -> bool:
        return (
            self._bulk_update_lock.locked()
            or mutation_guard.active() is not None
            or project_state.read_maintenance() is not None
        )

    # ---- lifecycle -----------------------------------------------------------

    def start(self) -> None:
        db.init_db()
        project_state.ensure_sqlite_cache_current(self._conn)
        recovery.reconcile_on_start(self._conn)
        preflight.check(self._conn)
        self.governance_agent.start()
        self.dispatcher.start()
        self.merger.start()
        self.target_app_rebuilder.start()
        self.state_committer.start()
        activity.append(
            self._conn, message="refine-server started",
            severity="info", category="state", actor="runner",
        )

    def shutdown(self) -> None:
        if self._closed:
            return
        self.chat.shutdown()
        self.sub_mgr.cancel_all("shutdown")
        try:
            self.state_committer.commit_now()
        except Exception:
            pass
        self.state_committer.stop()
        self.target_app_rebuilder.stop()
        self.governance_agent.stop()
        self.merger.stop()
        self.dispatcher.stop()
        try:
            activity.append(
                self._conn, message="refine-server stopping",
                severity="info", category="state", actor="runner",
            )
        finally:
            db.unregister_shared_connection(self._conn)
            self._conn.close()
            self._closed = True
    # ---- direct backend routing ---------------------------------------------

    def call(self, method: str, params: dict | None = None) -> dict:
        with self._diag_lock:
            self._last_call_at = now_iso()
        try:
            project_state.ensure_sqlite_cache_current(self._conn)
            return self._dispatch_method(method, params or {})
        except Exception as e:
            with self._diag_lock:
                self._recent_errors.append(f"{now_iso()} {method}: {e!r}")
            raise

    def _dispatch_method(self, method: str, params: dict) -> dict:
        handlers = {
            M_PING: self._h_ping,
            M_PREFLIGHT: self._h_preflight,
            M_RUNNING: self._h_running,
            M_DIAGNOSTICS: self._h_diagnostics,
            M_LAUNCH: self._h_launch,
            M_ENFORCE_SCHEDULING: self._h_enforce_scheduling,
            M_CANCEL: self._h_cancel,
            M_CANCEL_ALL: self._h_cancel_all,
            M_BACKGROUND_PROCESSES_SET: self._h_background_processes_set,
            M_VERIFY: self._h_verify,
            M_RETRY_MERGE: self._h_retry_merge,
            M_RETRY_QA: self._h_retry_qa,
            M_CREATE_GAP: self._h_create_gap,
            M_APPEND_ROUND: self._h_append_round,
            M_EDIT_ROUND: self._h_edit_round,
            M_BULK_UPDATE_GAPS: self._h_bulk_update_gaps,
            M_LOG_APPEND: self._h_log_append,
            M_DELETE_GAP: self._h_delete_gap,
            M_BULK_DELETE_GAPS: self._h_bulk_delete_gaps,
            M_SET_NOTES: self._h_set_notes,
            M_CHAT_START: self._h_chat_start,
            M_CHAT_INPUT: self._h_chat_input,
            M_CHAT_READ: self._h_chat_read,
            M_CHAT_STOP: self._h_chat_stop,
            M_CHAT_RESET_ALL: self._h_chat_reset_all,
            M_EXTRACT_GAPS: self._h_extract_gaps,
            M_MERGE_REPORTER: self._h_merge_reporter,
            M_RENAME_REPORTER: self._h_rename_reporter,
            M_RENAME_REPORTER_STRINGS: self._h_rename_reporter_strings,
            M_LIST_CHANGES: self._h_list_changes,
            M_UNDO_GAP: self._h_undo_gap,
            M_GOVERNANCE_GET: self._h_governance_get,
            M_GOVERNANCE_SAVE: self._h_governance_save,
            M_GOVERNANCE_GENERATE_RULES: self._h_governance_generate_rules,
            M_GOVERNANCE_WAKE: self._h_governance_wake,
            M_TARGET_APP_RUN: self._h_target_app_run,
            M_TARGET_APP_REBUILD_QUEUE: self._h_target_app_rebuild_queue,
            M_TARGET_APP_REBUILD_PENDING: self._h_target_app_rebuild_pending,
            M_TARGET_APP_GENERATE: self._h_target_app_generate,
            M_TARGET_APP_HEALTH: self._h_target_app_health,
            M_REGRESSION_RUN: self._h_regression_run,
            M_PROJECT_SYNC: self._h_project_sync,
            M_HARD_RESET_WORKTREE: self._h_hard_reset_worktree,
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

    def _h_project_sync(self, _: dict) -> dict:
        return self.merger.run_under_host_lock(
            lambda: project_sync.sync_latest(self._conn, actor="runner"),
            label="project sync",
        )

    def _background_processes_stopped(self) -> dict | None:
        if not db.get_setting_int(self._conn, "paused", 0):
            return None
        return {
            "ok": False,
            "code": "background_processes_stopped",
            "message": (
                "Background processes are stopped. Start Background before "
                "running worker actions."
            ),
        }

    def _agents_paused(self) -> bool:
        return bool(
            db.get_setting_int(self._conn, "paused", 0)
            or db.get_setting_int(self._conn, "agents_paused", 0)
        )

    def _h_hard_reset_worktree(self, _: dict) -> dict:
        stopped = self._background_processes_stopped()
        if stopped is not None:
            return stopped
        with mutation_guard.exclusive(
            "Hard worktree reset",
            kind="hard_worktree_reset",
            blocking=True,
        ):
            result = self.merger.run_under_host_lock(
                lambda: self._hard_reset_target_worktree(),
                label="hard worktree reset",
            )
        if result.get("ok"):
            try:
                project_state.rebuild_sqlite_cache(self._conn)
            except Exception as e:
                result["cache_rebuild_error"] = repr(e)
            self.dispatcher.enforce_now()
            self.governance_agent.wake()
            self.merger.wake()
        return result

    def status_snapshot(self) -> dict:
        """Return live runner state without routing through `call()`.

        Dashboard/status views use this as best-effort runtime context. It must
        stay cheap and avoid cache checks or SQLite reads so UI refreshes do not
        queue behind writer-heavy agent work.
        """
        return {
            "pid": os.getpid(),
            "running": self.sub_mgr.running_snapshot(),
            "chat": self.chat.snapshot(),
            "merger": self.merger.snapshot(),
            "governance": self.governance_agent.snapshot(),
            "target_app_rebuild": self.target_app_rebuilder.snapshot(),
        }

    def _h_running(self, _: dict) -> dict:
        return self.status_snapshot()

    def _h_diagnostics(self, _: dict) -> dict:
        with self._diag_lock:
            return {
                "mode": (
                    "worker-process"
                    if os.environ.get("REFINE_RUNNER_SOCKET") else "in-process"
                ),
                "last_call_at": self._last_call_at,
                "recent_errors": list(self._recent_errors[-10:]),
            }

    def _require_active_gap(
        self,
        gap_id: str,
        *,
        columns: str = "status, branch_name, node_id",
    ) -> sqlite3.Row:
        row = self._conn.execute(
            f"SELECT {columns} FROM gaps_index WHERE id = ?",
            (gap_id,),
        ).fetchone()
        if not row:
            raise ValueError("Gap not found")
        active = project_state.active_node_id()
        owner = str(row["node_id"] or project_state.DEFAULT_NODE_ID)
        if owner != active:
            owner_name = project_state.gap_node_display(owner)
            active_name = project_state.gap_node_display(active)
            raise ValueError(
                "Action not allowed: Gap is owned by another node "
                f"({owner_name}). Transfer to {active_name} before making changes."
            )
        return row

    def _h_launch(self, params: dict) -> dict:
        # The dispatcher launches automatically; this method exists mostly so
        # the webapp can nudge scheduling after a status change.
        stopped = self._background_processes_stopped()
        if stopped is not None:
            return {"queued": False, **stopped}
        self.dispatcher.enforce_now()
        self.governance_agent.wake()
        return {"queued": True}

    def _h_enforce_scheduling(self, params: dict) -> dict:
        if self._agents_paused():
            killed, still_running = self.sub_mgr.cancel_all_and_wait(
                "paused",
                timeout=float(params.get("settle_timeout_seconds") or 8.0),
            )
            cleanup = self._clean_target_worktree_for_pause()
            self.governance_agent.wake()
            return {
                "ok": bool(cleanup.get("ok")),
                "killed_subprocesses": killed,
                "still_running": still_running,
                "target_worktree_clean": bool(cleanup.get("clean")),
                "cleanup": cleanup,
            }
        self.dispatcher.enforce_now()
        self.governance_agent.wake()
        return {"ok": True}

    def _clean_target_worktree_for_pause(self) -> dict:
        """Leave the host/target worktree clean after the operator pauses agents."""
        def commit_and_stash() -> dict:
            committed = False
            try:
                committed = self.state_committer.commit_now(
                    ignore_mutation_block=True,
                )
            except Exception as e:
                return {
                    "ok": False,
                    "clean": False,
                    "stage": "commit_refine_state",
                    "message": f"could not commit Refine state after pause: {e!r}",
                }
            stuck = git_ops.in_progress_op()
            dirty_paths = self._target_worktree_dirty_paths()
            if stuck:
                op_name, hint = stuck
                return {
                    "ok": False,
                    "clean": False,
                    "stage": "git_operation",
                    "message": (
                        f"target worktree still has unfinished `{op_name}` "
                        f"after pause cleanup. {hint}"
                    ),
                }
            if dirty_paths:
                stash = git_ops.stash_push(
                    "refine pause cleanup auto-stash",
                )
                if not stash.ok:
                    return {
                        "ok": False,
                        "clean": False,
                        "stage": "dirty_worktree",
                        "message": "could not stash dirty target worktree after pause",
                        "details": stash.stderr or stash.stdout,
                    }

                still_stuck = git_ops.in_progress_op()
                still_dirty = self._target_worktree_dirty_paths()
                if still_stuck or still_dirty:
                    op_name = still_stuck[0] if still_stuck else ""
                    return {
                        "ok": False,
                        "clean": False,
                        "stage": "dirty_worktree",
                        "message": "target worktree is still dirty after pause stash",
                        "details": (
                            (f"unfinished git operation: {op_name}\n" if op_name else "")
                            + "\n".join(still_dirty[:200])
                        ).strip(),
                    }

                activity.append(
                    self._get_conn(),
                    message="Auto-stashed dirty target worktree after pausing agents",
                    severity="info", category="git", actor="runner",
                    details="\n".join(dirty_paths[:200]),
                )
                return {
                    "ok": True,
                    "clean": True,
                    "committed": committed,
                    "stashed": True,
                    "dirty_paths": dirty_paths,
                }
            return {"ok": True, "clean": True, "committed": committed}

        return self.merger.run_under_host_lock(
            commit_and_stash,
            label="pause agents",
        )

    def _clean_target_worktree_for_app_start(self) -> dict:
        """Leave the host/target worktree clean before starting the app."""
        def commit_and_stash() -> dict:
            committed = False
            try:
                committed = self.state_committer.commit_now(
                    ignore_mutation_block=True,
                )
            except Exception as e:
                return {
                    "ok": False,
                    "clean": False,
                    "stage": "commit_refine_state",
                    "message": f"could not commit Refine state before app start: {e!r}",
                }

            stuck = git_ops.in_progress_op()
            if stuck:
                op_name, hint = stuck
                return {
                    "ok": False,
                    "clean": False,
                    "stage": "git_operation",
                    "message": (
                        f"target worktree still has unfinished `{op_name}` "
                        f"before app start. {hint}"
                    ),
                }

            dirty_paths = self._target_worktree_dirty_paths()
            if not dirty_paths:
                return {"ok": True, "clean": True, "committed": committed}

            stash = git_ops.stash_push(
                "refine target-app start auto-stash",
            )
            if not stash.ok:
                return {
                    "ok": False,
                    "clean": False,
                    "stage": "dirty_worktree",
                    "message": "could not stash dirty target worktree before app start",
                    "details": stash.stderr or stash.stdout,
                }

            still_stuck = git_ops.in_progress_op()
            still_dirty = self._target_worktree_dirty_paths()
            if still_stuck or still_dirty:
                op_name = still_stuck[0] if still_stuck else ""
                return {
                    "ok": False,
                    "clean": False,
                    "stage": "dirty_worktree",
                    "message": "target worktree is still dirty after app-start stash",
                    "details": (
                        (f"unfinished git operation: {op_name}\n" if op_name else "")
                        + "\n".join(still_dirty[:200])
                    ).strip(),
                }

            activity.append(
                self._get_conn(),
                message="Auto-stashed dirty target worktree before target-app start",
                severity="info", category="git", actor="runner",
                details="\n".join(dirty_paths[:200]),
            )
            return {
                "ok": True,
                "clean": True,
                "committed": committed,
                "stashed": True,
                "dirty_paths": dirty_paths,
            }

        return self.merger.run_under_host_lock(
            commit_and_stash,
            label="target-app start",
        )

    def _hard_reset_target_worktree(self) -> dict:
        """Destructively recover the host target worktree.

        This is an operator escape hatch for a dedicated Refine checkout: abort
        stale git state first via the merger's normal pre-cleanup, then make
        the current branch match its upstream. If no upstream is configured,
        reset to the current HEAD and only clean the worktree.
        """
        repo = config.get().client_repo
        branch = git_ops.current_branch(cwd=repo)
        upstream = git_ops.upstream_branch(branch, cwd=repo) if branch else None
        before_head = git_ops.rev_parse("HEAD", cwd=repo) or ""
        before_dirty = self._target_worktree_dirty_paths()
        target_ref = upstream or "HEAD"

        if upstream:
            fetched = git_ops.fetch(cwd=repo)
            if not fetched.ok:
                return {
                    "ok": False,
                    "clean": False,
                    "stage": "fetch",
                    "branch": branch or "",
                    "upstream": upstream,
                    "message": "Could not fetch before hard worktree reset.",
                    "details": fetched.stderr or fetched.stdout,
                }

        reset = git_ops.reset_hard(target_ref, cwd=repo)
        if not reset.ok:
            return {
                "ok": False,
                "clean": False,
                "stage": "reset",
                "branch": branch or "",
                "upstream": upstream or "",
                "target_ref": target_ref,
                "message": "Could not hard reset the target worktree.",
                "details": reset.stderr or reset.stdout,
            }

        clean = git_ops.clean_untracked(cwd=repo)
        if not clean.ok:
            return {
                "ok": False,
                "clean": False,
                "stage": "clean",
                "branch": branch or "",
                "upstream": upstream or "",
                "target_ref": target_ref,
                "message": "Could not clean untracked files after hard reset.",
                "details": clean.stderr or clean.stdout,
            }

        stuck = git_ops.in_progress_op(cwd=repo)
        dirty_after = self._target_worktree_dirty_paths()
        after_head = git_ops.rev_parse("HEAD", cwd=repo) or ""
        if stuck or dirty_after:
            op_name = stuck[0] if stuck else ""
            return {
                "ok": False,
                "clean": False,
                "stage": "verify",
                "branch": branch or "",
                "upstream": upstream or "",
                "target_ref": target_ref,
                "before_head": before_head,
                "after_head": after_head,
                "message": "Target worktree is still not clean after hard reset.",
                "details": (
                    (f"unfinished git operation: {op_name}\n" if op_name else "")
                    + "\n".join(dirty_after[:200])
                ).strip(),
            }

        activity.append(
            self._get_conn(),
            message=(
                f"Hard reset target worktree to `{target_ref}`"
                if upstream else "Hard reset target worktree to HEAD"
            ),
            severity="warn",
            category="git",
            actor="runner",
            details=(
                f"branch: {branch or '(detached)'}\n"
                f"upstream: {upstream or ''}\n"
                f"before: {before_head}\n"
                f"after: {after_head}\n"
                f"discarded paths:\n" + "\n".join(before_dirty[:200])
            ).strip(),
        )
        return {
            "ok": True,
            "clean": True,
            "stage": "reset",
            "branch": branch or "",
            "upstream": upstream or "",
            "target_ref": target_ref,
            "before_head": before_head,
            "after_head": after_head,
            "discarded_paths": before_dirty,
            "reset_stdout": reset.stdout,
            "clean_stdout": clean.stdout,
            "message": (
                f"Hard reset `{branch}` to `{upstream}` and cleaned the worktree."
                if branch and upstream
                else "Hard reset and cleaned the target worktree."
            ),
        }

    def _target_worktree_dirty_paths(self) -> list[str]:
        paths = git_ops.dirty_paths()
        try:
            repo = config.get().client_repo.resolve()
            run_dir = config.local_run_dir().resolve()
            run_rel = run_dir.relative_to(repo).as_posix()
        except Exception:
            run_rel = ""
        if not run_rel:
            return paths
        git_ops.ensure_info_exclude(f"/{run_rel.rstrip('/')}/")
        paths = git_ops.dirty_paths()
        return [
            path for path in paths
            if path != run_rel and not path.startswith(run_rel.rstrip("/") + "/")
        ]

    def _h_cancel(self, params: dict) -> dict:
        gap_id = params["gap_id"]
        row = self._require_active_gap(gap_id)
        killed = self.sub_mgr.cancel(gap_id)
        # Move to cancelled (terminal). Clean up worktree + branch.
        if row:
            with db.transaction(self._conn):
                self._conn.execute(
                    "UPDATE gaps_index SET status = 'cancelled', updated = ? WHERE id = ?",
                    (now_iso(), gap_id),
                )
            git_ops.remove_worktree(gap_id)
            if row["branch_name"]:
                git_ops.delete_branch(row["branch_name"])
            try:
                gap_writer.update_fields(gap_id, status="cancelled")
                gap_writer.append_latest_round_log(
                    gap_id=gap_id,
                    severity="info",
                    category="state",
                    actor="refine",
                    message=f"Workflow status changed: {row['status']} → cancelled",
                )
            except Exception:
                pass
            activity.append(
                self._conn, message="Gap cancelled",
                severity="info", category="state",
                gap_id=gap_id, actor="refine",
            )
        return {"killed_subprocess": killed}

    def _h_cancel_all(self, params: dict) -> dict:
        reason = params.get("reason") or "paused"
        killed = self.sub_mgr.cancel_all(str(reason))
        return {"killed_subprocesses": killed}

    def _h_background_processes_set(self, params: dict) -> dict:
        stopped = bool(params.get("stopped"))
        if stopped:
            killed, still_running = self.sub_mgr.cancel_all_and_wait(
                "background_processes_stopped",
                timeout=float(params.get("settle_timeout_seconds") or 8.0),
            )
            cleanup = self._clean_target_worktree_for_pause()
            stopped_chats = self.chat.stop_all(reason="background processes stopped")
            rebuild_stop = self.target_app_rebuilder.stop_background_work(timeout=8.0)
            self.governance_agent.wake()
            self.merger.wake()
            return {
                "ok": bool(cleanup.get("ok")),
                "stopped": True,
                "killed_subprocesses": killed,
                "still_running": still_running,
                "target_worktree_clean": bool(cleanup.get("clean")),
                "cleanup": cleanup,
                "stopped_chats": stopped_chats,
                "cleared_target_app_rebuild_queue": rebuild_stop.get("cleared_queue"),
                "target_app_rebuild": rebuild_stop,
            }
        self.dispatcher.enforce_now()
        self.governance_agent.wake()
        self.merger.wake()
        self.target_app_rebuilder.queue_pending_awaiting_rebuild()
        return {"stopped": False, "started": True}

    def _h_verify(self, params: dict) -> dict:
        # Verify is user approval for a Gap already parked in `review`.
        # The Merge agent owns all git merge work while Gaps are in
        # `ready-merge`.
        self._require_active_gap(params["gap_id"])
        return verify_op.approve_review(self._conn, params["gap_id"])

    def _h_retry_merge(self, params: dict) -> dict:
        gap_id = params["gap_id"]
        row = self._require_active_gap(
            gap_id,
            columns="status, branch_name, node_id",
        )
        if row["status"] != "failed":
            return {
                "ok": False,
                "message": (
                    "Retry merge is only valid from failed "
                    f"(status={row['status']})"
                ),
            }
        if not self._failed_from_ready_merge(gap_id):
            return {
                "ok": False,
                "message": "Retry merge is only valid after a failed merge attempt.",
            }
        branch = row["branch_name"]
        if not branch:
            return {
                "ok": False,
                "message": "Retry merge needs the Gap branch to still exist.",
            }
        if not git_ops.local_branch_exists(branch):
            return {
                "ok": False,
                "message": f"Retry merge needs local branch `{branch}`.",
            }
        with db.transaction(self._conn):
            self._conn.execute(
                "UPDATE gaps_index SET status = 'ready-merge', updated = ? "
                "WHERE id = ?",
                (now_iso(), gap_id),
            )
        try:
            gap_writer.update_fields(gap_id, status="ready-merge")
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="info",
                category="state",
                actor="refine",
                message=(
                    "Workflow status changed: failed → ready-merge; "
                    "merge retry requested"
                ),
            )
        except Exception:
            pass
        activity.append(
            self._conn,
            message="Merge retry requested",
            severity="info",
            category="state",
            gap_id=gap_id,
            actor="refine",
        )
        self.merger.wake()
        return {"ok": True, "message": "Queued for merge"}

    def _h_retry_qa(self, params: dict) -> dict:
        gap_id = params["gap_id"]
        row = self._require_active_gap(
            gap_id,
            columns="status, branch_name, node_id",
        )
        if row["status"] != "failed":
            return {
                "ok": False,
                "message": f"Retry QA is only valid from failed (status={row['status']})",
            }
        if not self._failed_from_quality(gap_id):
            return {
                "ok": False,
                "message": "Retry QA is only valid after a failed Quality run.",
            }
        branch = row["branch_name"]
        if not branch:
            return {"ok": False, "message": "Retry QA needs the Gap branch to still exist."}
        if not git_ops.local_branch_exists(branch):
            return {"ok": False, "message": f"Retry QA needs local branch `{branch}`."}
        if not git_ops.gap_worktree_path(gap_id).exists():
            return {"ok": False, "message": "Retry QA needs the Gap worktree to still exist."}
        with db.transaction(self._conn):
            self._conn.execute(
                "UPDATE gaps_index SET status = 'qa', updated = ? WHERE id = ?",
                (now_iso(), gap_id),
            )
        try:
            gap_writer.update_fields(gap_id, status="qa")
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="info",
                category="state",
                actor="refine",
                message="Workflow status changed: failed → qa; QA retry requested",
            )
        except Exception:
            pass
        activity.append(
            self._conn,
            message="QA retry requested",
            severity="info",
            category="quality",
            gap_id=gap_id,
            actor="refine",
        )
        self.dispatcher.enforce_now()
        return {"ok": True, "message": "Queued for QA"}

    def _failed_from_ready_merge(self, gap_id: str) -> bool:
        gap = shared_gaps.read_gap_json(gap_id, include_logs=False) or {}
        rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
        if not rounds:
            return False
        latest_workflow_log = round_logs.latest_workflow_for_round(
            gap_id,
            len(rounds) - 1,
        )
        msg = str((latest_workflow_log or {}).get("message") or "")
        return (
            "Workflow status changed:" in msg
            and "ready-merge" in msg
            and "failed" in msg
        )

    def _failed_from_quality(self, gap_id: str) -> bool:
        gap = shared_gaps.read_gap_json(gap_id, include_logs=False) or {}
        rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
        if not rounds:
            return False
        latest_workflow_log = round_logs.latest_workflow_for_round(
            gap_id,
            len(rounds) - 1,
        )
        msg = str((latest_workflow_log or {}).get("message") or "")
        return (
            "Workflow status changed:" in msg
            and "qa" in msg
            and "failed" in msg
        )

    def _h_create_gap(self, params: dict) -> dict:
        gap_id = params["gap_id"]
        name = params.get("name", "Untitled Gap")
        priority = _normalize_priority(params.get("priority"))
        node_id = str(params.get("node_id") or project_state.active_node_id())
        round_obj = shared_gaps.new_round(
            reporter=params["reporter"],
            actual=params.get("actual", ""),
            target=params.get("target", ""),
        )
        gap = gap_writer.create_gap(
            gap_id=gap_id, name=name, initial_round=round_obj,
            status="backlog", priority=priority, node_id=node_id,
        )

        from refine_server.paths import relative_gap_path
        with db.transaction(self._conn):
            self._conn.execute(
                "INSERT INTO gaps_index "
                "(id, name, status, priority, reporter, created, updated, node_id, json_path) "
                "VALUES (?, ?, 'backlog', ?, ?, ?, ?, ?, ?) "
                "ON CONFLICT(id) DO UPDATE SET "
                "name = excluded.name, "
                "status = excluded.status, "
                "priority = excluded.priority, "
                "reporter = excluded.reporter, "
                "created = excluded.created, "
                "updated = excluded.updated, "
                "node_id = excluded.node_id, "
                "json_path = excluded.json_path",
                (gap_id, name, priority, params["reporter"],
                 gap["created"], gap["updated"], node_id, relative_gap_path(gap_id)),
            )
            search_index.upsert_gap(self._conn, gap)
        # ensure reporter exists in dropdown list
        try:
            reporters.add(self._conn, params["reporter"])
        except Exception:
            pass
        try:
            activity.append(
                self._conn, message=f"Gap created: {name}",
                severity="info", category="state",
                gap_id=gap_id, actor=params["reporter"],
            )
        except Exception:
            pass
        try:
            self.governance_agent.wake()
        except Exception:
            pass
        return {"gap": gap}

    def _h_append_round(self, params: dict) -> dict:
        gap_id = params["gap_id"]
        row = self._require_active_gap(gap_id, columns="status, node_id")
        round_obj = shared_gaps.new_round(
            reporter=params["reporter"],
            actual=params.get("actual", ""),
            target=params.get("target", ""),
        )
        gap = gap_writer.append_round(gap_id, round_obj)
        # review → todo (or todo if currently failed/review/done; webapp guards this).
        # The new round's reporter is now the latest, so mirror that onto
        # the index column.
        with db.transaction(self._conn):
            self._conn.execute(
                "UPDATE gaps_index SET status = 'todo', reporter = ?, updated = ? "
                "WHERE id = ?",
                (params["reporter"], now_iso(), gap_id),
            )
            search_index.upsert_gap(self._conn, gap)
        try:
            gap_writer.update_fields(gap_id, status="todo")
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="info",
                category="state",
                actor=params["reporter"],
                message=f"Workflow status changed: {row['status']} → todo; new round submitted",
            )
        except Exception:
            pass
        try:
            reporters.add(self._conn, params["reporter"])
        except Exception:
            pass
        activity.append(
            self._conn, message="New round submitted",
            severity="info", category="state",
            gap_id=gap_id, actor=params["reporter"],
        )
        self.dispatcher.enforce_now()
        self.governance_agent.wake()
        return {"gap": gap}

    def _h_edit_round(self, params: dict) -> dict:
        self._require_active_gap(params["gap_id"], columns="status, node_id")
        gap = gap_writer.edit_latest_round(
            params["gap_id"],
            actual=params.get("actual"),
            target=params.get("target"),
            reporter=params.get("reporter"),
        )
        if params.get("actual") is not None or params.get("target") is not None:
            round_idx = max(0, len(gap.get("rounds") or []) - 1)
            with db.transaction(self._conn):
                self._conn.execute(
                    "DELETE FROM guidance_decisions "
                    "WHERE gap_id = ? AND round_idx = ?",
                    (params["gap_id"], round_idx),
                )
        # Keep the index reporter in sync with the latest round when the
        # reporter was actually changed by this edit. (None means "leave
        # it alone" in `edit_latest_round`.)
        if params.get("reporter") is not None:
            with db.transaction(self._conn):
                self._conn.execute(
                    "UPDATE gaps_index SET reporter = ? WHERE id = ?",
                    (params["reporter"], params["gap_id"]),
                )
                search_index.upsert_gap(self._conn, gap)
        else:
            with db.transaction(self._conn):
                search_index.upsert_gap(self._conn, gap)
        return {"gap": gap}

    def _h_bulk_update_gaps(self, params: dict) -> dict:
        """Apply one bulk metadata update in runner-owned order.

        The UI sends an ordered id list after filter validation. The runner
        owns the SQLite write, gap.json mutation, round touch/reporter edit,
        and scheduling wake-up behind a single bulk lock so overlapping bulk
        requests do not interleave per-Gap file writes.
        """
        field = str(params.get("field") or "").strip().lower()
        value = str(params.get("value") or "").strip()
        raw_ids = params.get("gap_ids") or []
        if field not in {"priority", "status", "reporter"}:
            raise ValueError("field must be one of priority, status, or reporter")
        if field == "priority":
            value = _normalize_priority_required(value)
        elif field == "status":
            value = (
                _BULK_LAST_WORKFLOW_STATUS
                if value.strip().lower() == _BULK_LAST_WORKFLOW_STATUS
                else _normalize_status_required(value)
            )
        elif not value:
            raise ValueError("reporter is required")
        if not isinstance(raw_ids, list):
            raise ValueError("gap_ids must be a list")
        gap_ids = _unique_strings(raw_ids)
        if not gap_ids:
            return {
                "updated": 0,
                "ids": [],
                "field": field,
                "value": value,
                "failed": 0,
                "failures": [],
                "progress": {"completed": 0, "total": 0},
            }
        with self._bulk_update_lock:
            result = self._bulk_update_gaps_locked(field, value, gap_ids)
        if field in {"priority", "status"}:
            self.dispatcher.enforce_now()
            self.governance_agent.wake()
        if int(result.get("ready_merge") or 0) > 0:
            self.merger.wake()
        return result

    def _bulk_update_gaps_locked(
        self,
        field: str,
        value: str,
        gap_ids: list[str],
    ) -> dict:
        active = project_state.active_node_id()
        rows: list[sqlite3.Row] = []
        for chunk in _chunks(gap_ids):
            placeholders = ",".join("?" * len(chunk))
            rows.extend(self._conn.execute(
                "SELECT id, status, branch_name, node_id FROM gaps_index "
                f"WHERE id IN ({placeholders})",
                chunk,
            ).fetchall())
        by_id = {row["id"]: row for row in rows}
        missing = [gid for gid in gap_ids if gid not in by_id]
        if missing:
            raise ValueError(f"Gap not found: {missing[0]}")
        owners = {
            str(row["node_id"] or project_state.DEFAULT_NODE_ID)
            for row in rows
        }
        if owners != {active}:
            owner = sorted(owners - {active})[0]
            owner_name = project_state.gap_node_display(owner)
            active_name = project_state.gap_node_display(active)
            raise ValueError(
                "Action not allowed: Gap is owned by another node "
                f"({owner_name}). Transfer to {active_name} before making changes."
            )

        updated_ids: list[str] = []
        failures: list[dict[str, str]] = []
        previous_status_by_id = {
            gid: str(by_id[gid]["status"] or "")
            for gid in gap_ids
        }
        now = now_iso()

        if field == "status" and value == _BULK_LAST_WORKFLOW_STATUS:
            return self._bulk_restore_last_workflow_state_locked(
                gap_ids,
                by_id,
                now,
                active,
            )

        if field in {"priority", "status"}:
            with db.transaction(self._conn):
                for chunk in _chunks(gap_ids):
                    placeholders = ",".join("?" * len(chunk))
                    self._conn.execute(
                        f"UPDATE gaps_index SET {field} = ?, updated = ? "
                        f"WHERE id IN ({placeholders}) AND node_id = ?",
                        [value, now, *chunk, active],
                    )
            for idx, gap_id in enumerate(gap_ids, start=1):
                try:
                    gap_writer.update_fields(gap_id, **{field: value})
                    if field == "status":
                        previous = previous_status_by_id.get(gap_id)
                        if previous != value:
                            gap_writer.append_latest_round_log(
                                gap_id=gap_id,
                                severity="info",
                                category="state",
                                actor="refine",
                                message=(
                                    "Workflow status changed by bulk update: "
                                    f"{previous} → {value}"
                                ),
                            )
                    try:
                        gap_writer.edit_latest_round(
                            gap_id,
                            actual=None,
                            target=None,
                            reporter=None,
                        )
                    except Exception:
                        pass
                    updated_ids.append(gap_id)
                except Exception as e:
                    failures.append({"id": gap_id, "error": str(e) or repr(e)})
                self._record_bulk_progress(field, idx, len(gap_ids))
        else:
            for idx, gap_id in enumerate(gap_ids, start=1):
                try:
                    gap_writer.edit_latest_round(
                        gap_id,
                        actual=None,
                        target=None,
                        reporter=value,
                    )
                    with db.transaction(self._conn):
                        self._conn.execute(
                            "UPDATE gaps_index SET reporter = ?, updated = ? "
                            "WHERE id = ? AND node_id = ?",
                            (value, now_iso(), gap_id, active),
                        )
                    updated_ids.append(gap_id)
                except Exception as e:
                    failures.append({"id": gap_id, "error": str(e) or repr(e)})
                self._record_bulk_progress(field, idx, len(gap_ids))

        return {
            "updated": len(updated_ids),
            "ids": updated_ids,
            "field": field,
            "value": value,
            "failed": len(failures),
            "failures": failures,
            "progress": {
                "completed": len(updated_ids) + len(failures),
                "total": len(gap_ids),
            },
        }

    def _bulk_restore_last_workflow_state_locked(
        self,
        gap_ids: list[str],
        by_id: dict[str, sqlite3.Row],
        now: str,
        active: str,
    ) -> dict:
        targets_by_id: dict[str, str] = {}
        skipped: list[dict[str, str]] = []
        failures: list[dict[str, str]] = []
        target_counts = {"todo": 0, "qa": 0, "ready-merge": 0}

        for gap_id in gap_ids:
            row = by_id[gap_id]
            current = str(row["status"] or "")
            target, reason = self._last_workflow_bulk_target(
                gap_id,
                current,
                str(row["branch_name"] or ""),
            )
            if target is None:
                skipped.append({"id": gap_id, "reason": reason})
                continue
            targets_by_id[gap_id] = target
            if target in target_counts:
                target_counts[target] += 1

        for target in ("todo", "qa", "ready-merge"):
            ids = [gid for gid in gap_ids if targets_by_id.get(gid) == target]
            if not ids:
                continue
            with db.transaction(self._conn):
                for chunk in _chunks(ids):
                    placeholders = ",".join("?" * len(chunk))
                    self._conn.execute(
                        "UPDATE gaps_index SET status = ?, updated = ? "
                        f"WHERE id IN ({placeholders}) AND node_id = ?",
                        [target, now, *chunk, active],
                    )

        updated_ids: list[str] = []
        for idx, gap_id in enumerate(gap_ids, start=1):
            target = targets_by_id.get(gap_id)
            if target is None:
                self._record_bulk_progress("status", idx, len(gap_ids))
                continue
            previous = str(by_id[gap_id]["status"] or "")
            try:
                gap_writer.update_fields(gap_id, status=target)
                gap_writer.append_latest_round_log(
                    gap_id=gap_id,
                    severity="info",
                    category="state",
                    actor="refine",
                    message=(
                        "Workflow status changed by bulk last workflow state: "
                        f"{previous} → {target}"
                    ),
                )
                try:
                    gap_writer.edit_latest_round(
                        gap_id,
                        actual=None,
                        target=None,
                        reporter=None,
                    )
                except Exception:
                    pass
                updated_ids.append(gap_id)
            except Exception as e:
                failures.append({"id": gap_id, "error": str(e) or repr(e)})
            self._record_bulk_progress("status", idx, len(gap_ids))

        return {
            "updated": len(updated_ids),
            "ids": updated_ids,
            "field": "status",
            "value": _BULK_LAST_WORKFLOW_STATUS,
            "failed": len(failures),
            "failures": failures,
            "skipped": len(skipped),
            "skipped_details": skipped,
            "todo": target_counts["todo"],
            "qa": target_counts["qa"],
            "ready_merge": target_counts["ready-merge"],
            "progress": {
                "completed": len(updated_ids) + len(failures) + len(skipped),
                "total": len(gap_ids),
            },
        }

    def _last_workflow_bulk_target(
        self,
        gap_id: str,
        current_status: str,
        branch_name: str,
    ) -> tuple[str | None, str]:
        if current_status in {"backlog", "todo", "qa", "ready-merge"}:
            return None, f"status:{current_status}"
        if current_status in {"in-progress", "awaiting-rebuild"}:
            return None, f"automated:{current_status}"
        if current_status == "failed":
            previous = self._previous_workflow_status(gap_id)
            if previous == "ready-merge":
                if not branch_name:
                    return None, "missing-branch"
                if not git_ops.local_branch_exists(branch_name):
                    return None, f"missing-branch:{branch_name}"
                return "ready-merge", "failed-from-ready-merge"
            if previous == "qa":
                if not branch_name:
                    return None, "missing-branch"
                if not git_ops.local_branch_exists(branch_name):
                    return None, f"missing-branch:{branch_name}"
                return "qa", "failed-from-qa"
            return "todo", "failed-from-agent"
        if current_status in {"review", "done", "cancelled"}:
            return "todo", f"status:{current_status}"
        return "todo", f"status:{current_status or 'unknown'}"

    def _previous_workflow_status(self, gap_id: str) -> str | None:
        gap = shared_gaps.read_gap_json(gap_id, include_logs=False) or {}
        rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
        if not rounds:
            return None
        latest = round_logs.latest_workflow_for_round(gap_id, len(rounds) - 1)
        message = str((latest or {}).get("message") or "")
        prefix = "Workflow status changed:"
        if not message.startswith(prefix) or "→" not in message:
            return None
        return message[len(prefix):].split("→", 1)[0].strip()

    def _record_bulk_progress(self, field: str, completed: int, total: int) -> None:
        if completed == total or completed == 1 or completed % 25 == 0:
            try:
                activity.append(
                    self._conn,
                    message=f"Bulk {field} update progress: {completed}/{total}",
                    severity="info",
                    category="state",
                    actor="refine",
                )
            except Exception:
                pass

    def _h_log_append(self, params: dict) -> dict:
        self._require_active_gap(params["gap_id"], columns="status, node_id")
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
        row = self._require_active_gap(gap_id)
        # If running, cancel first.
        if self.sub_mgr.is_running(gap_id):
            self.sub_mgr.cancel(gap_id)
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
            search_index.delete_gap(self._conn, gap_id)
            self._conn.execute("DELETE FROM runs WHERE gap_id = ?", (gap_id,))
            self._conn.execute(
                "DELETE FROM guidance_decisions WHERE gap_id = ?", (gap_id,),
            )
        activity.append(
            self._conn, message="Gap deleted",
            severity="info", category="state",
            gap_id=gap_id, actor="refine",
        )
        return {"deleted": True}

    def _h_bulk_delete_gaps(self, params: dict) -> dict:
        raw_ids = params.get("gap_ids") or []
        if not isinstance(raw_ids, list):
            raise ValueError("gap_ids must be a list")
        gap_ids = _unique_strings(raw_ids)
        deleted_ids: list[str] = []
        failures: list[dict[str, str]] = []
        with self._bulk_update_lock:
            for idx, gap_id in enumerate(gap_ids, start=1):
                try:
                    result = self._h_delete_gap({"gap_id": gap_id})
                    if result.get("deleted"):
                        deleted_ids.append(gap_id)
                except Exception as e:
                    failures.append({"id": gap_id, "error": str(e) or repr(e)})
                self._record_bulk_progress("delete", idx, len(gap_ids))
        return {
            "deleted": len(deleted_ids),
            "ids": deleted_ids,
            "failed": len(failures),
            "failures": failures,
            "progress": {
                "completed": len(deleted_ids) + len(failures),
                "total": len(gap_ids),
            },
        }

    def _h_set_notes(self, params: dict) -> dict:
        gap_id = params["gap_id"]
        self._require_active_gap(gap_id, columns="status, node_id")
        notes = params.get("notes")
        if not isinstance(notes, list):
            raise ValueError("notes must be a list of {id, author, body, ...} objects")
        gap = gap_writer.set_notes(gap_id, notes)
        # Mirror the touch onto the index row so listings sort right.
        with db.transaction(self._conn):
            self._conn.execute(
                "UPDATE gaps_index SET updated = ? WHERE id = ?",
                (gap["updated"], gap_id),
            )
            search_index.upsert_gap(self._conn, gap)
        activity.append(
            self._conn,
            message=f"Notes updated ({len(gap['notes'])} note{'' if len(gap['notes']) == 1 else 's'})",
            severity="info", category="state",
            gap_id=gap_id, actor="refine",
        )
        return {"gap": gap}

    # ---- chat ----------------------------------------------------------------

    def _h_extract_gaps(self, params: dict) -> dict:
        stopped = self._background_processes_stopped()
        if stopped is not None:
            return stopped
        text = params.get("text") or ""
        drafts = llm.extract_gaps(
            text, provider=db.get_setting(self._conn, "agent_cli"),
        )
        return {"drafts": drafts}

    def _h_rename_reporter(self, params: dict) -> dict:
        """Rename a reporter in the table AND cascade the new name through
        every Gap's rounds so the dropdown and historical data stay in
        sync. Returns the old name plus how many Gaps were touched."""
        try:
            rid = int(params.get("rid"))
        except (TypeError, ValueError):
            raise ValueError("rid is required and must be an integer")
        new_name = (params.get("new_name") or "").strip()
        if not new_name:
            raise ValueError("new_name is required")
        row = self._conn.execute(
            "SELECT name FROM reporters WHERE id = ?", (rid,),
        ).fetchone()
        if not row:
            raise ValueError(f"reporter {rid} not found")
        old_name = row["name"]
        if old_name != new_name:
            reporters.rename(self._conn, rid, new_name)
        touched = gap_writer.rename_reporter_in_rounds(
            self._conn,
            old_name,
            new_name,
            node_id=project_state.active_node_id(),
        )
        activity.append(
            self._conn,
            message=(f"Reporter renamed: {old_name!r} → {new_name!r} "
                     f"({touched} gap{'' if touched == 1 else 's'} updated)"),
            severity="info", category="state", actor="refine",
        )
        return {"old": old_name, "new": new_name, "touched": touched}

    def _h_merge_reporter(self, params: dict) -> dict:
        """Merge one reporter into another.

        The source reporter is removed from the dropdown after every matching
        Gap round is rewritten to the target reporter name.
        """
        try:
            rid = int(params.get("rid"))
            target_rid = int(params.get("target_rid"))
        except (TypeError, ValueError):
            raise ValueError("rid and target_rid are required and must be integers")
        if rid == target_rid:
            raise ValueError("cannot merge a reporter into itself")
        rows = self._conn.execute(
            "SELECT id, name FROM reporters WHERE id IN (?, ?)",
            (rid, target_rid),
        ).fetchall()
        by_id = {int(row["id"]): row["name"] for row in rows}
        old_name = by_id.get(rid)
        new_name = by_id.get(target_rid)
        if not old_name:
            raise ValueError(f"reporter {rid} not found")
        if not new_name:
            raise ValueError(f"target reporter {target_rid} not found")
        if old_name == new_name:
            raise ValueError("cannot merge reporters with the same name")
        touched = gap_writer.rename_reporter_in_rounds(
            self._conn,
            old_name,
            new_name,
            node_id=project_state.active_node_id(),
        )
        reporters.remove(self._conn, rid)
        activity.append(
            self._conn,
            message=(f"Reporter merged: {old_name!r} → {new_name!r}; "
                     f"{old_name!r} removed "
                     f"({touched} gap{'' if touched == 1 else 's'} updated)"),
            severity="info", category="state", actor="refine",
        )
        return {"old": old_name, "new": new_name, "removed_id": rid, "touched": touched}

    def _h_rename_reporter_strings(self, params: dict) -> dict:
        """Cascade-only: rewrite round.reporter == old to new across all
        Gaps without touching the reporters table. Used for one-off data
        migrations (e.g. an orphan string that's not in the table)."""
        old = (params.get("old") or "").strip()
        new = (params.get("new") or "").strip()
        if not old or not new:
            raise ValueError("both `old` and `new` are required")
        touched = gap_writer.rename_reporter_in_rounds(
            self._conn,
            old,
            new,
            node_id=project_state.active_node_id(),
        )
        activity.append(
            self._conn,
            message=(f"Reporter strings rewritten: {old!r} → {new!r} "
                     f"({touched} gap{'' if touched == 1 else 's'} updated)"),
            severity="info", category="state", actor="refine",
        )
        return {"touched": touched}

    def _effective_target_branch(self) -> str | None:
        """The configured merge target branch, or the host's current
        branch when nothing's set. None if neither resolves (e.g. host
        is in detached-HEAD state with no setting)."""
        return changes_index.effective_target_branch(self._conn)

    def _h_list_changes(self, params: dict) -> dict:
        """List refine merge commits on the target branch.

        Returns each commit plus the Gap metadata refine knows about
        (name + current status). Powers the Changes screen.
        """
        metric_start = perf_metrics.now()
        target = self._effective_target_branch()
        limit = max(1, int(params.get("limit") or 100))
        offset = max(0, int(params.get("offset") or 0))
        q = str(params.get("q") or "").strip().lower()
        status = str(params.get("status") or "").strip()
        priority = str(params.get("priority") or "").strip()
        query_mode = "filtered" if (q or status or priority) else "paged"
        rows_scanned = 0
        rebuilt = False
        if not target:
            result = {
                "branch": None,
                "changes": [],
                "page": {"limit": limit, "offset": offset, "has_more": False},
            }
            perf_metrics.record(
                "runner.list_changes",
                conn=self._conn,
                elapsed_ms=perf_metrics.elapsed_ms(metric_start),
                query_mode="no_target",
                rows_scanned=0,
                rows_returned=0,
                details={"limit": limit, "offset": offset},
            )
            return result
        rebuilt = changes_index.ensure_branch_current(self._conn, target)
        page_rows = changes_index.list_changes(
            self._conn,
            target,
            limit=limit + 1,
            offset=offset,
            q=q,
            status=status,
            priority=priority,
        )
        rows_scanned = len(page_rows)
        has_more = len(page_rows) > limit
        merges = page_rows[:limit]
        if not merges:
            result = {
                "branch": target,
                "changes": [],
                "page": {"limit": limit, "offset": offset, "has_more": False},
            }
            perf_metrics.record(
                "runner.list_changes",
                conn=self._conn,
                elapsed_ms=perf_metrics.elapsed_ms(metric_start),
                query_mode=query_mode,
                rows_scanned=rows_scanned,
                rows_returned=0,
                details={
                    "limit": limit, "offset": offset, "q": bool(q),
                    "status": status, "priority": priority,
                    "rebuilt": rebuilt,
                },
            )
            return result
        result = {
            "branch": target,
            "changes": merges,
            "page": {"limit": limit, "offset": offset, "has_more": has_more},
        }
        perf_metrics.record(
            "runner.list_changes",
            conn=self._conn,
            elapsed_ms=perf_metrics.elapsed_ms(metric_start),
            query_mode=query_mode,
            rows_scanned=rows_scanned,
            rows_returned=len(merges),
            details={
                "limit": limit, "offset": offset, "q": bool(q),
                "status": status, "priority": priority, "rebuilt": rebuilt,
            },
        )
        return result

    def _h_governance_get(self, params: dict) -> dict:
        settings = governance.load_settings(self._conn)
        return {**settings, "configured": governance.is_configured(self._conn)}

    def _h_governance_save(self, params: dict) -> dict:
        settings = governance.save_settings(
            self._conn,
            product=params.get("product"),
            constitution=params.get("constitution"),
            rules=params.get("rules"),
        )
        activity.append(
            self._conn,
            message="Governance settings updated",
            severity="info",
            category="governance",
            actor="refine",
        )
        self.governance_agent.wake()
        self.dispatcher.enforce_now()
        return {**settings, "configured": governance.is_configured(self._conn)}

    def _h_governance_generate_rules(self, params: dict) -> dict:
        stopped = self._background_processes_stopped()
        if stopped is not None:
            return stopped
        product = str(params.get("product") or "").strip()
        constitution = str(params.get("constitution") or "").strip()
        if not product or not constitution:
            raise ValueError("product and constitution are required")
        provider = db.get_setting(self._conn, "agent_cli")
        result = governance.generate_rules(
            product, constitution, provider=provider,
        )
        activity.append(
            self._conn,
            message=(
                f"Governance generated {len(result.get('rules') or [])} rule"
                f"{'' if len(result.get('rules') or []) == 1 else 's'}"
            ),
            severity="info",
            category="governance",
            actor="refine",
        )
        return result

    def _h_governance_wake(self, params: dict) -> dict:
        self.governance_agent.wake()
        return {"ok": True}

    def _h_undo_gap(self, params: dict) -> dict:
        """Revert a refine merge commit and transition the Gap to
        `cancelled` with a log entry.

        Routes through the merger's host lock so a concurrent Merge-agent
        operation can't race with us on the shared host worktree. The
        merger also runs its cleanup-worktree pass
        before AND after — aborting any leftover stuck
        `merge`/`rebase` from a prior failure so the revert doesn't
        trip on stale state, and tearing down any partial revert if
        our own attempt fails.
        """
        commit_sha = (params.get("commit") or "").strip()
        if not commit_sha:
            raise ValueError("commit is required")
        return self.merger.run_under_host_lock(
            lambda: self._do_undo_gap(commit_sha),
            label="undo",
        )

    def _do_undo_gap(self, commit_sha: str) -> dict:
        gap_id = git_ops.gap_id_from_commit(commit_sha)
        if not gap_id:
            return {"ok": False, "stage": "lookup",
                    "message": f"commit {commit_sha[:10]}… isn't a refine merge"}
        # Reject undo on a Gap that's already cancelled — defends
        # against a stale UI where the button was visible/clickable
        # after a concurrent undo from another tab.
        row = self._conn.execute(
            "SELECT status, node_id FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
        if row:
            active = project_state.active_node_id()
            owner = str(row["node_id"] or project_state.DEFAULT_NODE_ID)
            if owner != active:
                owner_name = project_state.gap_node_display(owner)
                active_name = project_state.gap_node_display(active)
                return {
                    "ok": False,
                    "stage": "node",
                    "code": "node_ownership",
                    "message": (
                        "Action not allowed: Gap is owned by another node "
                        f"({owner_name}). Transfer to {active_name} before "
                        "making changes."
                    ),
                }
        if row and row["status"] == "cancelled":
            return {"ok": False, "stage": "state",
                    "message": "Gap is already cancelled — nothing to undo"}
        target = self._effective_target_branch()
        if not target:
            return {"ok": False, "stage": "precheck",
                    "message": ("could not resolve target branch — host is in "
                                "detached HEAD and no merge_target_branch is "
                                "configured")}
        if not git_ops.local_branch_exists(target):
            return {"ok": False, "stage": "precheck",
                    "message": f"target branch `{target}` doesn't exist locally"}

        host_branch = git_ops.current_branch()
        switched_from: str | None = None
        # Always stash WIP — even when we're already on the target
        # branch. `git revert` refuses to run with a dirty working
        # tree, so the prior "only stash if switching" rule left a
        # dead-end failure path on a fresh checkout that happened
        # to have local edits.
        stashed = False
        if git_ops.working_copy_dirty():
            sr = git_ops.stash_push(
                f"refine auto-stash before undo of {commit_sha[:10]}",
            )
            if not sr.ok:
                return {"ok": False, "stage": "precheck",
                        "message": "could not stash WIP before undo",
                        "details": sr.stderr}
            stashed = True
        if host_branch != target:
            ck = git_ops.checkout_branch(target)
            if not ck.ok:
                if stashed:
                    git_ops.stash_pop()
                return {"ok": False, "stage": "precheck",
                        "message": f"could not check out target `{target}`",
                        "details": ck.stderr}
            switched_from = host_branch

        pushed = False
        push_warning: str | None = None
        revert_message: str | None = None
        pre_revert_head = git_ops.rev_parse(target)
        try:
            # `git revert -m 1 <merge> --no-edit`
            r = git_ops.revert_merge_commit(commit_sha)
            if not r.ok:
                blob = (r.stdout or "") + "\n" + (r.stderr or "")
                # Abort the partial revert so the worktree is clean again.
                git_ops.revert_abort()
                return {"ok": False, "stage": "revert",
                        "message": ("revert conflicted — undo aborted; the "
                                    "merge commit touches paths that have "
                                    "since changed. Resolve manually with "
                                    "`git revert -m 1 <sha>` on the host."),
                        "details": blob[:2000]}
            # Push if there's an upstream; local-only repos still get
            # the revert in their working state.
            if git_ops.upstream_branch(target) is not None:
                p = push_ops.push_current_after_pull(
                    self._conn,
                    actor="refine",
                    gap_id=gap_id,
                    target=target,
                    merge_message=f"Merge upstream before pushing undo of {gap_id}",
                    prompt_context=(
                        f"A pull is in progress before pushing an undo on `{target}`.\n"
                        "HEAD contains a local revert of a Refine merge commit.\n"
                        "The incoming side contains newer upstream commits.\n"
                        "Preserve the local revert and integrate upstream changes."
                    ),
                )
                if p.get("ok") and p.get("pushed"):
                    pushed = True
                else:
                    push_warning = (
                        f"Revert committed locally on `{target}` but push "
                        f"failed — your remote still has the merge. Push "
                        f"manually once the underlying issue is resolved."
                    )
                    activity.append(
                        self._conn, message=push_warning,
                        severity="warn", category="git",
                        gap_id=gap_id, actor="refine",
                        details=str(p.get("details") or p.get("message") or "")[:2000],
                    )
        finally:
            if switched_from:
                back = git_ops.checkout_branch(switched_from)
                if not back.ok:
                    activity.append(
                        self._conn,
                        message=(f"Could not restore host HEAD to "
                                 f"`{switched_from}` after undo — host is "
                                 f"still on `{target}`"),
                        severity="warn", category="git",
                        gap_id=gap_id, actor="refine",
                        details=back.stderr[:2000],
                    )
            if stashed:
                pop = git_ops.stash_pop()
                if not pop.ok:
                    activity.append(
                        self._conn,
                        message=("Auto-stash pop after undo failed — your "
                                 "WIP remains in `git stash`. Recover with "
                                 "`git stash list` + `git stash pop`."),
                        severity="warn", category="git",
                        gap_id=gap_id, actor="refine",
                        details=pop.stderr[:2000],
                    )

        # Move the Gap to cancelled + log the undo on the latest round.
        with db.transaction(self._conn):
            self._conn.execute(
                "UPDATE gaps_index SET status = 'cancelled', updated = ? WHERE id = ?",
                (now_iso(), gap_id),
            )
        changes_index.advance_branch_head(
            self._conn, target, previous_head=pre_revert_head,
        )
        try:
            gap_writer.update_fields(gap_id, status="cancelled")
        except Exception:
            pass
        if pushed:
            revert_message = (
                f"Gap undone — reverted merge `{commit_sha[:10]}…` on "
                f"`{target}` and pushed"
            )
        elif push_warning:
            revert_message = push_warning
        else:
            revert_message = (
                f"Gap undone — reverted merge `{commit_sha[:10]}…` on "
                f"`{target}` (no upstream — push skipped)"
            )
        try:
            gap = shared_gaps.read_gap_json(gap_id, include_logs=False) or {}
            rounds = gap.get("rounds") or []
            if rounds:
                gap_writer.append_round_log(
                    gap_id=gap_id, round_idx=len(rounds) - 1,
                    severity="warn", category="git", actor="refine",
                    message=revert_message,
                )
        except Exception:
            pass
        activity.append(
            self._conn,
            message=revert_message,
            severity="warn", category="git",
            gap_id=gap_id, actor="refine",
        )
        return {"ok": True, "gap_id": gap_id,
                "commit": commit_sha, "pushed": pushed, "target": target,
                "push_warning": push_warning,
                "message": revert_message}

    def _handle_post_rebuild_quality_failure(
        self,
        gap_id: str,
        message: str,
        details: str | None = None,
    ) -> None:
        self.merger.run_under_host_lock(
            lambda: self._do_post_rebuild_quality_revert(gap_id, message, details),
            label="post-rebuild-qa-revert",
        )

    def _do_post_rebuild_quality_revert(
        self,
        gap_id: str,
        message: str,
        details: str | None = None,
    ) -> dict:
        target = self._effective_target_branch()
        if not target:
            cleanup_message = (
                "Post-rebuild QA failed; automatic revert needs manual follow-up "
                "because the target branch could not be resolved."
            )
            self._log_post_rebuild_quality_cleanup(
                gap_id,
                cleanup_message,
                details=details or message,
                severity="error",
            )
            return {"ok": False, "stage": "precheck", "message": cleanup_message}
        if not git_ops.local_branch_exists(target):
            cleanup_message = (
                f"Post-rebuild QA failed; automatic revert needs manual follow-up "
                f"because target branch `{target}` does not exist locally."
            )
            self._log_post_rebuild_quality_cleanup(
                gap_id,
                cleanup_message,
                details=details or message,
                severity="error",
            )
            return {"ok": False, "stage": "precheck", "message": cleanup_message}

        merge = next(
            (
                row for row in git_ops.list_refine_merges(target, limit=200)
                if row.get("gap_id") == gap_id
            ),
            None,
        )
        if not merge:
            cleanup_message = (
                "Post-rebuild QA failed; automatic revert needs manual follow-up "
                "because Refine could not find the Gap merge commit."
            )
            self._log_post_rebuild_quality_cleanup(
                gap_id,
                cleanup_message,
                details=details or message,
                severity="error",
            )
            return {"ok": False, "stage": "lookup", "message": cleanup_message}

        commit_sha = str(merge.get("commit") or "").strip()
        host_branch = git_ops.current_branch()
        switched_from: str | None = None
        stashed = False
        if git_ops.working_copy_dirty():
            stash = git_ops.stash_push(
                f"refine auto-stash before post-rebuild QA revert of {gap_id}",
            )
            if not stash.ok:
                cleanup_message = (
                    "Post-rebuild QA failed; automatic revert needs manual follow-up "
                    "because Refine could not stash target worktree changes."
                )
                self._log_post_rebuild_quality_cleanup(
                    gap_id,
                    cleanup_message,
                    details=stash.stderr or stash.stdout,
                    severity="error",
                )
                return {"ok": False, "stage": "precheck", "message": cleanup_message}
            stashed = True
        if host_branch != target:
            checkout = git_ops.checkout_branch(target)
            if not checkout.ok:
                if stashed:
                    git_ops.stash_pop()
                cleanup_message = (
                    f"Post-rebuild QA failed; automatic revert needs manual follow-up "
                    f"because Refine could not check out `{target}`."
                )
                self._log_post_rebuild_quality_cleanup(
                    gap_id,
                    cleanup_message,
                    details=checkout.stderr or checkout.stdout,
                    severity="error",
                )
                return {"ok": False, "stage": "precheck", "message": cleanup_message}
            switched_from = host_branch

        pushed = False
        push_warning: str | None = None
        pre_revert_head = git_ops.rev_parse(target)
        try:
            revert = git_ops.revert_merge_commit(commit_sha)
            if not revert.ok:
                blob = (revert.stdout or "") + "\n" + (revert.stderr or "")
                git_ops.revert_abort()
                cleanup_message = (
                    "Post-rebuild QA failed; automatic revert needs manual follow-up "
                    "because the revert conflicted."
                )
                self._log_post_rebuild_quality_cleanup(
                    gap_id,
                    cleanup_message,
                    details=blob[:2000],
                    severity="error",
                )
                return {"ok": False, "stage": "revert", "message": cleanup_message}

            if git_ops.upstream_branch(target) is not None:
                push = push_ops.push_current_after_pull(
                    self._conn,
                    actor="runner",
                    gap_id=gap_id,
                    target=target,
                    merge_message=f"Merge upstream before pushing QA revert of {gap_id}",
                    prompt_context=(
                        f"A pull is in progress before pushing a post-rebuild QA "
                        f"revert on `{target}`.\n"
                        "HEAD contains a local revert of a Refine merge commit.\n"
                        "The incoming side contains newer upstream commits.\n"
                        "Preserve the local revert and integrate upstream changes."
                    ),
                )
                if push.get("ok") and push.get("pushed"):
                    pushed = True
                else:
                    push_warning = (
                        f"Post-rebuild QA failed; reverted Gap merge `{commit_sha[:10]}…` "
                        f"locally on `{target}` but push failed. Push manually once "
                        "the underlying issue is resolved."
                    )
        finally:
            if switched_from:
                back = git_ops.checkout_branch(switched_from)
                if not back.ok:
                    activity.append(
                        self._conn,
                        message=(
                            f"Could not restore host HEAD to `{switched_from}` "
                            f"after post-rebuild QA revert; host is still on `{target}`"
                        ),
                        severity="warn",
                        category="git",
                        gap_id=gap_id,
                        actor="runner",
                        details=back.stderr[:2000],
                    )
            if stashed:
                pop = git_ops.stash_pop()
                if not pop.ok:
                    activity.append(
                        self._conn,
                        message=(
                            "Auto-stash pop after post-rebuild QA revert failed; "
                            "recover the WIP from git stash."
                        ),
                        severity="warn",
                        category="git",
                        gap_id=gap_id,
                        actor="runner",
                        details=pop.stderr[:2000],
                    )

        with db.transaction(self._conn):
            self._conn.execute(
                "UPDATE gaps_index SET status = 'failed', updated = ? WHERE id = ?",
                (now_iso(), gap_id),
            )
        if pre_revert_head:
            changes_index.advance_branch_head(
                self._conn,
                target,
                previous_head=pre_revert_head,
            )
        try:
            gap_writer.update_fields(gap_id, status="failed")
        except Exception:
            pass

        if push_warning:
            cleanup_message = push_warning
            severity = "warn"
        else:
            cleanup_message = (
                f"Post-rebuild QA failed; reverted Gap merge `{commit_sha[:10]}…` "
                f"on `{target}`"
                + (" and pushed" if pushed else " (no upstream; push skipped)")
            )
            severity = "error"
        self._log_post_rebuild_quality_cleanup(
            gap_id,
            cleanup_message,
            details=details or message,
            severity=severity,
        )
        return {
            "ok": True,
            "gap_id": gap_id,
            "commit": commit_sha,
            "target": target,
            "pushed": pushed,
            "push_warning": push_warning,
            "message": cleanup_message,
        }

    def _log_post_rebuild_quality_cleanup(
        self,
        gap_id: str,
        message: str,
        *,
        details: str | None,
        severity: str,
    ) -> None:
        try:
            gap = shared_gaps.read_gap_json(gap_id, include_logs=False) or {}
            rounds = gap.get("rounds") or []
            if rounds:
                gap_writer.append_round_log(
                    gap_id=gap_id,
                    round_idx=len(rounds) - 1,
                    severity=severity,
                    category="quality",
                    actor="runner",
                    message=message,
                    details=details,
                )
        except Exception:
            pass
        activity.append(
            self._conn,
            message=message,
            severity=severity,
            category="quality",
            gap_id=gap_id,
            actor="runner",
            details=details,
        )

    def _h_chat_start(self, params: dict) -> dict:
        gap_id = params.get("gap_id")
        purpose = str(params.get("purpose") or "").strip().lower()
        priming_prompt: str | None = None
        priming_intro: str | None = None
        chat_mode = "standalone"
        if gap_id:
            # Prefer the Gap's worktree when it exists (in-progress / todo
            # / review / failed Gaps with a registered worktree). For
            # done/cancelled Gaps the worktree is gone — fall back to the
            # client repo so chat still works for retrospectives. The
            # priming preamble will know which cwd it has.
            worktree = git_ops.gap_worktree_path(gap_id)
            worktree_present = worktree.exists()
            root = worktree if worktree_present else git_ops.client_repo_path()
            is_standalone = False
            chat_mode = "gap"
            priming_prompt, priming_intro = _build_gap_chat_preamble(
                self._conn, gap_id, worktree_present=worktree_present,
            )
        elif purpose == "plan":
            root = git_ops.client_repo_path()
            is_standalone = True
            chat_mode = "plan"
            priming_prompt, priming_intro = _build_plan_chat_preamble()
        else:
            root = git_ops.client_repo_path()
            is_standalone = True
        # Run chat inside the configured sub-project when one is set; fall
        # back to the worktree / client repo root if the subpath is empty
        # or missing.
        agent_subpath = db.get_setting(self._conn, "agent_subpath") or ""
        cwd = git_ops.apply_agent_subpath(
            root, agent_subpath,
            log=lambda msg: activity.append(
                self._conn,
                message=f"agent_subpath: {msg}",
                severity="warn", category="state",
                gap_id=gap_id, actor="runner",
            ),
        )
        sid = self.chat.start(
            cwd, is_standalone=is_standalone,
            provider=db.get_setting(self._conn, "agent_cli"),
            gap_id=gap_id,
            mode=chat_mode,
            priming_prompt=priming_prompt,
            priming_intro=priming_intro,
            show_priming_output=bool(gap_id),
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

    def _h_chat_reset_all(self, params: dict) -> dict:
        reason = params.get("reason") or "state reset"
        return {"stopped": self.chat.stop_all(reason=reason)}

    # ---- target-app ----------------------------------------------------------

    def _h_target_app_run(self, params: dict) -> dict:
        """Run a deterministic target-app start/stop/status operation."""
        kind = (params.get("kind") or "").strip().lower()
        if kind not in ("start", "stop", "status"):
            raise ValueError("kind must be 'start', 'stop', or 'status'")
        config = params.get("config") if isinstance(params.get("config"), dict) else {}
        quiet = bool(params.get("quiet"))
        if not self._target_app_lock.acquire(blocking=False):
            return {"ok": False, "busy": True, "state": "unknown",
                    "message": "another target-app operation is already running",
                    "checks": []}
        if not quiet:
            activity.append(
                self._conn,
                message=f"target-app: {kind} requested",
                severity="info", category="target_app", actor="refine",
            )
        try:
            cleanup = None
            if kind == "start":
                cleanup = self._clean_target_worktree_for_app_start()
                if not cleanup.get("ok"):
                    return {
                        "ok": False,
                        "kind": kind,
                        "state": "failed",
                        "message": cleanup.get("message")
                                   or "target worktree cleanup failed before app start",
                        "cleanup": cleanup,
                        "checks": [],
                    }
            result = target_app.run_operation(kind, config)
            if cleanup is not None:
                result["cleanup"] = cleanup
            sev = "info" if result.get("ok") else "error"
            msg = (
                f"target-app: {kind} "
                f"{'completed' if result.get('ok') else 'failed'}"
                f" — {result.get('message') or ''}"
            )
            details_parts = []
            if result.get("stdout_tail"):
                details_parts.append("stdout:\n" + result["stdout_tail"])
            if result.get("stderr_tail"):
                details_parts.append("stderr:\n" + result["stderr_tail"])
            if result.get("checks"):
                details_parts.append(
                    "checks:\n" + json.dumps(result["checks"], indent=2)
                )
            details = "\n\n".join(details_parts) if details_parts else None
            if not quiet:
                activity.append(
                    self._conn, message=msg, severity=sev,
                    category="target_app", actor="refine",
                    details=details,
                )
            return result
        finally:
            self._target_app_lock.release()

    def _h_regression_run(self, params: dict) -> dict:
        """Run managed regressions against the configured target app."""
        stopped = self._background_processes_stopped()
        if stopped is not None:
            return stopped
        if not self._target_app_lock.acquire(blocking=False):
            return {
                "ok": False,
                "busy": True,
                "message": "another target-app operation is already running",
                "runs": [],
            }
        try:
            return regressions.run_all(
                self._conn,
                only_enabled=bool(params.get("only_enabled", True)),
            )
        finally:
            self._target_app_lock.release()

    def _h_target_app_rebuild_pending(self, params: dict) -> dict:
        stopped = self._background_processes_stopped()
        if stopped is not None:
            return {"queued": False, **stopped}
        queued = self.target_app_rebuilder.queue_pending_awaiting_rebuild()
        return {"queued": queued}

    def _h_target_app_rebuild_queue(self, params: dict) -> dict:
        stopped = self._background_processes_stopped()
        if stopped is not None:
            return {"queued": False, **stopped}
        queued = self.target_app_rebuilder.queue_rebuild(
            "manual runner-worker rebuild",
        )
        return {"queued": queued}

    def _run_automatic_target_app_rebuild(
        self,
        reason: str,
        cancel_event=None,  # noqa: ANN001
    ) -> dict:
        """Run one queued automatic stop/rebuild/start cycle.

        Automatic rebuilds need the clean host worktree, so they run after
        merges park Gaps in `awaiting-rebuild`. The target application itself
        is cycled around the rebuild so users review the freshly rebuilt app:
        stop, rebuild if configured, then start.
        """
        with mutation_guard.exclusive(
            "Automatic target-app rebuild",
            kind="target_app_rebuild",
            blocking=True,
        ):
            if cancel_event is not None and cancel_event.is_set():
                return {
                    "ok": False,
                    "state": "failed",
                    "message": "automatic target-app rebuild cancelled",
                    "cancelled": True,
                }
            return self._run_automatic_target_app_rebuild_locked(
                reason,
                cancel_event=cancel_event,
            )

    def _run_automatic_target_app_rebuild_locked(
        self,
        reason: str,
        *,
        cancel_event=None,  # noqa: ANN001
    ) -> dict:
        with self._target_app_lock:
            conn = self._get_conn()
            settings = db.list_settings(conn)
            cfg = target_app.config_from_settings(settings)
            db.set_setting(conn, "target_app_state", "rebuilding")
            db.set_setting(conn, "target_app_last_error", "")
            db.set_setting(conn, "target_app_auto_rebuild_last_started_at", now_iso())
            activity.append(
                conn,
                message=f"target-app: automatic rebuild started ({reason})",
                severity="info", category="target_app", actor="runner",
            )
            if cancel_event is not None and cancel_event.is_set():
                result = {
                    "ok": False,
                    "state": "failed",
                    "message": "automatic target-app rebuild cancelled",
                    "cancelled": True,
                    "steps": [],
                }
            else:
                result = self._run_target_app_rebuild_sequence(
                    conn, cfg, cancel_event=cancel_event,
                )
            ok = bool(result.get("ok"))
            final_state = result.get("state") if result.get("state") in {
                "unknown", "running", "degraded", "stopped", "failed",
            } else ("unknown" if ok else "failed")
            err_msg = "" if ok else (result.get("message") or "target-app rebuild failed")
            db.set_setting(conn, "target_app_state", final_state)
            db.set_setting(conn, "target_app_last_error", err_msg)
            db.set_setting(conn, "target_app_auto_rebuild_last_finished_at", now_iso())
            db.set_setting(conn, "target_app_auto_rebuild_last_ok", "1" if ok else "0")
            db.set_setting(
                conn, "target_app_auto_rebuild_last_message",
                result.get("message") or "",
            )
            op_id = self._record_target_app_operation(conn, "rebuild", result, final_state)
            db.set_setting(conn, "target_app_last_operation_id", str(op_id))
            if result.get("checks_configured"):
                self._persist_target_app_checks(
                    conn, result.get("checks") or [], result.get("message") or "",
                )
            promoted = self._promote_rebuilt_gaps(conn) if ok else 0
            promoted_label = "ready for review"
            if ok and quality.enabled(conn) and quality.post_rebuild(conn):
                promoted_label = "queued for QA"
            details = _automatic_rebuild_details(result) if not ok else None
            if not ok:
                self._log_automatic_rebuild_failure_to_pending_gaps(
                    conn, err_msg, details=details,
                )
            activity.append(
                conn,
                message=(
                    f"target-app: automatic rebuild "
                    f"{'completed' if ok else 'failed'}"
                    + (f"; {promoted} Gap{'s' if promoted != 1 else ''} {promoted_label}"
                       if ok else "")
                ),
                severity="info" if ok else "error",
                category="target_app", actor="runner",
                details=details or err_msg or None,
            )
            if ok:
                self.merger.wake()
            return result

    def _run_target_app_rebuild_sequence(
        self,
        conn: sqlite3.Connection,
        cfg: dict[str, Any],
        *,
        cancel_event=None,  # noqa: ANN001
    ) -> dict:
        steps: list[dict[str, Any]] = []

        def run_step(kind: str) -> dict[str, Any]:
            if cancel_event is not None and cancel_event.is_set():
                result = {
                    "ok": False,
                    "kind": kind,
                    "state": "failed",
                    "command": "",
                    "cwd": "",
                    "exit_code": None,
                    "stdout_tail": "",
                    "stderr_tail": "",
                    "message": "automatic target-app rebuild cancelled",
                    "started_at": now_iso(),
                    "finished_at": now_iso(),
                    "checks_configured": False,
                    "checks": [],
                    "cancelled": True,
                }
                steps.append(result)
                self._log_target_app_rebuild_step(conn, result)
                return result
            command = (cfg.get(f"{kind}_command") or "").strip()
            if kind in {"start", "stop", "rebuild"} and not command:
                result = target_app.noop_operation(kind)
                steps.append(result)
                self._log_target_app_rebuild_step(conn, result)
                return result
            if kind == "start":
                cleanup = self._clean_target_worktree_for_app_start()
                if not cleanup.get("ok"):
                    result = {
                        "ok": False,
                        "kind": kind,
                        "state": "failed",
                        "command": "",
                        "cwd": "",
                        "exit_code": None,
                        "stdout_tail": "",
                        "stderr_tail": "",
                        "message": cleanup.get("message")
                                   or "target worktree cleanup failed before app start",
                        "started_at": now_iso(),
                        "finished_at": now_iso(),
                        "checks_configured": False,
                        "checks": [],
                        "cleanup": cleanup,
                    }
                    steps.append(result)
                    self._log_target_app_rebuild_step(conn, result)
                    return result
            result = target_app.run_operation(kind, cfg, cancel_event=cancel_event)
            steps.append(result)
            self._log_target_app_rebuild_step(conn, result)
            return result

        stop_result = run_step("stop")
        if not stop_result.get("ok"):
            return self._automatic_rebuild_sequence_result(steps, failed_step="stop")

        rebuild_result = run_step("rebuild")
        if not rebuild_result.get("ok"):
            return self._automatic_rebuild_sequence_result(steps, failed_step="rebuild")

        start_result = run_step("start")
        if not start_result.get("ok"):
            return self._automatic_rebuild_sequence_result(steps, failed_step="start")
        return self._automatic_rebuild_sequence_result(steps)

    def _log_target_app_rebuild_step(
        self,
        conn: sqlite3.Connection,
        result: dict[str, Any],
    ) -> None:
        kind = str(result.get("kind") or "operation")
        ok = bool(result.get("ok"))
        details_parts = []
        if result.get("stdout_tail"):
            details_parts.append("stdout:\n" + str(result["stdout_tail"]))
        if result.get("stderr_tail"):
            details_parts.append("stderr:\n" + str(result["stderr_tail"]))
        if result.get("checks"):
            details_parts.append(
                "checks:\n" + json.dumps(result["checks"], indent=2)
            )
        details = "\n\n".join(details_parts) if details_parts else None
        message = (
            f"target-app: automatic {kind} "
            f"{'completed' if ok else 'failed'}"
        )
        if result.get("message"):
            message += f" — {result['message']}"
        activity.append(
            conn,
            message=message,
            severity="info" if ok else "error",
            category="target_app",
            actor="runner",
            details=details,
        )

    def _automatic_rebuild_sequence_result(
        self,
        steps: list[dict[str, Any]],
        *,
        failed_step: str | None = None,
    ) -> dict[str, Any]:
        last = steps[-1] if steps else {}
        ok = failed_step is None
        if ok:
            if any(step.get("noop") for step in steps):
                message = (
                    "Automatic rebuild completed; empty target-app commands "
                    "were treated as no-ops."
                )
            else:
                message = "Automatic rebuild completed: stopped app, rebuilt app, started app."
        else:
            message = (
                f"Automatic rebuild failed during {failed_step}: "
                f"{last.get('message') or 'operation failed'}"
            )
        return {
            "ok": ok,
            "kind": "rebuild",
            "state": last.get("state") or ("running" if ok else "failed"),
            "command": "\n".join(
                f"{step.get('kind')}: {step.get('command') or '(no-op)'}"
                for step in steps
            ),
            "cwd": last.get("cwd") or "",
            "exit_code": 0 if ok else last.get("exit_code"),
            "stdout_tail": _combine_step_tail(steps, "stdout_tail"),
            "stderr_tail": _combine_step_tail(steps, "stderr_tail"),
            "message": message,
            "started_at": (steps[0].get("started_at") if steps else now_iso()),
            "finished_at": last.get("finished_at") or now_iso(),
            "checks_configured": bool(last.get("checks_configured")),
            "checks": last.get("checks") or [],
            "steps": [
                {
                    "kind": step.get("kind"),
                    "ok": bool(step.get("ok")),
                    "state": step.get("state"),
                    "message": step.get("message") or "",
                    "noop": bool(step.get("noop")),
                }
                for step in steps
            ],
        }

    def _log_automatic_rebuild_failure_to_pending_gaps(
        self,
        conn: sqlite3.Connection,
        message: str,
        *,
        details: str | None,
    ) -> None:
        active_node = project_state.active_node_id()
        rows = conn.execute(
            "SELECT id FROM gaps_index WHERE status = 'awaiting-rebuild' "
            "AND node_id = ? ORDER BY updated ASC",
            (active_node,),
        ).fetchall()
        for row in rows:
            gap_id = row["id"]
            try:
                gap_writer.append_latest_round_log(
                    gap_id=gap_id,
                    severity="error",
                    category="target_app",
                    actor="runner",
                    message=f"Automatic target-app rebuild failed: {message}",
                    details=details,
                )
            except Exception:
                pass

    def _promote_rebuilt_gaps(self, conn: sqlite3.Connection) -> int:
        active_node = project_state.active_node_id()
        rows = conn.execute(
            "SELECT id FROM gaps_index WHERE status = 'awaiting-rebuild' "
            "AND node_id = ? ORDER BY updated ASC",
            (active_node,),
        ).fetchall()
        if not rows:
            return 0
        post_rebuild_quality = quality.enabled(conn) and quality.post_rebuild(conn)
        next_status = "qa" if post_rebuild_quality else "review"
        message = (
            "Target application rebuilt; Gap queued for QA"
            if post_rebuild_quality
            else "Target application rebuilt; Gap is ready for review"
        )
        updated = now_iso()
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = ?, updated = ? "
                "WHERE status = 'awaiting-rebuild' AND node_id = ?",
                (next_status, updated, active_node),
            )
        for row in rows:
            gid = row["id"]
            try:
                gap_writer.update_fields(gid, status=next_status, branch_name=None)
                gap_writer.append_latest_round_log(
                    gap_id=gid,
                    severity="info",
                    category="state",
                    actor="runner",
                    message=message,
                )
            except Exception:
                pass
            activity.append(
                conn,
                message=message,
                severity="info", category="state", gap_id=gid, actor="runner",
            )
        return len(rows)

    def _record_target_app_operation(self, conn: sqlite3.Connection, kind: str,
                                     result: dict, state: str) -> int:
        cur = conn.execute(
            "INSERT INTO target_app_operations "
            "(kind, state, started_at, finished_at, command, cwd, exit_code, "
            "message, stdout_tail, stderr_tail, checks_json) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                kind, state,
                result.get("started_at") or now_iso(),
                result.get("finished_at") or now_iso(),
                result.get("command") or "",
                result.get("cwd") or "",
                result.get("exit_code"),
                result.get("message") or "",
                result.get("stdout_tail") or "",
                result.get("stderr_tail") or "",
                json.dumps(result.get("checks") or []),
            ),
        )
        return int(cur.lastrowid)

    def _persist_target_app_checks(self, conn: sqlite3.Connection, checks: list[dict],
                                   message: str) -> None:
        ok = bool(checks) and all(bool(c.get("ok")) for c in checks)
        db.set_setting(conn, "target_app_last_check_at", now_iso())
        db.set_setting(conn, "target_app_last_check_ok", "1" if ok else "0")
        db.set_setting(conn, "target_app_last_check_message", message or "")
        db.set_setting(conn, "target_app_last_health_at",
                       db.get_setting(conn, "target_app_last_check_at") or "")
        db.set_setting(conn, "target_app_last_health_ok", "1" if ok else "0")
        db.set_setting(conn, "target_app_last_health_message", message or "")

    def _h_target_app_health(self, params: dict) -> dict:
        """Probe the configured health URL from the host.

        Runs in the runner process so `localhost` and 127.0.0.1 in the
        URL resolve to the host where the target app is bound. The caller
        supplies the URL (the webapp reads it from the same SQLite settings
        table either way; passing it in keeps the handler stateless).
        """
        url = (params.get("url") or "").strip()
        timeout = float(params.get("timeout") or 5.0)
        return target_app.http_health(url, timeout=timeout)

    def _h_target_app_generate(self, params: dict) -> dict:
        """Generate structured target-app config via an agent analysis pass.

        Returns `{ok, config, message}`. The webapp lets the operator
        review and save `config` if `ok` is True.
        """
        stopped = self._background_processes_stopped()
        if stopped is not None:
            return stopped
        kind = (params.get("kind") or "all").strip().lower()
        if kind not in ("all", "start", "stop", "rebuild", "status"):
            raise ValueError("kind must be 'all', 'start', 'stop', 'rebuild', or 'status'")
        activity.append(
            self._conn,
            message=f"target-app: generating {kind} instructions",
            severity="info", category="target_app", actor="refine",
        )
        provider = db.get_setting(self._conn, "agent_cli")
        result = target_app.generate_config(provider=provider)
        sev = "info" if result["ok"] else "warn"
        activity.append(
            self._conn,
            message=(f"target-app: {kind} configuration "
                     f"{'generated' if result['ok'] else 'generation failed'} — {result['message']}"),
            severity=sev, category="target_app", actor="refine",
        )
        return result


_VALID_PRIORITIES = ("low", "medium", "high")
_VALID_STATUSES = (
    "backlog", "todo", "in-progress", "qa", "ready-merge", "awaiting-rebuild",
    "review", "done", "failed", "cancelled",
)
_BULK_LAST_WORKFLOW_STATUS = "__last_workflow_state"


def _normalize_priority(value: Any) -> str:
    if isinstance(value, str):
        v = value.strip().lower()
        if v in _VALID_PRIORITIES:
            return v
    return "low"


def _normalize_priority_required(value: Any) -> str:
    if isinstance(value, str):
        v = value.strip().lower()
        if v in _VALID_PRIORITIES:
            return v
    raise ValueError("priority must be one of low/medium/high")


def _normalize_status_required(value: Any) -> str:
    if isinstance(value, str):
        v = value.strip().lower()
        if v in _VALID_STATUSES:
            return v
    raise ValueError("invalid status")


def _unique_strings(values: list[Any]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for value in values:
        if not isinstance(value, str):
            continue
        text = value.strip()
        if not text or text in seen:
            continue
        seen.add(text)
        out.append(text)
    return out


def _chunks(values: list[str], size: int = 500) -> list[list[str]]:
    return [values[idx:idx + size] for idx in range(0, len(values), size)]


# ---- chat priming -----------------------------------------------------------

def _build_plan_chat_preamble() -> tuple[str, str]:
    prompt = """\
You are helping a user plan future software work for Refine.

Discuss the user's idea, ask clarifying questions, and help shape it into
one or more implementation-ready Gaps. A Gap is a single actionable software
change with:
- a short name,
- actual/current behavior,
- target/desired behavior.

Do not create Gaps yourself. When the user is ready, they will use Refine's
Draft Gaps action to review and save proposed Gaps.
"""
    intro = (
        "[refine] Plan mode loaded. Discuss the idea here, then use "
        "Draft Gaps to review proposed Gaps before saving."
    )
    return prompt, intro


def _build_gap_chat_preamble(conn: sqlite3.Connection, gap_id: str,
                              *, worktree_present: bool = True,
                              ) -> tuple[str | None, str | None]:
    """Build a context preamble for an attached-chat session.

    Returns (priming_prompt, user_intro_line). The priming prompt is sent to
    the selected provider with output discarded so it lives in session memory
    silently; the intro line is appended to the user-visible chat buffer so
    the user knows context was loaded.

    `worktree_present` controls how we describe the cwd: True when the
    Gap's git worktree still exists (in-progress / todo / review / failed
    Gaps), False when it's already been cleaned up (done / cancelled).
    In the latter case the chat runs in the client repo for retrospective
    Q&A — and the preamble tells the agent that explicitly.
    """
    row = conn.execute(
        "SELECT name, status, branch_name FROM gaps_index WHERE id = ?",
        (gap_id,),
    ).fetchone()
    if not row:
        return None, None
    gap_json = shared_gaps.read_gap_json(gap_id, include_logs=False) or {}
    rounds = gap_json.get("rounds") or []
    latest = rounds[-1] if rounds else {}
    recent_activity = activity.recent(conn, limit=10, gap_id=gap_id)
    # Activity rows from `recent` are ordered DESC; flip for chronological.
    recent_activity = list(reversed(recent_activity))

    subpath = (db.get_setting(conn, "agent_subpath") or "").strip()
    if worktree_present:
        cwd_note = (
            f"Your cwd is `{subpath}/` inside the Gap's git worktree — a sub-"
            f"project the operator configured. The rest of the worktree (and "
            f"all git history) lives one level up; `cd ..` or absolute paths "
            f"reach it."
            if subpath else
            "You're running inside the Gap's git worktree (your cwd)."
        )
    else:
        # The Gap's worktree has been cleaned up (done / cancelled) so
        # we're in the client repo instead. Set expectations: any code
        # state you see is the current main-line, not the in-progress
        # Gap branch.
        cwd_note = (
            "This Gap's git worktree has already been cleaned up (the Gap is "
            "in a terminal state), so your cwd is the client repo itself. "
            "The Gap's commits, if merged, are on the main-line history; the "
            "branch may or may not still exist."
        )
    parts: list[str] = [
        "You're in a refine chat session about a Gap (a behavior change",
        "the team is tracking). " + cwd_note + " You can",
        "read code, run `git log`,",
        "`git status`, `git diff`, and other tools to investigate the agent's",
        "progress.",
        "",
        "First, analyze the Gap logs and context below and respond with a",
        "concise summary for this Gap. Summarize the current status, what the",
        "latest round asks for, what the logs show happened recently, and any",
        "recent errors or blockers. Do not wait for another user message before",
        "giving that opening summary.",
        "",
        "After that opening summary, when the user asks about the Gap's",
        "progress, status, or what's happening, treat their question as being",
        "about THIS GAP — not as small talk. Use the context and the live",
        "worktree state to give a specific, grounded answer.",
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
        parts.append("## Recent Gap logs/activity (oldest first)")
        for entry in recent_activity:
            ts = entry.get("datetime", "")
            msg = entry.get("message", "")
            sev = entry.get("severity", "info")
            parts.append(f"- [{ts}] ({sev}) {msg}")
    notes = gap_json.get("notes") or []
    if notes:
        parts.append("")
        parts.append("## Notes from the user")
        parts.append("These are freeform notes the operator attached to this")
        parts.append("Gap — treat them as authoritative additional context.")
        for n in notes:
            if not isinstance(n, dict):
                continue
            body = (n.get("body") or "").strip()
            if not body:
                continue
            author = (n.get("author") or "").strip()
            created = (n.get("created") or "").strip()
            header_bits = [b for b in [author, created] if b]
            header = f" ({' · '.join(header_bits)})" if header_bits else ""
            parts.append("")
            parts.append(f"### Note{header}")
            parts.append(body)
    parts += [
        "",
        "Don't commit anything to git unless I explicitly ask. Don't repeat",
        "this context block back to me verbatim — synthesize it in your",
        "answer.",
    ]
    priming_prompt = "\n".join(parts)
    intro = (
        f"[refine] Loaded Gap context for {row['name']} "
        f"({gap_id[:10]}…, status={row['status']}). You can start chatting "
        f"as soon as the indicator clears."
    )
    return priming_prompt, intro
