"""Workflow transition guards for user and bulk status updates."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, init_refine, make_client_repo


def create_gap(conn, gap_id: str, *, status: str,
               branch: str | None = None) -> None:
    from refine_server import gap_writer

    create_indexed_gap(conn, gap_id, status=status, branch=branch)
    gap_writer.update_fields(gap_id, status=status, branch_name=branch)


def db_status(conn, gap_id: str) -> str:
    row = conn.execute(
        "SELECT status FROM gaps_index WHERE id = ?", (gap_id,),
    ).fetchone()
    return row["status"]


def main() -> int:
    tmp, client = make_client_repo("refine-workflow-guards-")
    conn = init_refine(client)
    try:
        from refine_server import db
        from refine_ui import api

        db.set_setting(conn, "paused", "1")

        gid_backlog = "01WORKFLOWBACKLOGAAAAAAA"
        create_gap(conn, gid_backlog, status="backlog")
        status, body = api.update_gap_name(gid_backlog, {"status": "todo"})
        assert status == 200, body
        assert db_status(conn, gid_backlog) == "todo"

        status, body = api.update_gap_name(gid_backlog, {"status": "ready-merge"})
        assert status == 409, body
        assert db_status(conn, gid_backlog) == "todo"

        gid_review = "01WORKFLOWREVIEWAAAAAAAA"
        create_gap(conn, gid_review, status="review")
        status, body = api.update_gap_name(gid_review, {"status": "done"})
        assert status == 409, body
        assert db_status(conn, gid_review) == "review"
        status, body = api.update_gap_name(gid_review, {"status": "todo"})
        assert status == 200, body
        assert db_status(conn, gid_review) == "todo"

        gid_done = "01WORKFLOWDONEAAAAAAAAAA"
        create_gap(conn, gid_done, status="done")
        status, body = api.update_gap_name(gid_done, {"status": "review"})
        assert status == 200, body
        assert db_status(conn, gid_done) == "review"

        gid_system = "01WORKFLOWSYSTEMAAAAAAA"
        create_gap(conn, gid_system, status="awaiting-rebuild")
        status, body = api.update_gap_name(gid_system, {"status": "review"})
        assert status == 409, body
        assert db_status(conn, gid_system) == "awaiting-rebuild"

        gid_bulk_backlog = "01WORKFLOWBULKBACKLOGAA"
        gid_bulk_todo = "01WORKFLOWBULKTODOAAAAA"
        gid_bulk_ready = "01WORKFLOWBULKREADYAAAA"
        create_gap(conn, gid_bulk_backlog, status="backlog")
        create_gap(conn, gid_bulk_todo, status="todo")
        create_gap(conn, gid_bulk_ready, status="ready-merge")

        status, body = api.bulk_update_gaps({
            "filter": {"status": "backlog"},
            "update": {"status": "failed"},
        })
        assert status == 200, body
        assert gid_bulk_backlog in body["ids"], body
        assert db_status(conn, gid_bulk_backlog) == "failed"

        status, body = api.bulk_update_gaps({
            "filter": {"status": "todo"},
            "update": {"status": "review"},
        })
        assert status == 200, body
        assert gid_bulk_todo in body["ids"], body
        assert db_status(conn, gid_bulk_todo) == "review"

        status, body = api.bulk_update_gaps({
            "filter": {"status": "ready-merge"},
            "update": {"status": "todo"},
        })
        assert status == 200, body
        assert body["updated"] == 0, body
        assert body["skipped_details"] == [{
            "id": gid_bulk_ready,
            "reason": "status:ready-merge",
        }], body
        assert db_status(conn, gid_bulk_ready) == "ready-merge"

        root = Path(__file__).resolve().parents[1]
        gaps_detail_js = (
            root / "refine_ui/static/js/features/gaps-detail.js"
        ).read_text(encoding="utf-8")
        assert "← Merge" in gaps_detail_js
        assert "/retry-merge" in gaps_detail_js
        assert "isMergeRetryGap" in gaps_detail_js
        assert "latest_workflow_log" in gaps_detail_js
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("workflow transition guard tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
