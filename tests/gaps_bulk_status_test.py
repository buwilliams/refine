"""Focused tests for Gaps bulk status updates."""
from __future__ import annotations

from pathlib import Path

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-gaps-bulk-status-")
    conn = init_refine(client)
    try:
        from refine_server import gap_writer, gaps
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

        for target in ("in-progress", "ready-merge"):
            code, body = api.bulk_update_gaps({
                "filter": {"reporter": "Reporter"},
                "update": {"status": target},
            })
            assert code == 409, (target, body)
            assert "cannot set in-progress or ready-merge" in body["error"]["message"]

        root = Path(__file__).resolve().parents[1]
        gaps_bulk = (
            root / "refine_ui/static/js/features/gaps-bulk.js"
        ).read_text(encoding="utf-8")
        expected_options = (
            '"backlog", "todo", "awaiting-rebuild", "review",\n'
            '  "done", "failed", "cancelled"'
        )
        assert expected_options in gaps_bulk
        assert "skip in-progress and ready-merge" in gaps_bulk
        assert "resolveBackgroundJobResponse" in gaps_bulk
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
