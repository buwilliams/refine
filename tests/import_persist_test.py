"""Focused tests for Gap import persistence."""
from __future__ import annotations

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-import-persist-")
    conn = init_refine(client)
    try:
        from refine_ui import api

        status, body = api.import_persist({
            "reporter": "Reporter",
            "drafts": [
                {
                    "name": f"Bulk import {i}",
                    "actual": f"Actual {i}",
                    "target": f"Target {i}",
                }
                for i in range(1, 261)
            ],
        })
        assert status == 201, body
        assert body["count"] == 260, body

        status, body = api.import_persist({
            "reporter": "Reporter",
            "drafts": [["not", "an", "object"]],
        })
        assert status == 200, body
        assert body["count"] == 0, body
        assert body["failed"] == 1, body
        assert body["failures"][0]["index"] == 1, body
        assert body["failures"][0]["error"] == "draft must be an object", body

        status, body = api.import_persist({
            "reporter": "Reporter",
            "drafts": [
                {
                    "name": "Partial one",
                    "actual": "Current one",
                    "target": "Target one",
                },
                {"name": "Needs correction", "actual": "", "target": ""},
                {
                    "name": "Partial two",
                    "actual": "Current two",
                    "target": "Target two",
                },
            ],
        })
        assert status == 200, body
        assert body["count"] == 2, body
        assert body["failed"] == 1, body
        assert body["failures"][0] == {
            "index": 2,
            "error": "actual or target must be non-empty",
            "draft": {
                "name": "Needs correction",
                "actual": "",
                "target": "",
            },
        }, body

        from refine_server import project_state
        from refine_server.paths import relative_gap_path
        from refine_server.runner import Runner
        import refine_server.runner as runner_mod

        runner = Runner()
        original_create_gap = runner_mod.gap_writer.create_gap

        def create_gap_and_project(*, gap_id, name, initial_round,
                                   status="backlog", priority="low",
                                   instance_id=None):
            gap = original_create_gap(
                gap_id=gap_id,
                name=name,
                initial_round=initial_round,
                status=status,
                priority=priority,
                instance_id=instance_id,
            )
            runner._conn.execute(
                "INSERT INTO gaps_index "
                "(id, name, status, priority, reporter, created, updated, "
                "instance_id, json_path) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    gap_id,
                    name,
                    status,
                    priority,
                    initial_round["reporter"],
                    gap["created"],
                    gap["updated"],
                    instance_id or project_state.active_instance_id(),
                    relative_gap_path(gap_id),
                ),
            )
            return gap

        runner_mod.gap_writer.create_gap = create_gap_and_project
        try:
            result = runner._h_create_gap({
                "gap_id": "01IMPORTPERSISTCACHE00000",
                "name": "Import race",
                "priority": "low",
                "reporter": "Reporter",
                "actual": "Current",
                "target": "Target",
            })
        finally:
            runner_mod.gap_writer.create_gap = original_create_gap
            runner._conn.close()

        assert result["gap"]["id"] == "01IMPORTPERSISTCACHE00000"
        row = conn.execute(
            "SELECT name, status, reporter FROM gaps_index WHERE id = ?",
            ("01IMPORTPERSISTCACHE00000",),
        ).fetchone()
        assert dict(row) == {
            "name": "Import race",
            "status": "backlog",
            "reporter": "Reporter",
        }
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("import persist tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
