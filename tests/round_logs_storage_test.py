"""Per-Gap append-only round log storage tests."""
from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-round-logs-")
    conn = init_refine(client)
    try:
        from refine_server import gap_writer, gaps
        from refine_server.paths import gap_json_path, gap_logs_path
        from refine_ui import api

        gid = "01ROUNDLOGSSTORAGEAAAAAAA"
        create_indexed_gap(conn, gid)
        conn.commit()

        json_path = gap_json_path(gid)
        before = json_path.read_bytes()
        gap_writer.append_round_log(
            gap_id=gid,
            round_idx=0,
            message="first line",
            severity="info",
            category="cli",
        )
        gap_writer.append_round_log(
            gap_id=gid,
            round_idx=0,
            message="second line",
            severity="warn",
            category="cli",
        )
        gap_writer.append_round_log(
            gap_id=gid,
            round_idx=0,
            message="third line",
            severity="error",
            category="cli",
        )
        after = json_path.read_bytes()
        assert after == before, "log appends must not rewrite gap.json"

        logs_path = gap_logs_path(gid)
        assert logs_path.is_file()
        assert len(logs_path.read_text(encoding="utf-8").splitlines()) == 3

        hydrated = gaps.read_gap_json(gid)
        assert hydrated is not None
        assert [log["message"] for log in hydrated["rounds"][0]["logs"]] == [
            "first line",
            "second line",
            "third line",
        ]
        metadata_only = gaps.read_gap_json(gid, include_logs=False)
        assert "logs" not in metadata_only["rounds"][0]

        status, body = api.get_gap(gid)
        assert status == 200, body
        round_obj = body["gap"]["rounds"][0]
        assert round_obj["log_count"] == 3
        assert "logs" not in round_obj
        assert round_obj["latest_log"]["message"] == "third line"
        assert round_obj["latest_error_log"]["message"] == "third line"

        status, page1 = api.get_gap_round_logs(gid, 0, limit=2, offset=0)
        assert status == 200, page1
        assert [log["message"] for log in page1["logs"]] == ["first line", "second line"]
        assert page1["page"]["has_more"] is True

        status, page2 = api.get_gap_round_logs(gid, 0, limit=2, offset=2)
        assert status == 200, page2
        assert [log["message"] for log in page2["logs"]] == ["third line"]
        assert page2["page"]["has_more"] is False

        legacy = "01ROUNDLOGSLEGACYAAAAAAA"
        create_indexed_gap(conn, legacy)
        conn.commit()
        legacy_path = gap_json_path(legacy)
        legacy_gap = json.loads(legacy_path.read_text(encoding="utf-8"))
        legacy_gap["rounds"][0]["logs"] = [
            gaps.new_log_entry("legacy embedded", category="cli"),
        ]
        legacy_path.write_text(json.dumps(legacy_gap, indent=2), encoding="utf-8")
        gap_writer.update_fields(legacy, status="todo")
        migrated_raw = json.loads(legacy_path.read_text(encoding="utf-8"))
        assert "logs" not in migrated_raw["rounds"][0]
        migrated = gaps.read_gap_json(legacy)
        assert migrated["rounds"][0]["logs"][0]["message"] == "legacy embedded"

        print("[ok] round logs append to per-Gap JSONL and paginate")
        return 0
    finally:
        conn.close()
        cleanup_tmp(tmp)


if __name__ == "__main__":
    raise SystemExit(main())
