"""Background thread that tails SQLite for changes and publishes SSE events.

The runner writes activity + status changes to SQLite. The webapp polls and
broadcasts events to connected SSE subscribers.
"""
from __future__ import annotations

import sqlite3
import threading
import time

from refine_server import activity, db, project_state, project_sync

from . import sse


class SqlitePoller:
    def __init__(self, interval: float = 1.0,
                 target_app_health_interval: float = 15.0) -> None:
        self.interval = interval
        # How often to probe the configured target-app health URL. Polled
        # less aggressively than the per-second SQLite scan since the
        # check is a real HTTP roundtrip.
        self.target_app_health_interval = target_app_health_interval
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._last_activity_id: int = 0
        # last-seen status by gap id; detect transitions
        self._last_status: dict[str, tuple[str, str]] = {}  # gap_id -> (status, updated)
        # last-seen runs.last_output_at by gap id; detect streaming subprocess output
        self._last_run_output: dict[str, str] = {}
        # When the last target-app health check ran (monotonic seconds);
        # the actual outcome is persisted in SQLite via the api helper.
        self._last_target_app_health_at: float = 0.0
        # Previously-observed target-app state — emit an SSE event when
        # it transitions so the nav button updates without a poll.
        self._last_target_app_state: str | None = None
        self._last_project_update_pulse_at: float = 0.0

    def start(self) -> None:
        self._thread = threading.Thread(target=self._loop, name="refine-poller",
                                        daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        if (
            self._thread is not None
            and self._thread.is_alive()
            and self._thread is not threading.current_thread()
        ):
            self._thread.join(timeout=2.0)
        self._thread = None

    def _conn(self) -> sqlite3.Connection:
        conn = db.connect()
        project_state.ensure_sqlite_cache_current(conn)
        return conn

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

            # Detect target-app state transitions (start/stop commands flip
            # the setting from another request handler; we re-broadcast so
            # other browser tabs update without polling).
            cur_state = db.get_setting(conn, "target_app_state") or "unknown"
            if self._last_target_app_state is None:
                self._last_target_app_state = cur_state
            elif cur_state != self._last_target_app_state:
                self._last_target_app_state = cur_state
                sse.publish("target_app_state", {"state": cur_state})
        finally:
            conn.close()

        # Periodic health probe — separate cadence from the SQLite tick so
        # we don't hammer the target-app's `/health` every second.
        now = time.monotonic()
        if (self.target_app_health_interval > 0
                and now - self._last_target_app_health_at
                >= self.target_app_health_interval):
            self._last_target_app_health_at = now
            try:
                self._run_target_app_health_check()
            except Exception:
                pass
        try:
            self._run_project_update_pulse(now)
        except Exception:
            pass

    def _run_target_app_health_check(self) -> None:
        """Probe configured target-app status checks and persist the result.

        Imports locally so an unconfigured webapp doesn't pull in the
        runner module on every tick. The api helper handles the
        no-checks-configured case as a no-op probe.
        """
        from . import api as web_api
        conn = self._conn()
        try:
            settings = db.list_settings(conn)
            has_checks = any((
                (settings.get("target_app_status_command") or "").strip(),
                (settings.get("target_app_http_check_url") or settings.get("target_app_health_url") or "").strip(),
                (settings.get("target_app_tcp_check_host") or "").strip()
                and (settings.get("target_app_tcp_check_port") or "").strip(),
                (settings.get("target_app_process_check_command") or "").strip(),
            ))
        finally:
            conn.close()
        if not has_checks:
            return
        snap = web_api._target_app_run_health_check()  # noqa: SLF001
        sse.publish("target_app_health", {
            "ok": snap.get("last_check_ok", snap.get("last_health_ok")),
            "at": snap.get("last_check_at", snap.get("last_health_at")),
            "state": snap.get("state"),
        })

    def _run_project_update_pulse(self, now: float) -> None:
        """Check for target-repo updates at the instance-configured cadence."""
        conn = self._conn()
        try:
            interval = db.get_setting_int(
                conn, "project_update_pulse_interval_seconds", 60,
            )
        finally:
            conn.close()
        if interval <= 0:
            return
        if now - self._last_project_update_pulse_at < interval:
            return
        self._last_project_update_pulse_at = now
        conn = self._conn()
        try:
            result = project_sync.pulse(conn, actor="runner")
        finally:
            conn.close()
        if result.get("ok") and result.get("changed"):
            sse.publish("project_updated", {
                "stage": result.get("stage"),
                "branch": result.get("branch"),
                "upstream": result.get("upstream"),
                "message": result.get("message"),
            })
