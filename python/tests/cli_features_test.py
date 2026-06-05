"""CLI parity for Feature shared operations."""
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
    tmp, client = make_client_repo("refine-cli-features-")
    conn = init_refine(client)
    try:
        from refine_server.ulid import new_ulid

        cfg = str(client / ".refine" / "refine.toml")
        prefix = ["--config", cfg]

        rc, out, err = _run_cli([
            *prefix,
            "features",
            "create",
            "--name",
            "Settings redesign",
            "--description",
            "Plan settings work",
            "--reporter",
            "Ada",
        ])
        assert rc == 0, err
        created = _json(out)
        feature_id = created["feature"]["id"]
        assert created["feature"]["name"] == "Settings redesign", created
        assert created["sync"]["stage"] in {"skipped", "synced"}, created

        rc, out, err = _run_cli([*prefix, "features", "list", "--q", "Settings"])
        assert rc == 0, err
        listed = _json(out)
        assert listed["features"][0]["id"] == feature_id, listed
        assert listed["features"][0]["gap_count"] == 0, listed

        gap_ids = [new_ulid(), new_ulid()]
        create_indexed_gap(conn, gap_ids[0], status="done")
        create_indexed_gap(conn, gap_ids[1], status="todo")
        conn.commit()
        for gap_id in gap_ids:
            rc, out, err = _run_cli([*prefix, "features", "add-gap", feature_id, gap_id])
            assert rc == 0, err
        detail = _json(out)["feature"]
        assert [g["id"] for g in detail["gaps"]] == gap_ids, detail
        assert detail["status"] == "todo", detail

        rc, out, err = _run_cli([
            *prefix,
            "features",
            "reorder",
            feature_id,
            gap_ids[1],
            "--before",
            gap_ids[0],
        ])
        assert rc == 0, err
        reordered = _json(out)["feature"]
        assert [g["id"] for g in reordered["gaps"]] == [gap_ids[1], gap_ids[0]], reordered
        assert [g["feature_order"] for g in reordered["gaps"]] == [1, 2], reordered

        rc, out, err = _run_cli([*prefix, "features", "remove-gap", feature_id, gap_ids[1]])
        assert rc == 0, err
        removed = _json(out)["feature"]
        assert [g["id"] for g in removed["gaps"]] == [gap_ids[0]], removed

        rc, out, err = _run_cli([*prefix, "features", "show", feature_id.lower()])
        assert rc == 0, err
        shown = _json(out)["feature"]
        assert shown["id"] == feature_id, shown
        assert shown["gap_count"] == 1, shown

        rc, out, err = _run_cli([
            *prefix,
            "features",
            "create",
            "--name",
            "Empty cleanup",
        ])
        assert rc == 0, err
        empty_feature_id = _json(out)["feature"]["id"]
        rc, out, err = _run_cli([*prefix, "features", "delete", empty_feature_id])
        assert rc == 1, (out, err)
        assert "requires --yes" in out, out

        from refine_cli import cli

        old_runner_for_cli = cli._backend_runner_for_cli
        old_sync = cli._sync_cli_refine_state
        cli._backend_runner_for_cli = (
            lambda _ctx, _port: (cli.config.get(reload=True), lambda _m, _p, _t: {"ok": True})
        )
        cli._sync_cli_refine_state = (
            lambda _cfg, *, message, rebuild_cache=True: {"ok": True, "message": message}
        )
        try:
            rc, out, err = _run_cli([*prefix, "features", "cancel", empty_feature_id])
            assert rc == 0, err
            cancelled = _json(out)
            assert cancelled["cancelled"] == 0, cancelled

            rc, out, err = _run_cli([*prefix, "features", "delete", empty_feature_id, "--yes"])
            assert rc == 0, err
            deleted = _json(out)
            assert deleted["deleted"] is True, deleted
        finally:
            cli._backend_runner_for_cli = old_runner_for_cli
            cli._sync_cli_refine_state = old_sync

        print("[ok] Feature CLI uses shared create/list/show/order operations")
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("\nALL OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
