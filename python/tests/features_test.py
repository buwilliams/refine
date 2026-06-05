"""Feature shared-operation and durable storage tests."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-features-")
    conn = init_refine(client)
    try:
        from refine_server import feature_ops, gap_ops, gap_writer, gaps, project_state
        from refine_server.backend_protocol import (
            M_BULK_DELETE_GAPS,
            M_CANCEL,
            M_FEATURE_WORKFLOW_MOVE,
        )
        from refine_server.paths import feature_json_path, relative_feature_path
        from refine_server.ulid import new_ulid

        active = project_state.active_node_id()
        feature_id = new_ulid()
        status, body = feature_ops.create_feature({
            "id": feature_id,
            "name": "Settings redesign",
            "description": "Plan the settings IA work.",
            "reporter": "Ada",
        })
        assert status == 201, body
        feature = body["feature"]
        assert feature["id"] == feature_id
        assert feature["node_id"] == active
        assert feature["json_path"] == relative_feature_path(feature_id)
        assert feature_json_path(feature_id).is_file()
        print("[ok] feature create writes durable JSON and index metadata")

        gap_ids = [new_ulid(), new_ulid(), new_ulid()]
        create_indexed_gap(conn, gap_ids[0], status="done", node_id=active)
        create_indexed_gap(conn, gap_ids[1], status="todo", node_id=active)
        create_indexed_gap(conn, gap_ids[2], status="backlog", node_id=active)
        for gap_id in gap_ids:
            status, body = feature_ops.assign_gap(feature_id, gap_id)
            assert status == 200, body
        status, body = feature_ops.get_feature(feature_id)
        assert status == 200, body
        detail = body["feature"]
        assert [g["id"] for g in detail["gaps"]] == gap_ids, detail["gaps"]
        assert [g["feature_order"] for g in detail["gaps"]] == [1, 2, 3], detail["gaps"]
        assert detail["status"] == "todo", detail
        assert detail["gap_count"] == 3
        assert detail["done_count"] == 1
        assert detail["blocked_count"] == 1
        assert detail["next_gap"]["id"] == gap_ids[1]
        gap_json = gaps.read_gap_json(gap_ids[1], include_logs=False)
        assert gap_json["feature_id"] == feature_id, gap_json
        assert gap_json["feature_order"] == 2, gap_json
        print("[ok] assignment appends ordered gaps and derives progress")

        project_state.rebuild_sqlite_cache(conn, force=True)
        status, body = feature_ops.get_feature(feature_id)
        assert status == 200, body
        rebuilt = body["feature"]
        assert [g["id"] for g in rebuilt["gaps"]] == gap_ids, rebuilt["gaps"]
        assert rebuilt["status"] == "todo", rebuilt
        print("[ok] feature and gap associations survive SQLite cache rebuild")

        status, body = feature_ops.reorder_gap(feature_id, gap_ids[2], before=gap_ids[1])
        assert status == 200, body
        reordered = body["feature"]["gaps"]
        assert [g["id"] for g in reordered] == [gap_ids[0], gap_ids[2], gap_ids[1]], reordered
        assert [g["feature_order"] for g in reordered] == [1, 2, 3], reordered
        print("[ok] reorder rewrites deterministic feature order")

        status, body = feature_ops.remove_gap(feature_id, gap_ids[2])
        assert status == 200, body
        removed = body["feature"]["gaps"]
        assert [g["id"] for g in removed] == [gap_ids[0], gap_ids[1]], removed
        assert [g["feature_order"] for g in removed] == [1, 2], removed
        removed_gap = gaps.read_gap_json(gap_ids[2], include_logs=False)
        assert removed_gap["feature_id"] is None, removed_gap
        assert removed_gap["feature_order"] is None, removed_gap
        print("[ok] remove nulls membership and compacts remaining order")

        status, body = gap_ops.list_gaps(feature=feature_id)
        assert status == 200, body
        assert {g["id"] for g in body["gaps"]} == {gap_ids[0], gap_ids[1]}, body
        assert all(g["feature_id"] == feature_id for g in body["gaps"]), body
        status, body = gap_ops.list_gaps(feature="standalone")
        assert status == 200, body
        assert gap_ids[2] in [g["id"] for g in body["gaps"]], body
        print("[ok] Gap list can filter by Feature or standalone")

        runner_calls: list[tuple[str, dict, float]] = []

        def fake_runner(method: str, params: dict, timeout: float) -> dict:
            runner_calls.append((method, params, timeout))
            if method == M_CANCEL:
                gap_id = params["gap_id"]
                conn.execute(
                    "UPDATE gaps_index SET status = 'cancelled' WHERE id = ?",
                    (gap_id,),
                )
                gap_writer.update_fields(gap_id, status="cancelled")
                return {"ok": True, "gap_id": gap_id}
            if method == M_BULK_DELETE_GAPS:
                ids = list(params["gap_ids"])
                for gap_id in ids:
                    gap_writer.delete_gap_file(gap_id)
                    conn.execute("DELETE FROM gaps_index WHERE id = ?", (gap_id,))
                return {
                    "deleted": len(ids),
                    "ids": ids,
                    "failures": [],
                    "failed": 0,
                    "progress": {"completed": len(ids), "total": len(ids)},
                }
            raise AssertionError(f"unexpected runner call: {method}")

        status, body = feature_ops.cancel_feature(conn, fake_runner, feature_id)
        assert status == 200, body
        assert body["cancelled_ids"] == [gap_ids[1]], body
        assert gaps.read_gap_json(gap_ids[0], include_logs=False)["status"] == "done"
        assert gaps.read_gap_json(gap_ids[1], include_logs=False)["status"] == "cancelled"
        assert [call[0] for call in runner_calls] == [M_CANCEL], runner_calls
        print("[ok] Feature cancel cascades through shared Gap cancel path")

        delete_feature_id = new_ulid()
        status, body = feature_ops.create_feature({
            "id": delete_feature_id,
            "name": "Delete cascade",
            "reporter": "Ada",
        })
        assert status == 201, body
        delete_gap_ids = [new_ulid(), new_ulid()]
        for gap_id in delete_gap_ids:
            create_indexed_gap(conn, gap_id, status="todo", node_id=active)
            status, body = feature_ops.assign_gap(delete_feature_id, gap_id)
            assert status == 200, body
        runner_calls.clear()
        status, body = feature_ops.delete_feature(conn, fake_runner, delete_feature_id)
        assert status == 200, body
        assert body["gaps"]["ids"] == delete_gap_ids, body
        status, body = feature_ops.get_feature(delete_feature_id)
        assert status == 404, body
        remaining = conn.execute(
            "SELECT COUNT(*) AS n FROM gaps_index WHERE feature_id = ?",
            (delete_feature_id,),
        ).fetchone()["n"]
        assert remaining == 0, remaining
        assert [call[0] for call in runner_calls] == [M_BULK_DELETE_GAPS], runner_calls
        print("[ok] Feature delete cascades through shared bulk delete path")

        source_feature = new_ulid()
        target_feature = new_ulid()
        for fid, name in ((source_feature, "Source Feature"), (target_feature, "Target Feature")):
            status, body = feature_ops.create_feature({
                "id": fid,
                "name": name,
                "reporter": "Ada",
            })
            assert status == 201, body
        move_gap_ids = [new_ulid(), new_ulid()]
        for gap_id in move_gap_ids:
            create_indexed_gap(conn, gap_id, status="todo", node_id=active)
            status, body = feature_ops.assign_gap(source_feature, gap_id)
            assert status == 200, body
        status, body = feature_ops.assign_gap(target_feature, move_gap_ids[0])
        assert status == 200, body
        target_detail = body["feature"]
        assert [g["id"] for g in target_detail["gaps"]] == [move_gap_ids[0]], target_detail
        assert target_detail["gaps"][0]["feature_order"] == 1, target_detail
        status, body = feature_ops.get_feature(source_feature)
        assert status == 200, body
        source_detail = body["feature"]
        assert [g["id"] for g in source_detail["gaps"]] == [move_gap_ids[1]], source_detail
        assert source_detail["gaps"][0]["feature_order"] == 1, source_detail
        print("[ok] moving a Gap between Features compacts both ordered lists")

        bulk_source = new_ulid()
        bulk_target = new_ulid()
        for fid, name in ((bulk_source, "Bulk Source Feature"), (bulk_target, "Bulk Target Feature")):
            status, body = feature_ops.create_feature({
                "id": fid,
                "name": name,
                "reporter": "Ada",
            })
            assert status == 201, body
        bulk_free = new_ulid()
        bulk_move = new_ulid()
        bulk_already = new_ulid()
        for gap_id in (bulk_free, bulk_move, bulk_already):
            create_indexed_gap(conn, gap_id, status="todo", node_id=active)
        status, body = feature_ops.assign_gap(bulk_source, bulk_move)
        assert status == 200, body
        status, body = feature_ops.assign_gap(bulk_target, bulk_already)
        assert status == 200, body
        status, body = feature_ops.bulk_assign_gaps(bulk_target, {
            "selected_ids": [bulk_free, bulk_move, bulk_already],
        })
        assert status == 200, body
        assert body["updated"] == 2, body
        assert body["ids"] == [bulk_free, bulk_move], body
        assert body["skipped_details"] == [{
            "id": bulk_already,
            "reason": "already-assigned",
        }], body
        assert "feature" not in body, body
        status, body = feature_ops.get_feature(bulk_target)
        assert status == 200, body
        assert [g["id"] for g in body["feature"]["gaps"]] == [
            bulk_already,
            bulk_free,
            bulk_move,
        ], body
        status, body = feature_ops.get_feature(bulk_source)
        assert status == 200, body
        assert body["feature"]["gaps"] == [], body
        assert gaps.read_gap_json(bulk_move, include_logs=False)["feature_id"] == bulk_target
        print("[ok] bulk Feature assignment moves selected Gaps and skips existing membership")

        workflow_feature = new_ulid()
        status, body = feature_ops.create_feature({
            "id": workflow_feature,
            "name": "Workflow Feature",
            "reporter": "Ada",
        })
        assert status == 201, body
        workflow_statuses = [
            "backlog",
            "todo",
            "in-progress",
            "qa",
            "failed",
            "cancelled",
            "review",
            "done",
            "ready-merge",
            "awaiting-rebuild",
        ]
        workflow_gap_ids: dict[str, str] = {}
        for idx, gap_status in enumerate(workflow_statuses):
            gid = f"01FEATUREWF{idx:02d}AAAAAAAAAAAAA"
            workflow_gap_ids[gap_status] = gid
            create_indexed_gap(
                conn,
                gid,
                status=gap_status,
                node_id=active,
                branch=f"refine/{gid.lower()}" if gap_status == "in-progress" else None,
            )
            status, body = feature_ops.assign_gap(workflow_feature, gid)
            assert status == 200, body

        runner_calls.clear()

        def fake_workflow_runner(method: str, params: dict, timeout: float) -> dict:
            runner_calls.append((method, params, timeout))
            assert method == M_FEATURE_WORKFLOW_MOVE, method
            assert params["status"] == "backlog", params
            return {
                "updated": len(params["gap_ids"]),
                "ids": list(params["gap_ids"]),
                "value": params["status"],
                "stopped": 1,
                "stopped_ids": [workflow_gap_ids["in-progress"]],
                "failed": 0,
                "failures": [],
                "skipped": 0,
                "skipped_details": [],
                "progress": {
                    "completed": len(params["gap_ids"]),
                    "total": len(params["gap_ids"]),
                },
            }

        status, body = feature_ops.move_feature_workflow(
            conn,
            fake_workflow_runner,
            workflow_feature,
            "backlog",
        )
        assert status == 200, body
        assert [call[0] for call in runner_calls] == [M_FEATURE_WORKFLOW_MOVE], runner_calls
        assert runner_calls[0][1]["gap_ids"] == [
            workflow_gap_ids["backlog"],
            workflow_gap_ids["todo"],
            workflow_gap_ids["in-progress"],
            workflow_gap_ids["qa"],
            workflow_gap_ids["failed"],
            workflow_gap_ids["cancelled"],
        ], runner_calls
        assert body["skipped_details"] == [
            {"id": workflow_gap_ids["review"], "reason": "status:review"},
            {"id": workflow_gap_ids["done"], "reason": "status:done"},
            {"id": workflow_gap_ids["ready-merge"], "reason": "status:ready-merge"},
            {"id": workflow_gap_ids["awaiting-rebuild"], "reason": "status:awaiting-rebuild"},
        ], body
        print("[ok] Feature workflow action selects only non-protected Gaps")

        from refine_server.runner import Runner

        class NoopDispatcher:
            def enforce_now(self) -> None:
                pass

            def stop(self) -> None:
                pass

        class NoopGovernance:
            def wake(self) -> None:
                pass

            def stop(self) -> None:
                pass

        class FakeSubprocessManager:
            def __init__(self) -> None:
                self.cancelled: list[tuple[str, str]] = []

            def is_running(self, gap_id: str) -> bool:
                return gap_id == workflow_gap_ids["in-progress"]

            def cancel(self, gap_id: str, reason: str = "cancel") -> bool:
                self.cancelled.append((gap_id, reason))
                return True

            def cancel_all(self, reason: str = "shutdown") -> int:  # noqa: ARG002
                return 0

        runner = Runner()
        runner.dispatcher = NoopDispatcher()  # type: ignore[assignment]
        runner.governance_agent = NoopGovernance()  # type: ignore[assignment]
        fake_sub_mgr = FakeSubprocessManager()
        runner.sub_mgr = fake_sub_mgr  # type: ignore[assignment]
        try:
            result = runner._h_feature_workflow_move({  # noqa: SLF001
                "feature_id": workflow_feature,
                "status": "todo",
                "gap_ids": [
                    workflow_gap_ids["todo"],
                    workflow_gap_ids["in-progress"],
                    workflow_gap_ids["qa"],
                    workflow_gap_ids["review"],
                    workflow_gap_ids["done"],
                    workflow_gap_ids["ready-merge"],
                    workflow_gap_ids["awaiting-rebuild"],
                ],
            })
        finally:
            runner.shutdown()
        assert result["ids"] == [
            workflow_gap_ids["in-progress"],
            workflow_gap_ids["qa"],
        ], result
        assert result["stopped_ids"] == [workflow_gap_ids["in-progress"]], result
        assert fake_sub_mgr.cancelled == [
            (workflow_gap_ids["in-progress"], "feature_workflow_move"),
        ], fake_sub_mgr.cancelled
        rows = {
            row["id"]: (row["status"], row["branch_name"])
            for row in conn.execute(
                "SELECT id, status, branch_name FROM gaps_index "
                "WHERE feature_id = ?",
                (workflow_feature,),
            )
        }
        assert rows[workflow_gap_ids["in-progress"]] == ("todo", None), rows
        assert rows[workflow_gap_ids["qa"]][0] == "todo", rows
        assert rows[workflow_gap_ids["review"]][0] == "review", rows
        assert rows[workflow_gap_ids["done"]][0] == "done", rows
        assert rows[workflow_gap_ids["ready-merge"]][0] == "ready-merge", rows
        assert rows[workflow_gap_ids["awaiting-rebuild"]][0] == "awaiting-rebuild", rows
        assert gaps.read_gap_json(
            workflow_gap_ids["in-progress"],
            include_logs=True,
        )["status"] == "todo"
        print("[ok] Feature workflow action stops in-progress and moves eligible Gaps")

        other_node = project_state.create_node("Other")
        other_gap = new_ulid()
        create_indexed_gap(conn, other_gap, status="todo", node_id=other_node["id"])
        status, body = feature_ops.assign_gap(feature_id, other_gap)
        assert status == 409, body
        assert "same node" in body["error"]["message"], body
        project_state.set_active_node(other_node["id"])
        project_state.rebuild_sqlite_cache(conn)
        status, body = feature_ops.update_feature(feature_id, {"name": "Blocked rename"})
        assert status == 409, body
        assert body["error"]["owner_node_id"] == active, body
        print("[ok] feature mutations enforce node ownership")
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("\nALL OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
