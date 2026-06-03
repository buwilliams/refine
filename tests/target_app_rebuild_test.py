"""Automatic target-app rebuild queue and review gating tests."""
from __future__ import annotations

import json
import sys
import threading
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, git, init_refine, make_client_repo


def gap_json_path(gap_id: str) -> str:
    return f".refine/gaps/{gap_id[:2]}/{gap_id[2:]}/gap.json"


def gap_json_at(client: Path, ref: str, gap_id: str) -> dict:
    return json.loads(git(client, "show", f"{ref}:{gap_json_path(gap_id)}").stdout)


def main() -> int:
    tmp, client = make_client_repo("refine-target-app-rebuild-")
    conn = init_refine(client)
    try:
        from refine_server import (
            db, gap_writer, gaps, runner as runner_mod,
            project_state, target_app, target_app_ops, target_app_rebuilder,
        )
        from refine_server.backend_protocol import (
            M_TARGET_APP_REBUILD_PENDING,
            M_TARGET_APP_REBUILD_QUEUE,
        )
        from refine_server.gaps import now_iso
        from refine_ui import api

        runs: list[str] = []

        def run_rebuild(reason: str, _cancel_event=None) -> dict:  # noqa: ANN001
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

        assert db.get_setting(conn, "target_app_auto_rebuild") == "on_worktree_merge"
        db.set_setting(conn, "target_app_auto_rebuild", "never")
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
        other_instance = project_state.create_node("Remote Rebuild Host")
        gid_remote_pending = "01TARGETAPPREMOTEPENDINGA"
        create_indexed_gap(
            conn,
            gid_remote_pending,
            status="awaiting-rebuild",
            branch=None,
            node_id=other_instance["id"],
        )
        assert rebuilder.queue_pending_awaiting_rebuild() is False
        gid_pending = "01TARGETAPPPENDINGREBUILDA"
        create_indexed_gap(conn, gid_pending, status="awaiting-rebuild", branch=None)
        assert rebuilder.queue_pending_awaiting_rebuild() is True
        assert rebuilder.queue_pending_awaiting_rebuild() is False
        rebuilder._drain_queue()  # noqa: SLF001
        assert runs[-1] == "1 Gap awaiting target-app rebuild", runs

        entered_rebuild = threading.Event()
        cancelled_rebuild = threading.Event()

        def cancellable_rebuild(reason: str, cancel_event) -> dict:  # noqa: ANN001
            entered_rebuild.set()
            deadline = time.time() + 5.0
            while time.time() < deadline and not cancel_event.is_set():
                time.sleep(0.01)
            if cancel_event.is_set():
                cancelled_rebuild.set()
                return {"ok": False, "cancelled": True, "reason": reason}
            return {"ok": True, "reason": reason}

        cancellable = target_app_rebuilder.TargetAppRebuilder(
            get_conn=db.connect,
            run_rebuild=cancellable_rebuild,
            interval=0.05,
        )
        cancellable.start()
        try:
            assert cancellable.queue_rebuild("manual stop test") is True
            assert entered_rebuild.wait(timeout=2.0)
            db.set_setting(conn, "paused", "1")
            stop_result = cancellable.stop_background_work(timeout=2.0)
            assert stop_result["cancelled_running"] is True, stop_result
            assert stop_result["running"] is False, stop_result
            assert cancelled_rebuild.wait(timeout=1.0)
            db.set_setting(conn, "paused", "0")
        finally:
            db.set_setting(conn, "paused", "0")
            cancellable.stop()

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

        calls.clear()

        status, body = api.update_settings({
            "target_app_auto_rebuild": "daily",
            "target_app_auto_rebuild_hour_utc": "23",
        })
        assert status == 200, body
        assert db.get_setting(conn, "target_app_auto_rebuild") == "daily"
        assert db.get_setting(conn, "target_app_auto_rebuild_hour_utc") == "23"
        status, body = api.update_settings({
            "target_app_auto_rebuild_hour_utc": "24",
        })
        assert status == 400, body
        assert "between 0 and 23" in body["error"]["message"], body
        status, body = api.update_settings({
            "target_app_auto_rebuild": "nightly",
        })
        assert status == 200, body
        assert db.get_setting(conn, "target_app_auto_rebuild") == "daily"

        class RebuildQueueClient:
            def call(self, method: str, params: dict | None = None, *, timeout: float = 30.0) -> dict:  # noqa: ARG002
                calls.append(method)
                assert method == M_TARGET_APP_REBUILD_QUEUE
                return {"queued": True}

        db.set_setting(conn, "target_app_rebuild_command", "")
        db.set_setting(conn, "target_app_state", "running")
        db.set_setting(conn, "target_app_last_error", "old failure")
        try:
            api.get_client = lambda: RebuildQueueClient()  # type: ignore[assignment]
            status, body = api.target_app_rebuild({})
        finally:
            api.get_client = old_get_client  # type: ignore[assignment]
        assert status == 202, body
        assert body["queued"] is True, body
        assert calls == [M_TARGET_APP_REBUILD_QUEUE], calls
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gid_pending,),
        ).fetchone()
        assert row["status"] == "awaiting-rebuild", dict(row)
        assert db.get_setting(conn, "target_app_last_error") == "old failure"

        db.set_setting(conn, "target_app_start_command", "")
        db.set_setting(conn, "target_app_stop_command", "")
        db.set_setting(conn, "target_app_state", "running")
        db.set_setting(conn, "target_app_last_error", "old failure")
        try:
            api.get_client = lambda: RebuildQueueClient()  # type: ignore[assignment]
            for endpoint in (api.target_app_start, api.target_app_stop):
                status, body = endpoint({})
                assert status == 200, body
                assert body["ok"] is True, body
                assert body["noop"] is True, body
                assert body["promoted_gaps"] == 0, body
                assert "no-op" in body["message"], body
        finally:
            api.get_client = old_get_client  # type: ignore[assignment]
        assert db.get_setting(conn, "target_app_last_error") == ""

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
            (client / ".refine" / "nodes" / "default" / "target-app.json")
            .read_text(encoding="utf-8")
        )
        assert target_settings["target_app_auto_rebuild"] == "hourly"

        db.set_setting(conn, "target_app_auto_rebuild", "daily")
        db.set_setting(conn, "target_app_auto_rebuild_hour_utc", "13")
        db.set_setting(conn, "target_app_auto_rebuild_last_started_at", "")
        daily_window = datetime.now(timezone.utc).replace(
            hour=13, minute=0, second=0, microsecond=0,
        )
        rebuilder._queue_scheduled_rebuild_if_due(  # noqa: SLF001
            daily_window.replace(hour=12),
        )
        rebuilder._drain_queue()  # noqa: SLF001
        assert runs[-1] == "hourly automatic rebuild", runs
        db.set_setting(
            conn,
            "target_app_auto_rebuild_last_started_at",
            (daily_window - timedelta(days=1)).isoformat(),
        )
        rebuilder._queue_scheduled_rebuild_if_due(daily_window)  # noqa: SLF001
        rebuilder._drain_queue()  # noqa: SLF001
        assert runs[-1] == "daily automatic rebuild (13:00 UTC)", runs
        daily_count = len(runs)
        db.set_setting(
            conn,
            "target_app_auto_rebuild_last_started_at",
            daily_window.isoformat(),
        )
        rebuilder._queue_scheduled_rebuild_if_due(daily_window)  # noqa: SLF001
        rebuilder._drain_queue()  # noqa: SLF001
        assert len(runs) == daily_count, runs

        db.set_setting(conn, "target_app_auto_rebuild", "nightly")
        db.set_setting(conn, "target_app_auto_rebuild_hour_utc", "0")
        db.set_setting(conn, "target_app_auto_rebuild_last_started_at", "")
        legacy_midnight = daily_window.replace(hour=0)
        rebuilder._queue_scheduled_rebuild_if_due(legacy_midnight)  # noqa: SLF001
        rebuilder._drain_queue()  # noqa: SLF001
        assert runs[-1] == "daily automatic rebuild (00:00 UTC)", runs
        snapshot = target_app_ops.snapshot(conn)
        assert snapshot["auto_rebuild"] == "daily", snapshot
        assert snapshot["auto_rebuild_hour_utc"] == "0", snapshot

        db.set_setting(conn, "target_app_stop_command", "stop-app")
        db.set_setting(conn, "target_app_rebuild_command", "build-app")
        db.set_setting(conn, "target_app_start_command", "start-app")
        auto_runner = runner_mod.Runner()
        db.set_setting(conn, "paused", "1")
        paused_queue = auto_runner._h_target_app_rebuild_queue({})  # noqa: SLF001
        paused_generate = auto_runner._h_target_app_generate({"kind": "all"})  # noqa: SLF001
        paused_reset = auto_runner._h_hard_reset_worktree({})  # noqa: SLF001
        paused_launch = auto_runner._h_launch({})  # noqa: SLF001
        paused_extract = auto_runner._h_extract_gaps({"text": "gap"})  # noqa: SLF001
        paused_regression = auto_runner._h_regression_run({})  # noqa: SLF001
        paused_governance = auto_runner._h_governance_generate_rules({  # noqa: SLF001
            "product": "Product",
            "constitution": "Rules",
        })
        assert paused_queue["ok"] is False, paused_queue
        assert paused_queue["code"] == "background_processes_stopped", paused_queue
        assert paused_generate["ok"] is False, paused_generate
        assert paused_reset["ok"] is False, paused_reset
        assert paused_launch["ok"] is False, paused_launch
        assert paused_extract["ok"] is False, paused_extract
        assert paused_regression["ok"] is False, paused_regression
        assert paused_governance["ok"] is False, paused_governance
        db.set_setting(conn, "paused", "0")
        operations: list[str] = []
        old_run_operation = target_app.run_operation
        fail_rebuild = False

        def fake_run_operation(kind: str, cfg: dict, **_kwargs) -> dict:  # noqa: ANN003
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
            for kind in ("start", "stop", "status"):
                result = auto_runner._h_target_app_run({  # noqa: SLF001
                    "kind": kind,
                    "config": {},
                })
                assert result["ok"], result
            try:
                auto_runner._h_target_app_run({  # noqa: SLF001
                    "kind": "rebuild",
                    "config": {},
                })
                raise AssertionError("direct runner rebuild should be rejected")
            except ValueError as e:
                assert "start', 'stop', or 'status" in str(e)
            manual_activity = [
                r["message"]
                for r in conn.execute(
                    "SELECT message FROM activity "
                    "WHERE category = 'target_app' AND actor = 'refine' "
                    "AND gap_id IS NULL ORDER BY id"
                )
            ]
            for kind in ("start", "stop", "status"):
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
            db.set_setting(conn, "target_app_stop_command", "")
            db.set_setting(conn, "target_app_rebuild_command", "")
            db.set_setting(conn, "target_app_start_command", "")
            gid_all_noop = "01TARGETAPPALLNOOPREBUIL"
            create_indexed_gap(conn, gid_all_noop, status="awaiting-rebuild", branch=None)
            gap_writer.update_fields(gid_all_noop, status="awaiting-rebuild")
            result = auto_runner._run_automatic_target_app_rebuild("test all no-op")  # noqa: SLF001
            assert result["ok"], result
            assert operations == [], operations
            assert [step["kind"] for step in result["steps"]] == [
                "stop", "rebuild", "start",
            ], result
            assert all(step["noop"] for step in result["steps"]), result
            row = conn.execute(
                "SELECT status FROM gaps_index WHERE id = ?", (gid_all_noop,),
            ).fetchone()
            assert row["status"] == "review", dict(row)
            assert db.get_setting(conn, "target_app_auto_rebuild_last_ok") == "1"

            operations.clear()
            db.set_setting(conn, "target_app_stop_command", "stop-app")
            fail_rebuild = True
            db.set_setting(conn, "target_app_rebuild_command", "build-app")
            db.set_setting(conn, "target_app_start_command", "start-app")
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
        assert target_app_ops.promote_rebuilt_gaps(conn) == 2
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
        head_gap = gap_json_at(client, "HEAD", gid)
        assert head_gap["status"] == "review", head_gap
        assert head_gap.get("branch_name") is None, head_gap
        messages = [log["message"] for log in gap["rounds"][-1]["logs"]]
        assert "Target application rebuilt; Gap is ready for review" in messages, messages

        db.set_setting(conn, "quality_enabled", "1")
        db.set_setting(conn, "quality_timing", "post_rebuild")
        gid_qa = "01TARGETAPPPOSTREBUILDQAAA"
        create_indexed_gap(conn, gid_qa, status="awaiting-rebuild", branch=None)
        assert target_app_ops.promote_rebuilt_gaps(conn) == 1
        row = conn.execute(
            "SELECT status, branch_name FROM gaps_index WHERE id = ?", (gid_qa,),
        ).fetchone()
        assert row["status"] == "qa", dict(row)
        assert row["branch_name"] is None, dict(row)
        gap = gaps.read_gap_json(gid_qa)
        assert gap["status"] == "qa"
        messages = [log["message"] for log in gap["rounds"][-1]["logs"]]
        assert "Target application rebuilt; Gap queued for QA" in messages, messages

        db.set_setting(conn, "quality_enabled", "0")
        gid_bypass = "01TARGETAPPPOSTQABYPASSAAA"
        create_indexed_gap(conn, gid_bypass, status="awaiting-rebuild", branch=None)
        assert target_app_ops.promote_rebuilt_gaps(conn) == 1
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gid_bypass,),
        ).fetchone()
        assert row["status"] == "review", dict(row)
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("target-app rebuild tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
