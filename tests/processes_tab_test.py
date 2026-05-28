"""System > Processes API behavior."""
from __future__ import annotations

import os
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-processes-tab-")
    conn = init_refine(client)
    try:
        from refine_server import db
        from refine_server.backend_protocol import (
            M_BACKGROUND_PROCESSES_SET, M_ENFORCE_SCHEDULING,
        )
        from refine_ui import api, background_jobs, runtime

        db.set_setting(conn, "paused", "1")
        db.set_setting(conn, "target_app_state", "running")
        db.set_setting(conn, "target_app_start_command", "npm run dev")
        db.set_setting(conn, "target_app_stop_command", "pkill -f node")
        db.set_setting(conn, "target_app_rebuild_command", "npm run build")
        db.set_setting(conn, "parallel_run_cap", "3")
        db.set_setting(conn, "worker_memory_limit_mb", "4096")
        db.set_setting(conn, "worker_cpu_priority", "normal")
        db.set_setting(conn, "ui_memory_limit_mb", "1024")
        conn.commit()

        original_snapshot = runtime.runner_status_snapshot
        original_active_background_job = api._active_background_job  # type: ignore[attr-defined]
        original_get_client = api.get_client  # type: ignore[attr-defined]
        original_supervisor_pid = os.environ.get("REFINE_SUPERVISOR_PID")
        calls: list[tuple[str, dict]] = []

        class FakeClient:
            def call(self, method: str, params: dict, timeout: float | None = None) -> dict:
                calls.append((method, params))
                return {"ok": True, "stopped": bool(params.get("stopped"))}

        def setting_value(key: str) -> str | None:
            fresh = db.connect()
            try:
                return db.get_setting(fresh, key)
            finally:
                fresh.close()

        def fake_snapshot() -> dict:
            return {
                "runner_reachable": True,
                "pid": 3131,
                "backend": {
                    "process_model": "supervisor",
                    "transport": "unix_socket",
                },
                "chat": [{
                    "session_id": "chat123",
                    "pid": None,
                    "status": "idle",
                    "provider": "claude",
                    "mode": "standalone",
                    "gap_id": None,
                    "elapsed_seconds": 5,
                    "idle_seconds": 5,
                }],
                "running": [{
                    "gap_id": "01PROCESSAGENTGAP0000000001",
                    "round_idx": 0,
                    "pid": 4242,
                    "elapsed_seconds": 12,
                    "idle_seconds": 3,
                }],
                "merger": {
                    "state": "idle",
                    "gap_id": None,
                    "elapsed_seconds": 0,
                    "queued": 0,
                },
                "governance": {
                    "configured": False,
                    "state": "idle",
                    "queued": 0,
                },
                "target_app_rebuild": {
                    "mode": "on_worktree_merge",
                    "running": True,
                    "queued": False,
                    "last_reason": "",
                },
            }

        try:
            os.environ["REFINE_SUPERVISOR_PID"] = "3030"
            runtime.runner_status_snapshot = fake_snapshot  # type: ignore[assignment]
            api.get_client = lambda: FakeClient()  # type: ignore[assignment]
            api._active_background_job = (  # type: ignore[attr-defined]
                lambda kind: {
                    "id": "job-import",
                    "kind": "import_persist",
                    "status": "running",
                    "started_at": "2000-01-01T00:00:00Z",
                    "progress": {"message": "Persisting imports"},
                } if kind == "import_persist" else None
            )
            status, body = api.process_summary()
            stop_status, stop_body = api.set_background_processes({"stopped": True})
            paused_action_results = [
                api.target_app_rebuild_queue({}),
                api.hard_reset_worktree({}),
                api.target_app_generate({"kind": "all"}),
                api.rebuild_sqlite_cache({"background": True}),
                api.cleanup_logs({"days": 7}),
                api.import_extract({"text": "one thing"}),
                api.import_parse_csv({
                    "text": "name,actual,target\nA,B,C",
                    "background": True,
                }),
                api.import_persist({
                    "reporter": "Operator",
                    "drafts": [
                        {"name": f"Gap {idx}", "actual": "A", "target": "B"}
                        for idx in range(api.IMPORT_BACKGROUND_THRESHOLD)
                    ],
                }),
                api.quality_regression_run({}),
                api.governance_generate_rules({
                    "product": "Product",
                    "constitution": "Rules",
                }),
            ]
            started = threading.Event()
            release = threading.Event()

            def hold_exclusive_job(progress=None):  # noqa: ANN001
                started.set()
                release.wait(timeout=5.0)
                return {"ok": True}

            job = background_jobs.start(
                "sqlite_cache_rebuild",
                "Automatic target-app rebuild",
                hold_exclusive_job,
            )
            assert started.wait(timeout=2.0), job
            settings_update_status, settings_update_body = api.update_settings({
                "target_app_url": "http://localhost:3000",
            })
            create_status, create_body = api.create_gap({
                "reporter": "Operator",
                "actual": "Background is stopped.",
                "target": "Creating a Gap still works.",
                "priority": "low",
            })
            release.set()
            deadline = time.time() + 5.0
            while time.time() < deadline:
                snap = background_jobs.snapshot(job["id"])
                if snap and snap.get("status") in {"done", "failed", "cancelled"}:
                    break
                time.sleep(0.05)
            start_status, start_body = api.set_background_processes({"stopped": False})
            agent_pause_status, agent_pause_body = api.set_agent_processes({"paused": True})
            paused_setting_after_agent_pause = setting_value("agents_paused")
            background_setting_after_agent_pause = setting_value("paused")
            rebuild_while_agents_paused = api.target_app_rebuild_queue({})
            agent_unpause_status, agent_unpause_body = api.set_agent_processes({
                "paused": False,
            })
        finally:
            runtime.runner_status_snapshot = original_snapshot  # type: ignore[assignment]
            api._active_background_job = original_active_background_job  # type: ignore[attr-defined]
            api.get_client = original_get_client  # type: ignore[assignment]
            if original_supervisor_pid is None:
                os.environ.pop("REFINE_SUPERVISOR_PID", None)
            else:
                os.environ["REFINE_SUPERVISOR_PID"] = original_supervisor_pid

        assert status == 200, body
        assert body["paused"] is True, body
        assert body["backend"]["process_model"] == "supervisor", body
        assert body["resource_caps"]["worker_max_memory"]["mb"] == 4096, body
        assert body["resource_caps"]["worker_cpu_priority"]["label"] == "normal (weight 100)", body
        kinds = [p["kind"] for p in body["processes"]]
        assert kinds[:4] == ["supervisor", "ui", "runner", "target_app"], kinds
        assert "agent" in kinds, kinds
        assert "chat" in kinds, kinds
        assert "merger" not in kinds, kinds
        assert "governance" not in kinds, kinds
        supervisor = next(p for p in body["processes"] if p["kind"] == "supervisor")
        assert supervisor["pid"] == 3030, supervisor
        assert "Supervises the UI and runner worker processes" in supervisor["details"], supervisor
        assert supervisor["background_processes_stopped"] is True, supervisor
        assert supervisor["actions"] == ["start_background_processes"], supervisor
        assert stop_status == 200, stop_body
        assert stop_body["stopped"] is True, stop_body
        assert [s for s, _ in paused_action_results] == [409] * 10, paused_action_results
        assert all(
            b["error"]["code"] == "background_processes_stopped"
            for _, b in paused_action_results
        ), paused_action_results
        assert settings_update_status == 200, settings_update_body
        assert create_status == 201, create_body
        assert start_status == 200, start_body
        assert start_body["stopped"] is False, start_body
        assert setting_value("paused") == "0"
        assert agent_pause_status == 200, agent_pause_body
        assert agent_pause_body["agents_paused"] is True, agent_pause_body
        assert paused_setting_after_agent_pause == "1"
        assert background_setting_after_agent_pause == "0"
        assert rebuild_while_agents_paused[0] != 409, rebuild_while_agents_paused
        assert agent_unpause_status == 200, agent_unpause_body
        assert agent_unpause_body["agents_paused"] is False, agent_unpause_body
        assert setting_value("agents_paused") == "0"
        background_calls = [call for call in calls if call[0] == M_BACKGROUND_PROCESSES_SET]
        assert background_calls == [
            (M_BACKGROUND_PROCESSES_SET, {"stopped": True}),
            (M_BACKGROUND_PROCESSES_SET, {"stopped": False}),
        ], calls
        enforce_calls = [call for call in calls if call[0] == M_ENFORCE_SCHEDULING]
        assert enforce_calls == [
            (M_ENFORCE_SCHEDULING, {"settle_timeout_seconds": 8.0}),
            (M_ENFORCE_SCHEDULING, {"settle_timeout_seconds": 8.0}),
        ], calls
        runner = next(p for p in body["processes"] if p["kind"] == "runner")
        assert runner["pid"] == 3131, runner
        assert runner["max_memory"]["label"] == "4096 MB", runner
        assert runner["cpu_priority"]["label"] == "normal (weight 100)", runner
        ui = next(p for p in body["processes"] if p["kind"] == "ui")
        assert ui["max_memory"]["label"] == "1024 MB", ui
        assert ui["cpu_priority"]["label"] == "normal (weight 100)", ui
        assert ui["details"] == "Serves the web UI, API routes, and SSE updates.", ui
        target = next(p for p in body["processes"] if p["kind"] == "target_app")
        assert target["status"] == "running", target
        assert target["actions"] == ["start", "rebuild", "stop", "check"], target
        assert target["max_memory"]["label"] == "unmanaged", target
        agent = next(p for p in body["processes"] if p["kind"] == "agent")
        assert agent["pid"] == 4242, agent
        assert agent["actions"] == ["cancel"], agent
        assert agent["max_memory"]["label"] == "4096 MB", agent
        chat = next(p for p in body["processes"] if p["kind"] == "chat")
        assert chat["pid"] is None, chat
        assert chat["status"] == "idle", chat
        assert chat["actions"] == ["stop"], chat
        assert chat["idle_seconds"] == 5, chat
        assert chat["cpu_priority"]["label"] == "normal (weight 100)", chat
        work_kinds = [w["kind"] for w in body["runner_work"]]
        assert work_kinds == [
            "merger",
            "governance",
            "target_app_rebuilder",
            "target_app_config_generator",
            "sqlite_cache_rebuild",
            "activity_log_cleanup",
            "import_prepare",
            "import_persist",
            "bulk_update_gaps",
            "bulk_delete_gaps",
        ], work_kinds
        assert [w["status"] for w in body["runner_work"]] == [
            "paused", "paused", "paused", "paused", "paused",
            "paused", "paused", "paused", "paused", "paused",
        ], body["runner_work"]
        assert all(w.get("paused") is True for w in body["runner_work"]), body["runner_work"]
        assert body["runner_work"][2]["details"] == "Rebuilds the target application after merged work.", body["runner_work"]
        import_worker = next(w for w in body["runner_work"] if w["kind"] == "import_persist")
        assert import_worker["details"] == "Persisting imports", import_worker
        assert import_worker["job_id"] == "job-import", import_worker
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("processes tab tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
