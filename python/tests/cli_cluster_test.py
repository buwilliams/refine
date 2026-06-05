"""CLI parity for cluster operations."""
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


def _json(out: str) -> dict:
    return json.loads(out)


def main() -> int:
    tmp, client = make_client_repo("refine-cli-cluster-")
    conn = init_refine(client)
    conn.close()
    try:
        from refine_cli import cli
        from refine_server import cluster

        cfg = str(client / ".refine" / "refine.toml")
        prefix = ["--config", cfg]

        sync_calls: list[str] = []
        old_sync = cli._sync_cli_refine_state
        cli._sync_cli_refine_state = (
            lambda _cfg, *, message, rebuild_cache=True:
            sync_calls.append(message) or {"ok": True, "message": message}
        )
        try:
            rc, out, err = _run_cli([
                *prefix,
                "cluster",
                "register",
                "remote-a",
                "buildbox.example",
                "--name",
                "Remote A",
                "--ssh-port",
                "2222",
                "--refine-checkout",
                "/opt/refine",
                "--target-app",
                "/srv/app",
                "--refine-port",
                "8090",
            ])
            assert rc == 0, err
            payload = _json(out)
            assert payload["node"]["id"] == "remote-a", payload
            assert payload["node"]["ssh_port"] == 2222, payload
            assert payload["sync"]["ok"] is True, payload

            rc, out, err = _run_cli([
                *prefix,
                "cluster",
                "update",
                "remote-a",
                "--name",
                "Remote Renamed",
            ])
            assert rc == 0, err
            payload = _json(out)
            assert payload["node"]["display_name"] == "Remote Renamed", payload
        finally:
            cli._sync_cli_refine_state = old_sync

        calls: list[list[str]] = []

        class Result:
            returncode = 0
            stdout = "remote ok\n"
            stderr = ""

        old_run = cluster.subprocess.run

        def fake_run(cmd, **kwargs):  # noqa: ANN001, ANN202
            if cmd and cmd[0] == "ssh":
                calls.append(cmd)
                return Result()
            return old_run(cmd, **kwargs)

        cluster.subprocess.run = fake_run
        try:
            rc, out, err = _run_cli([*prefix, "cluster", "run", "remote-a", "node", "list"])
            assert rc == 0, err
            assert out == "remote ok\n", out
            assert not err, err

            cli._sync_cli_refine_state = (
                lambda _cfg, *, message, rebuild_cache=True:
                sync_calls.append(message) or {"ok": True, "message": message}
            )
            try:
                rc, out, err = _run_cli([*prefix, "cluster", "bootstrap", "remote-a"])
            finally:
                cli._sync_cli_refine_state = old_sync
            assert rc == 0, err
            assert out == "remote ok\n", out
        finally:
            cluster.subprocess.run = old_run

        assert calls[0][0:5] == ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10"], calls
        assert "node list" in calls[0][-1], calls
        assert sync_calls == [
            "refine: update cluster node",
            "refine: update cluster node",
            "refine: update cluster node health",
        ], sync_calls
    finally:
        cleanup_tmp(tmp)

    print("CLI cluster tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
