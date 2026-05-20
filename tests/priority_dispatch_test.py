"""Focused scheduler tests for priority-gated dispatch.

These avoid a real agent CLI by replacing Dispatcher._launch_one and using a
fake SubprocessManager.
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path


class FakeSubprocessManager:
    def __init__(self) -> None:
        self.running: list[str] = []
        self.cancel_calls: list[tuple[str, str]] = []

    def running_snapshot(self) -> list[dict]:
        return [
            {
                "gap_id": gid,
                "round_idx": 0,
                "pid": 1000 + idx,
                "elapsed_seconds": 1,
                "idle_seconds": 1,
            }
            for idx, gid in enumerate(self.running)
        ]

    def is_running(self, gap_id: str) -> bool:
        return gap_id in self.running

    def cancel(self, gap_id: str, reason: str = "cancel") -> bool:
        self.cancel_calls.append((gap_id, reason))
        if gap_id in self.running:
            self.running.remove(gap_id)
        return True


def main() -> int:
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    tmp = Path(tempfile.mkdtemp(prefix="refine-priority-"))
    client = tmp / "client"
    client.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=client, check=True)
    subprocess.run(
        ["git", "-c", "user.email=t@x", "-c", "user.name=t",
         "commit", "--allow-empty", "-m", "init"],
        cwd=client,
        check=True,
    )
    os.chdir(client)

    try:
        from refine_server import config, db
        from refine_server.dispatcher import Dispatcher
        from refine_server.gaps import now_iso

        config.write_defaults(client / ".refine")
        config.get(reload=True)
        db.init_db()
        conn = db.connect()
        db.set_setting(conn, "backlog_promote_after_seconds", "-1")
        db.set_setting(conn, "parallel_run_cap", "3")

        fake = FakeSubprocessManager()
        dispatcher = Dispatcher(get_conn=lambda: conn, sub_mgr=fake)
        launched: list[str] = []

        def launch_one(_conn, gap_id: str, _branch: str | None) -> None:
            launched.append(gap_id)

        dispatcher._launch_one = launch_one  # type: ignore[method-assign]

        def reset() -> None:
            launched.clear()
            fake.running.clear()
            fake.cancel_calls.clear()
            conn.execute("DELETE FROM activity")
            conn.execute("DELETE FROM runs")
            conn.execute("DELETE FROM gaps_index")

        def insert_gap(gid: str, status: str, priority: str,
                       branch: str | None = None) -> None:
            ts = now_iso()
            conn.execute(
                "INSERT INTO gaps_index "
                "(id, name, status, priority, reporter, created, updated, "
                " branch_name, json_path) "
                "VALUES (?, ?, ?, ?, '', ?, ?, ?, ?)",
                (gid, gid, status, priority, ts, ts, branch, f"gaps/{gid}.json"),
            )

        reset()
        insert_gap("high-backlog", "backlog", "high")
        insert_gap("low-todo", "todo", "low")
        dispatcher._tick()
        assert launched == ["low-todo"], launched

        reset()
        insert_gap("high-todo", "todo", "high")
        insert_gap("medium-todo", "todo", "medium")
        insert_gap("low-todo", "todo", "low")
        dispatcher._tick()
        assert launched == ["high-todo"], launched

        reset()
        insert_gap("paused-high-todo", "todo", "high")
        db.set_setting(
            conn,
            "__refine_agent_limit_pause_until",
            f"{time.time() + 60:.3f}",
        )
        dispatcher._tick()
        assert launched == [], launched
        db.set_setting(
            conn,
            "__refine_agent_limit_pause_until",
            f"{time.time() - 1:.3f}",
        )
        dispatcher._tick()
        assert launched == ["paused-high-todo"], launched

        reset()
        insert_gap("high-running", "in-progress", "high")
        insert_gap("low-todo", "todo", "low")
        fake.running = ["high-running"]
        dispatcher._tick()
        assert launched == [], launched
        assert fake.cancel_calls == [], fake.cancel_calls

        reset()
        insert_gap("high-ready", "ready-merge", "high")
        insert_gap("low-todo", "todo", "low")
        dispatcher._tick()
        assert launched == [], launched

        for status in ("review", "done", "failed", "cancelled"):
            reset()
            insert_gap(f"high-{status}", status, "high")
            insert_gap("low-todo", "todo", "low")
            dispatcher._tick()
            assert launched == ["low-todo"], (status, launched)

        reset()
        insert_gap("high-todo", "todo", "high")
        insert_gap("medium-running", "in-progress", "medium",
                   "refine/medium-running")
        fake.running = ["medium-running"]
        dispatcher._tick()
        assert fake.cancel_calls == [
            ("medium-running", "priority_preempted"),
        ], fake.cancel_calls
        assert launched == [], launched

        reset()
        insert_gap("low-running", "in-progress", "low", "refine/low-running")
        fake.running = ["low-running"]
        db.set_setting(conn, "paused", "1")
        dispatcher._tick()
        assert fake.cancel_calls == [("low-running", "paused")], fake.cancel_calls
        db.set_setting(conn, "paused", "0")

        reset()
        insert_gap("low-running", "in-progress", "low", "refine/low-running")
        dispatcher._on_finished(
            "low-running",
            0,
            -15,
            "priority_preempted",
            "base",
        )
        row = conn.execute(
            "SELECT status, branch_name FROM gaps_index WHERE id = 'low-running'"
        ).fetchone()
        assert row["status"] == "todo", dict(row)
        assert row["branch_name"] is None, dict(row)
        activity_row = conn.execute(
            "SELECT message FROM activity WHERE gap_id = 'low-running'"
        ).fetchone()
        assert "higher-priority" in activity_row["message"], activity_row["message"]
    finally:
        os.chdir(tempfile.gettempdir())
        shutil.rmtree(tmp, ignore_errors=True)

    print("priority dispatch tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
