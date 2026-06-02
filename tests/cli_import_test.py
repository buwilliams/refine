"""CLI parity for Gap import operations."""
from __future__ import annotations

import json
import sys
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from refine_server.backend_protocol import M_CREATE_GAP, M_EXTRACT_GAPS
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
    tmp, client = make_client_repo("refine-cli-import-")
    conn = init_refine(client)
    try:
        create_indexed_gap(conn, "01CLIIMPORTDEDUP000000000", status="review")
        conn.commit()
        cfg = str(client / ".refine" / "refine.toml")
        prefix = ["--config", cfg]

        csv_path = client / "import.csv"
        csv_path.write_text(
            "actual,target,reporter,priority,name\n"
            "Current CLI import,Target CLI import,Reporter,high,CLI import\n",
            encoding="utf-8",
        )
        rc, out, err = _run_cli([*prefix, "import", "parse-csv", str(csv_path)])
        assert rc == 0, err
        payload = _json(out)
        assert payload["count"] == 1, payload
        assert payload["drafts"][0]["name"] == "CLI import", payload
        assert payload["drafts"][0]["priority"] == "high", payload

        duplicate_draft = [{
            "actual": "Current behavior for 01CLIIMPORTDEDUP000000000",
            "target": "Target behavior for 01CLIIMPORTDEDUP000000000",
        }]
        rc, out, err = _run_cli([
            *prefix,
            "import",
            "dedup",
            json.dumps(duplicate_draft),
        ])
        assert rc == 0, err
        payload = _json(out)
        assert payload["matches"][0]["match"]["id"] == "01CLIIMPORTDEDUP000000000", payload

        from refine_cli import cli

        calls: list[tuple[str, dict[str, object], float]] = []

        def fake_runner(method: str, params: dict[str, object], timeout: float) -> dict:
            calls.append((method, params, timeout))
            if method == M_EXTRACT_GAPS:
                return {"drafts": [{"actual": "A", "target": "T"}]}
            if method == M_CREATE_GAP:
                return {"gap": {"id": params["gap_id"]}}
            return {"ok": True}

        sync_calls: list[str] = []
        old_runner_for_cli = cli._backend_runner_for_cli
        old_sync = cli._sync_cli_refine_state
        cli._backend_runner_for_cli = lambda _ctx, _port: (cli.config.get(reload=True), fake_runner)
        cli._sync_cli_refine_state = (
            lambda _cfg, *, message, rebuild_cache=True: sync_calls.append(message)
            or {"ok": True, "message": message}
        )
        try:
            rc, out, err = _run_cli([*prefix, "import", "extract", "rough notes"])
            assert rc == 0, err
            assert _json(out)["drafts"] == [{"actual": "A", "target": "T"}]
            assert calls[-1][0] == M_EXTRACT_GAPS
            assert calls[-1][1] == {"text": "rough notes"}

            persist_body = {
                "reporter": "Reporter",
                "drafts": [{
                    "name": "CLI persisted",
                    "actual": "Actual",
                    "target": "Target",
                    "priority": "medium",
                    "duplicate_decision": "original",
                }],
            }
            rc, out, err = _run_cli([
                *prefix,
                "import",
                "persist",
                json.dumps(persist_body),
            ])
        finally:
            cli._backend_runner_for_cli = old_runner_for_cli
            cli._sync_cli_refine_state = old_sync

        assert rc == 0, err
        payload = _json(out)
        assert payload["count"] == 1, payload
        assert payload["sync"]["ok"] is True, payload
        assert sync_calls == ["refine: import gaps"], sync_calls
        create_call = [call for call in calls if call[0] == M_CREATE_GAP][-1]
        assert create_call[1]["reporter"] == "Reporter", create_call
        assert create_call[1]["priority"] == "medium", create_call
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("CLI import tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
