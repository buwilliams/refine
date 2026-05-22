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
                    "pid": 5151,
                    "provider": "claude",
                    "mode": "standalone",
                    "elapsed_seconds": 5,
                }],
                "running": [{
                    "gap_id": "01PROCESSAGENTGAP0000000001",
                    "round_idx": 0,
                    "pid": 4242,
                    "elapsed_seconds": 12,
                    "idle_seconds": 3,
                }],
                "merger": {
                    "state": "merging",
                    "gap_id": "01PROCESSMERGEGAP000000001",
                    "elapsed_seconds": 7,
                    "queued": 2,
                },
                "governance": {
                    "configured": True,
                    "state": "idle",
                    "queued": 1,
                },
                "target_app_rebuild": {
                    "running": False,
                    "queued": True,
                    "last_reason": "1 Gap awaiting target-app rebuild",
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
        kinds = [p["kind"] for p in body["processes"]]
        assert kinds[:4] == ["supervisor", "ui", "runner", "target_app"], kinds
        assert "agent" in kinds, kinds
        assert "chat" in kinds, kinds
        assert "merger" not in kinds, kinds
        assert "governance" not in kinds, kinds
        supervisor = next(p for p in body["processes"] if p["kind"] == "supervisor")
        assert supervisor["pid"] == 3030, supervisor
        runner = next(p for p in body["processes"] if p["kind"] == "runner")
        assert runner["pid"] == 3131, runner
        target = next(p for p in body["processes"] if p["kind"] == "target_app")
        assert target["status"] == "running", target
        assert target["actions"] == ["start", "rebuild", "stop", "check"], target
        agent = next(p for p in body["processes"] if p["kind"] == "agent")
        assert agent["pid"] == 4242, agent
        assert agent["actions"] == ["cancel"], agent
        chat = next(p for p in body["processes"] if p["kind"] == "chat")
        assert chat["pid"] == 5151, chat
        assert chat["actions"] == ["stop"], chat
        work_kinds = [w["kind"] for w in body["runner_work"]]
        assert work_kinds == ["merger", "governance", "target_app_rebuilder"], work_kinds
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
