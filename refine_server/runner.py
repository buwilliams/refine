"""Main runner orchestrator for dispatcher, subprocess manager, and backend calls.

The runner is the sole writer of gap.json.
"""
from __future__ import annotations

import json
import sqlite3
import threading
from pathlib import Path
from typing import Any

from refine_server import activity, db, features, gaps as shared_gaps, reporters
from refine_server.gaps import now_iso
from refine_server.backend_protocol import (
    M_APPEND_ROUND, M_CANCEL, M_CHAT_INPUT, M_CHAT_READ, M_CHAT_START,
    M_CHAT_STOP, M_CREATE_GAP, M_DELETE_GAP, M_DIAGNOSTICS, M_EDIT_ROUND,
    M_EXTRACT_GAPS, M_LAUNCH, M_LIST_CHANGES, M_LOG_APPEND, M_PING,
    M_PREFLIGHT, M_RENAME_REPORTER, M_RENAME_REPORTER_STRINGS, M_RUNNING,
    M_SET_NOTES, M_TARGET_APP_GENERATE, M_TARGET_APP_HEALTH,
    M_TARGET_APP_RUN, M_UNDO_GAP, M_VERIFY,
)

from . import dispatcher as _dispatcher
from . import gap_writer, git_ops, llm, merger as _merger, preflight, recovery, state_committer, subprocess_mgr, target_app, verify_op
from .chat_mgr import ChatManager


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

        self.sub_mgr = subprocess_mgr.SubprocessManager(self._get_conn)
        # The merger owns the host worktree — everything that merges,
        # auto-resolves conflicts, or cleans stale git op state goes
        # through it. See refine_server/merger.py for the rationale.
        self.merger = _merger.Merger(
            get_conn=self._get_conn, sub_mgr=self.sub_mgr,
        )
        self.dispatcher = _dispatcher.Dispatcher(
            get_conn=self._get_conn, sub_mgr=self.sub_mgr,
            on_run_finished=lambda _gid: self.merger.wake(),
        )
        self.chat = ChatManager(
            get_standalone_idle_timeout=lambda: db.get_setting_int(
                self._conn, "chat_idle_timeout_seconds", 300,
            ),
        )
        self.state_committer = state_committer.StateCommitter(
            get_conn=self._get_conn,
        )
        self._target_app_lock = threading.Lock()
        self._diag_lock = threading.Lock()
        self._last_call_at: str | None = None
        self._recent_errors: list[str] = []

    def _get_conn(self) -> sqlite3.Connection:
        return self._conn

    # ---- lifecycle -----------------------------------------------------------

    def start(self) -> None:
        db.init_db()
        recovery.reconcile_on_start(self._conn)
        preflight.check(self._conn)
        self.dispatcher.start()
        self.merger.start()
        self.state_committer.start()
        activity.append(
            self._conn, message="refine-server started",
            severity="info", category="state", actor="runner",
        )

    def shutdown(self) -> None:
        self.chat.shutdown()
        self.sub_mgr.cancel_all("shutdown")
        try:
            self.state_committer.commit_now()
        except Exception:
            pass
        self.state_committer.stop()
        self.merger.stop()
        self.dispatcher.stop()
        activity.append(
            self._conn, message="refine-server stopping",
            severity="info", category="state", actor="runner",
        )

    # ---- direct backend routing ---------------------------------------------

    def call(self, method: str, params: dict | None = None) -> dict:
        with self._diag_lock:
            self._last_call_at = now_iso()
        try:
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
            M_CANCEL: self._h_cancel,
            M_VERIFY: self._h_verify,
            M_CREATE_GAP: self._h_create_gap,
            M_APPEND_ROUND: self._h_append_round,
            M_EDIT_ROUND: self._h_edit_round,
            M_LOG_APPEND: self._h_log_append,
            M_DELETE_GAP: self._h_delete_gap,
            M_SET_NOTES: self._h_set_notes,
            M_CHAT_START: self._h_chat_start,
            M_CHAT_INPUT: self._h_chat_input,
            M_CHAT_READ: self._h_chat_read,
            M_CHAT_STOP: self._h_chat_stop,
            M_EXTRACT_GAPS: self._h_extract_gaps,
            M_RENAME_REPORTER: self._h_rename_reporter,
            M_RENAME_REPORTER_STRINGS: self._h_rename_reporter_strings,
            M_LIST_CHANGES: self._h_list_changes,
            M_UNDO_GAP: self._h_undo_gap,
            M_TARGET_APP_RUN: self._h_target_app_run,
            M_TARGET_APP_GENERATE: self._h_target_app_generate,
            M_TARGET_APP_HEALTH: self._h_target_app_health,
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
        return {
            "running": self.sub_mgr.running_snapshot(),
            "merger": self.merger.snapshot(),
        }

    def _h_diagnostics(self, _: dict) -> dict:
        with self._diag_lock:
            return {
                "mode": "in-process",
                "last_call_at": self._last_call_at,
                "recent_errors": list(self._recent_errors[-10:]),
            }

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
        # Route through the merger so a user-triggered Verify can't race
        # with an in-flight auto-verify of another Gap. The merger's
        # single lock serializes everything that touches the host
        # worktree.
        return self.merger.verify_now(params["gap_id"])

    def _h_create_gap(self, params: dict) -> dict:
        gap_id = params["gap_id"]
        name = params.get("name", "Untitled Gap")
        priority = _normalize_priority(params.get("priority"))
        round_obj = shared_gaps.new_round(
            reporter=params["reporter"],
            actual=params.get("actual", ""),
            target=params.get("target", ""),
        )
        gap = gap_writer.create_gap(gap_id=gap_id, name=name, initial_round=round_obj)

        from refine_server.paths import relative_gap_path
        with db.transaction(self._conn):
            self._conn.execute(
                "INSERT INTO gaps_index "
                "(id, name, status, priority, reporter, created, updated, json_path) "
                "VALUES (?, ?, 'backlog', ?, ?, ?, ?, ?)",
                (gap_id, name, priority, params["reporter"],
                 gap["created"], gap["updated"], relative_gap_path(gap_id)),
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
        # review → todo (or todo if currently failed/review/done; webapp guards this).
        # The new round's reporter is now the latest, so mirror that onto
        # the index column.
        with db.transaction(self._conn):
            self._conn.execute(
                "UPDATE gaps_index SET status = 'todo', reporter = ?, updated = ? "
                "WHERE id = ?",
                (params["reporter"], now_iso(), gap_id),
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
        # Keep the index reporter in sync with the latest round when the
        # reporter was actually changed by this edit. (None means "leave
        # it alone" in `edit_latest_round`.)
        if params.get("reporter") is not None:
            with db.transaction(self._conn):
                self._conn.execute(
                    "UPDATE gaps_index SET reporter = ? WHERE id = ?",
                    (params["reporter"], params["gap_id"]),
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

    def _h_set_notes(self, params: dict) -> dict:
        gap_id = params["gap_id"]
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
        activity.append(
            self._conn,
            message=f"Notes updated ({len(gap['notes'])} note{'' if len(gap['notes']) == 1 else 's'})",
            severity="info", category="state",
            gap_id=gap_id, actor="refine",
        )
        return {"gap": gap}

    # ---- chat ----------------------------------------------------------------

    def _h_extract_gaps(self, params: dict) -> dict:
        if not features.is_enabled(self._conn, "import_gaps"):
            provider = features.current_provider(self._conn)
            return {
                "ok": False,
                "code": "feature_disabled",
                "feature": "import_gaps",
                "provider": provider,
                "message": (
                    f"Import (LLM extraction) is disabled for the "
                    f"`{provider}` provider. Pick a supported provider on "
                    f"Settings -> AI Provider, or enable the override on "
                    f"the same tab's Feature flags section (experimental)."
                ),
                "drafts": [],
            }
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
            self._conn, old_name, new_name,
        )
        activity.append(
            self._conn,
            message=(f"Reporter renamed: {old_name!r} → {new_name!r} "
                     f"({touched} gap{'' if touched == 1 else 's'} updated)"),
            severity="info", category="state", actor="refine",
        )
        return {"old": old_name, "new": new_name, "touched": touched}

    def _h_rename_reporter_strings(self, params: dict) -> dict:
        """Cascade-only: rewrite round.reporter == old to new across all
        Gaps without touching the reporters table. Used for one-off data
        migrations (e.g. an orphan string that's not in the table)."""
        old = (params.get("old") or "").strip()
        new = (params.get("new") or "").strip()
        if not old or not new:
            raise ValueError("both `old` and `new` are required")
        touched = gap_writer.rename_reporter_in_rounds(self._conn, old, new)
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
        configured = (db.get_setting(self._conn, "merge_target_branch")
                      or "").strip()
        if configured:
            return configured
        return git_ops.current_branch()

    def _h_list_changes(self, params: dict) -> dict:
        """List refine merge commits on the target branch.

        Returns each commit plus the Gap metadata refine knows about
        (name + current status). Powers the Changes screen.
        """
        target = self._effective_target_branch()
        if not target:
            return {"branch": None, "changes": []}
        limit = int(params.get("limit") or 50)
        merges = git_ops.list_refine_merges(target, limit=limit)
        if not merges:
            return {"branch": target, "changes": []}
        ids = [m["gap_id"] for m in merges]
        placeholders = ",".join("?" * len(ids))
        rows = self._conn.execute(
            f"SELECT id, name, status, priority "
            f"FROM gaps_index WHERE id IN ({placeholders})",
            ids,
        ).fetchall()
        by_id = {r["id"]: r for r in rows}
        for m in merges:
            row = by_id.get(m["gap_id"])
            m["name"] = row["name"] if row else None
            m["status"] = row["status"] if row else None
            m["priority"] = row["priority"] if row else None
        return {"branch": target, "changes": merges}

    def _h_undo_gap(self, params: dict) -> dict:
        """Revert a refine merge commit and transition the Gap to
        `cancelled` with a log entry.

        Routes through the merger's host lock so a concurrent auto-
        merge or user Verify can't race with us on the shared host
        worktree. The merger also runs its cleanup-worktree pass
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
            "SELECT status FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
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
                p = git_ops.push_current()
                if p.ok:
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
                        details=p.stderr[:2000],
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
            gap = shared_gaps.read_gap_json(gap_id) or {}
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

    def _h_chat_start(self, params: dict) -> dict:
        if not features.is_enabled(self._conn, "chat"):
            provider = features.current_provider(self._conn)
            return {
                "ok": False,
                "code": "feature_disabled",
                "feature": "chat",
                "provider": provider,
                "message": (
                    f"Chat is disabled for the `{provider}` provider. "
                    f"Pick a supported provider on Settings -> AI Provider, or "
                    f"enable the override on the same tab's Feature "
                    f"flags section (experimental)."
                ),
            }
        gap_id = params.get("gap_id")
        priming_prompt: str | None = None
        priming_intro: str | None = None
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
            priming_prompt, priming_intro = _build_gap_chat_preamble(
                self._conn, gap_id, worktree_present=worktree_present,
            )
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
            result = target_app.run_operation(kind, config)
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
        kind = (params.get("kind") or "all").strip().lower()
        if kind not in ("all", "start", "stop", "status"):
            raise ValueError("kind must be 'all', 'start', 'stop', or 'status'")
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


def _normalize_priority(value: Any) -> str:
    if isinstance(value, str):
        v = value.strip().lower()
        if v in _VALID_PRIORITIES:
            return v
    return "low"


# ---- chat priming -----------------------------------------------------------

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
    gap_json = shared_gaps.read_gap_json(gap_id) or {}
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
        "answer. The user's first message follows.",
    ]
    priming_prompt = "\n".join(parts)
    intro = (
        f"[refine] Loaded Gap context for {row['name']} "
        f"({gap_id[:10]}…, status={row['status']}). You can start chatting "
        f"as soon as the indicator clears."
    )
    return priming_prompt, intro
