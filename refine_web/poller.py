"""Background thread that tails SQLite for changes and publishes SSE events.

The runner writes activity + status changes to SQLite. The webapp polls and
broadcasts events to connected SSE subscribers.
"""
from __future__ import annotations

import sqlite3
import threading
import time

from refine_shared import activity, db

from . import sse


class SqlitePoller:
    def __init__(self, interval: float = 1.0) -> None:
        self.interval = interval
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._last_activity_id: int = 0
        # last-seen status by gap id; detect transitions
        self._last_status: dict[str, tuple[str, str]] = {}  # gap_id -> (status, updated)
        # last-seen runs.last_output_at by gap id; detect streaming subprocess output
        self._last_run_output: dict[str, str] = {}

    def start(self) -> None:
        self._thread = threading.Thread(target=self._loop, name="refine-poller",
                                        daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()

    def _conn(self) -> sqlite3.Connection:
        return db.connect()

    def _loop(self) -> None:
        # Initialize cursor at the latest existing activity id (don't replay history).
        try:
            conn = self._conn()
            row = conn.execute("SELECT COALESCE(MAX(id), 0) AS m FROM activity").fetchone()
            self._last_activity_id = int(row["m"] or 0)
            conn.close()
        except Exception:
            pass

        while not self._stop.is_set():
            try:
                self._tick()
            except Exception:
                pass
            self._stop.wait(self.interval)

    def _tick(self) -> None:
        conn = self._conn()
        try:
            # New activity entries
            new_entries = activity.recent(
                conn, limit=200, since_id=self._last_activity_id,
            )
            # `recent` returns DESC by id; iterate ascending so subscribers see order
            for entry in reversed(new_entries):
                self._last_activity_id = max(self._last_activity_id, int(entry["id"]))
                sse.publish("activity_added", entry)

            # Status changes
            rows = conn.execute(
                "SELECT id, status, updated FROM gaps_index"
            ).fetchall()
            for r in rows:
                gid = r["id"]
                pair = (r["status"], r["updated"])
                prev = self._last_status.get(gid)
                if prev != pair:
                    self._last_status[gid] = pair
                    if prev is not None:
                        sse.publish("status_change", {
                            "gap_id": gid,
                            "from": prev[0],
                            "to": pair[0],
                            "updated": pair[1],
                        })

            # Active-run streaming output: when a runner flushes lines to a
            # round's logs[], it bumps runs.last_output_at. We can't watch the
            # gap.json file from here, but this column lets us nudge clients
            # to refresh the round view.
            for r in conn.execute(
                "SELECT gap_id, last_output_at FROM runs "
                "WHERE finished_at IS NULL"
            ):
                gid = r["gap_id"]
                ts = r["last_output_at"]
                prev = self._last_run_output.get(gid)
                if ts and ts != prev:
                    self._last_run_output[gid] = ts
                    if prev is not None:  # skip first-seen so we don't replay
                        sse.publish("round_log_added", {"gap_id": gid})
        finally:
            conn.close()
