"""End-to-end dispatcher/subprocess supervision tests with a fake agent CLI."""
from __future__ import annotations

import os
import sqlite3
import sys
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


def latest_run(conn, gap_id: str):
    return conn.execute(
        "SELECT status, failure_category FROM runs WHERE gap_id = ? "
        "ORDER BY id DESC LIMIT 1",
        (gap_id,),
    ).fetchone()


def latest_log_messages(gap_id: str) -> list[str]:
    from refine_server import gaps

    gap = gaps.read_gap_json(gap_id)
    assert gap is not None
    return [log["message"] for log in gap["rounds"][-1]["logs"]]


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
        from refine_server import chat_mgr, db
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
        assert gid_success in merger_wakeups
        run = latest_run(conn, gid_success)
        assert run["status"] == "finished"
        assert run["failure_category"] is None
        assert any("Agent run completed" in msg for msg in latest_log_messages(gid_success))

        gid_fail = "01SUBPROCESSFAILRUNAAAAA"
        assert run_gap(gid_fail) == "failed"
        run = latest_run(conn, gid_fail)
        assert run["status"] == "finished"
        assert run["failure_category"] is None
        assert any("exit 7" in msg for msg in latest_log_messages(gid_fail))

        gid_noop = "01SUBPROCESSNOOPRUNAAAAA"
        assert run_gap(gid_noop) == "ready-merge"
        assert any("already met" in msg for msg in latest_log_messages(gid_noop))

        gid_idle = "01SUBPROCESSIDLERUNAAAAA"
        assert run_gap(gid_idle, idle=1, hard=60) == "failed"
        run = latest_run(conn, gid_idle)
        assert run["status"] == "killed"
        assert run["failure_category"] == "idle"
        assert any("stuck" in msg for msg in latest_log_messages(gid_idle))

        gid_hard = "01SUBPROCESSHARDRUNAAAAA"
        assert run_gap(gid_hard, idle=0, hard=1) == "failed"
        run = latest_run(conn, gid_hard)
        assert run["status"] == "killed"
        assert run["failure_category"] == "hard_cap"
        assert any("wall-clock cap" in msg for msg in latest_log_messages(gid_hard))

        gid_cancel = "01SUBPROCESSCANCELRUNAAA"
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
        assert any("cancelled" in msg for msg in latest_log_messages(gid_cancel))
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("subprocess supervision tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
