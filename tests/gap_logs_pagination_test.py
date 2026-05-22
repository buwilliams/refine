"""Gap detail log pagination checks."""
from __future__ import annotations

from pathlib import Path

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, _client = make_client_repo("refine-gap-logs-")
    conn = init_refine(_client)
    try:
        from refine_server import activity, gap_writer, gaps as shared_gaps
        from refine_server.paths import relative_gap_path
        from refine_server.ulid import new_ulid
        from refine_ui import api, server

        gap_id = new_ulid()
        round0 = shared_gaps.new_round("Jane Doe", "actual", "target")
        round0["created"] = "2000-01-01T00:00:00Z"
        round0["updated"] = "2000-01-01T00:00:00Z"
        round0["logs"] = [
            {
                "datetime": "2001-01-01T00:00:00Z",
                "severity": "info",
                "category": "cli",
                "message": "round-0",
                "details": "large details stay out of metadata",
            },
            {
                "datetime": "2002-01-01T00:00:00Z",
                "severity": "error",
                "category": "cli",
                "message": "round-1",
            },
            {
                "datetime": "2030-01-01T00:00:00Z",
                "severity": "info",
                "category": "cli",
                "message": "round-2",
                "details": "latest details stay out too",
            },
        ]
        round1 = shared_gaps.new_round("Jane Doe", "actual 2", "target 2")
        round1["created"] = "2099-01-01T00:00:00Z"
        round1["updated"] = "2099-01-01T00:00:00Z"
        gap = gap_writer.create_gap(
            gap_id=gap_id,
            name="Paged logs",
            initial_round=round0,
            status="review",
        )
        gap["rounds"] = [round0, round1]
        shared_gaps.write_gap_json(gap)
        conn.execute(
            "INSERT INTO gaps_index "
            "(id, name, status, priority, reporter, created, updated, branch_name, instance_id, json_path) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                gap_id,
                "Paged logs",
                "review",
                "low",
                "Jane Doe",
                gap["created"],
                gap["updated"],
                None,
                "default",
                relative_gap_path(gap_id),
            ),
        )
        activity.append(
            conn,
            message="activity-mid",
            severity="info",
            category="state",
            gap_id=gap_id,
            actor="runner",
        )
        conn.commit()

        status, detail = api.get_gap(gap_id)
        assert status == 200, detail
        returned_round = detail["gap"]["rounds"][0]
        assert "logs" not in returned_round
        assert returned_round["log_count"] == 3
        assert returned_round["latest_error_log"]["message"] == "round-1"
        assert "details" not in returned_round["latest_log"]
        assert "activity" not in detail["gap"]

        status, page = api.get_gap_logs(gap_id, round_idx=0, limit=2, offset=1)
        assert status == 200, page
        assert page["pagination"]["total"] == 4
        assert page["pagination"]["has_more"] is True
        assert [log["message"] for log in page["logs"]] == [
            "round-1",
            "activity-mid",
        ]
        assert page["logs"][1]["source"] == "activity"

        status, last_page = api.get_gap_logs(gap_id, round_idx=0, limit=2, offset=3)
        assert status == 200, last_page
        assert last_page["pagination"]["has_more"] is False
        assert [log["message"] for log in last_page["logs"]] == ["round-2"]

        matched = None
        path = f"/api/gaps/{gap_id}/logs"
        for method, pattern, handler in server.ROUTES:
            if method == "GET":
                match = pattern.match(path)
                if match:
                    matched = (handler, match)
                    break
        assert matched is not None, "gap logs route is registered"
        handler, match = matched
        status, routed_page = handler(
            None,
            match,
            None,
            {"round_idx": ["0"], "limit": ["2"], "offset": ["1"]},
        )
        assert status == 200, routed_page
        assert [log["message"] for log in routed_page["logs"]] == [
            "round-1",
            "activity-mid",
        ]

        root = Path(__file__).resolve().parents[1]
        gaps_detail_js = (
            root / "refine_ui/static/js/features/gaps-detail.js"
        ).read_text(encoding="utf-8")
        common_js = (
            root / "refine_ui/static/js/common.js"
        ).read_text(encoding="utf-8")
        assert "renderPaginationControls(" in gaps_detail_js
        assert "bindPaginationControls(body, pagerId" in gaps_detail_js
        assert "const GAP_LOG_PAGE_SIZE = 10;" in gaps_detail_js
        assert "function refreshGapRoundLogs(gapId)" in gaps_detail_js
        assert "loadRoundLogs(gapId, roundIdx, { page })" in gaps_detail_js
        assert "data-round-logs-more" not in gaps_detail_js
        assert "Load more" not in gaps_detail_js
        activity_block = common_js.split(
            'sseSource.addEventListener("activity_added"', 1,
        )[1].split('sseSource.addEventListener("status_change"', 1)[0]
        round_log_block = common_js.split(
            'sseSource.addEventListener("round_log_added"', 1,
        )[1].split("sseSource.onerror", 1)[0]
        assert "refreshGapRoundLogs(state.currentGap)" in activity_block
        assert "loadGapDetail(state.currentGap)" not in activity_block
        assert "refreshGapRoundLogs(state.currentGap)" in round_log_block
        assert "loadGapDetail(state.currentGap)" not in round_log_block
        assert "invalidateGapRoundLogs(state.currentGap)" not in round_log_block
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("gap log pagination tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
