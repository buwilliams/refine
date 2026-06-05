"""End-to-end dispatcher/subprocess supervision tests with a fake agent CLI."""
from __future__ import annotations

import os
import sqlite3
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, init_refine, make_client_repo


def install_fake_claude(bin_dir: Path) -> Path:
    bin_dir.mkdir(parents=True, exist_ok=True)
    fake = bin_dir / "claude"
    fake.write_text(
        """#!/bin/sh
case "$*" in
  *SUCCESSRUN*)
    printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"making change"}]}}'
    printf 'success\n' > success.txt
    git add success.txt
    git commit -m 'agent success'
    printf '%s\n' '{"type":"result","is_error":false}'
    exit 0
    ;;
  *FAILRUN*)
    printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"about to fail"}]}}'
    exit 7
    ;;
  *NOOPRUN*)
    printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"already done"}]}}'
    printf '%s\n' '{"type":"result","is_error":false}'
    exit 0
    ;;
  *IDLERUN*)
    sleep 20
    exit 0
    ;;
  *HARDRUN*)
    while true; do
      printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"still working"}]}}'
      sleep 0.2
    done
    ;;
  *CANCELRUN*)
    while true; do
      sleep 1
    done
    ;;
  *MEMKILLRUN*)
    printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"allocating memory"}]}}'
    kill -9 $$
    ;;
  *)
    printf '%s\n' '{"type":"result","is_error":true,"error":"unknown prompt"}'
    exit 2
    ;;
esac
""",
        encoding="utf-8",
    )
    fake.chmod(0o755)
    return fake


def wait_for_status(conn, gap_id: str, statuses: set[str], *,
                    timeout: float = 12.0) -> str:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
        if row and row["status"] in statuses:
            return row["status"]
        time.sleep(0.1)
    row = conn.execute(
        "SELECT status FROM gaps_index WHERE id = ?", (gap_id,),
    ).fetchone()
    raise AssertionError(f"{gap_id} stuck at {row['status'] if row else '<missing>'}")


def wait_until_running(sub_mgr: "SubprocessManager", gap_id: str, *,
                       timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if sub_mgr.is_running(gap_id):
            return
        time.sleep(0.05)
    raise AssertionError(f"{gap_id} did not start running")


def wait_for_wakeup(wakeups: list[str], gap_id: str, *,
                    timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if gap_id in wakeups:
            return
        time.sleep(0.05)
    raise AssertionError(f"{gap_id} did not wake the merger")


def latest_run(conn, gap_id: str):
    return conn.execute(
        "SELECT status, failure_category, worker_node_id FROM runs WHERE gap_id = ? "
        "ORDER BY id DESC LIMIT 1",
        (gap_id,),
    ).fetchone()


def latest_logs(gap_id: str) -> list[dict]:
    from refine_server import gaps

    gap = gaps.read_gap_json(gap_id)
    assert gap is not None
    return gap["rounds"][-1]["logs"]


def latest_log_messages(gap_id: str) -> list[str]:
    return [log["message"] for log in latest_logs(gap_id)]


def activity_messages(conn, gap_id: str) -> list[str]:
    rows = conn.execute(
        "SELECT message FROM activity WHERE gap_id = ? ORDER BY id",
        (gap_id,),
    ).fetchall()
    return [row["message"] for row in rows]


def wait_for_log(gap_id: str, fragment: str, *, timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if any(fragment in msg for msg in latest_log_messages(gap_id)):
            return
        time.sleep(0.05)
    raise AssertionError(f"{gap_id} missing log fragment: {fragment}")


def main() -> int:
    tmp, client = make_client_repo("refine-subprocess-")
    conn = init_refine(client)
    conn.close()
    try:
        from refine_server.paths import sqlite_path
        conn = sqlite3.connect(
            str(sqlite_path()),
            isolation_level=None,
            check_same_thread=False,
            timeout=5.0,
        )
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA journal_mode = WAL")
        conn.execute("PRAGMA synchronous = NORMAL")
        conn.execute("PRAGMA foreign_keys = ON")
        from refine_server import chat_mgr, db, project_state
        from refine_server.dispatcher import Dispatcher
        from refine_server.subprocess_mgr import SubprocessManager

        bin_dir = tmp / "bin"
        install_fake_claude(bin_dir)
        chat_mgr._login_path_cache = f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}"
        chat_mgr._login_path_resolved = True

        db.set_setting(conn, "agent_cli", "claude")
        db.set_setting(conn, "backlog_promote_after_seconds", "-1")
        db.set_setting(conn, "agent_idle_timeout_seconds", "1")
        db.set_setting(conn, "agent_hard_cap_seconds", "60")
        db.set_setting(conn, "worker_cpu_priority", "low")

        sub_mgr = SubprocessManager(lambda: conn)
        merger_wakeups: list[str] = []
        dispatcher = Dispatcher(
            get_conn=lambda: conn,
            sub_mgr=sub_mgr,
            on_run_finished=lambda gid: merger_wakeups.append(gid),
        )

        def run_gap(gap_id: str, *, idle: int = 1, hard: int = 60) -> str:
            db.set_setting(conn, "agent_idle_timeout_seconds", str(idle))
            db.set_setting(conn, "agent_hard_cap_seconds", str(hard))
            create_indexed_gap(conn, gap_id, status="todo")
            dispatcher._launch_one(conn, gap_id, None)
            return wait_for_status(conn, gap_id, {"ready-merge", "failed"})

        gid_success = "01SUBPROCESSSUCCESSRUNAA"
        assert run_gap(gid_success) == "ready-merge"
        wait_for_wakeup(merger_wakeups, gid_success)
        run = latest_run(conn, gid_success)
        assert run["status"] == "finished"
        assert run["failure_category"] is None
        assert run["worker_node_id"] == project_state.local_node_id()
        wait_for_log(gid_success, "CPU throttling active")
        assert any(
            "CPU throttling active" in msg
            for msg in activity_messages(conn, gid_success)
        )
        wait_for_log(gid_success, "Agent run completed")

        gid_fail = "01SUBPROCESSFAILRUNAAAAA"
        assert run_gap(gid_fail) == "failed"
        run = latest_run(conn, gid_fail)
        assert run["status"] == "finished"
        assert run["failure_category"] is None
        wait_for_log(gid_fail, "exit 7")

        gid_noop = "01SUBPROCESSNOOPRUNAAAAA"
        assert run_gap(gid_noop) == "ready-merge"
        wait_for_log(gid_noop, "already met")

        gid_idle = "01SUBPROCESSIDLERUNAAAAA"
        assert run_gap(gid_idle, idle=1, hard=60) == "failed"
        run = latest_run(conn, gid_idle)
        assert run["status"] == "killed"
        assert run["failure_category"] == "idle"
        wait_for_log(gid_idle, "stuck")

        gid_hard = "01SUBPROCESSHARDRUNAAAAA"
        assert run_gap(gid_hard, idle=0, hard=1) == "failed"
        run = latest_run(conn, gid_hard)
        assert run["status"] == "killed"
        assert run["failure_category"] == "hard_cap"
        wait_for_log(gid_hard, "wall-clock cap")

        gid_mem = "01SUBPROCESSMEMKILLRUNAA"
        db.set_setting(conn, "worker_memory_limit_mb", "4096")
        assert run_gap(gid_mem, idle=0, hard=60) == "failed"
        run = latest_run(conn, gid_mem)
        assert run["status"] == "killed"
        assert run["failure_category"] == "memory_limit"
        wait_for_log(gid_mem, "memory limit")
        wait_for_log(gid_mem, "smaller-scope Gaps")
        assert any(
            log["severity"] == "error"
            and log["category"] == "resource"
            and "memory limit" in log["message"]
            for log in latest_logs(gid_mem)
        )
        assert any(
            "memory limit" in msg
            for msg in activity_messages(conn, gid_mem)
        )

        gid_order = "01SUBPROCESSORDERCANCELRUN"
        create_indexed_gap(conn, gid_order, status="todo")
        callback_started = threading.Event()
        release_callback = threading.Event()
        callback_seen_live = {"value": False}
        dispatcher._launch_one(conn, gid_order, None)
        wait_until_running(sub_mgr, gid_order)
        original_on_finished = dispatcher._on_finished

        def hold_on_finished(
            gap_id: str,
            round_idx: int,
            exit_code: int,
            killed_reason: str | None,
            base_ref: str,
            *,
            agent_reported_success=None,
            failure_text: str = "",
        ) -> None:
            if gap_id == gid_order:
                callback_seen_live["value"] = sub_mgr.is_running(gap_id)
                callback_started.set()
                release_callback.wait(timeout=5.0)
            original_on_finished(
                gap_id,
                round_idx,
                exit_code,
                killed_reason,
                base_ref,
                agent_reported_success=agent_reported_success,
                failure_text=failure_text,
            )

        dispatcher._on_finished = hold_on_finished  # type: ignore[method-assign]
        try:
            assert sub_mgr.cancel(gid_order, reason="priority_preempted") is True
            assert callback_started.wait(timeout=5.0)
            assert callback_seen_live["value"] is True
            assert sub_mgr.is_running(gid_order) is True
            release_callback.set()
            assert wait_for_status(conn, gid_order, {"todo"}) == "todo"
            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline and sub_mgr.is_running(gid_order):
                time.sleep(0.05)
            assert sub_mgr.is_running(gid_order) is False
        finally:
            release_callback.set()
            dispatcher._on_finished = original_on_finished  # type: ignore[method-assign]

        gid_cancel = "01SUBPROCESSCANCELRUNAAA"
        db.set_setting(conn, "worker_memory_limit_mb", "0")
        db.set_setting(conn, "agent_idle_timeout_seconds", "0")
        db.set_setting(conn, "agent_hard_cap_seconds", "60")
        create_indexed_gap(conn, gid_cancel, status="todo")
        dispatcher._launch_one(conn, gid_cancel, None)
        wait_until_running(sub_mgr, gid_cancel)
        assert sub_mgr.cancel(gid_cancel, reason="cancel") is True
        assert wait_for_status(conn, gid_cancel, {"failed"}) == "failed"
        run = latest_run(conn, gid_cancel)
        assert run["status"] == "killed"
        assert run["failure_category"] == "cancel"
        wait_for_log(gid_cancel, "cancelled")
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("subprocess supervision tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
