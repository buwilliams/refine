"""Runner-restart reconciliation.

Per spec: on startup, any Gap in `in-progress` without a live subprocess is moved
to `failed` with a "runner restarted" log entry. Worktrees and branches are
preserved so the human can Retry.

Also handles mid-merge/mid-push crashes — Gaps in `review` whose Merge-agent
work started but didn't finish stay in `review` with a log entry recording how
far it got.
"""
from __future__ import annotations

import os
import sqlite3

from refine_server import activity, db, project_state, quality
from refine_server.gaps import now_iso

from . import gap_writer


def reconcile_on_start(conn: sqlite3.Connection) -> int:
    """Categorize stranded in-progress and qa Gaps at runner startup.

    Two possibilities for a Gap left in `in-progress` across a restart:

    - The agent finished successfully but the dispatcher crashed
      before flipping to `ready-merge`. Its latest run has
      `finished_at` set, `status='finished'`, no `failure_category`.
      Promote the Gap to `ready-merge` so the merger picks it up on
      its first tick after startup.

    - The agent was actively running and got killed by the restart
      (or it never even finished spawning). The latest run is either
      missing, still has `finished_at IS NULL`, or has a failure
      category. Mark the Gap `failed` per spec — the operator can
      Retry once the underlying issue is resolved.

    `ready-merge` Gaps that survived the restart are already in the
    correct state — leave them; the merger picks them up.

    Returns count moved to `failed`.
    """
    return _reconcile_active_agent_states(
        conn,
        live_gap_ids=set(),
        startup=True,
    )


def reconcile_runtime_in_progress(
    conn: sqlite3.Connection,
    *,
    live_gap_ids: set[str],
) -> int:
    """Clean up in-progress rows that no live tracked agent owns anymore.

    This runs during dispatcher ticks. It is intentionally more conservative
    than startup reconciliation: an unfinished run with a still-live PID is
    left alone because it may belong to another still-running process.
    """
    return _reconcile_active_agent_states(
        conn,
        live_gap_ids=live_gap_ids,
        startup=False,
    )


def _reconcile_active_agent_states(
    conn: sqlite3.Connection,
    *,
    live_gap_ids: set[str],
    startup: bool,
) -> int:
    rows = conn.execute(
        "SELECT id, status FROM gaps_index "
        "WHERE status IN ('in-progress', 'qa') AND node_id = ?",
        (project_state.active_node_id(),),
    ).fetchall()
    moved = 0
    for row in rows:
        gid = row["id"]
        if gid in live_gap_ids:
            continue
        rrow = conn.execute(
            "SELECT round_idx, finished_at, status, failure_category, pid, kind "
            "FROM runs WHERE gap_id = ? ORDER BY id DESC LIMIT 1",
            (gid,),
        ).fetchone()
        round_idx = rrow["round_idx"] if rrow else 0
        current_status = row["status"]
        run_kind = str(rrow["kind"] or "implementation") if rrow else ""

        quality_enabled = quality.enabled(conn)
        if current_status == "qa" and not rrow:
            continue
        if current_status == "qa" and run_kind != "quality" and quality_enabled:
            continue

        # Awaiting-merge case — bump to `ready-merge` so the merger
        # picks the Gap up on its first tick after startup.
        if (rrow and rrow["finished_at"]
                and rrow["status"] == "finished"
                and not rrow["failure_category"]):
            next_status = (
                "ready-merge"
                if current_status == "qa" or not quality_enabled
                else "qa"
            )
            with db.transaction(conn):
                conn.execute(
                    "UPDATE gaps_index SET status = ?, "
                    "updated = ? WHERE id = ?",
                    (next_status, now_iso(), gid),
                )
            try:
                gap_writer.update_fields(gid, status=next_status)
                gap_writer.append_latest_round_log(
                    gap_id=gid,
                    severity="info",
                    category="state",
                    actor="runner",
                    message=(
                        f"Workflow status changed: {current_status} → {next_status}; "
                        "runner restarted after agent completion"
                    ),
                )
            except Exception:
                pass
            activity.append(
                conn,
                message=(
                    "Runner restarted after agent completion — "
                    f"promoted to {next_status}"
                ),
                severity="info", category="state",
                gap_id=gid, actor="runner",
            )
            continue

        if (
            not startup
            and rrow
            and not rrow["finished_at"]
            and _pid_may_be_alive(rrow["pid"])
        ):
            continue

        # Orphan agent case — kill the run record + flip to failed.
        failure_category = "runner_restart" if startup else "agent_orphaned"
        detail_message = (
            f"Runner restarted while this Gap was {current_status} — marked failed"
            if startup
            else f"No live agent subprocess is tracking this {current_status} Gap — marked failed"
        )
        activity_message = (
            "Runner restarted; marked Gap as failed"
            if startup
            else f"{current_status} Gap had no live agent subprocess; marked failed"
        )
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'failed', updated = ? WHERE id = ?",
                (now_iso(), gid),
            )
            conn.execute(
                "UPDATE runs SET finished_at = ?, status = 'killed', "
                "  failure_category = ? "
                "WHERE gap_id = ? AND finished_at IS NULL",
                (now_iso(), failure_category, gid),
            )
        try:
            gap_writer.update_fields(gid, status="failed")
        except Exception:
            pass

        try:
            gap_writer.append_round_log(
                gap_id=gid,
                round_idx=round_idx,
                severity="error", category="state",
                actor="runner",
                message=detail_message,
            )
        except Exception:
            pass

        activity.append(
            conn,
            message=activity_message,
            severity="warn", category="state",
            gap_id=gid, actor="runner",
        )
        moved += 1
    return moved


def _pid_may_be_alive(pid: object) -> bool:
    try:
        pid_int = int(pid)
    except (TypeError, ValueError):
        return False
    if pid_int <= 0:
        return False
    try:
        os.kill(pid_int, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True
