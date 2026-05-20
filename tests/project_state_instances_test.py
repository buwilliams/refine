"""Canonical JSON state, rebuildable cache, and instance ownership tests."""
from __future__ import annotations

import sys
import shutil
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, git, init_refine, make_client_repo


def assert_refine_gitignore(root: Path) -> None:
    lines = (root / ".gitignore").read_text(encoding="utf-8").splitlines()
    for expected in ("index.sqlite", "index.sqlite-shm", "index.sqlite-wal"):
        assert expected in lines, lines
    assert "run/" not in lines, lines


def main() -> int:
    tmp, client = make_client_repo("refine-project-state-")
    conn = init_refine(client)
    try:
        from refine_server import db, gap_writer, gaps, project_state, reporters
        from refine_ui import api

        root = client / ".refine"
        assert (root / "config.json").is_file()
        assert (root / "instances.json").is_file()
        assert_refine_gitignore(root)
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

        stale_gap = "01PROJECTSTATESTALECACHEAA"
        gap_writer.create_gap(
            gap_id=stale_gap,
            name="Stale projection",
            initial_round=gaps.new_round("Jane", "Actual", "Target"),
            status="todo",
            priority="medium",
            instance_id=active,
        )
        project_state.rebuild_sqlite_cache(conn)
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (stale_gap,),
        ).fetchone()
        assert row["status"] == "todo", dict(row)
        # Simulate another Refine process writing canonical JSON while this
        # process's SQLite projection still has the old status.
        gap_writer.update_fields(stale_gap, status="failed")
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (stale_gap,),
        ).fetchone()
        assert row["status"] == "todo", dict(row)
        original_fingerprint = project_state.state_fingerprint

        def fail_state_fingerprint(*, root=None):
            raise AssertionError("routine cache checks must not scan Gap JSON")

        project_state.state_fingerprint = fail_state_fingerprint
        try:
            assert project_state.ensure_sqlite_cache_current(conn) == active
            status, body = api.list_settings()
            assert status == 200, body
            status, body = api.list_instances()
            assert status == 200, body

            original_get_client = api.get_client

            class DashboardClient:
                def call(self, method, params=None, *, timeout=30.0):
                    return {"running": [], "merger": None, "governance": None}

                def is_reachable(self):
                    return True

            api.get_client = lambda: DashboardClient()
            try:
                status, body = api.dashboard_summary()
                assert status == 200, body
            finally:
                api.get_client = original_get_client

            status, body = api.get_gap(stale_gap)
        finally:
            project_state.state_fingerprint = original_fingerprint
        assert status == 200, body
        assert body["gap"]["status"] == "todo", body
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (stale_gap,),
        ).fetchone()
        assert row["status"] == "todo", dict(row)
        project_state.rebuild_sqlite_cache(conn)
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (stale_gap,),
        ).fetchone()
        assert row["status"] == "failed", dict(row)

        conn.close()
        (root / "index.sqlite").write_bytes(b"not a sqlite database")
        for suffix in ("-wal", "-shm"):
            try:
                (root / f"index.sqlite{suffix}").unlink()
            except FileNotFoundError:
                pass
        status, body = api.rebuild_sqlite_cache({"restart_services": False})
        assert status == 200, body
        assert body["mode"] == "recreated", body
        assert body["gaps"] >= 3, body
        conn = db.connect()
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (stale_gap,),
        ).fetchone()
        assert row["status"] == "failed", dict(row)

        archived = project_state.create_instance("Archived")
        project_state.update_instance(archived["id"], archived=True)
        try:
            project_state.set_active_instance(archived["id"])
            raise AssertionError("archived instance should not activate")
        except ValueError as e:
            assert "archived instance" in str(e)
        assert project_state.active_instance_id() == active

        try:
            project_state.transfer_gaps(None, archived["id"])
            raise AssertionError("archived instance should not receive transfers")
        except ValueError as e:
            assert "archived target" in str(e)

        status, body = api.activate_instance({"instance_id": archived["id"]})
        assert status == 400, body
        assert "archived instance" in body["error"]["message"]
        status, body = api.transfer_instance_gaps({
            "target_instance_id": archived["id"],
            "filter": {"instance": active},
        })
        assert status == 400, body
        assert "archived target" in body["error"]["message"]

        sqlite_paths = [
            ".refine/index.sqlite",
            ".refine/index.sqlite-shm",
            ".refine/index.sqlite-wal",
        ]
        for rel in sqlite_paths:
            p = client / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            if not p.exists():
                p.write_text("tracked cache\n", encoding="utf-8")
        git(client, "add", "-f", *sqlite_paths)
        git(client, "commit", "-m", "track sqlite cache")
        assert set(git(client, "ls-files", *sqlite_paths).stdout.splitlines()) == set(sqlite_paths)

        (root / ".gitignore").unlink()
        (root / "config.json").unlink()
        (root / "instances.json").unlink()
        shutil.rmtree(root / "instances", ignore_errors=True)
        project_state.ensure_initialized(conn, migrate=True)
        assert_refine_gitignore(root)
        from refine_ui.api import _commit_refine_state

        _commit_refine_state(client)
        assert git(client, "ls-files", *sqlite_paths).stdout.strip() == ""
        for rel in sqlite_paths:
            assert (client / rel).exists(), rel
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
