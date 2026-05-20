"""Automatic target-app rebuild queue and review gating tests."""
from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-target-app-rebuild-")
    conn = init_refine(client)
    try:
        from refine_server import db, gaps, target_app_rebuilder
        from refine_ui import api

        runs: list[str] = []

        def run_rebuild(reason: str) -> dict:
            runs.append(reason)
            if len(runs) == 1:
                rebuilder.queue_rebuild("queued while running")
                rebuilder.queue_rebuild("duplicate while running")
            return {"ok": True}

        rebuilder = target_app_rebuilder.TargetAppRebuilder(
            get_conn=lambda: conn,
            run_rebuild=run_rebuild,
        )
        rebuilder.queue_rebuild("initial")
        rebuilder.queue_rebuild("duplicate before running")
        rebuilder._drain_queue()  # noqa: SLF001
        assert len(runs) == 2, runs

        assert rebuilder.queue_for_worktree_merge("01GAP") is False
        db.set_setting(conn, "target_app_auto_rebuild", "on_worktree_merge")
        assert rebuilder.queue_for_worktree_merge("01GAP") is True
        rebuilder._drain_queue()  # noqa: SLF001
        assert len(runs) == 3, runs
        db.set_setting(conn, "target_app_auto_rebuild", "hourly")
        db.set_setting(conn, "target_app_auto_rebuild_last_started_at", "")
        rebuilder._queue_scheduled_rebuild_if_due()  # noqa: SLF001
        rebuilder._drain_queue()  # noqa: SLF001
        assert len(runs) == 4, runs
        target_settings = json.loads(
            (client / ".refine" / "instances" / "default" / "target-app.json")
            .read_text(encoding="utf-8")
        )
        assert target_settings["target_app_auto_rebuild"] == "hourly"

        gid = "01TARGETAPPREBUILDGATEAAA"
        create_indexed_gap(conn, gid, status="awaiting-rebuild", branch=None)
        assert api._promote_rebuilt_gaps(conn) == 1  # noqa: SLF001
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gid,),
        ).fetchone()
        assert row["status"] == "review"
        gap = gaps.read_gap_json(gid)
        assert gap["status"] == "review"
        assert gap.get("branch_name") is None
        messages = [log["message"] for log in gap["rounds"][-1]["logs"]]
        assert "Target application rebuilt; Gap is ready for review" in messages, messages
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("target-app rebuild tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
