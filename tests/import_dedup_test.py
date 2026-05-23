"""Focused tests for deterministic import duplicate detection."""
from __future__ import annotations

from tests.helpers import cleanup_tmp, create_indexed_gap, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-import-dedup-")
    conn = init_refine(client)
    try:
        from refine_server import project_state
        from refine_ui import api

        other = project_state.create_instance("External triage")
        create_indexed_gap(
            conn,
            "01IMPORTDEDUP0000000000001",
            instance_id=other["id"],
            status="review",
            priority="high",
        )
        create_indexed_gap(
            conn,
            "01IMPORTPROTECTED0000000001",
            status="todo",
            priority="high",
        )
        conn.commit()

        status, body = api.import_dedup({
            "drafts": [{
                "name": "Different name should not matter",
                "actual": "Current behavior for 01IMPORTDEDUP0000000000001",
                "target": "Target behavior for 01IMPORTDEDUP0000000000001",
                "reporter": "Reporter",
                "priority": "medium",
            }],
        })
        assert status == 200, body
        assert body["matches"], body
        match = body["matches"][0]
        assert match["index"] == 1, match
        assert match["score"] == 1.0, match
        assert match["match"]["id"] == "01IMPORTDEDUP0000000000001", match
        assert match["match"]["instance_id"] == other["id"], match
        assert match["match"]["instance_display_name"] == "External triage", match
        assert match["match"]["actual"].startswith("Current behavior"), match
        assert match["match"]["target"].startswith("Target behavior"), match

        status, body = api.import_dedup({
            "drafts": [
                {
                    "name": "Unrelated first row",
                    "actual": "A completely unrelated current behavior",
                    "target": "A completely unrelated future behavior",
                },
                {
                    "name": "Same CSV uploaded again",
                    "actual": "Current behavior for 01IMPORTDEDUP0000000000001",
                    "target": "Target behavior for 01IMPORTDEDUP0000000000001",
                    "reporter": "Reporter",
                    "priority": "medium",
                },
            ],
        })
        assert status == 200, body
        assert [m["index"] for m in body["matches"]] == [2], body
        assert body["matches"][0]["match"]["id"] == "01IMPORTDEDUP0000000000001", body

        score = api._import_dedup_score(  # noqa: SLF001
            "Login button is missing on the home page",
            "Users can sign in from the home page",
            "No login button appears on home page",
            "User can log in from homepage",
        )
        assert score >= api.IMPORT_DEDUP_THRESHOLD, score

        status, body = api.create_gap({
            "reporter": "Reporter",
            "actual": "Current behavior for 01IMPORTDEDUP0000000000001",
            "target": "Target behavior for 01IMPORTDEDUP0000000000001",
            "priority": "medium",
        })
        assert status == 409, body
        assert body["error"]["code"] == "duplicate_gap", body
        assert body["error"]["duplicate"]["match"]["id"] == "01IMPORTDEDUP0000000000001", body
        assert body["error"]["duplicate"]["match"]["can_move_to_backlog"] is True, body

        status, body = api.create_gap({
            "reporter": "Reporter",
            "actual": "Current behavior for 01IMPORTPROTECTED0000000001",
            "target": "Target behavior for 01IMPORTPROTECTED0000000001",
            "priority": "medium",
            "duplicate_decision": "move_original_to_backlog",
        })
        protected_status = conn.execute(
            "SELECT status FROM gaps_index WHERE id = '01IMPORTPROTECTED0000000001'",
        ).fetchone()["status"]
        assert status == 200, body
        assert body["created"] is False, body
        assert body["move"]["moved"] is False, body
        assert body["move"]["reason"] == "protected_status", body
        assert protected_status == "todo", protected_status

        status, body = api.create_gap({
            "reporter": "Reporter",
            "actual": "Current behavior for 01IMPORTDEDUP0000000000001",
            "target": "Target behavior for 01IMPORTDEDUP0000000000001",
            "priority": "medium",
            "duplicate_decision": "move_original_to_backlog",
        })
        moved_status = conn.execute(
            "SELECT status FROM gaps_index WHERE id = '01IMPORTDEDUP0000000000001'",
        ).fetchone()["status"]
        assert status == 200, body
        assert body["created"] is False, body
        assert body["move"]["moved"] is True, body
        assert body["move"]["from"] == "review", body
        assert moved_status == "backlog", moved_status

        class FakeClient:
            def __init__(self) -> None:
                self.calls: list[dict] = []

            def call(self, method: str, params: dict, *, timeout: float = 30.0):  # noqa: ANN201
                self.calls.append({"method": method, "params": params, "timeout": timeout})
                return {"gap": {"id": params["gap_id"]}}

        fake = FakeClient()
        original_get_client = api.get_client
        try:
            api.get_client = lambda: fake
            status, body = api.create_gap({
                "reporter": "Reporter",
                "actual": "Current behavior for 01IMPORTDEDUP0000000000001",
                "target": "Target behavior for 01IMPORTDEDUP0000000000001",
                "priority": "medium",
                "duplicate_decision": "original",
            })
        finally:
            api.get_client = original_get_client
        assert status == 201, body
        assert fake.calls and fake.calls[0]["params"]["priority"] == "medium", fake.calls

        status, body = api.import_dedup({
            "drafts": [{
                "name": "Original",
                "actual": "A completely unrelated current behavior",
                "target": "A completely unrelated future behavior",
            }],
        })
        assert status == 200, body
        assert body["matches"] == [], body

        status, body = api.import_persist({
            "reporter": "Reporter",
            "background": False,
            "drafts": [{
                "name": "Duplicate import",
                "actual": "Current behavior for 01IMPORTDEDUP0000000000001",
                "target": "Target behavior for 01IMPORTDEDUP0000000000001",
            }],
        })
        assert status == 200, body
        assert body["count"] == 0, body
        assert body["failures"][0]["code"] == "duplicate_gap", body
        assert body["failures"][0]["duplicate"]["match"]["id"] == "01IMPORTDEDUP0000000000001", body
        assert body["failures"][0]["duplicate"]["match"]["can_move_to_backlog"] is False, body
        before_count = conn.execute("SELECT COUNT(*) AS n FROM gaps_index").fetchone()["n"]
        status, body = api.import_persist({
            "reporter": "Reporter",
            "background": False,
            "drafts": [{
                "name": "Duplicate import",
                "actual": "Current behavior for 01IMPORTDEDUP0000000000001",
                "target": "Target behavior for 01IMPORTDEDUP0000000000001",
                "duplicate_decision": "duplicate",
            }],
        })
        after_count = conn.execute("SELECT COUNT(*) AS n FROM gaps_index").fetchone()["n"]
        assert status == 200, body
        assert body["count"] == 0, body
        assert body["failed"] == 0, body
        assert after_count == before_count, (before_count, after_count, body)
        assert body["duplicate_actions"]["ignored"] == 1, body

        conn.execute(
            "UPDATE gaps_index SET status = 'failed' "
            "WHERE id = '01IMPORTDEDUP0000000000001'",
        )
        conn.commit()
        status, body = api.import_persist({
            "reporter": "Reporter",
            "background": False,
            "drafts": [{
                "name": "Duplicate import",
                "actual": "Current behavior for 01IMPORTDEDUP0000000000001",
                "target": "Target behavior for 01IMPORTDEDUP0000000000001",
                "duplicate_decision": "move_original_to_backlog",
            }],
        })
        moved_status = conn.execute(
            "SELECT status FROM gaps_index WHERE id = '01IMPORTDEDUP0000000000001'",
        ).fetchone()["status"]
        assert status == 200, body
        assert body["count"] == 0, body
        assert body["failed"] == 0, body
        assert body["duplicate_actions"]["moved_to_backlog"] == 1, body
        assert moved_status == "backlog", moved_status

        restored = api._rollback_import_duplicate_moves([{  # noqa: SLF001
            "gap_id": "01IMPORTDEDUP0000000000001",
            "from": "failed",
        }])
        restored_status = conn.execute(
            "SELECT status FROM gaps_index WHERE id = '01IMPORTDEDUP0000000000001'",
        ).fetchone()["status"]
        assert restored == 1, restored
        assert restored_status == "failed", restored_status

        status, body = api.import_parse_csv({
            "background": True,
            "dedup": True,
            "text": (
                "actual,target,reporter,priority,name\n"
                "Current behavior for 01IMPORTDEDUP0000000000001,"
                "Target behavior for 01IMPORTDEDUP0000000000001,"
                "Reporter,low,Prepared duplicate\n"
            ),
        })
        assert status == 202, body
        prepared = wait_job(body["job"]["id"])
        assert prepared["http_status"] == 200, prepared
        assert prepared["count"] == 1, prepared
        assert prepared["drafts"][0]["duplicate"]["id"] == "01IMPORTDEDUP0000000000001", prepared
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("import dedup tests OK")
    return 0


def wait_job(job_id: str, *, timeout: float = 10) -> dict:
    import time
    from refine_ui import background_jobs

    deadline = time.time() + timeout
    while time.time() < deadline:
        job = background_jobs.snapshot(job_id)
        if job and job["status"] == "complete":
            return job["result"]
        if job and job["status"] in {"failed", "cancelled"}:
            raise AssertionError(job)
        time.sleep(0.02)
    raise AssertionError(f"job did not finish: {job_id}")


if __name__ == "__main__":
    raise SystemExit(main())
