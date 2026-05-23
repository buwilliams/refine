"""Single-threaded Merge agent that owns the host worktree.

Everything that touches the host repo's `HEAD` / index for Gap merging
is serialized through this one component. The two problems it solves:

1. **Race condition.** Multiple agent runs finishing in close
   succession used to each fire their own `verify_op.perform_verify`
   from the dispatcher thread, fighting over `git merge` on the
   shared host worktree. With one merger, only one merge is ever in
   flight at a time.

2. **Stale state cleanup.** A merge that hit conflicts in the past
   could leave `MERGE_HEAD` (or `REBASE_HEAD`, etc.) lying around in
   `.git/`. Every later verify then tripped on the precheck and
   bailed straight to `review`, cascading every queued Gap into
   `review` for "merge conflict — human resolution". A conflicted
   `git stash apply` can also leave unmerged index entries without any
   `.git/` sentinel. The merger now aborts or resets leftover stuck
   state at the start of every tick — and again before each merge
   attempt — so a single stuck Gap can never block the rest of the
   queue.

Status semantics:

- A Gap whose agent succeeded transitions to `ready-merge` (the
  dispatcher owns that flip). `ready-merge` is system-owned: the
  user never sets or clears it. The merger's `_find_one_ready()`
  query picks up any `ready-merge` Gap, oldest-finished first.
- The merger calls `verify_op.perform_verify`. On success, it parks
  the Gap in `awaiting-rebuild`; a target-app rebuild promotes it to
  `review`. On failure, the merger transitions the Gap to `failed`
  and cleans the host worktree so the next ready Gap can proceed.

On runner restart, `recovery.reconcile_on_start` distinguishes "agent
crashed mid-run" (orphan → `failed`) from "agent finished, awaiting
merge" (was already `ready-merge`, or now bumped to it from
`in-progress` if the dispatcher crashed mid-flip). The merger's
first tick after start drains anything that piled up.
"""
from __future__ import annotations

import sqlite3
import threading
import time
from collections.abc import Callable

from refine_server import activity, db, project_state
from refine_server.gaps import now_iso

from . import gap_writer, git_ops, mutation_guard, subprocess_mgr, verify_op


# How long the merger sleeps between scans when there's no signal. A
# wake() from the dispatcher (or another caller) shortcuts this.
_POLL_INTERVAL_SECONDS = 10.0


class Merger:
    def __init__(
        self,
        *,
        get_conn,
        sub_mgr: subprocess_mgr.SubprocessManager,
        on_worktree_merged: Callable[[str], None] | None = None,
        queue_rebuild_for_pending: Callable[[], bool] | None = None,
    ) -> None:
        self._get_conn = get_conn
        self._sub_mgr = sub_mgr
        self._on_worktree_merged = on_worktree_merged
        self._queue_rebuild_for_pending = queue_rebuild_for_pending
        self._wake = threading.Event()
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        # Serializes anything that mutates the host worktree for merge,
        # undo, or conflict resolution.
        self._host_lock = threading.Lock()
        # Snapshot state for the Agents screen. Updated as the merger
        # picks up / releases each Gap so the UI can render the
        # current activity without polling git.
        self._snap_lock = threading.Lock()
        self._current_gap_id: str | None = None
        self._current_started: float | None = None  # monotonic
        self._last_outcome: str | None = None

    # ---- lifecycle -----------------------------------------------------------

    def start(self) -> None:
        self._thread = threading.Thread(
            target=self._loop, name="refine-merger", daemon=True,
        )
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._wake.set()
        if (
            self._thread is not None
            and self._thread.is_alive()
            and self._thread is not threading.current_thread()
        ):
            self._thread.join(timeout=5.0)

    def wake(self) -> None:
        """Dispatcher calls this when an agent run finishes successfully;
        backend handlers call it after edits that might unblock a Gap (Retry,
        new round). Just sets the event — the loop scans the next tick."""
        self._wake.set()

    def snapshot(self) -> dict:
        """One-shot status view for the Agents screen. Returns the
        Gap currently being merged (if any), how long the merger has
        been working on it, the count of Gaps queued behind it, and
        whether the global `paused` toggle is in effect — the merger
        respects the same pause flag as the dispatcher."""
        paused = bool(db.get_setting_int(self._get_conn(), "paused", 0))
        with self._snap_lock:
            gap_id = self._current_gap_id
            started = self._current_started
            last = self._last_outcome
        elapsed = (int(time.monotonic() - started)
                   if started is not None else 0)
        # Queue depth: `ready-merge` Gaps waiting on the merger, minus
        # the one currently merging (if any). `ready-merge` is
        # system-owned and only ever set by the dispatcher after a
        # successful agent run, so the count is the merge backlog.
        queued = 0
        active_instance = project_state.active_instance_id()
        for row in self._get_conn().execute(
            "SELECT id FROM gaps_index "
            "WHERE status = 'ready-merge' AND instance_id = ?",
            (active_instance,),
        ):
            gid = row["id"]
            if gid == gap_id:
                continue
            queued += 1
        if paused and gap_id is None:
            state = "paused"
        elif gap_id is not None:
            state = "merging"
        else:
            state = "idle"
        return {
            "state": state,
            "paused": paused,
            "gap_id": gap_id,
            "elapsed_seconds": elapsed,
            "queued": queued,
            "last_outcome": last,
        }

    # ---- synchronous entry point ---------------------------------------------

    def run_under_host_lock(self, thunk, *, label: str) -> dict:
        """Serialize an arbitrary host-worktree operation through the
        same lock the merger holds during merge work. Before invoking
        `thunk`, abort any leftover half-finished `merge`/`rebase`/etc.
        After a failed thunk, run cleanup again so the next op starts
        clean. Used by Undo (Changes screen) and could be used by any
        future feature that mutates `HEAD` on the host."""
        with self._host_lock:
            self._cleanup_worktree(reason=f"pre-{label} cleanup")
            try:
                result = thunk()
            except Exception as e:
                activity.append(
                    self._get_conn(),
                    message=f"{label} raised: {e!r}",
                    severity="error", category="git", actor="runner",
                )
                result = {"ok": False, "stage": "internal",
                          "message": f"{label} raised: {e!r}"}
            if not result.get("ok"):
                self._cleanup_worktree(reason=f"post-{label} failure cleanup")
            return result

    # ---- internals -----------------------------------------------------------

    def _loop(self) -> None:
        while not self._stop.is_set():
            self._wake.clear()
            try:
                self._tick()
            except Exception as e:
                try:
                    activity.append(
                        self._get_conn(),
                        message=f"Merger tick error: {e!r}",
                        severity="error", category="git", actor="runner",
                    )
                except Exception:
                    pass
            self._wake.wait(timeout=_POLL_INTERVAL_SECONDS)

    def _tick(self) -> None:
        # Honor the same `paused` toggle as the dispatcher: when paused,
        # don't pick up new merges (or auto-cleanup). User-triggered
        # Manual Verify is review approval only and does not run merge
        # work; pause only gates this Merge-agent loop.
        if db.get_setting_int(self._get_conn(), "paused", 0):
            return
        try:
            with mutation_guard.exclusive("Merge agent", kind="merge_agent"):
                with self._host_lock:
                    self._cleanup_worktree(reason="pre-tick cleanup")
                    if self._defer_for_pending_rebuild():
                        return
                    gap_id = self._find_one_ready()
                    if not gap_id:
                        return
                    self._merge_one(gap_id)
                    # If there were more queued, run the next one promptly.
                    self._wake.set()
        except mutation_guard.MutationBusy:
            return

    def _find_one_ready(self) -> str | None:
        """`ready-merge` Gaps are waiting on the merger. Process
        oldest-flipped first (FIFO) so Gaps don't starve."""
        row = self._get_conn().execute(
            "SELECT id FROM gaps_index "
            "WHERE status = 'ready-merge' AND instance_id = ? "
            "ORDER BY updated ASC LIMIT 1",
            (project_state.active_instance_id(),),
        ).fetchone()
        return row["id"] if row else None

    def _defer_for_pending_rebuild(self) -> bool:
        """When on-worktree-merge rebuilds are enabled, do not merge more
        work onto the host branch while already-merged work is waiting to be
        rebuilt. Agents may keep working in their own worktrees; only the
        host-worktree merge queue is gated.
        """
        conn = self._get_conn()
        mode = (
            db.get_setting(conn, "target_app_auto_rebuild", "never") or "never"
        ).strip()
        if mode != "on_worktree_merge":
            return False
        row = conn.execute(
            "SELECT COUNT(*) AS n FROM gaps_index "
            "WHERE status = 'awaiting-rebuild' AND instance_id = ?",
            (project_state.active_instance_id(),),
        ).fetchone()
        pending = int(row["n"] if row else 0)
        if pending <= 0:
            return False
        if self._queue_rebuild_for_pending is not None:
            self._queue_rebuild_for_pending()
        return True

    def _merge_one(self, gap_id: str) -> None:
        conn = self._get_conn()
        active_instance = project_state.active_instance_id()
        row = conn.execute(
            "SELECT instance_id FROM gaps_index WHERE id = ?",
            (gap_id,),
        ).fetchone()
        if (
            row
            and str(row["instance_id"] or project_state.DEFAULT_INSTANCE_ID)
            != active_instance
        ):
            return
        # Mark this Gap as the one we're working on so the snapshot
        # surfaces it on the Agents screen with a live elapsed timer.
        with self._snap_lock:
            self._current_gap_id = gap_id
            self._current_started = time.monotonic()
        try:
            try:
                # Auto-merge lands the branch, then parks the Gap in
                # `awaiting-rebuild`. A target-app rebuild promotes it to
                # `review`, so review always means merged + deployed/live.
                result = verify_op.perform_verify(
                    conn, gap_id, actor="runner",
                    final_status="awaiting-rebuild",
                )
            except Exception as e:
                activity.append(
                    conn,
                    message=f"Merge raised: {e!r}",
                    severity="error", category="git",
                    gap_id=gap_id, actor="runner",
                )
                result = {"ok": False, "stage": "internal",
                          "message": f"merge raised: {e!r}"}
        finally:
            with self._snap_lock:
                self._current_gap_id = None
                self._current_started = None
                self._last_outcome = (
                    result.get("final_status") if result.get("ok") else "failed"
                )

        if result.get("ok"):
            if self._on_worktree_merged is not None:
                self._on_worktree_merged(gap_id)
            return

        # Merge failed somewhere recoverable. Clean up any new stuck
        # state left behind by this attempt, then move the Gap to
        # `failed` so review remains reserved for rebuilt/live work.
        self._cleanup_worktree(reason="post-failure cleanup")
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
        if row and row["status"] == "ready-merge":
            with db.transaction(conn):
                conn.execute(
                    "UPDATE gaps_index SET status = 'failed', updated = ? "
                    "WHERE id = ?",
                    (now_iso(), gap_id),
                )
            try:
                gap_writer.update_fields(gap_id, status="failed")
                gap_writer.append_latest_round_log(
                    gap_id=gap_id,
                    severity="warn",
                    category="state",
                    actor="runner",
                    message=(
                        "Workflow status changed: ready-merge → failed; "
                        + (result.get("message")
                           or "merge failed")
                    ),
                    details=result.get("details"),
                )
            except Exception:
                pass
            activity.append(
                conn,
                message=(result.get("message")
                         or "Merge failed — moved Gap to failed"),
                severity="warn", category="state",
                gap_id=gap_id, actor="runner",
                details=result.get("details"),
            )

    def _cleanup_worktree(self, *, reason: str) -> None:
        """Abort any half-finished git op left on the host worktree.

        Operational assumption (per spec): the host running refine is
        dedicated to refine — no human edits the working copy
        directly. So aborting a stale merge/rebase here is safe; the
        only reason it would be sitting there is a prior refine merge
        that conflicted and never got cleaned up.
        """
        op = git_ops.in_progress_op()
        if not op:
            return
        op_name, _hint = op
        abort_args = _ABORT_ARGS.get(op_name)
        if abort_args is None:
            return
        r = git_ops._run(abort_args)
        conn = self._get_conn()
        if r.ok:
            activity.append(
                conn,
                message=f"Merger cleanup: aborted leftover `{op_name}` "
                        f"on host worktree ({reason})",
                severity="info", category="git", actor="runner",
            )
        else:
            activity.append(
                conn,
                message=f"Merger cleanup: failed to abort leftover "
                        f"`{op_name}` ({reason})",
                severity="warn", category="git", actor="runner",
                details=r.stderr[:2000],
            )


_ABORT_ARGS = {
    "merge":       ["merge", "--abort"],
    "rebase":      ["rebase", "--abort"],
    "cherry-pick": ["cherry-pick", "--abort"],
    "revert":      ["revert", "--abort"],
    "bisect":      ["bisect", "reset"],
    "unmerged-index": ["reset", "--hard", "HEAD"],
}
