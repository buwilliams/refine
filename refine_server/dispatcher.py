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
import traceback
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone

from refine_server import activity, changes_index, db, governance, project_state, quality, regressions
from refine_server.gaps import now_iso, read_gap_json
from refine_server.priorities import BLOCKING_STATUSES, priority_case_sql, priority_rank

from . import gap_writer, git_ops, guidance, preflight, recovery, subprocess_mgr
from .friendly_outcome import classify_outcome


_RESET_TO_TODO_REASONS = {"priority_preempted", "paused"}
_LIMIT_PAUSE_UNTIL_KEY = "__refine_agent_limit_pause_until"
_LIMIT_PAUSE_REASON_KEY = "__refine_agent_limit_pause_reason"
_DISPATCH_NEXT_LANE_KEY = "__refine_dispatch_next_lane"
_LANES = ("todo", "qa")
_DISPATCH_PRIORITY_STATUSES = tuple(
    status for status in BLOCKING_STATUSES if status != "awaiting-rebuild"
)


@dataclass
class Dispatcher:
    """Polls SQLite for ready Gaps and launches subprocesses."""

    get_conn: callable  # type: ignore[type-arg]
    sub_mgr: subprocess_mgr.SubprocessManager
    # Called when an agent run finishes successfully — the merger
    # uses this signal to scan promptly for the new "awaiting merge"
    # Gap rather than waiting on its 10s poll. Optional so tests can
    # instantiate a Dispatcher without one.
    on_run_finished: callable | None = None  # type: ignore[type-arg]
    launch_blocked: callable | None = None  # type: ignore[type-arg]
    on_post_rebuild_quality_failed: callable | None = None  # type: ignore[type-arg]
    target_app_lock: threading.Lock | None = None
    poll_interval: float = 2.0

    _stop: threading.Event = None  # type: ignore[assignment]
    _thread: threading.Thread = None  # type: ignore[assignment]
    _tick_lock: threading.Lock = None  # type: ignore[assignment]

    def start(self) -> None:
        self._stop = threading.Event()
        self._tick_lock = threading.Lock()
        self._thread = threading.Thread(
            target=self._loop, name="refine-dispatcher", daemon=True,
        )
        self._thread.start()

    def stop(self) -> None:
        if self._stop:
            self._stop.set()
        if (
            self._thread is not None
            and self._thread.is_alive()
            and self._thread is not threading.current_thread()
        ):
            self._thread.join(timeout=5.0)

    def _loop(self) -> None:
        while not self._stop.is_set():
            try:
                self.enforce_now()
            except Exception as e:
                conn = self.get_conn()
                activity.append(
                    conn,
                    message=f"Dispatcher error: {e!r}",
                    severity="error", category="runner", actor="runner",
                    details=traceback.format_exc(limit=20),
                )
            self._stop.wait(self.poll_interval)

    def enforce_now(self) -> None:
        if self._tick_lock is None:
            self._tick_lock = threading.Lock()
        with self._tick_lock:
            self._tick()

    def _tick(self) -> None:
        conn = self.get_conn()
        paused = (
            db.get_setting_int(conn, "paused", 0)
            or db.get_setting_int(conn, "agents_paused", 0)
        )
        if paused:
            self._stop_running_agents(reason="paused")
            return
        if self.launch_blocked is not None and self.launch_blocked():
            return
        running_snapshot = self.sub_mgr.running_snapshot()
        active_node = project_state.active_node_id()
        active_running_snapshot = self._running_snapshot_for_node(
            conn,
            active_node,
            running_snapshot,
        )
        recovery.reconcile_runtime_in_progress(
            conn,
            live_gap_ids={
                r["gap_id"] for r in active_running_snapshot if r.get("gap_id")
            },
        )
        self._promote_backlog(conn)
        self._promote_disabled_quality(conn)
        if self._agent_limit_pause_active(conn):
            return

        active_rank = self._highest_blocking_priority_rank(conn)
        if active_rank is None:
            return
        if self._preempt_lower_priority_runs(conn, active_rank):
            return

        cap = self._parallel_run_cap(conn)
        running = self._active_run_count(
            conn,
            active_node=active_node,
            running_snapshot=active_running_snapshot,
        )
        if running >= cap:
            return
        self._launch_ready_lanes(conn, active_rank, cap - running)

    def _parallel_run_cap(self, conn: sqlite3.Connection) -> int:
        return max(1, db.get_setting_int(conn, "parallel_run_cap", 5))

    def _active_run_count(
        self,
        conn: sqlite3.Connection,
        *,
        active_node: str | None = None,
        running_snapshot: list[dict] | None = None,
    ) -> int:
        active_node = active_node or project_state.active_node_id()
        row = conn.execute(
            "SELECT COUNT(*) AS n FROM gaps_index "
            "WHERE status = 'in-progress' AND node_id = ?",
            (active_node,),
        ).fetchone()
        in_progress = int(row["n"] if row else 0)
        row = conn.execute(
            "SELECT COUNT(DISTINCT r.gap_id) AS n "
            "FROM runs r "
            "JOIN gaps_index g ON g.id = r.gap_id "
            "WHERE r.status = 'running' AND r.kind = 'quality' "
            "AND g.status = 'qa' AND g.node_id = ?",
            (active_node,),
        ).fetchone()
        indexed = in_progress + int(row["n"] if row else 0)
        # The SQLite state is the shared cross-runner reservation source. Keep
        # the local process snapshot in the count as a defensive fallback for
        # any launch window before the index is updated.
        local = len(self._running_snapshot_for_node(
            conn,
            active_node,
            running_snapshot,
        ))
        return max(indexed, local)

    def _running_snapshot_for_node(
        self,
        conn: sqlite3.Connection,
        active_node: str,
        running_snapshot: list[dict] | None = None,
    ) -> list[dict]:
        snapshot = (
            running_snapshot
            if running_snapshot is not None
            else self.sub_mgr.running_snapshot()
        )
        if not snapshot:
            return []
        scoped: list[dict] = []
        unknown_node: list[dict] = []
        for run in snapshot:
            node_id = str(run.get("node_id") or "")
            if node_id:
                if node_id == active_node:
                    scoped.append(run)
                continue
            if run.get("gap_id"):
                unknown_node.append(run)
        if not unknown_node:
            return scoped
        gap_ids = [str(run["gap_id"]) for run in unknown_node if run.get("gap_id")]
        placeholders = ",".join("?" * len(gap_ids))
        rows = conn.execute(
            f"SELECT id FROM gaps_index WHERE id IN ({placeholders}) AND node_id = ?",
            (*gap_ids, active_node),
        ).fetchall()
        active_gap_ids = {str(row["id"]) for row in rows}
        scoped.extend(
            run for run in unknown_node
            if str(run.get("gap_id") or "") in active_gap_ids
        )
        return scoped

    def _launch_ready_lanes(
        self,
        conn: sqlite3.Connection,
        active_rank: int,
        available_slots: int,
    ) -> None:
        if available_slots <= 0:
            return
        active_node = project_state.active_node_id()
        limit = max(1, available_slots * 2)
        pending = {
            lane: list(conn.execute(
                "SELECT id, name, branch_name FROM gaps_index "
                f"WHERE status = ? AND node_id = ? AND {priority_case_sql()} = ? "
                "ORDER BY updated ASC LIMIT ?",
                (lane, active_node, active_rank, limit),
            ).fetchall())
            for lane in _LANES
        }
        next_lane = self._next_lane(conn)
        launched = 0
        while launched < available_slots and (pending["todo"] or pending["qa"]):
            lane = next_lane if pending[next_lane] else self._other_lane(next_lane)
            if not pending[lane]:
                return
            row = pending[lane].pop(0)
            next_lane = self._other_lane(lane)
            db.set_setting(conn, _DISPATCH_NEXT_LANE_KEY, next_lane)
            gid = row["id"]
            if self.sub_mgr.is_running(gid):
                continue
            if lane == "todo":
                if governance.is_configured(conn):
                    gap = read_gap_json(gid, include_logs=False)
                    latest = (
                        (gap.get("rounds") or [])[-1]
                        if gap and gap.get("rounds") else None
                    )
                    if not latest or not governance.has_passed(latest):
                        continue
                if self._active_run_count(conn) >= self._parallel_run_cap(conn):
                    return
                self._launch_one(conn, gid, row["branch_name"])
            else:
                if not quality.enabled(conn):
                    if row["branch_name"]:
                        self._promote_quality_bypass(conn, gid)
                    else:
                        self._promote_post_rebuild_quality_bypass(conn, gid)
                    continue
                if self._active_run_count(conn) >= self._parallel_run_cap(conn):
                    return
                if not row["branch_name"] and self._post_rebuild_quality_active(conn):
                    return
                if not self._launch_quality(conn, gid, row["branch_name"]):
                    return
            launched += 1

    def _next_lane(self, conn: sqlite3.Connection) -> str:
        lane = (db.get_setting(conn, _DISPATCH_NEXT_LANE_KEY, "todo") or "todo").strip()
        return lane if lane in _LANES else "todo"

    def _other_lane(self, lane: str) -> str:
        return "qa" if lane == "todo" else "todo"

    def _highest_blocking_priority_rank(self, conn: sqlite3.Connection) -> int | None:
        placeholders = ",".join("?" * len(_DISPATCH_PRIORITY_STATUSES))
        row = conn.execute(
            f"SELECT {priority_case_sql()} AS rank FROM gaps_index "
            f"WHERE node_id = ? AND status IN ({placeholders}) "
            "ORDER BY rank ASC LIMIT 1",
            (project_state.active_node_id(), *_DISPATCH_PRIORITY_STATUSES),
        ).fetchone()
        return int(row["rank"]) if row else None

    def _preempt_lower_priority_runs(self, conn: sqlite3.Connection,
                                     active_rank: int) -> int:
        running = self._running_snapshot_for_node(
            conn,
            project_state.active_node_id(),
            self.sub_mgr.running_snapshot(),
        )
        ids = [r["gap_id"] for r in running if r.get("gap_id")]
        if not ids:
            return 0
        placeholders = ",".join("?" * len(ids))
        rows = conn.execute(
            f"SELECT id, priority FROM gaps_index WHERE id IN ({placeholders})",
            ids,
        ).fetchall()
        by_id = {r["id"]: r["priority"] for r in rows}
        preempted = 0
        for gid in ids:
            if priority_rank(by_id.get(gid)) <= active_rank:
                continue
            if self.sub_mgr.cancel(gid, reason="priority_preempted"):
                preempted += 1
        return preempted

    def _stop_running_agents(self, *, reason: str) -> int:
        stopped = 0
        for run in self.sub_mgr.running_snapshot():
            gid = run.get("gap_id")
            if gid and self.sub_mgr.cancel(gid, reason=reason):
                stopped += 1
        return stopped

    def _promote_backlog(self, conn: sqlite3.Connection) -> None:
        """Auto-promote `backlog` Gaps to `todo` once they've sat idle for
        the configured interval. -1 = never (disabled), 0 = instant,
        otherwise seconds since the row's `updated` timestamp."""
        n = db.get_setting_int(conn, "backlog_promote_after_seconds", 3600)
        if n < 0:
            return
        active_node = project_state.active_node_id()
        if n == 0:
            rows = conn.execute(
                "SELECT id FROM gaps_index WHERE status = 'backlog' AND node_id = ?",
                (active_node,),
            ).fetchall()
        else:
            cutoff = (datetime.now(timezone.utc) - timedelta(seconds=n)).strftime(
                "%Y-%m-%dT%H:%M:%SZ"
            )
            rows = conn.execute(
                "SELECT id FROM gaps_index "
                "WHERE status = 'backlog' AND node_id = ? AND updated <= ?",
                (active_node, cutoff),
            ).fetchall()
        if not rows:
            return
        ts = now_iso()
        for row in rows:
            gid = row["id"]
            if governance.is_configured(conn):
                gap = read_gap_json(gid, include_logs=False)
                if governance.latest_round_is_governance_blocked(gap):
                    continue
            with db.transaction(conn):
                cur = conn.execute(
                    "UPDATE gaps_index SET status = 'todo', updated = ? "
                    "WHERE id = ? AND status = 'backlog' AND node_id = ?",
                    (ts, gid, active_node),
                )
            if cur.rowcount:
                try:
                    gap_writer.update_fields(gid, status="todo")
                    gap_writer.append_latest_round_log(
                        gap_id=gid,
                        severity="info",
                        category="state",
                        actor="runner",
                        message="Auto-promoted from backlog to todo",
                    )
                except Exception:
                    pass
                activity.append(
                    conn,
                    message="Auto-promoted from backlog to todo",
                    severity="info", category="state",
                    gap_id=gid, actor="runner",
                )

    def _promote_disabled_quality(self, conn: sqlite3.Connection) -> None:
        if quality.enabled(conn):
            return
        rows = conn.execute(
            "SELECT id, branch_name FROM gaps_index "
            "WHERE status = 'qa' AND node_id = ? "
            "ORDER BY updated ASC",
            (project_state.active_node_id(),),
        ).fetchall()
        for row in rows:
            if self.sub_mgr.is_running(row["id"]):
                continue
            if row["branch_name"]:
                self._promote_quality_bypass(conn, row["id"])
            else:
                self._promote_post_rebuild_quality_bypass(conn, row["id"])

    def _promote_quality_bypass(self, conn: sqlite3.Connection, gap_id: str) -> None:
        active_node = project_state.active_node_id()
        with db.transaction(conn):
            cur = conn.execute(
                "UPDATE gaps_index SET status = 'ready-merge', updated = ? "
                "WHERE id = ? AND status = 'qa' AND node_id = ?",
                (now_iso(), gap_id, active_node),
            )
        if not cur.rowcount:
            return
        fields = {
            "quality_state": "passed",
            "quality_message": "Quality disabled; QA bypassed.",
            "quality_details": "",
            "quality_checked_at": now_iso(),
        }
        try:
            gap_writer.update_fields(gap_id, status="ready-merge")
            gap_writer.set_latest_round_quality(gap_id, fields)
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="info",
                category="quality",
                actor="runner",
                message="Quality disabled; QA bypassed",
            )
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="info",
                category="state",
                actor="runner",
                message="Workflow status changed: qa → ready-merge; quality disabled",
            )
        except Exception:
            pass
        activity.append(
            conn,
            message="Quality disabled; QA bypassed",
            severity="info",
            category="quality",
            gap_id=gap_id,
            actor="runner",
        )
        if self.on_run_finished is not None:
            try:
                self.on_run_finished(gap_id)
            except Exception:
                pass

    def _promote_post_rebuild_quality_bypass(
        self,
        conn: sqlite3.Connection,
        gap_id: str,
    ) -> None:
        active_node = project_state.active_node_id()
        with db.transaction(conn):
            cur = conn.execute(
                "UPDATE gaps_index SET status = 'review', updated = ? "
                "WHERE id = ? AND status = 'qa' AND branch_name IS NULL "
                "AND node_id = ?",
                (now_iso(), gap_id, active_node),
            )
        if not cur.rowcount:
            return
        fields = {
            "quality_state": "passed",
            "quality_message": "Quality disabled; post-rebuild QA bypassed.",
            "quality_details": "",
            "quality_checked_at": now_iso(),
        }
        try:
            gap_writer.update_fields(gap_id, status="review", branch_name=None)
            gap_writer.set_latest_round_quality(gap_id, fields)
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="info",
                category="quality",
                actor="runner",
                message="Quality disabled; post-rebuild QA bypassed",
            )
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="info",
                category="state",
                actor="runner",
                message="Workflow status changed: qa → review; quality disabled",
            )
        except Exception:
            pass
        activity.append(
            conn,
            message="Quality disabled; post-rebuild QA bypassed",
            severity="info",
            category="quality",
            gap_id=gap_id,
            actor="runner",
        )

    def _post_rebuild_quality_active(self, conn: sqlite3.Connection) -> bool:
        row = conn.execute(
            "SELECT COUNT(DISTINCT r.gap_id) AS n "
            "FROM runs r JOIN gaps_index g ON g.id = r.gap_id "
            "WHERE r.status = 'running' AND r.kind = 'quality' "
            "AND g.status = 'qa' AND g.branch_name IS NULL "
            "AND g.node_id = ?",
            (project_state.active_node_id(),),
        ).fetchone()
        return bool(int(row["n"] if row else 0))

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
                try:
                    gap_writer.append_latest_round_log(
                        gap_id=gap_id,
                        severity="warn",
                        category="auth",
                        actor="runner",
                        message="Retry blocked — auth pre-flight still failing",
                        details=msg or "",
                    )
                except Exception:
                    pass
                activity.append(
                    conn,
                    message="Retry blocked — auth pre-flight still failing",
                    severity="warn", category="auth",
                    gap_id=gap_id, actor="runner", details=msg or "",
                )
                return

        # Pre-checks: target branch + upstream. The agent's worktree is
        # based off the same branch the Merge agent will merge back into
        # — by default that's the host's checked-out branch, but the
        # operator can pin it via the `merge_target_branch` setting
        # (e.g. on a monorepo where you want all Gaps to merge to `main`
        # regardless of what the host happens to be on).
        target = (db.get_setting(conn, "merge_target_branch") or "").strip()
        if target:
            if not git_ops.local_branch_exists(target):
                self._abort_to_failed(
                    conn, gap_id,
                    f"Configured merge_target_branch `{target}` does not "
                    f"exist locally — create/track it first or clear the setting",
                    category="git",
                )
                return
        else:
            host_branch = git_ops.current_branch()
            if host_branch is None:
                self._abort_to_failed(
                    conn, gap_id,
                    "Client repo is in detached-HEAD state and no "
                    "merge_target_branch is configured — pickup aborted",
                    category="git",
                )
                return
            target = host_branch
        # An upstream is nice-to-have, not required. Without one we
        # operate in local-only mode: skip the fetch, base the worktree
        # off the local branch's HEAD, and (later) skip the push at
        # verify time. The Gap still ships locally.
        has_upstream = git_ops.upstream_branch(target) is not None
        if has_upstream:
            r = git_ops.fetch()
            if not r.ok:
                self._abort_to_failed(
                    conn, gap_id, "git fetch failed",
                    category="git", details=r.stderr[:2000],
                )
                return
            base_ref = f"origin/{target}"
        else:
            activity.append(
                conn,
                message=(f"Branch `{target}` has no upstream — running "
                         f"in local-only mode (skipping fetch / push)."),
                severity="info", category="git",
                gap_id=gap_id, actor="runner",
            )
            base_ref = target

        # Pre-launch validation: has this round's work already been
        # merged into target? One `Refine Gap: <gap_id>` trailered
        # merge commit lands on target per completed round, so the
        # count divides cleanly per round. If existing merges already
        # cover the latest round, skip the agent run — the work is
        # done, just queue the Gap for human approval.
        if self._maybe_skip_already_implemented(conn, gap_id, target):
            return

        # Read the Gap and run the pre-work guidance classification before
        # any agent subprocess begins.
        gap = read_gap_json(gap_id, include_logs=False)
        if not gap or not gap.get("rounds"):
            self._abort_to_failed(
                conn, gap_id, "Gap has no rounds — cannot launch",
                category="state",
            )
            return
        round_idx = len(gap["rounds"]) - 1
        latest = gap["rounds"][round_idx]
        prompt = _format_prompt(latest)
        try:
            accepted_guidance, raw_guidance = guidance.select_for_gap(conn, gap)
        except Exception as e:
            self._abort_to_failed(
                conn, gap_id, "Guidance classification failed",
                category="cli", details=repr(e)[:2000],
            )
            return
        guidance.log_selection(conn, gap, accepted_guidance, raw_guidance)
        if accepted_guidance:
            prompt = guidance.prepend_to_prompt(prompt, accepted_guidance)

        # Compute the branch name + worktree.
        pattern = db.get_setting(conn, "branch_name_pattern", "refine/{gap_id}") or "refine/{gap_id}"
        branch_name = existing_branch or pattern.format(gap_id=gap_id)

        wt = git_ops.create_worktree(gap_id, base_ref, branch_name)
        if not wt.ok:
            self._abort_to_failed(
                conn, gap_id, "git worktree create failed",
                category="git", details=wt.stderr[:2000],
            )
            return

        # Capture base for "no commits produced" detection.
        rev = git_ops._run(
            ["rev-parse", "HEAD"], cwd=git_ops.gap_worktree_path(gap_id),
        )
        base_commit = rev.stdout.strip() if rev.ok else base_ref

        # Transition: todo → in-progress
        if not self._reserve_in_progress_slot(conn, gap_id, branch_name):
            git_ops.remove_worktree(gap_id)
            if not existing_branch:
                git_ops.delete_branch(branch_name)
            return
        try:
            gap_writer.update_fields(
                gap_id, status="in-progress", branch_name=branch_name,
            )
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="info",
                category="state",
                actor="runner",
                message=(
                    "Workflow status changed: todo → in-progress; "
                    f"agent work started on `{branch_name}`"
                ),
            )
        except Exception:
            pass
        activity.append(
            conn,
            message="Agent run started",
            severity="info", category="cli",
            gap_id=gap_id, actor="runner",
        )

        idle = db.get_setting_int(conn, "agent_idle_timeout_seconds", 900)
        hard_cap = db.get_setting_int(conn, "agent_hard_cap_seconds", 86400)
        # The agent runs inside the operator-configured sub-project when set
        # (e.g. a monorepo's `apps/web`). Worktree creation + base_ref + on-
        # finished git plumbing all stay at the worktree root above; only
        # the agent subprocess cwd changes.
        worktree_root = git_ops.gap_worktree_path(gap_id)
        agent_subpath = db.get_setting(conn, "agent_subpath") or ""
        agent_cwd = git_ops.apply_agent_subpath(
            worktree_root, agent_subpath,
            log=lambda msg: activity.append(
                conn, message=f"agent_subpath: {msg}",
                severity="warn", category="state",
                gap_id=gap_id, actor="runner",
            ),
        )

        try:
            self.sub_mgr.launch(
                gap_id=gap_id,
                round_idx=round_idx,
                prompt=prompt,
                cwd=agent_cwd,
                base_ref=base_commit,
                idle_window=idle,
                hard_cap=hard_cap,
                node_id=project_state.active_node_id(),
                on_finished=(
                    lambda gid, code, reason, agent_ok, failure_text="": self._on_finished(
                        gid, round_idx, code, reason, base_commit,
                        agent_reported_success=agent_ok,
                        failure_text=failure_text,
                    )
                ),
            )
        except Exception as e:
            self._fail_reserved_launch(
                conn,
                gap_id,
                round_idx,
                f"Agent subprocess failed to start: {e!r}",
            )

    def _launch_quality(
        self,
        conn: sqlite3.Connection,
        gap_id: str,
        branch_name: str | None,
    ) -> bool:
        if not branch_name:
            return self._launch_post_rebuild_quality(conn, gap_id)
        worktree_root = git_ops.gap_worktree_path(gap_id)
        if not worktree_root.exists():
            self._fail_quality(
                conn,
                gap_id,
                "Quality cannot run because the Gap worktree is missing.",
                category="git",
            )
            return True
        gap = read_gap_json(gap_id, include_logs=False)
        if not gap or not gap.get("rounds"):
            self._fail_quality(
                conn,
                gap_id,
                "Quality cannot run because the Gap has no rounds.",
                category="state",
            )
            return True
        round_idx = len(gap["rounds"]) - 1
        rev = git_ops._run(["rev-parse", "HEAD"], cwd=worktree_root)
        if not rev.ok:
            self._fail_quality(
                conn,
                gap_id,
                "Quality cannot determine the Gap branch HEAD.",
                category="git",
                details=rev.stderr or rev.stdout,
            )
            return True
        base_commit = rev.stdout.strip()
        regression_result = None
        if regressions.enabled(conn):
            if self.target_app_lock is not None:
                acquired = self.target_app_lock.acquire(blocking=False)
            else:
                acquired = True
            if not acquired:
                regression_result = {
                    "enabled": True,
                    "ok": False,
                    "infra": True,
                    "runs": [],
                    "message": "another target-app operation is already running",
                }
            else:
                try:
                    regression_result = regressions.run_all(
                        conn,
                        target_root=worktree_root,
                    )
                finally:
                    if self.target_app_lock is not None:
                        self.target_app_lock.release()
            try:
                gap_writer.append_latest_round_log(
                    gap_id=gap_id,
                    severity="info" if regression_result.get("ok") else "warn",
                    category="quality",
                    actor="runner",
                    message=(
                        "Managed regression checks completed: "
                        f"{regression_result.get('message') or 'complete'}"
                    ),
                    details=regressions.summarize_for_prompt(regression_result),
                )
            except Exception:
                pass
            activity.append(
                conn,
                message=(
                    "Managed regression checks completed: "
                    f"{regression_result.get('message') or 'complete'}"
                ),
                severity="info" if regression_result.get("ok") else "warn",
                category="quality",
                gap_id=gap_id,
                actor="runner",
                details=regressions.summarize_for_prompt(regression_result),
            )
        prompt = quality.format_prompt(
            gap,
            settings=quality.load_settings(conn),
            regression_result=regression_result,
        )
        try:
            gap_writer.set_latest_round_quality(
                gap_id,
                {
                    "quality_state": "unclassified",
                    "quality_message": "Quality review started.",
                    "quality_details": "",
                    "quality_checked_at": "",
                },
            )
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="info",
                category="quality",
                actor="runner",
                message="Quality review started",
            )
        except Exception:
            pass
        activity.append(
            conn,
            message="Quality review started",
            severity="info",
            category="quality",
            gap_id=gap_id,
            actor="runner",
        )
        idle = db.get_setting_int(conn, "agent_idle_timeout_seconds", 900)
        hard_cap = db.get_setting_int(conn, "agent_hard_cap_seconds", 86400)
        agent_subpath = db.get_setting(conn, "agent_subpath") or ""
        agent_cwd = git_ops.apply_agent_subpath(
            worktree_root,
            agent_subpath,
            log=lambda msg: activity.append(
                conn,
                message=f"agent_subpath: {msg}",
                severity="warn",
                category="state",
                gap_id=gap_id,
                actor="runner",
            ),
        )
        try:
            self.sub_mgr.launch(
                gap_id=gap_id,
                round_idx=round_idx,
                prompt=prompt,
                cwd=agent_cwd,
                base_ref=base_commit,
                idle_window=idle,
                hard_cap=hard_cap,
                kind="quality",
                node_id=project_state.active_node_id(),
                on_finished=(
                    lambda gid, code, reason, agent_ok, failure_text="": self._on_quality_finished(
                        gid,
                        round_idx,
                        code,
                        reason,
                        base_commit,
                        agent_reported_success=agent_ok,
                        failure_text=failure_text,
                    )
                ),
            )
        except Exception as e:
            self._fail_quality(
                conn,
                gap_id,
                f"Quality subprocess failed to start: {e!r}",
                category="cli",
            )
        return True

    def _launch_post_rebuild_quality(
        self,
        conn: sqlite3.Connection,
        gap_id: str,
    ) -> bool:
        if self.target_app_lock is not None:
            acquired = self.target_app_lock.acquire(blocking=False)
            if not acquired:
                return False
        else:
            acquired = False
        worktree_root = git_ops.client_repo_path()
        gap = read_gap_json(gap_id, include_logs=False)
        if not gap or not gap.get("rounds"):
            if acquired:
                self.target_app_lock.release()
            self._fail_quality(
                conn,
                gap_id,
                "Quality cannot run because the Gap has no rounds.",
                category="state",
                revert_post_rebuild=True,
            )
            return True
        round_idx = len(gap["rounds"]) - 1
        base_commit = git_ops.rev_parse("HEAD", cwd=worktree_root) or "HEAD"
        regression_result = None
        if regressions.enabled(conn):
            regression_result = regressions.run_all(
                conn,
                target_root=worktree_root,
            )
            try:
                gap_writer.append_latest_round_log(
                    gap_id=gap_id,
                    severity="info" if regression_result.get("ok") else "warn",
                    category="quality",
                    actor="runner",
                    message=(
                        "Managed regression checks completed: "
                        f"{regression_result.get('message') or 'complete'}"
                    ),
                    details=regressions.summarize_for_prompt(regression_result),
                )
            except Exception:
                pass
            activity.append(
                conn,
                message=(
                    "Managed regression checks completed: "
                    f"{regression_result.get('message') or 'complete'}"
                ),
                severity="info" if regression_result.get("ok") else "warn",
                category="quality",
                gap_id=gap_id,
                actor="runner",
                details=regressions.summarize_for_prompt(regression_result),
            )
        prompt = quality.format_prompt(
            gap,
            settings=quality.load_settings(conn),
            regression_result=regression_result,
            timing_value=quality.POST_REBUILD,
        )
        try:
            gap_writer.set_latest_round_quality(
                gap_id,
                {
                    "quality_state": "unclassified",
                    "quality_message": "Post-rebuild QA started.",
                    "quality_details": "",
                    "quality_checked_at": "",
                },
            )
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="info",
                category="quality",
                actor="runner",
                message="Post-rebuild QA started",
            )
        except Exception:
            pass
        activity.append(
            conn,
            message="Post-rebuild QA started",
            severity="info",
            category="quality",
            gap_id=gap_id,
            actor="runner",
        )
        idle = db.get_setting_int(conn, "agent_idle_timeout_seconds", 900)
        hard_cap = db.get_setting_int(conn, "agent_hard_cap_seconds", 86400)
        agent_subpath = db.get_setting(conn, "agent_subpath") or ""
        agent_cwd = git_ops.apply_agent_subpath(
            worktree_root,
            agent_subpath,
            log=lambda msg: activity.append(
                conn,
                message=f"agent_subpath: {msg}",
                severity="warn",
                category="state",
                gap_id=gap_id,
                actor="runner",
            ),
        )
        try:
            self.sub_mgr.launch(
                gap_id=gap_id,
                round_idx=round_idx,
                prompt=prompt,
                cwd=agent_cwd,
                base_ref=base_commit,
                idle_window=idle,
                hard_cap=hard_cap,
                kind="quality",
                node_id=project_state.active_node_id(),
                on_finished=(
                    lambda gid, code, reason, agent_ok, failure_text="": self._finish_post_rebuild_quality(
                        gid,
                        round_idx,
                        code,
                        reason,
                        base_commit,
                        acquired,
                        agent_reported_success=agent_ok,
                        failure_text=failure_text,
                    )
                ),
            )
        except Exception as e:
            if acquired:
                self.target_app_lock.release()
            self._fail_quality(
                conn,
                gap_id,
                f"Post-rebuild QA subprocess failed to start: {e!r}",
                category="cli",
                revert_post_rebuild=True,
            )
        return True

    def _reserve_in_progress_slot(
        self,
        conn: sqlite3.Connection,
        gap_id: str,
        branch_name: str,
    ) -> bool:
        active_node = project_state.active_node_id()
        parallel_cap = self._parallel_run_cap(conn)
        with db.transaction(conn):
            cur = conn.execute(
                "UPDATE gaps_index SET status = 'in-progress', updated = ?, branch_name = ? "
                "WHERE id = ? AND status = 'todo' AND node_id = ? "
                "AND ("
                "  SELECT COUNT(*) FROM gaps_index "
                "  WHERE status = 'in-progress' AND node_id = ?"
                ") + ("
                "  SELECT COUNT(DISTINCT r.gap_id) FROM runs r "
                "  JOIN gaps_index g ON g.id = r.gap_id "
                "  WHERE r.status = 'running' AND r.kind = 'quality' "
                "  AND g.status = 'qa' AND g.node_id = ?"
                ") < ?",
                (
                    now_iso(), branch_name, gap_id, active_node,
                    active_node, active_node, parallel_cap,
                ),
            )
        return bool(cur.rowcount)

    def _fail_reserved_launch(
        self,
        conn: sqlite3.Connection,
        gap_id: str,
        round_idx: int,
        message: str,
    ) -> None:
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'failed', updated = ? "
                "WHERE id = ? AND status = 'in-progress'",
                (now_iso(), gap_id),
            )
        try:
            gap_writer.update_fields(gap_id, status="failed")
            gap_writer.append_round_log(
                gap_id=gap_id,
                round_idx=round_idx,
                severity="error",
                category="cli",
                actor="runner",
                message=message,
            )
        except Exception:
            pass
        activity.append(
            conn,
            message=message,
            severity="error", category="cli",
            gap_id=gap_id, actor="runner",
        )

    def _maybe_skip_already_implemented(self, conn: sqlite3.Connection,
                                          gap_id: str, target: str) -> bool:
        """Pre-launch idempotency guard. Returns True if we skipped
        the launch because the latest round's work is already on the
        target branch.

        The signal: count merge commits on `target` whose body carries
        `Refine Gap: <gap_id>`. Each completed round produces exactly
        one such commit, so:

            n_merges >= n_rounds  ⇒  the latest round is already done

        Round 1 with no prior merges: 0 merges, 1 round → 0 < 1 → run.
        Round 2 after round 1 merged: 1 merge, 2 rounds → 1 < 2 → run.
        Round 1 with a leftover merge (e.g., runner crashed after
        verify): 1 merge, 1 round → 1 >= 1 → skip.

        On skip we send the Gap to `review` directly (todo → review,
        skipping in-progress) since the work is already on target and
        the human just needs to approve it. No worktree created, no
        agent spawned.
        """
        gap = read_gap_json(gap_id, include_logs=False)
        if not gap:
            return False
        rounds = gap.get("rounds") or []
        n_rounds = len(rounds)
        if n_rounds == 0:
            return False
        n_merges = changes_index.count_for_gap(conn, gap_id, target)
        if n_merges < n_rounds:
            return False

        round_idx = n_rounds - 1
        msg = (f"Skipped agent run — this round's work is already on "
               f"`{target}` ({n_merges} merge commit"
               f"{'' if n_merges == 1 else 's'} for this Gap). "
               f"Waiting for target-app rebuild before review.")
        # Log to the latest round's log file so the audit trail shows
        # why we bypassed the agent for this specific round.
        try:
            gap_writer.append_round_log(
                gap_id=gap_id, round_idx=round_idx,
                severity="info", category="state", actor="runner",
                message=msg,
            )
        except Exception:
            pass
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'awaiting-rebuild', updated = ? "
                "WHERE id = ? AND status = 'todo'",
                (now_iso(), gap_id),
            )
        try:
            gap_writer.update_fields(gap_id, status="awaiting-rebuild")
        except Exception:
            pass
        activity.append(
            conn, message=msg,
            severity="info", category="state",
            gap_id=gap_id, actor="runner",
        )
        return True

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
        try:
            gap_writer.update_fields(gap_id, status="failed")
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="error",
                category=category,
                actor="runner",
                message=f"Workflow status changed: todo → failed; {message}",
                details=details or None,
            )
        except Exception:
            pass

    def _on_finished(self, gap_id: str, round_idx: int, exit_code: int,
                     killed_reason: str | None, base_commit: str,
                     *, agent_reported_success: bool | None = None,
                     failure_text: str | None = None) -> None:
        conn = self.get_conn()
        if killed_reason in _RESET_TO_TODO_REASONS:
            self._reset_stopped_run_to_todo(conn, gap_id, round_idx, killed_reason)
            return

        cwd = git_ops.gap_worktree_path(gap_id)
        new_commits = git_ops.commits_on_branch_since(base_commit, cwd)
        no_new_commits = new_commits == 0

        outcome = classify_outcome(
            exit_code=exit_code,
            killed_reason=killed_reason,
            no_new_commits=no_new_commits,
            agent_reported_success=agent_reported_success,
            failure_text=failure_text,
        )

        success = outcome.kind == "success"

        # Failure path: move straight to `failed` and we're done.
        # Success path depends on Quality timing. Pre-merge Quality waits in
        # `qa`; post-rebuild Quality lets the Merger land work first and runs
        # after the shared target app is rebuilt.
        if success and quality.enabled(conn) and quality.post_rebuild(conn):
            next_status = "ready-merge"
        else:
            next_status = "qa" if success else "failed"
        with db.transaction(conn):
            cur = conn.execute(
                "UPDATE gaps_index SET status = ?, updated = ? "
                "WHERE id = ? AND status = 'in-progress'",
                (next_status, now_iso(), gap_id),
            )
        if cur.rowcount == 0:
            return
        try:
            gap_writer.update_fields(gap_id, status=next_status)
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="info" if success else outcome.severity,
                category="state",
                actor="runner",
                message=f"Workflow status changed: in-progress → {next_status}",
            )
        except Exception:
            pass

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
        if outcome.limit_kind:
            self._pause_after_limit_failure(conn, outcome.limit_kind, gap_id=gap_id)

        if success and not quality.enabled(conn):
            self._promote_quality_bypass(conn, gap_id)
            return

        if success and next_status == "ready-merge" and self.on_run_finished is not None:
            try:
                self.on_run_finished(gap_id)
            except Exception:
                pass

        # Passing pre-merge implementation now waits in `qa`; the Quality run
        # will wake the Merger after it promotes the Gap to `ready-merge`.

    def _on_quality_finished(
        self,
        gap_id: str,
        round_idx: int,
        exit_code: int,
        killed_reason: str | None,
        base_commit: str,
        *,
        agent_reported_success: bool | None = None,
        failure_text: str | None = None,
        post_rebuild: bool = False,
    ) -> None:
        conn = self.get_conn()
        if killed_reason in _RESET_TO_TODO_REASONS:
            self._reset_stopped_quality(
                conn,
                gap_id,
                round_idx,
                killed_reason,
                base_commit,
                post_rebuild=post_rebuild,
            )
            return

        outcome = classify_outcome(
            exit_code=exit_code,
            killed_reason=killed_reason,
            no_new_commits=False,
            agent_reported_success=agent_reported_success,
            failure_text=failure_text,
        )
        success = outcome.kind == "success"
        if post_rebuild:
            self._discard_post_rebuild_quality_changes(base_commit)
        elif success:
            commit = self._commit_quality_changes(gap_id)
            if not commit.get("ok"):
                outcome = type(outcome)(
                    "failure",
                    "git",
                    "error",
                    commit.get("message") or "Quality changes could not be committed",
                    commit.get("details"),
                    None,
                )
                success = False

        next_status = "review" if post_rebuild and success else (
            "ready-merge" if success else "failed"
        )
        with db.transaction(conn):
            cur = conn.execute(
                "UPDATE gaps_index SET status = ?, updated = ? "
                "WHERE id = ? AND status = 'qa' AND node_id = ?",
                (next_status, now_iso(), gap_id, project_state.active_node_id()),
            )
        if cur.rowcount == 0:
            return

        fields = {
            "quality_state": "passed" if success else "failed",
            "quality_message": (
                (
                    "Post-rebuild QA passed; Gap is ready for review."
                    if post_rebuild
                    else "Quality passed."
                ) if success
                else (outcome.message or "Quality failed.")
            ),
            "quality_details": outcome.details or "",
            "quality_checked_at": now_iso(),
        }
        try:
            gap_writer.update_fields(gap_id, status=next_status)
            gap_writer.set_latest_round_quality(gap_id, fields)
            gap_writer.append_round_log(
                gap_id=gap_id,
                round_idx=round_idx,
                severity="info" if success else outcome.severity,
                category="quality",
                actor="runner",
                message=fields["quality_message"],
                details=fields["quality_details"] or None,
            )
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="info" if success else outcome.severity,
                category="state",
                actor="runner",
                message=f"Workflow status changed: qa → {next_status}",
            )
        except Exception:
            pass

        activity.append(
            conn,
            message=fields["quality_message"],
            severity="info" if success else outcome.severity,
            category="quality",
            gap_id=gap_id,
            actor="runner",
            details=fields["quality_details"] or None,
        )
        if outcome.limit_kind:
            self._pause_after_limit_failure(conn, outcome.limit_kind, gap_id=gap_id)
        if post_rebuild and not success and self.on_post_rebuild_quality_failed is not None:
            try:
                self.on_post_rebuild_quality_failed(
                    gap_id,
                    fields["quality_message"],
                    fields["quality_details"],
                )
            except Exception:
                pass
        if success and not post_rebuild and self.on_run_finished is not None:
            try:
                self.on_run_finished(gap_id)
            except Exception:
                pass

    def _finish_post_rebuild_quality(
        self,
        gap_id: str,
        round_idx: int,
        exit_code: int,
        killed_reason: str | None,
        base_commit: str,
        release_target_lock: bool,
        *,
        agent_reported_success: bool | None = None,
        failure_text: str | None = None,
    ) -> None:
        try:
            self._on_quality_finished(
                gap_id,
                round_idx,
                exit_code,
                killed_reason,
                base_commit,
                agent_reported_success=agent_reported_success,
                failure_text=failure_text,
                post_rebuild=True,
            )
        finally:
            if release_target_lock and self.target_app_lock is not None:
                self.target_app_lock.release()

    def _commit_quality_changes(self, gap_id: str) -> dict:
        cwd = git_ops.gap_worktree_path(gap_id)
        paths = git_ops.dirty_paths(cwd=cwd)
        if not paths:
            return {"ok": True, "message": "No quality test changes to commit."}
        result = git_ops.add_and_commit(
            paths,
            f"refine: quality checks for {gap_id}",
            cwd=cwd,
        )
        if not result.ok:
            return {
                "ok": False,
                "message": "Quality changes could not be committed.",
                "details": result.stderr or result.stdout,
            }
        return {"ok": True, "message": "Quality changes committed."}

    def _discard_post_rebuild_quality_changes(self, base_commit: str) -> None:
        repo = git_ops.client_repo_path()
        git_ops.reset_hard(base_commit, cwd=repo)
        git_ops._run(["clean", "-fd", "--", ".", ":!.refine"], cwd=repo)

    def _fail_quality(
        self,
        conn: sqlite3.Connection,
        gap_id: str,
        message: str,
        *,
        category: str,
        details: str | None = None,
        revert_post_rebuild: bool = False,
    ) -> None:
        with db.transaction(conn):
            cur = conn.execute(
                "UPDATE gaps_index SET status = 'failed', updated = ? "
                "WHERE id = ? AND status = 'qa' AND node_id = ?",
                (now_iso(), gap_id, project_state.active_node_id()),
            )
        if not cur.rowcount:
            return
        fields = {
            "quality_state": "failed",
            "quality_message": message,
            "quality_details": details or "",
            "quality_checked_at": now_iso(),
        }
        try:
            gap_writer.update_fields(gap_id, status="failed")
            gap_writer.set_latest_round_quality(gap_id, fields)
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="error",
                category="quality",
                actor="runner",
                message=message,
                details=details,
            )
            gap_writer.append_latest_round_log(
                gap_id=gap_id,
                severity="error",
                category="state",
                actor="runner",
                message=f"Workflow status changed: qa → failed; {message}",
                details=details,
            )
        except Exception:
            pass
        activity.append(
            conn,
            message=message,
            severity="error",
            category=category,
            gap_id=gap_id,
            actor="runner",
            details=details,
        )
        if revert_post_rebuild and self.on_post_rebuild_quality_failed is not None:
            try:
                self.on_post_rebuild_quality_failed(
                    gap_id,
                    fields["quality_message"],
                    fields["quality_details"],
                )
            except Exception:
                pass

    def _agent_limit_pause_active(self, conn: sqlite3.Connection) -> bool:
        raw = db.get_setting(conn, _LIMIT_PAUSE_UNTIL_KEY, "") or ""
        try:
            until = float(raw)
        except ValueError:
            return False
        if until <= time.time():
            db.set_setting(conn, _LIMIT_PAUSE_UNTIL_KEY, "")
            db.set_setting(conn, _LIMIT_PAUSE_REASON_KEY, "")
            return False
        return True

    def _pause_after_limit_failure(
        self,
        conn: sqlite3.Connection,
        limit_kind: str,
        *,
        gap_id: str,
    ) -> None:
        seconds = db.get_setting_int(conn, "agent_limit_pause_seconds", 60)
        if seconds <= 0:
            return
        until = time.time() + seconds
        db.set_setting(conn, _LIMIT_PAUSE_UNTIL_KEY, f"{until:.3f}")
        db.set_setting(conn, _LIMIT_PAUSE_REASON_KEY, limit_kind)
        label = "rate limit" if limit_kind == "rate_limit" else "token limit"
        activity.append(
            conn,
            message=f"Agent scheduling paused for {seconds} seconds after {label}",
            severity="warn",
            category="cli",
            gap_id=gap_id,
            actor="runner",
        )

    def _reset_stopped_run_to_todo(self, conn: sqlite3.Connection, gap_id: str,
                                   round_idx: int, reason: str) -> None:
        row = conn.execute(
            "SELECT status, branch_name, node_id FROM gaps_index WHERE id = ?",
            (gap_id,),
        ).fetchone()
        if not row:
            return

        branch_name = row["branch_name"]
        active_node = project_state.active_node_id()
        if row["status"] == "in-progress" and row["node_id"] == active_node:
            with db.transaction(conn):
                conn.execute(
                    "UPDATE gaps_index SET status = 'todo', branch_name = NULL, "
                    "updated = ? WHERE id = ? AND status = 'in-progress' "
                    "AND node_id = ?",
                    (now_iso(), gap_id, active_node),
                )
            try:
                gap_writer.update_fields(gap_id, status="todo", branch_name=None)
            except Exception:
                pass

        git_ops.remove_worktree(gap_id)
        if branch_name:
            git_ops.delete_branch(branch_name)

        if reason == "priority_preempted":
            message = (
                "Agent run stopped because higher-priority Gap work is blocking "
                "lower priorities — moved back to todo and discarded partial work."
            )
        else:
            message = (
                "Agent run stopped because agents were paused — moved back to "
                "todo and discarded partial work."
            )
        try:
            gap_writer.append_round_log(
                gap_id=gap_id,
                round_idx=round_idx,
                severity="warn",
                category="state",
                actor="runner",
                message=message,
            )
        except Exception:
            pass
        activity.append(
            conn,
            message=message,
            severity="warn",
            category="state",
            gap_id=gap_id,
            actor="runner",
        )

    def _reset_stopped_quality(
        self,
        conn: sqlite3.Connection,
        gap_id: str,
        round_idx: int,
        reason: str,
        base_commit: str,
        *,
        post_rebuild: bool = False,
    ) -> None:
        cwd = git_ops.client_repo_path() if post_rebuild else git_ops.gap_worktree_path(gap_id)
        git_ops.reset_hard(base_commit, cwd=cwd)
        if post_rebuild:
            git_ops._run(["clean", "-fd", "--", ".", ":!.refine"], cwd=cwd)
        else:
            git_ops.clean_untracked(cwd=cwd)
        if reason == "priority_preempted":
            message = (
                "Quality run stopped because higher-priority Gap work is blocking "
                "lower priorities — moved back to qa and discarded partial QA work."
            )
        else:
            message = (
                "Quality run stopped because agents were paused — moved back to "
                "qa and discarded partial QA work."
            )
        try:
            gap_writer.append_round_log(
                gap_id=gap_id,
                round_idx=round_idx,
                severity="warn",
                category="quality",
                actor="runner",
                message=message,
            )
        except Exception:
            pass
        activity.append(
            conn,
            message=message,
            severity="warn",
            category="quality",
            gap_id=gap_id,
            actor="runner",
        )


def _format_prompt(round_obj: dict) -> str:
    return (
        f"You are working on a software change.\n\n"
        f"Current behavior (actual):\n{round_obj.get('actual','').strip()}\n\n"
        f"Desired behavior (target):\n{round_obj.get('target','').strip()}\n\n"
        f"Make the necessary code changes in this worktree. Commit your changes "
        f"with clear messages. Run any relevant tests. When you're satisfied, exit."
    )
