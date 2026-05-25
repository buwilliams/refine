"""Hard worktree reset recovers stuck/diverged target repositories."""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, git, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-worktree-hard-reset-", with_remote=True)
    conn = init_refine(client)
    try:
        from refine_server import git_ops
        from refine_ui import api

        git(client, "add", ".refine")
        git(client, "commit", "-m", "init refine state")
        git(client, "push")

        git(client, "checkout", "-b", "conflict-branch")
        (client / "app.txt").write_text("branch conflict\n", encoding="utf-8")
        git(client, "add", "app.txt")
        git(client, "commit", "-m", "branch conflict")
        git(client, "checkout", "main")

        remote_clone = tmp / "remote-clone"
        git(tmp, "clone", "--branch", "main", str(tmp / "origin.git"), str(remote_clone))
        git(remote_clone, "config", "user.email", "t@x")
        git(remote_clone, "config", "user.name", "t")
        (remote_clone / "app.txt").write_text("remote main\n", encoding="utf-8")
        git(remote_clone, "add", "app.txt")
        git(remote_clone, "commit", "-m", "remote main")
        git(remote_clone, "push")

        (client / "app.txt").write_text("local main\n", encoding="utf-8")
        git(client, "add", "app.txt")
        git(client, "commit", "-m", "local main")

        merge = subprocess.run(
            ["git", "merge", "conflict-branch"],
            cwd=client,
            capture_output=True,
            text=True,
        )
        assert merge.returncode != 0, merge.stdout + merge.stderr
        assert git_ops.in_progress_op() and git_ops.in_progress_op()[0] == "merge"
        (client / "scratch.txt").write_text("discard me\n", encoding="utf-8")
        assert git(client, "status", "--porcelain").stdout.strip()

        status, body = api.hard_reset_worktree({})
        assert status == 200, body
        assert body["ok"] is True, body
        assert body["clean"] is True, body
        assert body["branch"] == "main", body
        assert body["upstream"] == "origin/main", body
        assert body["after_head"] == git(client, "rev-parse", "origin/main").stdout.strip()
        assert git_ops.in_progress_op() is None
        assert git(client, "status", "--porcelain").stdout.strip() == ""
        assert (client / "app.txt").read_text(encoding="utf-8") == "remote main\n"
        assert not (client / "scratch.txt").exists()
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

    print("worktree hard reset tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
