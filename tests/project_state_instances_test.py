"""Canonical JSON state, rebuildable cache, and instance ownership tests."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-project-state-")
    conn = init_refine(client)
    try:
        from refine_server import db, gap_writer, gaps, project_state, reporters

        root = client / ".refine"
        assert (root / "config.json").is_file()
        assert (root / "instances.json").is_file()
        active = project_state.active_instance_id()
        assert active == "default"

        reporters.add(conn, "Jane")
        gid = "01PROJECTSTATECACHEAAAAAA"
        gap_writer.create_gap(
            gap_id=gid,
            name="Cache rebuild",
            initial_round=gaps.new_round("Jane", "Actual", "Target"),
            status="todo",
            priority="high",
            instance_id=active,
        )

        conn.close()
        for suffix in ("", "-wal", "-shm"):
            try:
                (root / f"index.sqlite{suffix}").unlink()
            except FileNotFoundError:
                pass
        db.init_db()
        conn = db.connect()
        row = conn.execute(
            "SELECT status, priority, reporter, instance_id FROM gaps_index WHERE id = ?",
            (gid,),
        ).fetchone()
        assert row["status"] == "todo"
        assert row["priority"] == "high"
        assert row["reporter"] == "Jane"
        assert row["instance_id"] == active
        assert reporters.list_all(conn)[0]["name"] == "Jane"

        laptop = project_state.create_instance("Laptop")
        result = project_state.transfer_gaps(active, laptop["id"])
        assert result["updated"] == 1, result
        project_state.rebuild_sqlite_cache(conn)
        row = conn.execute(
            "SELECT instance_id FROM gaps_index WHERE id = ?", (gid,),
        ).fetchone()
        assert row["instance_id"] == laptop["id"]

        blocked = "01PROJECTSTATESKIPAAAAAAA"
        gap_writer.create_gap(
            gap_id=blocked,
            name="Skip in progress",
            initial_round=gaps.new_round("Jane", "Actual", "Target"),
            status="in-progress",
            priority="medium",
            instance_id=laptop["id"],
        )
        result = project_state.transfer_gaps(laptop["id"], active)
        assert result["updated"] == 1, result
        assert result["skipped"] == 1, result
        assert result["skipped_details"][0]["reason"] == "status:in-progress"
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("project state and instances tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
