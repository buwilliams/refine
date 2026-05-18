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

    tmp = Path(tempfile.mkdtemp(prefix="refine-db-tx-"))
    db_path = tmp / "refine.sqlite3"
    db.init_db(db_path)
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
        conn.close()
        shutil.rmtree(tmp, ignore_errors=True)

    print("db transaction tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
