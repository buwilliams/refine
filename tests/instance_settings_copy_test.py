"""Copying Application and Runtime settings between instances."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-instance-settings-copy-")
    conn = init_refine(client)
    try:
        from refine_server import db, project_state
        from refine_ui import api

        default = project_state.active_instance_id()
        source = project_state.create_instance("Source")
        project_state.set_active_instance(source["id"])
        db.set_setting(conn, "agent_subpath", "apps/web")
        db.set_setting(conn, "merge_target_branch", "release")
        db.set_setting(conn, "quality_enabled", "1")
        db.set_setting(conn, "target_app_url", "http://localhost:5173")
        db.set_setting(conn, "target_app_start_command", "npm run dev")
        db.set_setting(conn, "target_app_rebuild_command", "npm run build")
        db.set_setting(conn, "parallel_run_cap", "3")
        db.set_setting(conn, "branch_name_pattern", "work/{gap_id}")
        db.set_setting(conn, "agent_idle_timeout_seconds", "120")
        db.set_setting(conn, "worker_memory_limit_mb", "4096")
        db.set_setting(conn, "agent_cli", "codex")
        db.set_setting(conn, "paused", "1")

        project_state.set_active_instance(default)
        db.set_setting(conn, "agent_subpath", "")
        db.set_setting(conn, "merge_target_branch", "")
        db.set_setting(conn, "quality_enabled", "0")
        db.set_setting(conn, "target_app_url", "")
        db.set_setting(conn, "target_app_start_command", "")
        db.set_setting(conn, "target_app_rebuild_command", "")
        db.set_setting(conn, "parallel_run_cap", "10")
        db.set_setting(conn, "branch_name_pattern", "refine/{gap_id}")
        db.set_setting(conn, "agent_idle_timeout_seconds", "900")
        db.set_setting(conn, "worker_memory_limit_mb", "2000")
        db.set_setting(conn, "agent_cli", "claude")
        db.set_setting(conn, "paused", "0")

        status, body = api.copy_instance_settings({
            "source_instance_id": source["id"],
            "section": "application",
        })
        assert status == 200, body
        settings = body["settings"]
        assert settings["agent_subpath"] == "apps/web", settings
        assert settings["merge_target_branch"] == "release", settings
        assert settings["target_app_url"] == "http://localhost:5173", settings
        assert settings["target_app_start_command"] == "npm run dev", settings
        assert settings["target_app_rebuild_command"] == "npm run build", settings
        assert settings["quality_enabled"] == "0", settings

        status, body = api.copy_instance_settings({
            "source_instance_id": source["id"],
            "section": "runtime",
        })
        assert status == 200, body
        settings = body["settings"]
        assert settings["parallel_run_cap"] == "3", settings
        assert settings["branch_name_pattern"] == "work/{gap_id}", settings
        assert settings["agent_idle_timeout_seconds"] == "120", settings
        assert settings["worker_memory_limit_mb"] == "4096", settings
        assert settings["agent_cli"] == "claude", settings
        assert settings["paused"] == "0", settings

        status, body = api.copy_instance_settings({
            "source_instance_id": default,
            "section": "runtime",
        })
        assert status == 400, body
        assert "different from the active instance" in body["error"]["message"]
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("instance settings copy tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
