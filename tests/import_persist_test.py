"""Focused tests for Gap import persistence."""
from __future__ import annotations

import threading

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-import-persist-")
    conn = init_refine(client)
    try:
        from refine_ui import api

        status, body = api.import_persist({
            "reporter": "Reporter",
            "background": False,
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

        original_import_threshold = api.IMPORT_BACKGROUND_THRESHOLD
        api.IMPORT_BACKGROUND_THRESHOLD = 3
        try:
            status, body = api.import_persist({
                "reporter": "Reporter",
                "drafts": [
                    {
                        "name": f"Async import {i}",
                        "actual": f"Async actual {i}",
                        "target": f"Async target {i}",
                        "duplicate_decision": "original",
                    }
                    for i in range(1, 4)
                ],
            })
            assert status == 202, body
            result = wait_job(body["job"]["id"])
            assert result["http_status"] == 201, result
            assert result["count"] == 3, result
        finally:
            api.IMPORT_BACKGROUND_THRESHOLD = original_import_threshold

        status, body = api.import_persist({
            "reporter": "Reporter",
            "drafts": [
                {
                    "name": f"Large async import {i}",
                    "actual": f"Large async actual unique batch item {i}",
                    "target": f"Large async target unique batch item {i}",
                    "duplicate_decision": "original",
                }
                for i in range(1, 701)
            ],
        })
        assert status == 202, body
        assert body["drafts"] == 700, body
        result = wait_job(body["job"]["id"], timeout=30)
        assert result["http_status"] == 201, result
        assert result["count"] == 700, result

        from refine_server.backend_protocol import M_CREATE_GAP

        real_get_client = api.get_client
        real_client = real_get_client()
        first_created = threading.Event()
        release_create = threading.Event()

        class SlowCreateClient:
            def call(self, method, params, timeout=None):  # noqa: ANN001, ANN202
                if method == M_CREATE_GAP:
                    first_created.set()
                if timeout is None:
                    result = real_client.call(method, params)
                else:
                    result = real_client.call(method, params, timeout=timeout)
                if method == M_CREATE_GAP:
                    assert release_create.wait(timeout=2), "cancel rollback test was not released"
                return result

        api.get_client = lambda: SlowCreateClient()
        try:
            status, body = api.import_persist({
                "reporter": "Reporter",
                "background": True,
                "drafts": [
                    {
                        "name": f"Cancel rollback {i}",
                        "actual": f"Cancel rollback actual {i}",
                        "target": f"Cancel rollback target {i}",
                        "duplicate_decision": "original",
                    }
                    for i in range(1, 4)
                ],
            })
            assert status == 202, body
            job_id = body["job"]["id"]
            assert first_created.wait(timeout=2), "import job did not create first Gap"
            from refine_ui import background_jobs

            active_job = background_jobs.snapshot(job_id)
            assert active_job["progress"]["total"] == 3, active_job
            assert active_job["progress"]["message"] == "Importing Gap 1 of 3", active_job
            cancelled = background_jobs.cancel(job_id)
            assert cancelled and cancelled["progress"]["message"] == "Cancelling", cancelled
            release_create.set()
            job = wait_job(job_id, timeout=10)
        finally:
            api.get_client = real_get_client
            release_create.set()

        assert job["status"] == "cancelled", job
        assert job["result"]["rolled_back"] == 1, job
        rows = conn.execute(
            "SELECT id FROM gaps_index WHERE name LIKE 'Cancel rollback%'",
        ).fetchall()
        assert rows == [], rows

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
                "reporter": "Reporter",
                "priority": "low",
            },
        }, body

        status, body = api.import_persist({
            "drafts": [{
                "name": "Per draft metadata",
                "actual": "Metadata actual",
                "target": "Metadata target",
                "reporter": "Csv Reporter",
                "priority": "high",
            }],
        })
        assert status == 201, body
        row = conn.execute(
            "SELECT reporter, priority FROM gaps_index WHERE id = ?",
            (body["created"][0],),
        ).fetchone()
        assert dict(row) == {"reporter": "Csv Reporter", "priority": "high"}

        from refine_server import feature_ops, import_ops

        status, body = api.import_persist({
            "reporter": "Reporter",
            "new_feature_name": "Imported Feature",
            "new_feature_description": "Imported as ordered work",
            "drafts": [
                {
                    "name": "Feature import one",
                    "actual": "Current feature-backed import alpha",
                    "target": "Target feature-backed import alpha",
                    "duplicate_decision": "original",
                },
                {
                    "name": "Feature import two",
                    "actual": "Current feature-backed import beta",
                    "target": "Target feature-backed import beta",
                    "duplicate_decision": "original",
                },
            ],
        })
        assert status == 201, body
        assert body["count"] == 2, body
        assert body["feature_destination"] == "new", body
        feature_id = body["feature_id"]
        status, detail = feature_ops.get_feature(feature_id)
        assert status == 200, detail
        assert detail["feature"]["name"] == "Imported Feature", detail
        assert [g["id"] for g in detail["feature"]["gaps"]] == body["created"], detail
        assert [g["feature_order"] for g in detail["feature"]["gaps"]] == [1, 2], detail
        print("[ok] import can create a Feature and preserve reviewed order")

        status, body = api.import_persist({
            "reporter": "Reporter",
            "feature_id": feature_id,
            "drafts": [{
                "name": "Feature import append",
                "actual": "Current feature-backed import append",
                "target": "Target feature-backed import append",
                "duplicate_decision": "original",
            }],
        })
        assert status == 201, body
        status, detail = feature_ops.get_feature(feature_id)
        assert status == 200, detail
        assert [g["feature_order"] for g in detail["feature"]["gaps"]] == [1, 2, 3], detail
        assert detail["feature"]["gaps"][-1]["id"] == body["created"][0], detail
        print("[ok] import can append to an existing Feature")

        status, body = api.import_persist({
            "reporter": "Reporter",
            "new_feature_name": "Rollback Feature",
            "drafts": [
                {
                    "name": "Rollback import one",
                    "actual": "Current feature rollback alpha",
                    "target": "Target feature rollback alpha",
                    "duplicate_decision": "original",
                },
                {"name": "Rollback invalid", "actual": "", "target": ""},
            ],
        })
        assert status == 200, body
        assert body["count"] == 0, body
        assert body["failed"] == 1, body
        assert body["rolled_back"] == 1, body
        status, missing = feature_ops.get_feature(body["feature_id"])
        assert status == 404, missing
        rows = conn.execute(
            "SELECT id FROM gaps_index WHERE name LIKE 'Rollback import%'",
        ).fetchall()
        assert rows == [], rows
        print("[ok] failed Feature import rolls back created Feature and Gaps")

        status, body = api.import_persist({
            "new_feature_name": "No-op Feature",
            "drafts": [{
                "actual": "Current ignored duplicate",
                "target": "Target ignored duplicate",
                "reporter": "Reporter",
                "duplicate_decision": "duplicate",
            }],
        })
        assert status == 200, body
        assert body["count"] == 0, body
        assert "feature_id" not in body, body
        status, listed = feature_ops.list_features(q="No-op Feature")
        assert status == 200, listed
        assert listed["features"] == [], listed
        print("[ok] no-op Feature import does not leave an empty Feature")

        from refine_server.paths import relative_gap_path
        from refine_server.runner import Runner
        import refine_server.runner as runner_mod

        original_activity_append = runner_mod.activity.append

        def fail_activity_append(*args, **kwargs):
            raise IndexError("tuple index out of range")

        runner_mod.activity.append = fail_activity_append
        try:
            status, body = api.import_persist({
                "reporter": "Reporter",
                "drafts": [{
                "name": "Activity side effect should not fail import",
                "actual": "Current",
                "target": "Target",
                "duplicate_decision": "original",
            }],
        })
        finally:
            runner_mod.activity.append = original_activity_append

        assert status == 201, body
        assert body["count"] == 1, body
        assert body["failed"] == 0, body

        from refine_server import project_state

        runner = Runner()
        original_create_gap = runner_mod.gap_writer.create_gap

        def create_gap_and_project(*, gap_id, name, initial_round,
                                   status="backlog", priority="low",
                                   node_id=None, feature_id=None,
                                   feature_order=None):
            gap = original_create_gap(
                gap_id=gap_id,
                name=name,
                initial_round=initial_round,
                status=status,
                priority=priority,
                node_id=node_id,
                feature_id=feature_id,
                feature_order=feature_order,
            )
            runner._conn.execute(
                "INSERT INTO gaps_index "
                "(id, name, status, priority, reporter, created, updated, "
                "node_id, feature_id, feature_order, json_path) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    gap_id,
                    name,
                    status,
                    priority,
                    initial_round["reporter"],
                    gap["created"],
                    gap["updated"],
                    node_id or project_state.active_node_id(),
                    feature_id,
                    feature_order,
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


def wait_job(job_id: str, *, timeout: float = 10) -> dict:
    import time
    from refine_ui import background_jobs

    deadline = time.time() + timeout
    while time.time() < deadline:
        job = background_jobs.snapshot(job_id)
        if job and job["status"] == "complete":
            return job["result"]
        if job and job["status"] == "cancelled":
            return job
        if job and job["status"] == "failed":
            raise AssertionError(job["error"])
        time.sleep(0.05)
    raise AssertionError(f"job did not finish: {job_id}")


if __name__ == "__main__":
    raise SystemExit(main())
