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
            target_app, target_app_rebuilder,
        )
        from refine_server.backend_protocol import M_TARGET_APP_REBUILD_PENDING
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
        assert rebuilder.queue_for_worktree_merge("01GAP") is True
        rebuilder._drain_queue()  # noqa: SLF001
        assert len(runs) == 3, runs
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
        finally:
            api.get_client = old_get_client  # type: ignore[assignment]
        assert M_TARGET_APP_REBUILD_PENDING in calls, calls
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
        midnight = datetime.now().astimezone().replace(
            hour=0, minute=0, second=0, microsecond=0,
        )
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
            assert db.get_setting(conn, "target_app_auto_rebuild_last_ok") == "1"

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
