"""Copying Application and Runtime settings between nodes."""
from __future__ import annotations

import json
import sys
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def _run_cli(args: list[str]) -> tuple[int, str, str]:
    from refine_cli import cli

    stdout = StringIO()
    stderr = StringIO()
    with redirect_stdout(stdout), redirect_stderr(stderr):
        rc = cli.main(args)
    return rc, stdout.getvalue(), stderr.getvalue()


def main() -> int:
    tmp, client = make_client_repo("refine-node-settings-copy-")
    conn = init_refine(client)
    try:
        from refine_server import db, project_state
        from refine_ui import api

        default = project_state.active_node_id()
        source = project_state.create_node("Source")
        project_state.set_active_node(source["id"])
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

        project_state.set_active_node(default)
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

        status, body = api.copy_node_settings({
            "source_node_id": source["id"],
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

        status, body = api.copy_node_settings({
            "source_node_id": source["id"],
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

        status, body = api.copy_node_settings({
            "source_node_id": default,
            "section": "runtime",
        })
        assert status == 400, body
        assert "different from the active node" in body["error"]["message"]

        db.set_setting(conn, "parallel_run_cap", "12")
        rc, out, err = _run_cli(["node", "copy-settings", source["id"], "runtime"])
        assert rc == 0, err
        payload = json.loads(out)
        assert payload["source_node_id"] == source["id"], payload
        assert payload["target_node_id"] == default, payload
        assert payload["section"] == "runtime", payload
        assert payload["sync"]["ok"] is True, payload
        settings = db.list_settings(conn)
        assert settings["parallel_run_cap"] == "3", settings
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("node settings copy tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
