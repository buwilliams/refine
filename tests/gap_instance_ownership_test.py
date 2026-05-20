"""Gap mutation ownership tests for multi-instance projects."""
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
    tmp, client = make_client_repo("refine-gap-instance-ownership-")
    conn = init_refine(client)
    try:
        from refine_server import db, gap_writer, gaps, project_state
        from refine_server.dispatcher import Dispatcher
        from refine_ui import api

        default = project_state.active_instance_id()
        refine2 = project_state.create_instance("Refine2")

        def create(gap_id: str, status: str, instance_id: str) -> None:
            gap_writer.create_gap(
                gap_id=gap_id,
                name=gap_id,
                initial_round=gaps.new_round("Jane", "Actual", "Target"),
                status=status,
                priority="medium",
                instance_id=instance_id,
            )

        default_gap = "01OWNERSHIPDEFAULTAAAAAAAA"
        refine2_gap = "01OWNERSHIPREFINE2AAAAAAAA"
        create(default_gap, "backlog", default)
        create(refine2_gap, "backlog", refine2["id"])
        project_state.set_active_instance(refine2["id"])
        project_state.rebuild_sqlite_cache(conn)

        status, body = api.update_gap_name(default_gap, {"status": "todo"})
        assert status == 409, body
        assert body["error"]["code"] == "instance_ownership", body
        row = conn.execute(
            "SELECT status, instance_id FROM gaps_index WHERE id = ?",
            (default_gap,),
        ).fetchone()
        assert row["status"] == "backlog", dict(row)
        assert row["instance_id"] == default, dict(row)

        status, body = api.bulk_update_gaps({
            "filter": {"status": "backlog", "instance": "all"},
            "update": {"status": "todo"},
        })
        assert status == 409, body
        assert body["error"]["code"] == "instance_ownership", body
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
            assert payload["error"]["code"] == "instance_ownership", payload

        assert_ownership_blocked(api.delete_gap(default_gap))
        assert_ownership_blocked(api.bulk_delete_gaps({
            "filter": {"status": "backlog", "instance": "all"},
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
        assert_ownership_blocked(api.cancel(default_gap))

        status, body = api.transfer_instance_gaps({
            "target_instance_id": refine2["id"],
            "filter": {"instance": default},
        })
        assert status == 200, body
        assert body["updated"] == 1, body
        row = conn.execute(
            "SELECT instance_id FROM gaps_index WHERE id = ?",
            (default_gap,),
        ).fetchone()
        assert row["instance_id"] == refine2["id"], dict(row)

        db.set_setting(conn, "paused", "1")
        status, body = api.update_gap_name(default_gap, {"status": "todo"})
        assert status == 200, body
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?",
            (default_gap,),
        ).fetchone()
        assert row["status"] == "todo", dict(row)

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
        assert "instance_ownership" in common_js
        assert "await showActionError(e);" in gaps_detail_js
        assert 'await showActionError(e, "Bulk update failed");' in gaps_bulk_js
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("gap instance ownership tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
