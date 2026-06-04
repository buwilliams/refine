"""Round-count filters for Gaps list API and CLI."""
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
    tmp, client = make_client_repo("refine-gaps-round-filter-")
    conn = init_refine(client)
    try:
        from refine_server import gap_ops, gap_writer, gaps
        from refine_ui import api

        one = "01ROUNDCOUNTONE00000000000"
        two = "01ROUNDCOUNTTWO00000000000"
        three = "01ROUNDCOUNTTHR00000000000"
        for gap_id in (one, two, three):
            create_indexed_gap(conn, gap_id, status="todo")

        gap_writer.append_round(
            two,
            gaps.new_round("Reporter", "Second actual", "Second target"),
        )
        gap_writer.append_round(
            three,
            gaps.new_round("Reporter", "Second actual", "Second target"),
        )
        gap_writer.append_round(
            three,
            gaps.new_round("Reporter", "Third actual", "Third target"),
        )
        for gap_id in (two, three):
            gap = gaps.read_gap_json(gap_id, include_logs=False) or {}
            conn.execute(
                "UPDATE gaps_index SET round_count = ?, updated = ? WHERE id = ?",
                (len(gap.get("rounds") or []), gap.get("updated"), gap_id),
            )
        conn.commit()

        code, payload = api.list_gaps(rounds_gte=2, limit=10)
        assert code == 200, payload
        assert {g["id"] for g in payload["gaps"]} == {two, three}, payload
        assert all(g["round_count"] >= 2 for g in payload["gaps"]), payload

        code, payload = api.list_gaps(rounds_gte=2, limit=1, include_facets=True)
        assert code == 200, payload
        assert payload["facets"]["status_counts"] == {"todo": 2}, payload
        assert len(payload["gaps"]) == 1, payload

        code, payload = api.list_gaps(rounds_lte=2, limit=10)
        assert code == 200, payload
        assert {g["id"] for g in payload["gaps"]} == {one, two}, payload

        code, payload = api.list_gaps(rounds_gte=2, rounds_lte=2, limit=10)
        assert code == 200, payload
        assert [g["id"] for g in payload["gaps"]] == [two], payload

        code, selected = gap_ops.select_bulk_update_candidates(
            conn,
            {"rounds_gte": "2", "rounds_lte": "2"},
            set(),
            skip_automated=False,
        )
        assert code == 200, selected
        assert [g["id"] for g in selected["gaps"]] == [two], selected

        code, payload = api.list_gaps(rounds_gte="", rounds_lte="", limit=10)
        assert code == 200, payload
        assert {g["id"] for g in payload["gaps"]} == {one, two, three}, payload

        code, payload = api.list_gaps(rounds_gte="many")
        assert code == 400, payload
        assert "rounds_gte must be a non-negative integer" in payload["error"]["message"]

        cfg = str(client / ".refine" / "refine.toml")
        prefix = ["--config", cfg]
        rc, out, err = _run_cli([
            *prefix,
            "gaps",
            "list",
            "--rounds-gte",
            "2",
            "--rounds-lte",
            "2",
            "--limit",
            "10",
        ])
        assert rc == 0, err
        payload = _json(out)
        assert [g["id"] for g in payload["gaps"]] == [two], payload

        root = Path(__file__).resolve().parents[1]
        gaps_list_js = (
            root / "refine_ui/static/js/features/gaps-list.js"
        ).read_text(encoding="utf-8")
        gaps_bulk_js = (
            root / "refine_ui/static/js/features/gaps-bulk.js"
        ).read_text(encoding="utf-8")
        assert 'id="filter-rounds-gte"' in gaps_list_js
        assert 'id="filter-rounds-lte"' in gaps_list_js
        assert 'params.set("rounds_gte", f.rounds_gte)' in gaps_list_js
        assert 'params.set("rounds_lte", f.rounds_lte)' in gaps_list_js
        assert "rounds_gte: f.rounds_gte" in gaps_bulk_js
        assert "rounds_lte: f.rounds_lte" in gaps_bulk_js
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("gaps round filter tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
