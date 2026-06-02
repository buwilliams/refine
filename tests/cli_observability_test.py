"""CLI parity for activity, performance, and background job observability."""
from __future__ import annotations

import json
import sys
import time
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
    tmp, client = make_client_repo("refine-cli-observability-")
    conn = init_refine(client)
    try:
        from refine_server import activity, perf_metrics
        from refine_ui import background_jobs

        activity.append(
            conn,
            message="CLI activity entry",
            severity="info",
            category="test",
            actor="cli-test",
        )
        perf_metrics.record(
            "cli.test.operation",
            conn=conn,
            elapsed_ms=12.3,
            success=True,
            details={"source": "test"},
        )
        cfg = str(client / ".refine" / "refine.toml")
        prefix = ["--config", cfg]

        rc, out, err = _run_cli([*prefix, "activity", "list", "--limit", "5", "--facets"])
        assert rc == 0, err
        payload = _json(out)
        assert any(row["message"] == "CLI activity entry" for row in payload["activity"])
        assert payload["facets"]["categories"] == ["test"]
        assert payload["facets"]["actors"] == ["cli-test"]

        rc, out, err = _run_cli([
            *prefix,
            "performance",
            "summary",
            "--operation",
            "cli.test.operation",
        ])
        assert rc == 0, err
        payload = _json(out)
        assert payload["filtered_event_count"] >= 1
        assert any(row["operation"] == "cli.test.operation" for row in payload["recent"])

        job = background_jobs.start(
            "cli_observability",
            "CLI observability",
            lambda progress=None: {"ok": True},
        )
        job_id = str(job["id"])
        for _ in range(50):
            snap = background_jobs.snapshot(job_id)
            if snap and snap.get("status") in {"complete", "failed", "cancelled"}:
                break
            time.sleep(0.02)

        rc, out, err = _run_cli(["job", "get", job_id])
        assert rc == 0, err
        assert _json(out)["job"]["id"] == job_id

        rc, out, err = _run_cli(["job", "cancel", job_id])
        assert rc == 0, err
        assert _json(out)["job"]["id"] == job_id

        rc, out, err = _run_cli([*prefix, "performance", "cleanup"])
        assert rc == 0, err
        assert "deleted" in _json(out)

        rc, out, err = _run_cli([*prefix, "performance", "rebuild-cache"])
        assert rc == 0, err
        payload = _json(out)
        assert payload["ok"] is True, payload
        assert payload["mode"] == "rebuilt", payload
        assert payload["poller_restarted"] is False, payload

        rc, out, err = _run_cli([*prefix, "activity", "cleanup", "--days", "365"])
        assert rc == 0, err
        assert _json(out)["days_kept"] == 365
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("CLI observability tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
