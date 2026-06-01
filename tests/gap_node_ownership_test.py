"""Gap mutation ownership tests for multi-node projects."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


class FakeSubprocessManager:
    def running_snapshot(self) -> list[dict]:
        return []

    def is_running(self, _gap_id: str) -> bool:
        return False

    def cancel(self, _gap_id: str, reason: str = "cancel") -> bool:
        return False


def main() -> int:
    tmp, client = make_client_repo("refine-gap-node-ownership-")
    conn = init_refine(client)
    try:
        from refine_server import db, gap_writer, gaps, project_state
        from refine_server.dispatcher import Dispatcher
        from refine_ui import api

        default = project_state.active_node_id()
        refine2 = project_state.create_node("Refine2")

        def create(gap_id: str, status: str, node_id: str) -> None:
            gap_writer.create_gap(
                gap_id=gap_id,
                name=gap_id,
                initial_round=gaps.new_round("Jane", "Actual", "Target"),
                status=status,
                priority="medium",
                node_id=node_id,
            )

        default_gap = "01OWNERSHIPDEFAULTAAAAAAAA"
        refine2_gap = "01OWNERSHIPREFINE2AAAAAAAA"
        create(default_gap, "backlog", default)
        create(refine2_gap, "backlog", refine2["id"])
        project_state.set_active_node(refine2["id"])
        project_state.rebuild_sqlite_cache(conn)

        status, body = api.update_gap_name(default_gap, {"status": "todo"})
        assert status == 409, body
        assert body["error"]["code"] == "node_ownership", body
        row = conn.execute(
            "SELECT status, node_id FROM gaps_index WHERE id = ?",
            (default_gap,),
        ).fetchone()
        assert row["status"] == "backlog", dict(row)
        assert row["node_id"] == default, dict(row)

        status, body = api.bulk_update_gaps({
            "filter": {"status": "backlog", "node": "all"},
            "update": {"status": "todo"},
        })
        assert status == 409, body
        assert body["error"]["code"] == "node_ownership", body
        rows = {
            row["id"]: row["status"]
            for row in conn.execute(
                "SELECT id, status FROM gaps_index WHERE id IN (?, ?)",
                (default_gap, refine2_gap),
            )
        }
        assert rows == {default_gap: "backlog", refine2_gap: "backlog"}, rows

        def assert_ownership_blocked(result: tuple[int, dict]) -> None:
            status_code, payload = result
            assert status_code == 409, payload
            assert payload["error"]["code"] == "node_ownership", payload

        assert_ownership_blocked(api.delete_gap(default_gap))
        assert_ownership_blocked(api.bulk_delete_gaps({
            "filter": {"status": "backlog", "node": "all"},
        }))
        assert_ownership_blocked(api.append_round(default_gap, {
            "reporter": "Jane",
            "actual": "Actual",
            "target": "Target",
        }))
        assert_ownership_blocked(api.edit_latest_round(default_gap, {
            "reporter": "Jane",
            "actual": "Actual",
            "target": "Target",
        }))
        assert_ownership_blocked(api.verify(default_gap))
        assert_ownership_blocked(api.retry(default_gap))
        assert_ownership_blocked(api.retry_merge(default_gap))
        assert_ownership_blocked(api.cancel(default_gap))

        status, body = api.transfer_node_gaps({
            "target_node_id": refine2["id"],
            "filter": {"node": default},
        })
        assert status == 200, body
        assert body["updated"] == 1, body
        row = conn.execute(
            "SELECT node_id FROM gaps_index WHERE id = ?",
            (default_gap,),
        ).fetchone()
        assert row["node_id"] == refine2["id"], dict(row)

        db.set_setting(conn, "paused", "1")
        status, body = api.update_gap_name(default_gap, {"status": "todo"})
        assert status == 200, body
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?",
            (default_gap,),
        ).fetchone()
        assert row["status"] == "todo", dict(row)
        updated_gap = gaps.read_gap_json(default_gap)
        messages = [log["message"] for log in updated_gap["rounds"][-1]["logs"]]
        assert "Workflow status changed: backlog → todo" in messages, messages

        automation_default = "01OWNERSHIPAUTODEFAULTAAAA"
        automation_refine2 = "01OWNERSHIPAUTOREFINE2AAAA"
        create(automation_default, "backlog", default)
        create(automation_refine2, "backlog", refine2["id"])
        project_state.rebuild_sqlite_cache(conn)
        db.set_setting(conn, "backlog_promote_after_seconds", "0")
        dispatcher = Dispatcher(get_conn=lambda: conn, sub_mgr=FakeSubprocessManager())
        dispatcher._promote_backlog(conn)
        rows = {
            row["id"]: row["status"]
            for row in conn.execute(
                "SELECT id, status FROM gaps_index WHERE id IN (?, ?)",
                (automation_default, automation_refine2),
            )
        }
        assert rows == {
            automation_default: "backlog",
            automation_refine2: "todo",
        }, rows
        auto_gap = gaps.read_gap_json(automation_refine2)
        auto_messages = [log["message"] for log in auto_gap["rounds"][-1]["logs"]]
        assert "Auto-promoted from backlog to todo" in auto_messages, auto_messages

        root = Path(__file__).resolve().parents[1]
        common_js = (root / "refine_ui/static/js/common.js").read_text(
            encoding="utf-8",
        )
        gaps_detail_js = (
            root / "refine_ui/static/js/features/gaps-detail.js"
        ).read_text(encoding="utf-8")
        gaps_bulk_js = (
            root / "refine_ui/static/js/features/gaps-bulk.js"
        ).read_text(encoding="utf-8")
        assert "function modalAlert" in common_js
        assert "node_ownership" in common_js
        assert "function isBackgroundJobActiveError" in common_js
        assert "background_job_active" in common_js
        assert 'title: "Refine is busy"' in common_js
        assert "err.code = raw.code" in common_js
        assert "await showActionError(e);" in gaps_detail_js
        assert 'await showActionError(e, "Bulk update failed");' in gaps_bulk_js
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("gap node ownership tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
