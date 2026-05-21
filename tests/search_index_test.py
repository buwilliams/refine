"""Regression tests for SQLite FTS-backed search."""
from __future__ import annotations

from pathlib import Path

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-search-index-")
    conn = init_refine(client)
    try:
        from refine_server import activity, gap_writer, gaps, project_state
        from refine_ui import api

        first_id = "01SEARCHINDEXFIRSTAAAAAAA"
        second_id = "01SEARCHINDEXSECONDAAAAAA"
        gap_writer.create_gap(
            gap_id=first_id,
            name="Checkout polish",
            initial_round=gaps.new_round(
                "Ada",
                "Cart summary drops discount details",
                "Checkout summary keeps the aurora coupon explanation visible",
            ),
            status="todo",
            priority="medium",
        )
        gap_writer.create_gap(
            gap_id=second_id,
            name="Billing copy",
            initial_round=gaps.new_round(
                "Grace",
                "Invoices use old wording",
                "Invoices use updated wording",
            ),
            status="todo",
            priority="medium",
        )
        project_state.rebuild_sqlite_cache(conn)

        original_read_gap_json = api.shared_gaps.read_gap_json

        def fail_read_gap_json(gap_id: str):
            raise AssertionError(f"search read gap.json for {gap_id}")

        api.shared_gaps.read_gap_json = fail_read_gap_json
        try:
            code, body = api.list_gaps(q="aurora coupon", limit=20)
        finally:
            api.shared_gaps.read_gap_json = original_read_gap_json
        assert code == 200, body
        assert [g["id"] for g in body["gaps"]] == [first_id], body

        code, body = api.list_gaps(q="billing", limit=20)
        assert code == 200, body
        assert [g["id"] for g in body["gaps"]] == [second_id], body

        code, body = api.list_gaps(q=first_id[:16], limit=20)
        assert code == 200, body
        assert [g["id"] for g in body["gaps"]] == [first_id], body

        code, body = api.update_gap_name(first_id, {"name": "Nebula checkout"})
        assert code == 200, body
        code, body = api.list_gaps(q="nebula", limit=20)
        assert code == 200, body
        assert [g["id"] for g in body["gaps"]] == [first_id], body

        activity.append(
            conn,
            message="Dispatcher noticed a sapphire token limit",
            details="The provider paused because the context window was exhausted",
            category="runner",
            actor="refine",
        )
        activity.append(
            conn,
            message="Reporter renamed",
            details="No provider issue involved",
            category="state",
            actor="refine",
        )
        code, body = api.list_activity(q="sapphire token", limit=20)
        assert code == 200, body
        assert len(body["activity"]) == 1, body
        assert body["activity"][0]["message"].startswith("Dispatcher noticed"), body

        repo_root = Path(__file__).resolve().parents[1]
        api_py = repo_root / "refine_ui" / "api.py"
        activity_py = repo_root / "refine_server" / "activity.py"
        assert "_augment_with_round_search" not in api_py.read_text(encoding="utf-8")
        assert "read_gap_json(r[\"id\"])" not in api_py.read_text(encoding="utf-8")
        assert "message LIKE" not in activity_py.read_text(encoding="utf-8")
        assert "details LIKE" not in activity_py.read_text(encoding="utf-8")
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("search index tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
