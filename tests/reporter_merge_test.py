"""Reporter merge behavior."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def _create_gap_for_reporter(conn, gap_id: str, reporter: str) -> None:
    from refine_server import gap_writer, gaps, project_state
    from refine_server.paths import relative_gap_path

    active_instance = project_state.active_instance_id()
    gap = gap_writer.create_gap(
        gap_id=gap_id,
        name=gap_id,
        initial_round=gaps.new_round(
            reporter,
            f"Current behavior for {gap_id}",
            f"Target behavior for {gap_id}",
        ),
        status="todo",
        priority="medium",
        instance_id=active_instance,
    )
    conn.execute(
        "INSERT INTO gaps_index "
        "(id, name, status, priority, reporter, created, updated, "
        "branch_name, instance_id, json_path) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, ?)",
        (
            gap_id,
            gap_id,
            "todo",
            "medium",
            reporter,
            gap["created"],
            gap["updated"],
            active_instance,
            relative_gap_path(gap_id),
        ),
    )


def test_runner_merges_reporter_and_removes_source() -> None:
    tmp, client = make_client_repo("refine-reporter-merge-")
    conn = init_refine(client)
    try:
        from refine_server import gap_writer, gaps, reporters
        from refine_server.runner import Runner

        source = reporters.add(conn, "William")
        target = reporters.add(conn, "Buddy Williams")
        _create_gap_for_reporter(conn, "reporter-merge-source", "William")
        gap_writer.append_round(
            "reporter-merge-source",
            gaps.new_round("William", "Still wrong", "Still right"),
        )
        _create_gap_for_reporter(conn, "reporter-merge-target", "Buddy Williams")

        runner = Runner()
        try:
            result = runner._h_merge_reporter({  # noqa: SLF001
                "rid": source["id"],
                "target_rid": target["id"],
            })
        finally:
            runner._conn.close()  # noqa: SLF001

        assert result["old"] == "William"
        assert result["new"] == "Buddy Williams"
        assert result["removed_id"] == source["id"]
        assert result["touched"] == 1

        names = [r["name"] for r in reporters.list_all(conn)]
        assert names == ["Buddy Williams"], names

        merged = gaps.read_gap_json("reporter-merge-source", include_logs=False)
        assert merged is not None
        assert [r["reporter"] for r in merged["rounds"]] == [
            "Buddy Williams",
            "Buddy Williams",
        ]
        row = conn.execute(
            "SELECT reporter FROM gaps_index WHERE id = ?",
            ("reporter-merge-source",),
        ).fetchone()
        assert row["reporter"] == "Buddy Williams"

        untouched = gaps.read_gap_json("reporter-merge-target", include_logs=False)
        assert untouched is not None
        assert untouched["rounds"][0]["reporter"] == "Buddy Williams"
    finally:
        conn.close()
        cleanup_tmp(tmp)


def test_api_merge_reporter_routes_to_runner() -> None:
    from refine_server.backend_protocol import M_MERGE_REPORTER
    from refine_ui import api

    calls = []

    class FakeClient:
        def call(self, method, params, timeout=None):
            calls.append((method, params, timeout))
            return {
                "old": "William",
                "new": "Buddy Williams",
                "removed_id": params["rid"],
                "touched": 3,
            }

    original_get_client = api.get_client
    try:
        api.get_client = lambda: FakeClient()
        status, body = api.merge_reporter(12, {"target_id": "34"})
    finally:
        api.get_client = original_get_client

    assert status == 200, body
    assert body["ok"] is True
    assert body["touched"] == 3
    assert calls == [
        (M_MERGE_REPORTER, {"rid": 12, "target_rid": 34}, 60.0),
    ]


def main() -> int:
    test_runner_merges_reporter_and_removes_source()
    test_api_merge_reporter_routes_to_runner()
    print("reporter merge tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
