"""Canonical JSON state, rebuildable cache, and node ownership tests."""
from __future__ import annotations

import json
import os
import sys
import shutil
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, git, init_refine, make_client_repo


def assert_refine_gitignore(root: Path) -> None:
    lines = (root / ".gitignore").read_text(encoding="utf-8").splitlines()
    for expected in (
        "index.sqlite", "index.sqlite-shm", "index.sqlite-wal",
        "app.log", "app.pid", "logs/", "gaps/**/logs.jsonl",
    ):
        assert expected in lines, lines
    assert "run/" not in lines, lines


def main() -> int:
    tmp, client = make_client_repo("refine-project-state-")
    conn = init_refine(client)
    try:
        from refine_server import config, db, gap_writer, gaps, project_state, reporters
        from refine_ui import api

        root = client / ".refine"
        assert (root / "config.json").is_file()
        assert (root / "nodes.json").is_file()
        assert_refine_gitignore(root)
        active = project_state.active_node_id()
        assert active == "default"

        project_state.write_maintenance({"reason": "test maintenance"}, root=root)
        status, body = api.create_node({"display_name": "Blocked"})
        assert status == 409, body
        assert "maintenance" in body["error"]["message"].lower()
        project_state.clear_maintenance(root=root)

        reporters.add(conn, "Jane")
        gid = "01PROJECTSTATECACHEAAAAAA"
        gap_writer.create_gap(
            gap_id=gid,
            name="Cache rebuild",
            initial_round=gaps.new_round("Jane", "Actual", "Target"),
            status="todo",
            priority="high",
            node_id=active,
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
            "SELECT status, priority, reporter, node_id FROM gaps_index WHERE id = ?",
            (gid,),
        ).fetchone()
        assert row["status"] == "todo"
        assert row["priority"] == "high"
        assert row["reporter"] == "Jane"
        assert row["node_id"] == active
        assert reporters.list_all(conn)[0]["name"] == "Jane"

        db.set_setting(conn, "paused", "1")
        db.set_setting(conn, "agents_paused", "1")
        db.set_setting(conn, "quality_timing", "post_rebuild")
        config_settings = json.loads(
            (root / "config.json").read_text(encoding="utf-8")
        )["settings"]
        assert config_settings["quality_timing"] == "post_rebuild"
        assert project_state.list_settings()["paused"] == "1"
        assert project_state.list_settings()["agents_paused"] == "1"
        assert project_state.list_settings()["quality_timing"] == "post_rebuild"
        project_state.rebuild_sqlite_cache(conn)
        assert db.get_setting(conn, "quality_timing") == "post_rebuild"

        runtime_path = root / "nodes" / active / "runtime.json"
        runtime = json.loads(runtime_path.read_text(encoding="utf-8"))
        runtime.pop("backlog_promote_after_seconds", None)
        runtime_path.write_text(json.dumps(runtime), encoding="utf-8")
        conn.execute(
            "INSERT INTO settings(key, value) VALUES(?, ?) "
            "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            ("backlog_promote_after_seconds", "-1"),
        )
        conn.execute(
            "DELETE FROM settings WHERE key = ?",
            (project_state.CACHE_ACTIVE_NODE_KEY,),
        )
        project_state.ensure_initialized(conn)
        runtime = json.loads(runtime_path.read_text(encoding="utf-8"))
        assert runtime["backlog_promote_after_seconds"] == "-1"
        project_state.rebuild_sqlite_cache(conn)
        assert db.get_setting(conn, "backlog_promote_after_seconds") == "-1"

        cfg = json.loads((root / "config.json").read_text(encoding="utf-8"))
        cfg["settings"].pop("quality_timing", None)
        (root / "config.json").write_text(json.dumps(cfg), encoding="utf-8")
        db.set_setting(conn, "quality_timing", "post_rebuild", persist=False)
        project_state.ensure_initialized(conn)
        config_settings = json.loads(
            (root / "config.json").read_text(encoding="utf-8")
        )["settings"]
        assert config_settings["quality_timing"] == "post_rebuild"

        assert project_state.resume_agents_for_startup(conn) is True
        assert db.get_setting(conn, "paused") == "0"
        assert db.get_setting(conn, "agents_paused") == "0"
        assert project_state.list_settings()["paused"] == "0"
        assert project_state.list_settings()["agents_paused"] == "0"
        assert project_state.resume_agents_for_startup(conn) is False

        reporters.add(conn, "Alex")

        laptop = project_state.create_node("Laptop")
        old_local_node = os.environ.get("REFINE_LOCAL_NODE_ID")
        try:
            os.environ["REFINE_LOCAL_NODE_ID"] = laptop["id"]
            assert project_state.local_node_id() == laptop["id"]
            os.environ["REFINE_LOCAL_NODE_ID"] = "missing-node"
            assert project_state.local_node_id() == active
        finally:
            if old_local_node is None:
                os.environ.pop("REFINE_LOCAL_NODE_ID", None)
            else:
                os.environ["REFINE_LOCAL_NODE_ID"] = old_local_node
        result = project_state.transfer_gaps(active, laptop["id"])
        assert result["updated"] == 1, result
        project_state.rebuild_sqlite_cache(conn)
        row = conn.execute(
            "SELECT node_id FROM gaps_index WHERE id = ?", (gid,),
        ).fetchone()
        assert row["node_id"] == laptop["id"]

        blocked = "01PROJECTSTATESKIPAAAAAAA"
        gap_writer.create_gap(
            gap_id=blocked,
            name="Skip in progress",
            initial_round=gaps.new_round("Jane", "Actual", "Target"),
            status="in-progress",
            priority="medium",
            node_id=laptop["id"],
        )
        result = project_state.transfer_gaps(laptop["id"], active)
        assert result["updated"] == 1, result
        assert result["skipped"] == 1, result
        assert result["skipped_details"][0]["reason"] == "status:in-progress"

        failed_active = "01PROJECTSTATEDASHFAILAAA"
        failed_laptop = "01PROJECTSTATEDASHFAILBBB"
        done_laptop = "01PROJECTSTATEDASHDONEBBB"
        alex_done_laptop = "01PROJECTSTATEDASHDONECCC"
        gap_writer.create_gap(
            gap_id=failed_active,
            name="Active failed",
            initial_round=gaps.new_round("Jane", "Actual", "Target"),
            status="failed",
            priority="medium",
            node_id=active,
        )
        gap_writer.create_gap(
            gap_id=failed_laptop,
            name="Laptop failed",
            initial_round=gaps.new_round("Jane", "Actual", "Target"),
            status="failed",
            priority="medium",
            node_id=laptop["id"],
        )
        gap_writer.create_gap(
            gap_id=done_laptop,
            name="Laptop done",
            initial_round=gaps.new_round("Jane", "Actual", "Target"),
            status="done",
            priority="medium",
            node_id=laptop["id"],
        )
        gap_writer.create_gap(
            gap_id=alex_done_laptop,
            name="Laptop Alex done",
            initial_round=gaps.new_round("Alex", "Actual", "Target"),
            status="done",
            priority="medium",
            node_id=laptop["id"],
        )
        project_state.rebuild_sqlite_cache(conn)
        status, dash_current = api.dashboard_summary()
        assert status == 200, dash_current
        assert dash_current["node_scope"] == "current", dash_current
        assert dash_current["counts"].get("failed") == 1, dash_current
        assert dash_current["counts"].get("in-progress", 0) == 0, dash_current
        jane_current = next(
            r for r in dash_current["reporter_stats"] if r["reporter"] == "Jane"
        )
        assert jane_current["reported"] == 2, dash_current
        assert all(
            r["reporter"] != "Alex" for r in dash_current["reporter_stats"]
        ), dash_current
        assert dash_current["needs_attention"][-1]["filter"] == {
            "status": "failed",
            "node": "current",
        }
        status, dash_all = api.dashboard_summary(node="all")
        assert status == 200, dash_all
        assert dash_all["node_scope"] == "all", dash_all
        assert dash_all["counts"].get("failed") == 2, dash_all
        assert dash_all["counts"].get("in-progress") == 1, dash_all
        jane_all = next(
            r for r in dash_all["reporter_stats"] if r["reporter"] == "Jane"
        )
        assert jane_all["reported"] == 5, dash_all
        alex_all = next(
            r for r in dash_all["reporter_stats"] if r["reporter"] == "Alex"
        )
        assert alex_all["reported"] == 1, dash_all
        assert alex_all["done"] == 1, dash_all
        assert dash_all["needs_attention"][-1]["filter"] == {
            "status": "failed",
            "node": "all",
        }
        status, body = api.list_gaps(status="failed", node="current")
        assert status == 200, body
        assert [g["id"] for g in body["gaps"]] == [failed_active], body
        status, body = api.list_gaps(status="failed", node="all")
        assert status == 200, body
        assert {g["id"] for g in body["gaps"]} == {
            failed_active,
            failed_laptop,
        }, body

        stale_gap = "01PROJECTSTATESTALECACHEAA"
        gap_writer.create_gap(
            gap_id=stale_gap,
            name="Stale projection",
            initial_round=gaps.new_round("Jane", "Actual", "Target"),
            status="todo",
            priority="medium",
            node_id=active,
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
            status, body = api.list_nodes()
            assert status == 200, body

            original_get_client = api.get_client

            def fail_get_client():
                raise AssertionError("dashboard should not block on backend client")

            api.get_client = fail_get_client
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

        inc_a = "01PROJECTSTATEINCAAAAAAAA"
        inc_b = "01PROJECTSTATEINCBBBBBBBB"
        inc_c = "01PROJECTSTATEINCCCCCCCCC"
        for inc_gid in (inc_a, inc_b, inc_c):
            gap_writer.create_gap(
                gap_id=inc_gid,
                name=f"Incremental {inc_gid[-1]}",
                initial_round=gaps.new_round("Jane", "Actual", "Target"),
                status="todo",
                priority="low",
                node_id=active,
            )
        project_state.rebuild_sqlite_cache(conn)
        meta_count = conn.execute(
            "SELECT COUNT(*) AS n FROM gap_cache_meta WHERE gap_id IN (?, ?, ?)",
            (inc_a, inc_b, inc_c),
        ).fetchone()["n"]
        assert meta_count == 3, meta_count

        original_read_cache_bytes = project_state._read_gap_cache_bytes
        cache_reads: list[str] = []

        def counted_cache_read(path: Path) -> bytes:
            cache_reads.append(path.relative_to(root).as_posix())
            return original_read_cache_bytes(path)

        project_state._read_gap_cache_bytes = counted_cache_read
        try:
            project_state.rebuild_sqlite_cache(conn)
            assert cache_reads == [], cache_reads

            gap_writer.update_fields(inc_b, status="review")
            project_state.rebuild_sqlite_cache(conn)
            expected_changed = f"gaps/{inc_b[:2]}/{inc_b[2:]}/gap.json"
            assert cache_reads == [expected_changed], cache_reads
            row = conn.execute(
                "SELECT status FROM gaps_index WHERE id = ?", (inc_b,),
            ).fetchone()
            assert row["status"] == "review", dict(row)

            cache_reads.clear()
            conn.execute("DELETE FROM gaps_index WHERE id = ?", (inc_a,))
            project_state.rebuild_sqlite_cache(conn)
            expected_restored = f"gaps/{inc_a[:2]}/{inc_a[2:]}/gap.json"
            assert cache_reads == [expected_restored], cache_reads
            row = conn.execute(
                "SELECT status FROM gaps_index WHERE id = ?", (inc_a,),
            ).fetchone()
            assert row["status"] == "todo", dict(row)

            cache_reads.clear()
            (root / f"gaps/{inc_c[:2]}/{inc_c[2:]}/gap.json").unlink()
            project_state.rebuild_sqlite_cache(conn)
            assert cache_reads == [], cache_reads
            row = conn.execute(
                "SELECT id FROM gaps_index WHERE id = ?", (inc_c,),
            ).fetchone()
            assert row is None
            row = conn.execute(
                "SELECT gap_id FROM gap_cache_meta WHERE gap_id = ?", (inc_c,),
            ).fetchone()
            assert row is None
        finally:
            project_state._read_gap_cache_bytes = original_read_cache_bytes

        conn.close()
        cfg = config.get(reload=True)
        cfg.sqlite_path.parent.mkdir(parents=True, exist_ok=True)
        cfg.sqlite_path.write_bytes(b"not a sqlite database")
        for suffix in ("-wal", "-shm"):
            try:
                Path(f"{cfg.sqlite_path}{suffix}").unlink()
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

        archived = project_state.create_node("Archived")
        project_state.update_node(archived["id"], archived=True)
        try:
            project_state.set_active_node(archived["id"])
            raise AssertionError("archived node should not activate")
        except ValueError as e:
            assert "archived node" in str(e)
        assert project_state.active_node_id() == active

        try:
            project_state.transfer_gaps(None, archived["id"])
            raise AssertionError("archived node should not receive transfers")
        except ValueError as e:
            assert "archived target" in str(e)

        status, body = api.activate_node({"node_id": archived["id"]})
        assert status == 400, body
        assert "archived node" in body["error"]["message"]
        status, body = api.transfer_node_gaps({
            "target_node_id": archived["id"],
            "filter": {"node": active},
        })
        assert status == 400, body
        assert "archived target" in body["error"]["message"]

        runtime_paths = [
            ".refine/index.sqlite",
            ".refine/index.sqlite-shm",
            ".refine/index.sqlite-wal",
            ".refine/app.pid",
            ".refine/app.log",
            ".refine/gaps/01/PROJECTSTATELOGNOISE/logs.jsonl",
        ]
        for rel in runtime_paths:
            p = client / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            if not p.exists():
                p.write_text("tracked cache\n", encoding="utf-8")
        git(client, "add", "-f", *runtime_paths)
        git(client, "commit", "-m", "track runtime cache")
        assert set(git(client, "ls-files", *runtime_paths).stdout.splitlines()) == set(runtime_paths)

        (root / ".gitignore").unlink()
        (root / "config.json").unlink()
        (root / "nodes.json").unlink()
        shutil.rmtree(root / "nodes", ignore_errors=True)
        project_state.ensure_initialized(conn, migrate=True)
        assert_refine_gitignore(root)
        from refine_ui.api import _commit_refine_state

        _commit_refine_state(client)
        assert git(client, "ls-files", *runtime_paths).stdout.strip() == ""
        for rel in runtime_paths:
            assert (client / rel).exists(), rel
        head = git(client, "rev-parse", "HEAD").stdout.strip()
        for rel in runtime_paths[3:]:
            (client / rel).write_text("new runtime noise\n", encoding="utf-8")
        _commit_refine_state(client)
        assert git(client, "rev-parse", "HEAD").stdout.strip() == head

        cfg_path = root / "config.json"
        cfg = json.loads(cfg_path.read_text(encoding="utf-8"))
        cfg["schema_version"] = 1
        cfg_path.write_text(json.dumps(cfg, indent=2), encoding="utf-8")
        if (root / "instances.json").exists():
            (root / "instances.json").unlink()
        if (root / "nodes.json").exists():
            (root / "nodes.json").rename(root / "instances.json")
        shutil.rmtree(root / "instances", ignore_errors=True)
        if (root / "nodes").exists():
            (root / "nodes").rename(root / "instances")
        legacy_registry = json.loads(
            (root / "instances.json").read_text(encoding="utf-8")
        )
        extra_legacy_ids = {"worker-alpha", "worker-beta"}
        present = {str(entry.get("id") or "") for entry in legacy_registry["nodes"]}
        for node_id in sorted(extra_legacy_ids - present):
            legacy_registry["nodes"].append({
                "id": node_id,
                "display_name": node_id.replace("-", " ").title(),
                "created_at": "2026-06-01T15:00:00Z",
                "updated_at": "2026-06-01T15:00:00Z",
                "archived": False,
            })
            d = root / "instances" / node_id
            d.mkdir(parents=True, exist_ok=True)
            (d / "application.json").write_text(
                json.dumps({"merge_target_branch": node_id}, indent=2),
                encoding="utf-8",
            )
            (d / "runtime.json").write_text("{}", encoding="utf-8")
            (d / "target-app.json").write_text(
                json.dumps({"target_app_cwd": f"apps/{node_id}"}, indent=2),
                encoding="utf-8",
            )
            (d / "reporters.json").write_text('{"reporters": []}', encoding="utf-8")
        (root / "instances.json").write_text(
            json.dumps(legacy_registry, indent=2),
            encoding="utf-8",
        )
        legacy_node_ids = {
            str(entry["id"])
            for entry in legacy_registry["nodes"]
            if entry.get("id")
        }
        assert {active, *extra_legacy_ids}.issubset(legacy_node_ids)
        legacy_default_target = root / "instances" / active / "target-app.json"
        default_target = json.loads(
            legacy_default_target.read_text(encoding="utf-8")
        )
        default_target["target_app_start_command"] = "./linux-docker.sh start"
        default_target["target_app_rebuild_command"] = "./linux-docker.sh build"
        legacy_default_target.write_text(
            json.dumps(default_target, indent=2),
            encoding="utf-8",
        )
        # Simulate an old worker touching v2 node state before the manual
        # instance-to-node migration. The migration must recover from this
        # polluted partial v2 layout instead of treating it as authoritative.
        (root / "nodes").mkdir()
        (root / "nodes" / active).mkdir()
        (root / "nodes.json").write_text(
            json.dumps({
                "nodes": [{
                    "id": active,
                    "display_name": "Default",
                    "created_at": "2026-06-01T15:59:53Z",
                    "updated_at": "2026-06-01T15:59:53Z",
                    "archived": False,
                }],
            }, indent=2),
            encoding="utf-8",
        )
        for name in (
            "application.json",
            "runtime.json",
            "target-app.json",
            "reporters.json",
        ):
            (root / "nodes" / active / name).write_text(
                "{}" if name != "reporters.json" else '{"reporters": []}',
                encoding="utf-8",
            )
        status = project_state.schema_status(root)
        assert status["migration_id"] == "instance_to_node_v2"
        assert status["safe_auto"] is False
        try:
            project_state.active_node_id(root=root)
            raise AssertionError("v1 schema should block node state writes")
        except RuntimeError as e:
            assert "refine migrate run" in str(e)
        try:
            project_state.resume_agents_for_startup(conn)
            raise AssertionError("v1 schema should block worker startup")
        except RuntimeError as e:
            assert "refine migrate run" in str(e)
        polluted_registry = json.loads(
            (root / "nodes.json").read_text(encoding="utf-8")
        )
        assert [entry["id"] for entry in polluted_registry["nodes"]] == [active]
        project_state.ensure_initialized(conn, migrate=True, root=root)
        assert json.loads(cfg_path.read_text(encoding="utf-8"))["schema_version"] == 1
        try:
            project_state.rebuild_sqlite_cache(conn)
            raise AssertionError("manual migration unexpectedly rebuilt cache")
        except RuntimeError as e:
            assert "refine migrate run" in str(e)
        project_state.ensure_initialized(
            conn,
            migrate=True,
            allow_manual_migrations=True,
            root=root,
        )
        assert (root / "nodes.json").exists()
        assert not (root / "instances.json").exists()
        assert not (root / "instances").exists()
        migrated_registry = json.loads(
            (root / "nodes.json").read_text(encoding="utf-8")
        )
        migrated_node_ids = {
            str(entry["id"])
            for entry in migrated_registry["nodes"]
            if entry.get("id")
        }
        assert migrated_node_ids == legacy_node_ids
        migrated_default_target = json.loads(
            (root / "nodes" / active / "target-app.json").read_text(encoding="utf-8")
        )
        assert (
            migrated_default_target["target_app_start_command"]
            == "./linux-docker.sh start"
        )
        assert (
            migrated_default_target["target_app_rebuild_command"]
            == "./linux-docker.sh build"
        )
        assert json.loads(cfg_path.read_text(encoding="utf-8"))["schema_version"] == 2
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("project state and nodes tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
