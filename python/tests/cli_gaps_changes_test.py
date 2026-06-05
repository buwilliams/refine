"""CLI parity for Gap and Changes commands."""
from __future__ import annotations

import json
import sys
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from refine_server.backend_protocol import (
    M_BULK_DELETE_GAPS,
    M_BULK_UPDATE_GAPS,
    M_LIST_CHANGES,
)
from tests.helpers import cleanup_tmp, create_indexed_gap, init_refine, make_client_repo


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
    tmp, client = make_client_repo("refine-cli-gaps-")
    conn = init_refine(client)
    try:
        gap_id = "01CLIGAPSREAD000000000000"
        create_indexed_gap(conn, gap_id, status="todo", priority="high")
        conn.commit()

        from refine_server import gap_writer

        gap_writer.append_round_log(
            gap_id=gap_id,
            round_idx=0,
            severity="info",
            category="user",
            actor="tester",
            message="CLI-visible round log",
        )

        cfg = str(client / ".refine" / "refine.toml")
        prefix = ["--config", cfg]

        rc, out, err = _run_cli([*prefix, "gaps", "list", "--status", "todo"])
        assert rc == 0, err
        payload = _json(out)
        assert payload["gaps"][0]["id"] == gap_id, payload
        assert payload["gaps"][0]["priority"] == "high", payload

        from refine_cli import cli

        list_ports: list[int | None] = []
        old_cli_project_config = cli._cli_project_config
        cli._cli_project_config = (
            lambda _ctx, *, port=None:
            list_ports.append(port) or old_cli_project_config(_ctx, port=None)
        )
        try:
            rc, out, err = _run_cli([
                *prefix,
                "gaps",
                "list",
                "--status",
                "todo",
                "--port",
                "19042",
            ])
        finally:
            cli._cli_project_config = old_cli_project_config
        assert rc == 0, err
        assert list_ports == [19042], list_ports
        payload = _json(out)
        assert payload["gaps"][0]["id"] == gap_id, payload

        rc, out, err = _run_cli([*prefix, "gaps", "get", gap_id.lower()])
        assert rc == 0, err
        payload = _json(out)
        assert payload["gap"]["id"] == gap_id, payload
        assert payload["gap"]["rounds"][0]["log_count"] == 1, payload

        rc, out, err = _run_cli([*prefix, "gaps", "logs", gap_id, "--round-idx", "0"])
        assert rc == 0, err
        payload = _json(out)
        assert payload["round_log_count"] == 1, payload
        assert payload["logs"][0]["message"] == "CLI-visible round log", payload

        calls: list[tuple[str, dict[str, object], float]] = []

        def fake_runner(method: str, params: dict[str, object], timeout: float) -> dict:
            calls.append((method, params, timeout))
            return {
                "changes": [{"commit": "abc123", "gap_id": gap_id}],
                "branch": "main",
                "page": {
                    "limit": params["limit"],
                    "offset": params["offset"],
                    "has_more": False,
                    "total": 1,
                },
            }

        old_runner_for_cli = cli._backend_runner_for_cli
        cli._backend_runner_for_cli = lambda _ctx, _port: (cli.config.get(reload=True), fake_runner)
        try:
            rc, out, err = _run_cli([
                *prefix,
                "changes",
                "list",
                "--limit",
                "10",
                "--offset",
                "2",
                "--q",
                "login",
                "--status",
                "done",
                "--priority",
                "high",
            ])
        finally:
            cli._backend_runner_for_cli = old_runner_for_cli

        assert rc == 0, err
        payload = _json(out)
        assert payload["changes"][0]["commit"] == "abc123", payload
        assert calls == [(
            M_LIST_CHANGES,
            {
                "limit": 10,
                "offset": 2,
                "q": "login",
                "status": "done",
                "priority": "high",
            },
            15.0,
        )], calls

        bulk_gap_id = "01CLIGAPSBULK00000000000"
        create_indexed_gap(conn, bulk_gap_id, status="backlog", priority="low")
        conn.commit()

        bulk_calls: list[tuple[str, dict[str, object], float]] = []
        sync_calls: list[str] = []

        def fake_bulk_runner(method: str, params: dict[str, object], timeout: float) -> dict:
            bulk_calls.append((method, params, timeout))
            ids = list(params["gap_ids"])
            if method == M_BULK_UPDATE_GAPS:
                return {
                    "updated": len(ids),
                    "ids": ids,
                    "failed": 0,
                    "failures": [],
                    "progress": {"completed": len(ids), "total": len(ids)},
                }
            if method == M_BULK_DELETE_GAPS:
                return {
                    "deleted": len(ids),
                    "ids": ids,
                    "failed": 0,
                    "failures": [],
                    "progress": {"completed": len(ids), "total": len(ids)},
                }
            raise AssertionError(method)

        old_runner_for_cli = cli._backend_runner_for_cli
        old_sync = cli._sync_cli_refine_state
        cli._backend_runner_for_cli = lambda _ctx, _port: (cli.config.get(reload=True), fake_bulk_runner)
        cli._sync_cli_refine_state = (
            lambda _cfg, *, message, rebuild_cache=True:
            sync_calls.append(message) or {"ok": True, "message": message}
        )
        try:
            rc, out, err = _run_cli([
                *prefix,
                "gaps",
                "bulk-update",
                "--priority",
                "high",
                "--selected-ids",
                json.dumps([gap_id, bulk_gap_id]),
            ])
            assert rc == 0, err
            payload = _json(out)
            assert payload["updated"] == 2, payload
            assert payload["field"] == "priority", payload
            assert payload["value"] == "high", payload
            assert payload["sync"]["ok"] is True, payload

            rc, out, err = _run_cli([
                *prefix,
                "gaps",
                "bulk-delete",
                "--selected-ids",
                f"{gap_id},{bulk_gap_id}",
            ])
            assert rc == 0, err
            payload = _json(out)
            assert payload["deleted"] == 2, payload
            assert payload["sync"]["ok"] is True, payload
        finally:
            cli._backend_runner_for_cli = old_runner_for_cli
            cli._sync_cli_refine_state = old_sync

        assert bulk_calls == [
            (
                M_BULK_UPDATE_GAPS,
                {"field": "priority", "value": "high", "gap_ids": [gap_id, bulk_gap_id]},
                30.0,
            ),
            (
                M_BULK_DELETE_GAPS,
                {"gap_ids": [gap_id, bulk_gap_id]},
                60.0,
            ),
        ], bulk_calls
        assert sync_calls == [
            "refine: bulk update gaps",
            "refine: bulk delete gaps",
        ], sync_calls
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("CLI gaps and changes tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
