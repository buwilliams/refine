"""Periodic auto-commit of durable Refine project state under `.refine/**`.

The runner writes to `gap.json` (and friends under `.refine/`) as part of its
normal operation. Per-round logs, SQLite, PID files, and process logs are local
runtime state and are deliberately excluded. This committer wakes periodically,
collects sync-worthy dirty `.refine/` paths, and rolls them up into a single
commit on the currently checked-out branch.

We deliberately scope to `.refine/` — user files outside that directory are
the operator's concern and aren't auto-touched.
"""
from __future__ import annotations

import threading
from typing import Callable

from refine_server import activity, config

from . import git_ops


class StateCommitter:
    def __init__(
        self,
        get_conn: Callable,
        *,
        mutation_blocked: Callable[[], bool] | None = None,
        interval: float = 30.0,
    ) -> None:
        self.get_conn = get_conn
        self.mutation_blocked = mutation_blocked
        self.interval = interval
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def start(self) -> None:
        self._thread = threading.Thread(
            target=self._loop, name="refine-state-committer", daemon=True,
        )
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()

    def commit_now(self) -> bool:
        """Synchronously commit any dirty .refine/** paths. Safe to call from
        other code paths that want a clean tree (e.g., right before verify).
        Returns True if anything was committed.
        """
        return self._tick()

    def _loop(self) -> None:
        # Initial brief delay so the runner finishes wiring up.
        if self._stop.wait(self.interval):
            return
        while not self._stop.is_set():
            try:
                self._tick()
            except Exception:
                pass
            self._stop.wait(self.interval)

    def _tick(self) -> bool:
        if self.mutation_blocked is not None and self.mutation_blocked():
            return False
        config.ensure_refine_gitignore(config.get().volume_root)
        paths = git_ops.dirty_paths_under(".refine")
        if not paths:
            return False
        r = git_ops.commit_refine_sync_state(paths)
        if not r.ok:
            return False
        if r.stderr == "(nothing to commit)":
            return False
        try:
            syncable_count = len(git_ops.syncable_refine_paths(paths))
            activity.append(
                self.get_conn(),
                message=(
                    f"Auto-committed Refine project state "
                    f"({syncable_count} sync path{'' if syncable_count == 1 else 's'})"
                ),
                severity="info", category="git", actor="runner",
            )
        except Exception:
            pass
        return True
