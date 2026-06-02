"""CLI parity for dashboard summary data."""
from __future__ import annotations

import json
import sys
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

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
    tmp, client = make_client_repo("refine-cli-dashboard-")
    conn = init_refine(client)
    try:
        create_indexed_gap(conn, "01CLIDASHBOARDTODO00000000", status="todo")
        create_indexed_gap(conn, "01CLIDASHBOARDFAILED000000", status="failed")
        conn.commit()
        cfg = str(client / ".refine" / "refine.toml")

        rc, out, err = _run_cli(["--config", cfg, "dashboard", "summary"])
        assert rc == 0, err
        payload = _json(out)
        assert payload["counts"]["todo"] == 1, payload
        assert payload["counts"]["failed"] == 1, payload
        assert payload["node_scope"] == "current", payload
        assert payload["runner_reachable"] is False, payload
        assert any(
            item.get("filter", {}).get("status") == "failed"
            for item in payload["needs_attention"]
        )

        rc, out, err = _run_cli(["--config", cfg, "dashboard", "summary", "--node", "all"])
        assert rc == 0, err
        assert _json(out)["node_scope"] == "all"
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("CLI dashboard tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
