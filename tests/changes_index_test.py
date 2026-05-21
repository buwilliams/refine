"""Changes screen merge history is projected into SQLite."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, git, init_refine, make_client_repo


def merge_gap(client: Path, gap_id: str, branch: str, filename: str) -> None:
    git(client, "checkout", "-q", "-b", branch, "main")
    (client / filename).write_text(f"{gap_id}\n", encoding="utf-8")
    git(client, "add", filename)
    git(client, "commit", "-m", f"work for {gap_id}")
    git(client, "checkout", "-q", "main")
    git(
        client,
        "merge",
        "--no-ff",
        branch,
        "-m",
        f"Merge {branch}\n\nRefine Gap: {gap_id}",
    )


def main() -> int:
    tmp, client = make_client_repo("refine-changes-index-")
    conn = init_refine(client)
    try:
        from refine_server import changes_index, gap_writer, git_ops, project_state
        from refine_server.runner import Runner

        gid_done = "01CHANGESINDEXDONEAAAAAAAA"
        gid_failed = "01CHANGESINDEXFAILEDAAAAAA"
        create_indexed_gap(conn, gid_done, status="done", priority="high")
        create_indexed_gap(conn, gid_failed, status="failed", priority="low")
        gap_writer.update_fields(
            gid_done,
            name="Payment merge cache",
            status="done",
            priority="high",
        )
        gap_writer.update_fields(
            gid_failed,
            name="Search miss cache",
            status="failed",
            priority="low",
        )
        merge_gap(client, gid_done, "refine/change-done", "payment.txt")
        merge_gap(client, gid_failed, "refine/change-failed", "search.txt")

        project_state.rebuild_sqlite_cache(conn)
        indexed = conn.execute("SELECT COUNT(*) AS n FROM refine_merges").fetchone()
        assert indexed["n"] == 2, indexed["n"]

        all_rows = changes_index.list_changes(
            conn, "main", limit=10, offset=0,
        )
        assert {row["gap_id"] for row in all_rows} == {gid_done, gid_failed}

        status_rows = changes_index.list_changes(
            conn, "main", limit=10, offset=0, status="done",
        )
        assert [row["gap_id"] for row in status_rows] == [gid_done]
        assert status_rows[0]["name"] == "Payment merge cache"
        assert status_rows[0]["priority"] == "high"

        priority_rows = changes_index.list_changes(
            conn, "main", limit=10, offset=0, priority="low",
        )
        assert [row["gap_id"] for row in priority_rows] == [gid_failed]

        search_rows = changes_index.list_changes(
            conn, "main", limit=10, offset=0, q="payment",
        )
        assert [row["gap_id"] for row in search_rows] == [gid_done]

        original_list_all = git_ops.list_all_refine_merges
        original_list_page = git_ops.list_refine_merges

        def fail_git_history_scan(*_args, **_kwargs):
            raise AssertionError("Changes reads must use SQLite, not git log")

        git_ops.list_all_refine_merges = fail_git_history_scan
        git_ops.list_refine_merges = fail_git_history_scan
        runner = Runner()
        try:
            result = runner._h_list_changes({
                "limit": 10,
                "offset": 0,
                "q": "payment",
                "status": "done",
                "priority": "high",
            })
        finally:
            runner._conn.close()
            git_ops.list_all_refine_merges = original_list_all
            git_ops.list_refine_merges = original_list_page
        assert [row["gap_id"] for row in result["changes"]] == [gid_done]
        assert result["page"]["has_more"] is False
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("changes index tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
