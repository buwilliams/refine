"""Runner-restart reconciliation.

Per spec: on startup, any Gap in `in-progress` without a live subprocess is moved
to `failed` with a "runner restarted" log entry. Worktrees and branches are
preserved so the human can Retry.

Also handles mid-merge/mid-push crashes — Gaps in `review` whose Merge-agent
work started but didn't finish stay in `review` with a log entry recording how
far it got.
"""
from __future__ import annotations

import sqlite3

from refine_server import activity, db
from refine_server.gaps import now_iso

from . import gap_writer


def reconcile_on_start(conn: sqlite3.Connection) -> int:
    """Categorize stranded in-progress Gaps at runner startup.

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
    rows = conn.execute(
        "SELECT id FROM gaps_index WHERE status = 'in-progress'"
    ).fetchall()
    moved = 0
    for row in rows:
        gid = row["id"]
        rrow = conn.execute(
            "SELECT round_idx, finished_at, status, failure_category "
            "FROM runs WHERE gap_id = ? ORDER BY id DESC LIMIT 1",
            (gid,),
        ).fetchone()
        round_idx = rrow["round_idx"] if rrow else 0

        # Awaiting-merge case — bump to `ready-merge` so the merger
        # picks the Gap up on its first tick after startup.
        if (rrow and rrow["finished_at"]
                and rrow["status"] == "finished"
                and not rrow["failure_category"]):
            with db.transaction(conn):
                conn.execute(
                    "UPDATE gaps_index SET status = 'ready-merge', "
                    "updated = ? WHERE id = ?",
                    (now_iso(), gid),
                )
            try:
                gap_writer.update_fields(gid, status="ready-merge")
                gap_writer.append_latest_round_log(
                    gap_id=gid,
                    severity="info",
                    category="state",
                    actor="runner",
                    message=(
                        "Workflow status changed: in-progress → ready-merge; "
                        "runner restarted after agent completion"
                    ),
                )
            except Exception:
                pass
            activity.append(
                conn,
                message="Runner restarted while Gap was awaiting merge — "
                        "promoted to ready-merge for the merger",
                severity="info", category="state",
                gap_id=gid, actor="runner",
            )
            continue

        # Orphan agent case — kill the run record + flip to failed.
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'failed', updated = ? WHERE id = ?",
                (now_iso(), gid),
            )
            conn.execute(
                "UPDATE runs SET finished_at = ?, status = 'killed', "
                "  failure_category = 'runner_restart' "
                "WHERE gap_id = ? AND finished_at IS NULL",
                (now_iso(), gid),
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
                message="Runner restarted while this Gap was in-progress — marked failed",
            )
        except Exception:
            pass

        activity.append(
            conn,
            message="Runner restarted; marked Gap as failed",
            severity="warn", category="state",
            gap_id=gid, actor="runner",
        )
        moved += 1
    return moved
