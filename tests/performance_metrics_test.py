"""Focused tests for local performance metrics storage and aggregation."""
from __future__ import annotations

import shutil
import sys
import tempfile
from pathlib import Path


def main() -> int:
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

    from refine_server import db, perf_metrics

    tmp = Path(tempfile.mkdtemp(prefix="refine-perf-metrics-"))
    db_path = tmp / "refine.sqlite3"
    try:
        db.init_db(db_path)
        conn = db.connect(db_path)
        try:
            tables = {
                row["name"]
                for row in conn.execute(
                    "SELECT name FROM sqlite_master WHERE type = 'table'"
                )
            }
            assert "performance_events" in tables
            indexes = {
                row["name"]
                for row in conn.execute(
                    "SELECT name FROM sqlite_master WHERE type = 'index'"
                )
            }
            assert "idx_performance_operation" in indexes
            assert "idx_performance_occurred" in indexes
            assert "idx_performance_gap" in indexes
            assert "idx_performance_success" in indexes

            perf_metrics.record(
                "api.list_gaps",
                conn=conn,
                elapsed_ms=10,
                rows_scanned=12,
                rows_returned=3,
                details={"query_mode": "indexed"},
            )
            perf_metrics.record(
                "api.list_gaps",
                conn=conn,
                elapsed_ms=30,
                success=False,
                rows_scanned=12,
                rows_returned=0,
            )
            perf_metrics.record(
                "gap_json_read",
                conn=conn,
                elapsed_ms=5,
                gap_id="01PERFMETRICSGAPID000000",
            )

            snap = perf_metrics.snapshot(conn)
            by_op = {row["operation"]: row for row in snap["summary"]}
            assert by_op["api.list_gaps"]["count"] == 2
            assert by_op["api.list_gaps"]["failures"] == 1
            assert by_op["api.list_gaps"]["max_ms"] == 30
            assert snap["event_count"] == 3
            assert len(snap["recent"]) == 3

            filtered = perf_metrics.snapshot(conn, operation="gap_json_read")
            assert len(filtered["recent"]) == 1
            assert filtered["recent"][0]["gap_id"] == "01PERFMETRICSGAPID000000"

            deleted = perf_metrics.clear(conn)
            assert deleted == 3
            assert perf_metrics.snapshot(conn)["event_count"] == 0
        finally:
            conn.close()

        # Best-effort recording must never raise into callers.
        perf_metrics.record("closed_conn_event", conn=conn, elapsed_ms=1)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print("performance metrics tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
