"""Focused tests for Gaps bulk status updates."""
from __future__ import annotations

from pathlib import Path

from tests.helpers import cleanup_tmp, git, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-gaps-bulk-status-")
    conn = init_refine(client)
    try:
        from refine_server import db, gap_writer, gaps, mutation_guard
        from refine_server.paths import relative_gap_path
        from refine_ui import api

        statuses = [
            "backlog",
            "todo",
            "awaiting-rebuild",
            "review",
            "done",
            "failed",
            "cancelled",
            "in-progress",
            "ready-merge",
        ]
        gap_ids = {}
        for status in statuses:
            gid = "01BULKSTATUS" + status.upper().replace("-", "")[:12].ljust(12, "A")
            gap_ids[status] = gid
            gap = gap_writer.create_gap(
                gap_id=gid,
                name=gid,
                initial_round=gaps.new_round("Reporter", "Actual", "Target"),
                status=status,
                priority="medium",
            )
            conn.execute(
                "INSERT INTO gaps_index "
                "(id, name, status, priority, reporter, created, updated, json_path) "
                "VALUES (?, ?, ?, 'medium', 'Reporter', ?, ?, ?)",
                (
                    gid,
                    gid,
                    status,
                    gap["created"],
                    gap["updated"],
                    relative_gap_path(gid),
                ),
            )

        code, body = api.bulk_update_gaps({
            "filter": {"reporter": "Reporter"},
            "update": {"status": "review"},
        })
        assert code == 200, body
        assert body["updated"] == 7, body
        assert set(body["ids"]) == {
            gap_ids["backlog"],
            gap_ids["todo"],
            gap_ids["awaiting-rebuild"],
            gap_ids["review"],
            gap_ids["done"],
            gap_ids["failed"],
            gap_ids["cancelled"],
        }, body
        assert body["skipped"] == 2, body
        assert body["skipped_details"] == [
            {"id": gap_ids["in-progress"], "reason": "status:in-progress"},
            {"id": gap_ids["ready-merge"], "reason": "status:ready-merge"},
        ], body

        rows = {
            row["id"]: row["status"]
            for row in conn.execute(
                "SELECT id, status FROM gaps_index",
            )
        }
        for status in _bulk_status_options():
            assert rows[gap_ids[status]] == "review", (status, rows)
        assert rows[gap_ids["in-progress"]] != "review", rows
        assert rows[gap_ids["ready-merge"]] != "review", rows

        original_bulk_threshold = api.BULK_UPDATE_BACKGROUND_THRESHOLD
        api.BULK_UPDATE_BACKGROUND_THRESHOLD = 2
        try:
            code, body = api.bulk_update_gaps({
                "filter": {"reporter": "Reporter", "status": "review"},
                "update": {"status": "done"},
            })
            assert code == 202, body
            result = wait_job(body["job"]["id"])
            assert result["http_status"] == 200, result
            assert result["updated"] == 7, result
            assert result["skipped"] == 0, result
        finally:
            api.BULK_UPDATE_BACKGROUND_THRESHOLD = original_bulk_threshold

        rows = {
            row["id"]: row["status"]
            for row in conn.execute(
                "SELECT id, status FROM gaps_index",
            )
        }
        for status in _bulk_status_options():
            assert rows[gap_ids[status]] == "done", (status, rows)

        selected_a = "01BULKSELECTEDAAAAAAAAAA"
        selected_b = "01BULKSELECTEDBBBBBBBBBB"
        unselected = "01BULKSELECTEDCCCCCCCCCC"
        for gid in (selected_a, selected_b, unselected):
            gap = gap_writer.create_gap(
                gap_id=gid,
                name=gid,
                initial_round=gaps.new_round("Selected Reporter", "Actual", "Target"),
                status="backlog",
                priority="low",
            )
            conn.execute(
                "INSERT INTO gaps_index "
                "(id, name, status, priority, reporter, created, updated, json_path) "
                "VALUES (?, ?, 'backlog', 'low', 'Selected Reporter', ?, ?, ?)",
                (
                    gid,
                    gid,
                    gap["created"],
                    gap["updated"],
                    relative_gap_path(gid),
                ),
            )
        code, body = api.bulk_update_gaps({
            "filter": {"reporter": "Selected Reporter"},
            "selected_ids": [selected_a, selected_b],
            "update": {"priority": "high"},
            "background": False,
        })
        assert code == 200, body
        assert body["updated"] == 2, body
        assert body["ids"] == [selected_a, selected_b], body
        selected_rows = {
            row["id"]: row["priority"]
            for row in conn.execute(
                "SELECT id, priority FROM gaps_index WHERE id IN (?, ?, ?)",
                (selected_a, selected_b, unselected),
            )
        }
        assert selected_rows == {
            selected_a: "high",
            selected_b: "high",
            unselected: "low",
        }, selected_rows

        from refine_server.backend_protocol import (
            M_BULK_DELETE_GAPS,
            M_BULK_UPDATE_GAPS,
            M_DELETE_GAP,
            M_EDIT_ROUND,
        )
        from refine_ui import runtime

        class TrackingClient:
            def __init__(self) -> None:
                self.calls = []

            def call(self, method, params=None, *, timeout=30.0):  # noqa: ANN001, ANN202
                self.calls.append((method, params or {}, timeout))
                return runtime.runner_call(method, params or {})

        tracking = TrackingClient()
        original_get_client = api.get_client
        api.get_client = lambda: tracking
        try:
            for idx in range(2):
                gid = f"01BULKPROTO{idx}AAAAAAAAAAA"
                gap = gap_writer.create_gap(
                    gap_id=gid,
                    name=gid,
                    initial_round=gaps.new_round("Batch Reporter", "Actual", "Target"),
                    status="backlog",
                    priority="low",
                )
                conn.execute(
                    "INSERT INTO gaps_index "
                    "(id, name, status, priority, reporter, created, updated, json_path) "
                    "VALUES (?, ?, 'backlog', 'low', 'Batch Reporter', ?, ?, ?)",
                    (
                        gid,
                        gid,
                        gap["created"],
                        gap["updated"],
                        relative_gap_path(gid),
                    ),
                )

            code, body = api.bulk_update_gaps({
                "filter": {"reporter": "Batch Reporter"},
                "update": {"priority": "high"},
                "background": False,
            })
            assert code == 200, body
            assert body["updated"] == 2, body
            assert [c[0] for c in tracking.calls] == [M_BULK_UPDATE_GAPS], tracking.calls
            assert tracking.calls[0][1]["gap_ids"] == body["ids"], tracking.calls
            assert M_EDIT_ROUND not in [c[0] for c in tracking.calls], tracking.calls

            tracking.calls.clear()
            code, body = api.bulk_update_gaps({
                "filter": {"reporter": "Batch Reporter"},
                "update": {"reporter": "Batch Renamed"},
                "background": False,
            })
            assert code == 200, body
            assert body["updated"] == 2, body
            assert [c[0] for c in tracking.calls] == [M_BULK_UPDATE_GAPS], tracking.calls
            for gid in body["ids"]:
                gap = gaps.read_gap_json(gid)
                assert gap["rounds"][-1]["reporter"] == "Batch Renamed"

            tracking.calls.clear()
            for idx in range(2):
                gid = f"01BULKDELETE{idx}AAAAAAAAAA"
                gap = gap_writer.create_gap(
                    gap_id=gid,
                    name=gid,
                    initial_round=gaps.new_round("Delete Reporter", "Actual", "Target"),
                    status="backlog",
                    priority="low",
                )
                conn.execute(
                    "INSERT INTO gaps_index "
                    "(id, name, status, priority, reporter, created, updated, json_path) "
                    "VALUES (?, ?, 'backlog', 'low', 'Delete Reporter', ?, ?, ?)",
                    (
                        gid,
                        gid,
                        gap["created"],
                        gap["updated"],
                        relative_gap_path(gid),
                    ),
                )
            code, body = api.bulk_delete_gaps({
                "filter": {"reporter": "Delete Reporter"},
            })
            assert code == 200, body
            assert body["deleted"] == 2, body
            assert [c[0] for c in tracking.calls] == [M_BULK_DELETE_GAPS], tracking.calls
            assert M_DELETE_GAP not in [c[0] for c in tracking.calls], tracking.calls

            tracking.calls.clear()
            delete_selected = "01BULKDELETESELECTEDAAA"
            delete_unselected = "01BULKDELETEUNSELECTEDAA"
            for gid in (delete_selected, delete_unselected):
                gap = gap_writer.create_gap(
                    gap_id=gid,
                    name=gid,
                    initial_round=gaps.new_round("Delete Selected", "Actual", "Target"),
                    status="backlog",
                    priority="low",
                )
                conn.execute(
                    "INSERT INTO gaps_index "
                    "(id, name, status, priority, reporter, created, updated, json_path) "
                    "VALUES (?, ?, 'backlog', 'low', 'Delete Selected', ?, ?, ?)",
                    (
                        gid,
                        gid,
                        gap["created"],
                        gap["updated"],
                        relative_gap_path(gid),
                    ),
                )
            code, body = api.bulk_delete_gaps({
                "filter": {"reporter": "Delete Selected"},
                "selected_ids": [delete_selected],
            })
            assert code == 200, body
            assert body["deleted"] == 1, body
            assert body["ids"] == [delete_selected], body
            remaining = conn.execute(
                "SELECT id FROM gaps_index WHERE id = ?",
                (delete_unselected,),
            ).fetchone()
            assert remaining is not None, body

            db.set_setting(conn, "paused", "1")
            last_backlog = "01BULKLASTBACKLOGAAAAAA"
            last_merge = "01BULKLASTMERGEAAAAAAAA"
            last_agent = "01BULKLASTAGENTAAAAAAAA"
            last_review = "01BULKLASTREVIEWAAAAAAA"
            last_active = "01BULKLASTACTIVEAAAAAAA"
            branch = "refine/bulk-last-workflow"
            git(client, "branch", branch)
            for gid, status, branch_name in (
                (last_backlog, "backlog", None),
                (last_merge, "failed", branch),
                (last_agent, "failed", None),
                (last_review, "review", None),
                (last_active, "in-progress", None),
            ):
                gap = gap_writer.create_gap(
                    gap_id=gid,
                    name=gid,
                    initial_round=gaps.new_round(
                        "Workflow Reporter",
                        "Actual",
                        "Target",
                    ),
                    status=status,
                    priority="low",
                )
                if branch_name:
                    gap_writer.update_fields(gid, branch_name=branch_name)
                conn.execute(
                    "INSERT INTO gaps_index "
                    "(id, name, status, priority, reporter, created, updated, "
                    "branch_name, json_path) "
                    "VALUES (?, ?, ?, 'low', 'Workflow Reporter', ?, ?, ?, ?)",
                    (
                        gid,
                        gid,
                        status,
                        gap["created"],
                        gap["updated"],
                        branch_name,
                        relative_gap_path(gid),
                    ),
                )
            gap_writer.append_latest_round_log(
                gap_id=last_merge,
                severity="warn",
                category="state",
                actor="runner",
                message=(
                    "Workflow status changed: ready-merge → failed; "
                    "merge failed"
                ),
            )
            gap_writer.append_latest_round_log(
                gap_id=last_agent,
                severity="warn",
                category="state",
                actor="runner",
                message=(
                    "Workflow status changed: in-progress → failed; "
                    "agent failed"
                ),
            )
            tracking.calls.clear()
            with mutation_guard.exclusive("Merge agent", kind="merge_agent"):
                code, body = api.bulk_update_gaps({
                    "filter": {"reporter": "Workflow Reporter"},
                    "update": {"status": "__last_workflow_state"},
                    "background": False,
                })
            assert code == 200, body
            assert body["updated"] == 3, body
            assert body["todo"] == 2, body
            assert body["ready_merge"] == 1, body
            assert {c[0] for c in tracking.calls} == {M_BULK_UPDATE_GAPS}
            rows = {
                row["id"]: row["status"]
                for row in conn.execute(
                    "SELECT id, status FROM gaps_index "
                    "WHERE reporter = 'Workflow Reporter'",
                )
            }
            assert rows[last_backlog] == "backlog", rows
            assert rows[last_merge] == "ready-merge", rows
            assert rows[last_agent] == "todo", rows
            assert rows[last_review] == "todo", rows
            assert rows[last_active] == "in-progress", rows
        finally:
            api.get_client = original_get_client

        for target in ("in-progress", "qa", "ready-merge"):
            code, body = api.bulk_update_gaps({
                "filter": {"reporter": "Reporter"},
                "update": {"status": target},
            })
            assert code == 409, (target, body)
            assert "cannot set in-progress, qa, or ready-merge" in body["error"]["message"]

        root = Path(__file__).resolve().parents[1]
        gaps_bulk = (
            root / "refine_ui/static/js/features/gaps-bulk.js"
        ).read_text(encoding="utf-8")
        assert '"__last_workflow_state"' in gaps_bulk
        assert "(Last workflow state)" in gaps_bulk
        assert 'value: "awaiting-rebuild", label: "awaiting-rebuild"' in gaps_bulk
        assert 'value: "cancelled", label: "cancelled"' in gaps_bulk
        assert "failed merge" in gaps_bulk
        assert "attempts back to ready-merge" in gaps_bulk
        assert "failed QA attempts back to qa" in gaps_bulk
        assert "resolveBackgroundJobResponse" in gaps_bulk
        assert "filter, ...selectionFields" in gaps_bulk
        assert "exclude_ids: Array.from(gapsExcludedIds)" in gaps_bulk
        assert "selected_ids: Array.from(gapsIncludedIds)" in gaps_bulk
        assert "all matching Gaps selected" in gaps_bulk
        assert 'toast("No Gaps selected.", "warn");' in gaps_bulk
        gaps_list = (
            root / "refine_ui/static/js/features/gaps-list.js"
        ).read_text(encoding="utf-8")
        assert "Bulk update selected:" in gaps_list
        assert 'id="gap-select-page"' in gaps_list
        assert "selectCurrentGapsPage" in gaps_list
        assert "Select all matching Gaps" in gaps_list
        assert "let gapsSelectAllMatching = true" in gaps_list
        assert "const gapsIncludedIds = new Set()" in gaps_list
        assert "for (const gap of gaps) gapsIncludedIds.add(gap.id)" in gaps_list
        assert "Gaps on this page" in gaps_bulk
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("gaps bulk status tests OK")
    return 0


def _bulk_status_options() -> tuple[str, ...]:
    return (
        "backlog",
        "todo",
        "awaiting-rebuild",
        "review",
        "done",
        "failed",
        "cancelled",
    )


def wait_job(job_id: str) -> dict:
    import time
    from refine_ui import background_jobs

    deadline = time.time() + 10
    while time.time() < deadline:
        job = background_jobs.snapshot(job_id)
        if job and job["status"] == "complete":
            return job["result"]
        if job and job["status"] == "failed":
            raise AssertionError(job["error"])
        time.sleep(0.05)
    raise AssertionError(f"job did not finish: {job_id}")


if __name__ == "__main__":
    raise SystemExit(main())
