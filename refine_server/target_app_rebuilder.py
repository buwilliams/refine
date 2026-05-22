"""Automatic target-application rebuild scheduling."""
from __future__ import annotations

import threading
from datetime import datetime, timezone
from typing import Callable

from refine_server import db, project_state


AUTO_REBUILD_MODES = ("never", "on_worktree_merge", "hourly", "nightly")
DEFAULT_AUTO_REBUILD_MODE = "never"
NIGHTLY_REBUILD_HOUR = 0


class TargetAppRebuilder:
    def __init__(
        self,
        *,
        get_conn: Callable,
        run_rebuild: Callable[[str], dict],
        interval: float = 15.0,
    ) -> None:
        self._get_conn = get_conn
        self._run_rebuild = run_rebuild
        self._interval = interval
        self._wake = threading.Event()
        self._stop = threading.Event()
        self._state_lock = threading.Lock()
        self._thread: threading.Thread | None = None
        self._queued = False
        self._running = False
        self._last_reason = ""
        self._last_mode: str | None = None

    def start(self) -> None:
        self._thread = threading.Thread(
            target=self._loop, name="refine-target-app-rebuilder", daemon=True,
        )
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._wake.set()

    def queue_rebuild(self, reason: str, *, mode: str | None = None) -> bool:
        with self._state_lock:
            already_pending = self._queued
            self._queued = True
            self._last_reason = reason
            self._last_mode = mode
        self._wake.set()
        return not already_pending

    def queue_for_worktree_merge(self, gap_id: str) -> bool:
        if self._mode() != "on_worktree_merge":
            return False
        return self.queue_rebuild(
            f"worktree merge for Gap {gap_id}",
            mode="on_worktree_merge",
        )

    def queue_pending_awaiting_rebuild(self) -> bool:
        if self._mode() != "on_worktree_merge":
            return False
        with self._state_lock:
            if self._queued or self._running:
                return False
        row = self._get_conn().execute(
            "SELECT COUNT(*) AS n FROM gaps_index "
            "WHERE status = 'awaiting-rebuild' AND instance_id = ?",
            (project_state.active_instance_id(),),
        ).fetchone()
        n = int(row["n"] if row else 0)
        if n <= 0:
            return False
        return self.queue_rebuild(
            f"{n} Gap{'' if n == 1 else 's'} awaiting target-app rebuild",
            mode="on_worktree_merge",
        )

    def snapshot(self) -> dict:
        with self._state_lock:
            return {
                "mode": self._mode(),
                "running": self._running,
                "queued": self._queued,
                "last_reason": self._last_reason,
            }

    def _loop(self) -> None:
        while not self._stop.is_set():
            try:
                self._queue_scheduled_rebuild_if_due()
                self._drain_queue()
            except Exception:
                pass
            self._wake.wait(timeout=self._interval)
            self._wake.clear()

    def _drain_queue(self) -> None:
        while not self._stop.is_set():
            with self._state_lock:
                if not self._queued:
                    return
                self._queued = False
                self._running = True
                reason = self._last_reason or "automatic rebuild"
                queued_mode = self._last_mode
            try:
                if queued_mode is None or self._mode() == queued_mode:
                    self._run_rebuild(reason)
            finally:
                with self._state_lock:
                    self._running = False
                    if not self._queued:
                        self._last_mode = None

    def _queue_scheduled_rebuild_if_due(self, now: datetime | None = None) -> None:
        mode = self._mode()
        if mode == "on_worktree_merge":
            self.queue_pending_awaiting_rebuild()
            return
        if mode not in ("hourly", "nightly"):
            return
        now = now or datetime.now().astimezone()
        last = _parse_iso(db.get_setting(
            self._get_conn(), "target_app_auto_rebuild_last_started_at", "",
        ) or "")
        if mode == "hourly":
            elapsed = (
                None if last is None
                else (now.astimezone(timezone.utc) - last).total_seconds()
            )
            if elapsed is None or elapsed >= 3600:
                self.queue_rebuild("hourly automatic rebuild", mode="hourly")
            return
        # Run once per local day as soon as the scheduler sees the local date
        # roll over during the midnight hour. If Refine starts later in the
        # day, wait for the next nightly window instead of rebuilding on boot.
        if now.hour != NIGHTLY_REBUILD_HOUR:
            return
        if last is not None and last.astimezone().date() == now.date():
            return
        self.queue_rebuild("nightly automatic rebuild", mode="nightly")

    def _mode(self) -> str:
        mode = (db.get_setting(
            self._get_conn(), "target_app_auto_rebuild", DEFAULT_AUTO_REBUILD_MODE,
        ) or DEFAULT_AUTO_REBUILD_MODE).strip()
        return mode if mode in AUTO_REBUILD_MODES else DEFAULT_AUTO_REBUILD_MODE


def _parse_iso(value: str) -> datetime | None:
    raw = (value or "").strip()
    if not raw:
        return None
    try:
        return datetime.fromisoformat(raw.replace("Z", "+00:00")).astimezone(timezone.utc)
    except ValueError:
        return None
