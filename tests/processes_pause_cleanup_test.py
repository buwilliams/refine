"""Pausing agents from System > Processes leaves the target worktree clean."""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, git, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-processes-pause-clean-", with_remote=True)
    conn = init_refine(client)
    try:
        from refine_server import git_ops
        from refine_ui import api, runtime

        git(client, "add", ".refine")
        git(client, "commit", "-m", "init refine state")
        git(client, "push")

        git(client, "checkout", "-b", "pause-conflict")
        (client / "app.txt").write_text("branch change\n", encoding="utf-8")
        git(client, "add", "app.txt")
        git(client, "commit", "-m", "branch change")
        git(client, "checkout", "main")
        (client / "app.txt").write_text("main change\n", encoding="utf-8")
        git(client, "add", "app.txt")
        git(client, "commit", "-m", "main change")
        git(client, "push")

        merge = subprocess.run(
            ["git", "merge", "pause-conflict"],
            cwd=client,
            capture_output=True,
            text=True,
        )
        assert merge.returncode != 0, merge.stdout + merge.stderr
        stuck = git_ops.in_progress_op()
        assert stuck and stuck[0] == "merge", stuck
        assert git(client, "status", "--porcelain").stdout.strip()

        status, body = api.update_settings({"paused": "1"})
        assert status == 200, body
        assert body["ok"] is True, body
        assert git_ops.in_progress_op() is None
        assert git(client, "status", "--porcelain").stdout.strip() == ""
        assert git(client, "log", "-1", "--format=%s").stdout.strip() == (
            "refine: sync project state"
        )

        (client / "app.txt").write_text("operator wip\n", encoding="utf-8")
        assert git(client, "status", "--porcelain").stdout.strip()
        status, body = api.update_settings({"paused": "1"})
        assert status == 200, body
        assert body["ok"] is True, body
        assert git(client, "status", "--porcelain").stdout.strip() == ""
        assert "refine pause cleanup auto-stash" in git(
            client, "stash", "list",
        ).stdout
    finally:
        try:
            from refine_ui import runtime

            runtime.stop_runner()
        except Exception:
            pass
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("processes pause cleanup tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
