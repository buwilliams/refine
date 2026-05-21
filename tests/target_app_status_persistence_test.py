"""Target-app status persistence tests."""
from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-target-app-status-")
    conn = init_refine(client)
    try:
        from refine_server import db
        from refine_server.backend_protocol import M_TARGET_APP_RUN
        from refine_ui import api

        db.set_setting(conn, "target_app_state", "stopped")
        db.set_setting(conn, "target_app_last_check_at", "2000-01-01T00:00:00+00:00")
        conn.close()
        target_app_path = client / ".refine" / "instances" / "default" / "target-app.json"
        before = target_app_path.read_bytes()

        calls: list[dict] = []

        class FakeClient:
            def call(self, method: str, params: dict | None = None, *, timeout: float = 30.0) -> dict:  # noqa: ARG002
                assert method == M_TARGET_APP_RUN
                calls.append(params or {})
                return {
                    "ok": True,
                    "state": "running",
                    "message": "ok",
                    "checks": [{"name": "status", "ok": True}],
                }

        old_get_client = api.get_client
        try:
            api.get_client = lambda: FakeClient()  # type: ignore[assignment]
            status, body = api.target_app_check({"quiet": True})
        finally:
            api.get_client = old_get_client  # type: ignore[assignment]

        assert status == 200, body
        assert calls and calls[0].get("quiet") is True, calls
        assert body["state"] == "running", body
        assert body["last_check_ok"] is True, body
        assert body["last_check_at"] != "2000-01-01T00:00:00+00:00", body
        assert target_app_path.read_bytes() == before

        try:
            api.get_client = lambda: FakeClient()  # type: ignore[assignment]
            explicit_status, explicit_body = api.target_app_check({})
        finally:
            api.get_client = old_get_client  # type: ignore[assignment]
        assert explicit_status == 200, explicit_body
        assert calls[-1].get("quiet") is False, calls
        assert target_app_path.read_bytes() == before

        conn = db.connect()
        try:
            assert db.get_setting(conn, "target_app_state") == "running"
            assert db.get_setting(conn, "target_app_last_check_ok") == "1"
            assert db.get_setting(conn, "target_app_last_check_message") == "ok"
        finally:
            conn.close()

        target_settings = json.loads(target_app_path.read_text(encoding="utf-8"))
        target_settings["target_app_state"] = "failed"
        target_settings["target_app_last_check_at"] = "1999-01-01T00:00:00+00:00"
        target_settings["target_app_auto_rebuild_last_message"] = "stale"
        target_app_path.write_text(json.dumps(target_settings, indent=2), encoding="utf-8")

        from refine_server import project_state

        listed = project_state.list_settings()
        assert listed["target_app_state"] != "failed"
        pruned = json.loads(target_app_path.read_text(encoding="utf-8"))
        assert "target_app_state" not in pruned
        assert "target_app_last_check_at" not in pruned
        assert "target_app_auto_rebuild_last_message" not in pruned
    finally:
        cleanup_tmp(tmp)

    print("target-app status persistence tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
