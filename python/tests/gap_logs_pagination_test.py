"""Gap log pagination and Logs-screen handoff checks."""
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
            "(id, name, status, priority, reporter, created, updated, branch_name, node_id, json_path) "
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
        assert detail["gap"]["node_id"], detail
        assert detail["gap"]["node_display_name"], detail
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

        status, logs_page = api.list_activity(
            gap_id=gap_id,
            limit=10,
            offset=0,
            sort="datetime",
            direction="asc",
            include_facets=True,
        )
        assert status == 200, logs_page
        assert logs_page["page"]["total"] == 4
        assert [log["message"] for log in logs_page["activity"]] == [
            "round-0",
            "round-1",
            "activity-mid",
            "round-2",
        ]
        assert logs_page["activity"][0]["source"] == "round"
        assert logs_page["activity"][2]["source"] == "activity"
        assert logs_page["activity"][0]["gap_id"] == gap_id
        assert logs_page["facets"]["categories"] == ["cli", "state"]
        assert logs_page["facets"]["actors"] == ["runner"]

        status, detail_match = api.list_activity(gap_id=gap_id, q="latest details", limit=10)
        assert status == 200, detail_match
        assert [log["message"] for log in detail_match["activity"]] == ["round-2"]

        status, severity_match = api.list_activity(gap_id=gap_id, severity="error", limit=10)
        assert status == 200, severity_match
        assert [log["message"] for log in severity_match["activity"]] == ["round-1"]

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
        logs_js = (
            root / "refine_ui/static/js/features/logs.js"
        ).read_text(encoding="utf-8")
        gaps_css = (
            root / "refine_ui/static/css/gaps.css"
        ).read_text(encoding="utf-8")
        common_css = (
            root / "refine_ui/static/css/common.css"
        ).read_text(encoding="utf-8")
        assert '<div class="gap-action-group">' in gaps_detail_js
        assert '<button class="gap-action-primary" id="btn-chat">Open Chat</button>' in gaps_detail_js
        assert 'const nodeDisplayName = gap.node_display_name || gap.node_id || "Unknown";' in gaps_detail_js
        assert 'updated ${fmtTime(gap.updated)} · node <span title="${htmlEscape(nodeOwnerTitle)}">' in gaps_detail_js
        assert "${htmlEscape(nodeDisplayName)}</span>" in gaps_detail_js
        menu_block = gaps_detail_js.split('<div class="nav-menu-panel gap-action-panel">', 1)[1].split("</div>", 1)[0]
        expected_menu_order = [
            'id="btn-view-logs">View Logs</button>',
            'id="btn-reporter">Reporter</button>',
            'id="btn-rename">Rename</button>',
            'id="btn-priority">Change Priority</button>',
            'id="btn-cancel"',
            'id="btn-delete">Delete</button>',
        ]
        cursor = -1
        for item in expected_menu_order:
            next_cursor = menu_block.find(item)
            assert next_cursor > cursor, item
            cursor = next_cursor
        assert 'openGapReporterModal(gap)' in gaps_detail_js
        assert 'api("POST", "/api/gaps/bulk", {' in gaps_detail_js
        assert 'location.hash = `#/logs?gap_id=${encodeURIComponent(gap.id)}`;' in gaps_detail_js
        assert ".gap-action-group" in gaps_css
        assert ".gap-action-more::after" in gaps_css
        assert 'hashQs.get("gap_id") || ""' in logs_js
        assert 'params.set("gap_id", f.gap_id);' in logs_js
        assert 'value="${htmlEscape(f.gap_id)}"' in logs_js
        assert '{ key: "message", label: "Message" }' not in logs_js
        assert '<td class="logs-entry-cell" colspan="5">' in logs_js
        assert '<div class="logs-entry-meta">' in logs_js
        assert '<div class="logs-entry-message logs-message-cell" data-label="Message">' in logs_js
        assert ".logs-entry-meta" in common_css
        assert ".logs-message-text" in common_css
        assert "data-role=\"round-logs\"" not in gaps_detail_js
        assert "function refreshGapRoundLogs" not in gaps_detail_js
        assert "function loadRoundLogs" not in gaps_detail_js
        assert "Load more" not in gaps_detail_js
        activity_block = common_js.split(
            'sseSource.addEventListener("activity_added"', 1,
        )[1].split('sseSource.addEventListener("status_change"', 1)[0]
        round_log_block = common_js.split(
            'sseSource.addEventListener("round_log_added"', 1,
        )[1].split("sseSource.onerror", 1)[0]
        assert "refreshGapRoundLogs" not in activity_block
        assert "loadGapDetail(state.currentGap)" not in activity_block
        assert "refreshGapRoundLogs" not in round_log_block
        assert 'if (state.currentRoute === "logs") loadLogs();' in round_log_block
        assert "loadGapDetail(state.currentGap)" not in round_log_block
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
