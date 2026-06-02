"""Forced vs incremental SQLite cache rebuild tests."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-cache-rebuild-force-")
    conn = init_refine(client)
    try:
        from refine_server import config, db, gap_writer, project_state
        from refine_ui import api, background_jobs

        gids = [
            "01FORCEREBUILDAAAAAAAAAAA",
            "01FORCEREBUILDBBBBBBBBBBB",
            "01FORCEREBUILDCCCCCCCCCCC",
        ]
        for gid in gids:
            create_indexed_gap(conn, gid, status="todo")
        gap_writer.set_round_guidance_decision(
            gids[0],
            0,
            {
                "accepted_names": ["Force rebuild"],
                "accepted": [{"name": "Force rebuild"}],
                "details": {"reason": "test"},
                "round_fingerprint": "round",
                "guidance_fingerprint": "guidance",
                "decided_at": "2026-05-21T00:00:00Z",
            },
        )

        project_state.rebuild_sqlite_cache(conn)
        original_read_cache_bytes = project_state._read_gap_cache_bytes
        cache_reads: list[str] = []
        progress_updates: list[tuple[int, int, str]] = []

        def counted_cache_read(path: Path) -> bytes:
            cache_reads.append(path.relative_to(client / ".refine").as_posix())
            return original_read_cache_bytes(path)

        def record_progress(completed: int, total: int, message: str) -> None:
            progress_updates.append((completed, total, message))

        project_state._read_gap_cache_bytes = counted_cache_read
        try:
            project_state.rebuild_sqlite_cache(conn)
            assert cache_reads == [], cache_reads

            conn.execute("DELETE FROM guidance_decisions")
            conn.execute("DELETE FROM gap_search_docs")
            cache_reads.clear()
            project_state.rebuild_sqlite_cache(
                conn,
                force=True,
                progress=record_progress,
            )
        finally:
            project_state._read_gap_cache_bytes = original_read_cache_bytes

        expected = {
            f"gaps/{gid[:2]}/{gid[2:]}/gap.json"
            for gid in gids
        }
        assert expected.issubset(set(cache_reads)), cache_reads
        assert progress_updates, progress_updates
        assert progress_updates[0][1] == len(gids), progress_updates
        assert progress_updates[-1][0] == len(gids), progress_updates
        assert progress_updates[-1][1] == len(gids), progress_updates

        gap_count = conn.execute(
            "SELECT COUNT(*) AS n FROM gaps_index WHERE id IN (?, ?, ?)",
            gids,
        ).fetchone()["n"]
        assert gap_count == len(gids), gap_count
        search_count = conn.execute(
            "SELECT COUNT(*) AS n FROM gap_search_docs WHERE gap_id IN (?, ?, ?)",
            gids,
        ).fetchone()["n"]
        assert search_count == len(gids), search_count
        meta_count = conn.execute(
            "SELECT COUNT(*) AS n FROM gap_cache_meta WHERE gap_id IN (?, ?, ?)",
            gids,
        ).fetchone()["n"]
        assert meta_count == len(gids), meta_count
        decision = conn.execute(
            "SELECT accepted_json FROM guidance_decisions WHERE gap_id = ?",
            (gids[0],),
        ).fetchone()
        assert decision is not None
        assert "Force rebuild" in decision["accepted_json"], decision["accepted_json"]

        conn.close()
        cfg = config.get(reload=True)
        for suffix in ("", "-wal", "-shm"):
            try:
                Path(f"{cfg.sqlite_path}{suffix}").unlink()
            except FileNotFoundError:
                pass
        conn = db.connect(cfg.sqlite_path)
        assert db.schema_ready(conn) is False
        assert (
            project_state.ensure_sqlite_cache_current(conn)
            == project_state.active_node_id()
        )
        assert db.schema_ready(conn) is True
        gap_count = conn.execute(
            "SELECT COUNT(*) AS n FROM gaps_index WHERE id IN (?, ?, ?)",
            gids,
        ).fetchone()["n"]
        assert gap_count == len(gids), gap_count

        status, body = api.rebuild_sqlite_cache({
            "restart_services": False,
            "background": True,
        })
        assert status == 202, body
        job_id = body["job"]["id"]
        import time

        for _ in range(100):
            job = background_jobs.snapshot(job_id)
            if job and job["status"] in {"complete", "failed"}:
                break
            time.sleep(0.05)
        assert job and job["status"] == "complete", job
        assert job["progress"]["completed"] == job["progress"]["total"], job
        assert job["result"]["http_status"] == 200, job
        assert job["result"]["mode"] in {"rebuilt", "recreated"}, job

        conn.execute("DROP INDEX IF EXISTS idx_performance_operation")
        conn.execute("DROP INDEX IF EXISTS idx_performance_occurred")
        conn.execute("DROP INDEX IF EXISTS idx_performance_gap")
        conn.execute("DROP INDEX IF EXISTS idx_performance_success")
        conn.execute("DROP TABLE performance_events")
        missing = conn.execute(
            "SELECT name FROM sqlite_master WHERE type = 'table' "
            "AND name = 'performance_events'",
        ).fetchone()
        assert missing is None

        status, body = api.rebuild_sqlite_cache({"restart_services": False})
        assert status == 200, body
        assert body["mode"] == "rebuilt", body
        migrated = conn.execute(
            "SELECT name FROM sqlite_master WHERE type = 'table' "
            "AND name = 'performance_events'",
        ).fetchone()
        assert migrated is not None
        event = conn.execute(
            "SELECT operation FROM performance_events "
            "WHERE operation = 'sqlite_cache_rebuild' "
            "ORDER BY id DESC LIMIT 1",
        ).fetchone()
        assert event is not None
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("cache rebuild force tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
