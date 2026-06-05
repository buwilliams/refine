"""CLI parity for managed process controls."""
from __future__ import annotations

import json
import sys
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from refine_server.backend_protocol import M_BACKGROUND_PROCESSES_SET, M_ENFORCE_SCHEDULING
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
    tmp, client = make_client_repo("refine-cli-processes-")
    conn = init_refine(client)
    conn.close()
    from refine_cli import cli
    from refine_server import config, db

    cfg = config.get(reload=True)
    calls: list[tuple[str, dict[str, object], float]] = []

    def fake_runner(method: str, params: dict[str, object], timeout: float) -> dict:
        calls.append((method, params, timeout))
        return {"ok": True, "method": method, "params": params}

    old_runner_for_cli = cli._backend_runner_for_cli
    cli._backend_runner_for_cli = lambda _ctx, _port: (cfg, fake_runner)
    try:
        rc, out, err = _run_cli(["processes", "list"])
        assert rc == 0, err
        payload = _json(out)
        kinds = [item["kind"] for item in payload["processes"]]
        assert kinds[:4] == ["supervisor", "ui", "runner", "target_app"], kinds
        ui_row = next(item for item in payload["processes"] if item["kind"] == "ui")
        assert ui_row["pid"] is None, payload
        assert payload["runner_reachable"] is False, payload

        rc, out, err = _run_cli(["processes", "background", "--stopped"])
        assert rc == 0, err
        payload = _json(out)
        assert payload["stopped"] is True
        assert payload["runner"]["method"] == M_BACKGROUND_PROCESSES_SET

        rc, out, err = _run_cli(["processes", "agents", "--paused"])
        assert rc == 0, err
        payload = _json(out)
        assert payload["paused"] is True
        assert payload["runner"]["method"] == M_ENFORCE_SCHEDULING

        check_conn = db.connect(cfg.sqlite_path)
        try:
            assert db.get_setting(check_conn, "paused") == "1"
            assert db.get_setting(check_conn, "agents_paused") == "1"
        finally:
            check_conn.close()
    finally:
        cli._backend_runner_for_cli = old_runner_for_cli
        cleanup_tmp(tmp)

    assert [call[0] for call in calls] == [
        M_BACKGROUND_PROCESSES_SET,
        M_ENFORCE_SCHEDULING,
    ]
    print("CLI process control tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
