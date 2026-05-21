"""Gaps-list bulk instance transfer tests."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-gaps-bulk-instances-")
    conn = init_refine(client)
    try:
        from refine_server import db, gap_writer, gaps, project_state
        from refine_ui import api

        active = project_state.active_instance_id()
        other = project_state.create_instance("Other")
        target = project_state.create_instance("Target")

        def create(gap_id: str, reporter: str, status: str,
                   instance_id: str = active) -> None:
            gap_writer.create_gap(
                gap_id=gap_id,
                name=gap_id,
                initial_round=gaps.new_round(reporter, "Actual", "Target"),
                status=status,
                priority="medium",
                instance_id=instance_id,
            )

        transfer_me = "01GAPSBULKTRANSFERAAAAAA"
        excluded = "01GAPSBULKEXCLUDEDAAAAAA"
        wrong_reporter = "01GAPSBULKREPORTERAAAAA"
        blocked_status = "01GAPSBULKBLOCKEDAAAAAA"
        wrong_instance = "01GAPSBULKINSTANCEAAAAA"
        create(transfer_me, "Bulk Jane", "todo")
        create(excluded, "Bulk Jane", "failed")
        create(wrong_reporter, "Other Reporter", "todo")
        create(blocked_status, "Bulk Jane", "in-progress")
        create(wrong_instance, "Bulk Jane", "todo", other["id"])
        project_state.rebuild_sqlite_cache(conn)

        status, body = api.transfer_instance_gaps({
            "target_instance_id": target["id"],
            "filter": {
                "reporter": "Bulk Jane",
                "instance": active,
            },
            "exclude_ids": [excluded],
        })
        assert status == 200, body
        assert body["updated"] == 1, body
        assert body["ids"] == [transfer_me], body
        assert body["skipped"] == 1, body
        assert body["skipped_details"] == [{
            "id": blocked_status,
            "reason": "status:in-progress",
        }], body

        rows = {
            row["id"]: row["instance_id"]
            for row in conn.execute(
                "SELECT id, instance_id FROM gaps_index WHERE id IN (?, ?, ?, ?, ?)",
                (transfer_me, excluded, wrong_reporter, blocked_status, wrong_instance),
            )
        }
        assert rows[transfer_me] == target["id"], rows
        assert rows[excluded] == active, rows
        assert rows[wrong_reporter] == active, rows
        assert rows[blocked_status] == active, rows
        assert rows[wrong_instance] == other["id"], rows

        selected_transfer = "01GAPSBULKSELECTEDAAAAA"
        visible_but_unchecked = "01GAPSBULKUNCHECKEDAAAA"
        create(selected_transfer, "Selected Jane", "todo")
        create(visible_but_unchecked, "Selected Jane", "todo")
        project_state.rebuild_sqlite_cache(conn)
        status, body = api.transfer_instance_gaps({
            "target_instance_id": target["id"],
            "filter": {"reporter": "Selected Jane", "instance": active},
            "selected_ids": [selected_transfer],
        })
        assert status == 200, body
        assert body["updated"] == 1, body
        assert body["ids"] == [selected_transfer], body
        rows = {
            row["id"]: row["instance_id"]
            for row in conn.execute(
                "SELECT id, instance_id FROM gaps_index WHERE id IN (?, ?)",
                (selected_transfer, visible_but_unchecked),
            )
        }
        assert rows[selected_transfer] == target["id"], rows
        assert rows[visible_but_unchecked] == active, rows

        root = Path(__file__).resolve().parents[1]
        gaps_list = (root / "refine_ui/static/js/features/gaps-list.js").read_text(
            encoding="utf-8",
        )
        gaps_bulk = (root / "refine_ui/static/js/features/gaps-bulk.js").read_text(
            encoding="utf-8",
        )
        assert 'id="bulk-transfer-instance"' in gaps_list
        assert "openBulkTransferInstanceModal" in gaps_bulk
        assert 'api("POST", "/api/instances/transfer-gaps"' in gaps_bulk
        assert "filter, ...selectionFields" in gaps_bulk
        assert "exclude_ids: Array.from(gapsExcludedIds)" in gaps_bulk
        assert "selected_ids: Array.from(gapsIncludedIds)" in gaps_bulk
        assert "instance: f.instance" in gaps_bulk
        assert '"filter-instance": !!f.instance' in gaps_bulk
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("gaps bulk instance transfer tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
