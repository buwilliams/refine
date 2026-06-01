"""Focused tests for distributed cluster configuration."""
from __future__ import annotations

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-cluster-")
    conn = init_refine(client)
    conn.close()
    try:
        from refine_server import cluster, project_state

        node = cluster.upsert_node({
            "id": "remote-a",
            "display_name": "Remote A",
            "ssh_host": "buildbox.example",
            "ssh_port": 2222,
            "refine_checkout": "/opt/refine",
            "target_app_path": "/srv/app",
            "refine_port": 8090,
        })
        assert "ssh_user" not in node, node
        assert project_state.node_by_id("remote-a")["display_name"] == "Remote A"

        original_run = cluster.subprocess.run
        calls = []

        class Result:
            returncode = 0
            stdout = "ok\n"
            stderr = ""

        def fake_run(cmd, **kwargs):  # noqa: ANN001, ANN202
            calls.append((cmd, kwargs))
            return Result()

        cluster.subprocess.run = fake_run
        try:
            result = cluster.run_remote("remote-a", ["node", "list"])
        finally:
            cluster.subprocess.run = original_run

        assert result["ok"] is True, result
        assert calls, calls
        cmd = calls[0][0]
        assert cmd[:3] == ["ssh", "-p", "2222"], cmd
        assert cmd[3] == "buildbox.example", cmd
        assert "@" not in cmd[3], cmd
        assert cmd[4] == "cd /opt/refine && uv run refine node list", cmd

        cluster.upsert_node({
            "id": "remote-b",
            "ssh_host": "other.example",
        })
        try:
            cluster.bootstrap("remote-b")
            raise AssertionError("bootstrap without target_app_path should fail")
        except ValueError as e:
            assert "target_app_path is required" in str(e), e
    finally:
        cleanup_tmp(tmp)
    print("cluster tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
