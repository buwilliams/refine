"""Target-app start cleans stale host worktree state before launching."""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, git, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-target-app-start-clean-")
    conn = init_refine(client)
    try:
        from refine_server import git_ops
        from refine_ui import api, runtime

        status, body = api.update_settings({
            "target_app_start_command": 'test -z "$(git status --porcelain)"',
            "target_app_start_timeout_seconds": "5",
        })
        assert status == 200, body
        git(client, "add", ".refine")
        git(client, "commit", "-m", "configure target app")

        conflict_path = client / "stash-conflict.txt"
        conflict_path.write_text("base\n", encoding="utf-8")
        git(client, "add", "stash-conflict.txt")
        git(client, "commit", "-m", "stash conflict base")
        conflict_path.write_text("stashed\n", encoding="utf-8")
        git(client, "stash", "push", "-m", "test conflicting stash")
        conflict_path.write_text("upstream\n", encoding="utf-8")
        git(client, "add", "stash-conflict.txt")
        git(client, "commit", "-m", "stash conflict upstream")

        apply = subprocess.run(
            ["git", "stash", "apply"],
            cwd=client,
            capture_output=True,
            text=True,
        )
        assert apply.returncode != 0, apply.stdout + apply.stderr
        stuck = git_ops.in_progress_op()
        assert stuck and stuck[0] == "unmerged-index", stuck

        status, body = api.target_app_start({})
        assert status == 200, body
        assert body["ok"] is True, body
        assert git_ops.in_progress_op() is None
        assert git(client, "status", "--porcelain").stdout.strip() == ""
        stash_list = git(client, "stash", "list").stdout
        assert "refine cleanup auto-stash (pre-target-app start cleanup)" in stash_list

        (client / "ordinary-wip.txt").write_text("wip\n", encoding="utf-8")
        status, body = api.target_app_start({})
        assert status == 200, body
        assert body["ok"] is True, body
        assert git(client, "status", "--porcelain").stdout.strip() == ""
        stash_list = git(client, "stash", "list").stdout
        assert "refine target-app start auto-stash" in stash_list

        runtime.stop_runner()
        from refine_server.runner import Runner

        runner = Runner()
        (client / "automatic-wip.txt").write_text("wip\n", encoding="utf-8")
        try:
            sequence = runner._run_target_app_rebuild_sequence(conn, {  # noqa: SLF001
                "stop_command": "true",
                "rebuild_command": "true",
                "start_command": 'test -z "$(git status --porcelain)"',
                "cwd": "",
                "env": {},
                "stop_timeout_seconds": 5,
                "rebuild_timeout_seconds": 5,
                "start_timeout_seconds": 5,
            })
        finally:
            runner.shutdown()
        assert sequence["ok"] is True, sequence
        assert git(client, "status", "--porcelain").stdout.strip() == ""
    finally:
        try:
            runtime.stop_runner()
        except Exception:
            pass
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("target-app start cleanup tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
