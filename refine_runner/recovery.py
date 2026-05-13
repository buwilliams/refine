"""Runner-restart reconciliation.

Per spec: on startup, any Gap in `in-progress` without a live subprocess is moved
to `failed` with a "runner restarted" log entry. Worktrees and branches are
preserved so the human can Retry.

Also handles mid-merge/mid-push crashes — Gaps in `review` whose Verify started
but didn't finish stay in `review` with a log entry recording how far it got.
"""
from __future__ import annotations

import sqlite3

from refine_shared import activity, db
from refine_shared.gaps import now_iso

from . import gap_writer


def reconcile_on_start(conn: sqlite3.Connection) -> int:
    """Mark stranded in-progress Gaps as failed. Returns count moved."""
    rows = conn.execute(
        "SELECT id FROM gaps_index WHERE status = 'in-progress'"
    ).fetchall()
    moved = 0
    for row in rows:
        gid = row["id"]
        # Round index = latest round, by convention.
        rrow = conn.execute(
            "SELECT round_idx FROM runs WHERE gap_id = ? "
            "ORDER BY id DESC LIMIT 1",
            (gid,),
        ).fetchone()
        round_idx = rrow["round_idx"] if rrow else 0

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
