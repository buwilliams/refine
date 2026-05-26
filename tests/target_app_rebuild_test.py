"""Automatic target-app rebuild queue and review gating tests."""
from __future__ import annotations

import json
import sys
from datetime import datetime, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-target-app-rebuild-")
    conn = init_refine(client)
    try:
        from refine_server import (
            db, gap_writer, gaps, runner as runner_mod,
            project_state, target_app, target_app_rebuilder,
        )
        from refine_server.backend_protocol import (
            M_TARGET_APP_REBUILD_PENDING,
            M_TARGET_APP_REBUILD_QUEUE,
        )
        from refine_server.gaps import now_iso
        from refine_ui import api

        runs: list[str] = []

        def run_rebuild(reason: str) -> dict:
            runs.append(reason)
            if len(runs) == 1:
                rebuilder.queue_rebuild("queued while running")
                rebuilder.queue_rebuild("duplicate while running")
            return {"ok": True}

        rebuilder = target_app_rebuilder.TargetAppRebuilder(
            get_conn=lambda: conn,
            run_rebuild=run_rebuild,
        )
        rebuilder.queue_rebuild("initial")
        rebuilder.queue_rebuild("duplicate before running")
        rebuilder._drain_queue()  # noqa: SLF001
        assert len(runs) == 2, runs

        assert rebuilder.queue_for_worktree_merge("01GAP") is False
        db.set_setting(conn, "target_app_auto_rebuild", "on_worktree_merge")
        db.set_setting(conn, "paused", "1")
        assert rebuilder.queue_rebuild("blocked while paused") is False
        assert rebuilder.queue_for_worktree_merge("01PAUSED") is False
        assert rebuilder.queue_pending_awaiting_rebuild() is False
        assert rebuilder.snapshot()["queued"] is False
        db.set_setting(conn, "paused", "0")
        assert rebuilder.queue_for_worktree_merge("01GAP") is True
        rebuilder._drain_queue()  # noqa: SLF001
        assert len(runs) == 3, runs
        assert rebuilder.queue_for_worktree_merge("02GAP") is True
        db.set_setting(conn, "target_app_auto_rebuild", "never")
        rebuilder._drain_queue()  # noqa: SLF001
        assert len(runs) == 3, runs
        db.set_setting(conn, "target_app_auto_rebuild", "on_worktree_merge")
        other_instance = project_state.create_instance("Remote Rebuild Host")
        gid_remote_pending = "01TARGETAPPREMOTEPENDINGA"
        create_indexed_gap(
            conn,
            gid_remote_pending,
            status="awaiting-rebuild",
            branch=None,
            instance_id=other_instance["id"],
        )
        assert rebuilder.queue_pending_awaiting_rebuild() is False
        gid_pending = "01TARGETAPPPENDINGREBUILDA"
        create_indexed_gap(conn, gid_pending, status="awaiting-rebuild", branch=None)
        assert rebuilder.queue_pending_awaiting_rebuild() is True
        assert rebuilder.queue_pending_awaiting_rebuild() is False
        rebuilder._drain_queue()  # noqa: SLF001
        assert runs[-1] == "1 Gap awaiting target-app rebuild", runs
        calls: list[str] = []

        class FakeClient:
            def call(self, method: str, params: dict | None = None, *, timeout: float = 30.0) -> dict:  # noqa: ARG002
                calls.append(method)
                return {"queued": True}

        old_get_client = api.get_client
        try:
            api.get_client = lambda: FakeClient()  # type: ignore[assignment]
            status, body = api.update_settings({
                "target_app_auto_rebuild": "on_worktree_merge",
            })
            assert status == 200, body
            status, body = api.target_app_rebuild_queue({})
            assert status == 202, body
        finally:
            api.get_client = old_get_client  # type: ignore[assignment]
        assert M_TARGET_APP_REBUILD_PENDING in calls, calls
        assert M_TARGET_APP_REBUILD_QUEUE in calls, calls

        class RebuildNoopClient:
            def call(self, method: str, params: dict | None = None, *, timeout: float = 30.0) -> dict:  # noqa: ARG002
                raise AssertionError("empty rebuild command should not call runner")

        db.set_setting(conn, "target_app_rebuild_command", "")
        db.set_setting(conn, "target_app_state", "running")
        db.set_setting(conn, "target_app_last_error", "old failure")
        try:
            api.get_client = lambda: RebuildNoopClient()  # type: ignore[assignment]
            status, body = api.target_app_rebuild({})
        finally:
            api.get_client = old_get_client  # type: ignore[assignment]
        assert status == 200, body
        assert body["ok"] is True, body
        assert body["noop"] is True, body
        assert body["state"] == "running", body
        assert body["promoted_gaps"] == 0, body
        assert "no-op" in body["message"], body
        assert db.get_setting(conn, "target_app_last_error") == ""
        noop_row = conn.execute(
            "SELECT message, severity FROM activity "
            "WHERE category = 'target_app' "
            "AND message LIKE 'target-app: rebuild skipped%' "
            "ORDER BY id DESC LIMIT 1"
        ).fetchone()
        assert noop_row is not None, body
        assert noop_row["severity"] == "info", dict(noop_row)

        conn.execute(
            "UPDATE gaps_index SET status = 'review' WHERE id = ?",
            (gid_pending,),
        )
        gap_writer.update_fields(gid_pending, status="review")
        db.set_setting(conn, "target_app_auto_rebuild", "hourly")
        db.set_setting(conn, "target_app_auto_rebuild_last_started_at", "")
        rebuilder._queue_scheduled_rebuild_if_due()  # noqa: SLF001
        rebuilder._drain_queue()  # noqa: SLF001
        assert len(runs) == 5, runs
        assert runs[-1] == "hourly automatic rebuild", runs
        target_settings = json.loads(
            (client / ".refine" / "instances" / "default" / "target-app.json")
            .read_text(encoding="utf-8")
        )
        assert target_settings["target_app_auto_rebuild"] == "hourly"

        db.set_setting(conn, "target_app_auto_rebuild", "nightly")
        db.set_setting(conn, "target_app_auto_rebuild_last_started_at", "")
        midnight = datetime.now().astimezone().replace(
            hour=0, minute=0, second=0, microsecond=0,
        )
        rebuilder._queue_scheduled_rebuild_if_due(  # noqa: SLF001
            midnight.replace(hour=12),
        )
        rebuilder._drain_queue()  # noqa: SLF001
        assert runs[-1] == "hourly automatic rebuild", runs
        db.set_setting(
            conn,
            "target_app_auto_rebuild_last_started_at",
            (midnight - timedelta(days=1)).isoformat(),
        )
        rebuilder._queue_scheduled_rebuild_if_due(midnight)  # noqa: SLF001
        rebuilder._drain_queue()  # noqa: SLF001
        assert runs[-1] == "nightly automatic rebuild", runs
        nightly_count = len(runs)
        db.set_setting(
            conn,
            "target_app_auto_rebuild_last_started_at",
            midnight.isoformat(),
        )
        rebuilder._queue_scheduled_rebuild_if_due(  # noqa: SLF001
            midnight.replace(hour=12),
        )
        rebuilder._drain_queue()  # noqa: SLF001
        assert len(runs) == nightly_count, runs

        db.set_setting(conn, "target_app_stop_command", "stop-app")
        db.set_setting(conn, "target_app_rebuild_command", "build-app")
        db.set_setting(conn, "target_app_start_command", "start-app")
        auto_runner = runner_mod.Runner()
        operations: list[str] = []
        old_run_operation = target_app.run_operation
        fail_rebuild = False

        def fake_run_operation(kind: str, cfg: dict) -> dict:
            operations.append(kind)
            if kind == "rebuild" and fail_rebuild:
                return {
                    "ok": False,
                    "kind": kind,
                    "state": "failed",
                    "command": cfg.get(f"{kind}_command") or "",
                    "cwd": "",
                    "exit_code": 127,
                    "stdout_tail": "",
                    "stderr_tail": "missing dependency: vite",
                    "message": "command exited 127",
                    "started_at": now_iso(),
                    "finished_at": now_iso(),
                    "checks_configured": False,
                    "checks": [],
                }
            return {
                "ok": True,
                "kind": kind,
                "state": "running" if kind == "start" else "stopped",
                "command": cfg.get(f"{kind}_command") or "",
                "cwd": "",
                "exit_code": 0,
                "stdout_tail": f"{kind} stdout",
                "stderr_tail": "",
                "message": f"{kind} ok",
                "started_at": now_iso(),
                "finished_at": now_iso(),
                "checks_configured": kind == "start",
                "checks": [{"ok": True}] if kind == "start" else [],
            }

        try:
            target_app.run_operation = fake_run_operation  # type: ignore[assignment]
            for kind in ("start", "stop", "rebuild"):
                result = auto_runner._h_target_app_run({  # noqa: SLF001
                    "kind": kind,
                    "config": {},
                })
                assert result["ok"], result
            manual_activity = [
                r["message"]
                for r in conn.execute(
                    "SELECT message FROM activity "
                    "WHERE category = 'target_app' AND actor = 'refine' "
                    "AND gap_id IS NULL ORDER BY id"
                )
            ]
            for kind in ("start", "stop", "rebuild"):
                assert any(
                    f"target-app: {kind} requested" in msg
                    for msg in manual_activity
                ), manual_activity
                assert any(
                    f"target-app: {kind} completed" in msg
                    for msg in manual_activity
                ), manual_activity
            operations.clear()

            gid_auto = "01TARGETAPPAUTOREBUILDAAA"
            create_indexed_gap(conn, gid_auto, status="awaiting-rebuild", branch=None)
            gap_writer.update_fields(gid_auto, status="awaiting-rebuild")
            result = auto_runner._run_automatic_target_app_rebuild("test sequence")  # noqa: SLF001
            assert result["ok"], result
            assert operations == ["stop", "rebuild", "start"], operations
            assert [step["kind"] for step in result["steps"]] == [
                "stop", "rebuild", "start",
            ], result
            row = conn.execute(
                "SELECT status FROM gaps_index WHERE id = ?", (gid_auto,),
            ).fetchone()
            assert row["status"] == "review", dict(row)
            row = conn.execute(
                "SELECT status FROM gaps_index WHERE id = ?", (gid_remote_pending,),
            ).fetchone()
            assert row["status"] == "awaiting-rebuild", dict(row)
            assert db.get_setting(conn, "target_app_auto_rebuild_last_ok") == "1"
            target_activity = [
                r["message"]
                for r in conn.execute(
                    "SELECT message FROM activity "
                    "WHERE category = 'target_app' AND actor = 'runner' "
                    "ORDER BY id"
                )
            ]
            assert any(
                "target-app: automatic stop completed" in msg
                for msg in target_activity
            ), target_activity
            assert any(
                "target-app: automatic rebuild completed" in msg
                for msg in target_activity
            ), target_activity
            assert any(
                "target-app: automatic start completed" in msg
                for msg in target_activity
            ), target_activity

            operations.clear()
            db.set_setting(conn, "target_app_rebuild_command", "")
            gid_noop = "01TARGETAPPNOOPREBUILDAA"
            create_indexed_gap(conn, gid_noop, status="awaiting-rebuild", branch=None)
            gap_writer.update_fields(gid_noop, status="awaiting-rebuild")
            result = auto_runner._run_automatic_target_app_rebuild("test no-op")  # noqa: SLF001
            assert result["ok"], result
            assert operations == ["stop", "start"], operations
            assert [step["kind"] for step in result["steps"]] == [
                "stop", "rebuild", "start",
            ], result
            assert "no-op" in result["steps"][1]["message"], result
            noop_activity = conn.execute(
                "SELECT message FROM activity "
                "WHERE category = 'target_app' "
                "AND message LIKE 'target-app: automatic rebuild completed%' "
                "AND message LIKE '%no-op%' "
                "ORDER BY id DESC LIMIT 1"
            ).fetchone()
            assert noop_activity is not None
            assert "no-op" in noop_activity["message"], dict(noop_activity)

            operations.clear()
            fail_rebuild = True
            db.set_setting(conn, "target_app_rebuild_command", "build-app")
            gid_failed = "01TARGETAPPFAILEDREBUILDA"
            create_indexed_gap(conn, gid_failed, status="awaiting-rebuild", branch=None)
            gap_writer.update_fields(gid_failed, status="awaiting-rebuild")
            result = auto_runner._run_automatic_target_app_rebuild("test failure")  # noqa: SLF001
            assert not result["ok"], result
            assert operations == ["stop", "rebuild"], operations
            assert "missing dependency: vite" in result["stderr_tail"], result
            assert db.get_setting(conn, "target_app_auto_rebuild_last_ok") == "0"
            row = conn.execute(
                "SELECT status FROM gaps_index WHERE id = ?", (gid_failed,),
            ).fetchone()
            assert row["status"] == "awaiting-rebuild", dict(row)
            gap = gaps.read_gap_json(gid_failed)
            logs = gap["rounds"][-1]["logs"]
            assert any(
                "Automatic target-app rebuild failed" in log["message"]
                and "missing dependency: vite" in (log.get("details") or "")
                for log in logs
            ), logs
            activity_row = conn.execute(
                "SELECT message, details FROM activity "
                "WHERE category = 'target_app' AND severity = 'error' "
                "ORDER BY id DESC LIMIT 1"
            ).fetchone()
            assert "automatic rebuild failed" in activity_row["message"], dict(activity_row)
            assert "missing dependency: vite" in activity_row["details"], dict(activity_row)
        finally:
            target_app.run_operation = old_run_operation  # type: ignore[assignment]
            auto_runner._conn.close()  # noqa: SLF001

        gid = "01TARGETAPPREBUILDGATEAAA"
        create_indexed_gap(conn, gid, status="awaiting-rebuild", branch=None)
        assert api._promote_rebuilt_gaps(conn) == 2  # noqa: SLF001
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gid,),
        ).fetchone()
        assert row["status"] == "review"
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = '01TARGETAPPFAILEDREBUILDA'",
        ).fetchone()
        assert row["status"] == "review"
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gid_remote_pending,),
        ).fetchone()
        assert row["status"] == "awaiting-rebuild"
        gap = gaps.read_gap_json(gid)
        assert gap["status"] == "review"
        assert gap.get("branch_name") is None
        messages = [log["message"] for log in gap["rounds"][-1]["logs"]]
        assert "Target application rebuilt; Gap is ready for review" in messages, messages
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("target-app rebuild tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
