"""System > Processes API behavior."""
from __future__ import annotations

import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-processes-tab-")
    conn = init_refine(client)
    try:
        from refine_server import db
        from refine_ui import api, runtime

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
        original_supervisor_pid = os.environ.get("REFINE_SUPERVISOR_PID")

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
                    "running": False,
                    "queued": False,
                    "last_reason": "",
                },
            }

        try:
            os.environ["REFINE_SUPERVISOR_PID"] = "3030"
            runtime.runner_status_snapshot = fake_snapshot  # type: ignore[assignment]
            status, body = api.process_summary()
        finally:
            runtime.runner_status_snapshot = original_snapshot  # type: ignore[assignment]
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
        assert work_kinds == ["merger", "governance", "target_app_rebuilder"], work_kinds
        assert [w["status"] for w in body["runner_work"]] == ["idle", "idle", "idle"], body["runner_work"]
        assert body["runner_work"][2]["details"] == "Rebuilds the target application after merged work.", body["runner_work"]
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
