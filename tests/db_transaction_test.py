"""Focused tests for SQLite transaction behavior on the shared runner connection."""
from __future__ import annotations

import sqlite3
import sys
import tempfile
import threading
import time
import shutil
from pathlib import Path


def main() -> int:
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

    from refine_server import db
    from refine_server import runner as runner_mod

    tmp = Path(tempfile.mkdtemp(prefix="refine-db-tx-"))
    db_path = tmp / "refine.sqlite3"
    db.init_db(db_path)
    assert db._shared_transaction_lock_count() == 0  # noqa: SLF001
    for _ in range(10):
        short_conn = db.connect(db_path)
        try:
            with db.transaction(short_conn):
                short_conn.execute("SELECT 1").fetchone()
        finally:
            short_conn.close()
    assert db._shared_transaction_lock_count() == 0  # noqa: SLF001

    conn = sqlite3.connect(
        str(db_path),
        isolation_level=None,
        check_same_thread=False,
        timeout=5.0,
    )
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA journal_mode = WAL")
    conn.execute("PRAGMA synchronous = NORMAL")
    conn.execute("PRAGMA foreign_keys = ON")
    db.register_shared_connection(conn)
    assert db._shared_transaction_lock_count() == 1  # noqa: SLF001

    try:
        # Nested writes on the same shared connection should use a savepoint,
        # not try to start a second top-level transaction.
        with db.transaction(conn):
            conn.execute(
                "INSERT INTO activity "
                "(datetime, severity, category, message) "
                "VALUES ('outer', 'info', 'state', 'outer')"
            )
            with db.transaction(conn):
                conn.execute(
                    "INSERT INTO activity "
                    "(datetime, severity, category, message) "
                    "VALUES ('inner', 'info', 'state', 'inner')"
                )
        rows = conn.execute(
            "SELECT message FROM activity ORDER BY id"
        ).fetchall()
        assert [r["message"] for r in rows] == ["outer", "inner"]

        with db.transaction(conn):
            conn.execute(
                "INSERT INTO activity "
                "(datetime, severity, category, message) "
                "VALUES ('kept', 'info', 'state', 'kept')"
            )
            try:
                with db.transaction(conn):
                    conn.execute(
                        "INSERT INTO activity "
                        "(datetime, severity, category, message) "
                        "VALUES ('rolled-back', 'info', 'state', 'rolled-back')"
                    )
                    raise RuntimeError("rollback inner savepoint")
            except RuntimeError:
                pass
        rows = conn.execute(
            "SELECT message FROM activity ORDER BY id"
        ).fetchall()
        assert [r["message"] for r in rows] == ["outer", "inner", "kept"]

        try:
            with db.transaction(conn):
                conn.execute("ROLLBACK")
                raise IndexError("tuple index out of range")
            raise AssertionError("transaction should have raised the original error")
        except IndexError as e:
            assert "tuple index out of range" in str(e)

        # Runner background services share one check_same_thread=False
        # connection. Concurrent writers should serialize instead of racing
        # into "cannot start a transaction within a transaction".
        barrier = threading.Barrier(6)
        errors: list[BaseException] = []

        def write_one(idx: int) -> None:
            try:
                barrier.wait(timeout=5.0)
                with db.transaction(conn):
                    time.sleep(0.02)
                    conn.execute(
                        "INSERT INTO activity "
                        "(datetime, severity, category, message) "
                        "VALUES (?, 'info', 'state', ?)",
                        (f"thread-{idx}", f"thread-{idx}"),
                    )
            except BaseException as e:
                errors.append(e)

        threads = [
            threading.Thread(target=write_one, args=(i,))
            for i in range(6)
        ]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(timeout=5.0)

        assert not errors, errors
        count = conn.execute(
            "SELECT COUNT(*) AS n FROM activity WHERE message LIKE 'thread-%'"
        ).fetchone()["n"]
        assert count == 6, count
    finally:
        db.unregister_shared_connection(conn)
        assert db._shared_transaction_lock_count() == 0  # noqa: SLF001
        conn.close()
        shutil.rmtree(tmp, ignore_errors=True)

    runner = runner_mod.Runner.__new__(runner_mod.Runner)
    runner._conn_lock = threading.Lock()  # noqa: SLF001
    runner._diag_lock = threading.Lock()  # noqa: SLF001
    runner._last_call_at = None  # noqa: SLF001
    runner._recent_errors = []  # noqa: SLF001
    runner._conn = object()  # noqa: SLF001
    runner.local_node_id = "default"
    active_calls = 0
    max_active_calls = 0
    active_lock = threading.Lock()
    dispatches: list[str] = []

    old_ensure_current = runner_mod.project_state.ensure_sqlite_cache_current

    def fake_ensure_current(_conn, *, node_id=None):  # noqa: ANN001
        nonlocal active_calls, max_active_calls
        with active_lock:
            active_calls += 1
            max_active_calls = max(max_active_calls, active_calls)
        time.sleep(0.02)
        with active_lock:
            active_calls -= 1
        return node_id or "default"

    def fake_dispatch(method, params):  # noqa: ANN001
        dispatches.append(str(method))
        return {"ok": True}

    runner._dispatch_method = fake_dispatch  # type: ignore[method-assign]  # noqa: SLF001
    barrier = threading.Barrier(4)
    call_errors: list[BaseException] = []

    def call_runner(idx: int) -> None:
        try:
            barrier.wait(timeout=5.0)
            runner.call(f"method-{idx}", {})
        except BaseException as e:
            call_errors.append(e)

    try:
        runner_mod.project_state.ensure_sqlite_cache_current = fake_ensure_current
        threads = [threading.Thread(target=call_runner, args=(i,)) for i in range(4)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(timeout=5.0)
    finally:
        runner_mod.project_state.ensure_sqlite_cache_current = old_ensure_current

    assert not call_errors, call_errors
    assert len(dispatches) == 4, dispatches
    assert max_active_calls == 1, max_active_calls

    print("db transaction tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
