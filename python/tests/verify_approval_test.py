"""Manual Verify approves review only; Merge agent owns merge work."""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def git(cwd: Path, *args: str) -> None:
    subprocess.run(["git", *args], cwd=cwd, check=True)


def main() -> int:
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    tmp = Path(tempfile.mkdtemp(prefix="refine-verify-approval-"))
    client = tmp / "client"
    client.mkdir()
    git(client, "init", "-q")
    git(client, "-c", "user.email=t@x", "-c", "user.name=t",
        "commit", "--allow-empty", "-m", "init")
    base_branch = subprocess.check_output(
        ["git", "branch", "--show-current"],
        cwd=client, text=True,
    ).strip()
    os.chdir(client)

    try:
        from refine_server import config, db
        from refine_server.gaps import now_iso
        from refine_server import verify_op

        config.write_defaults(client / ".refine")
        config.get(reload=True)
        db.init_db()
        conn = db.connect()

        def insert_gap(gid: str, status: str, branch: str | None) -> None:
            ts = now_iso()
            conn.execute(
                "INSERT INTO gaps_index "
                "(id, name, status, priority, reporter, created, updated, "
                " branch_name, json_path) "
                "VALUES (?, ?, ?, 'medium', '', ?, ?, ?, ?)",
                (gid, gid, status, ts, ts, branch, f"gaps/{gid}.json"),
            )

        insert_gap("review-no-branch", "review", None)
        result = verify_op.approve_review(conn, "review-no-branch")
        assert result["ok"] is True, result
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = 'review-no-branch'"
        ).fetchone()
        assert row["status"] == "done", dict(row)

        git(client, "checkout", "-q", "-b", "refine/unmerged")
        (client / "change.txt").write_text("change\n", encoding="utf-8")
        git(client, "add", "change.txt")
        git(client, "-c", "user.email=t@x", "-c", "user.name=t",
            "commit", "-m", "unmerged branch")
        git(client, "checkout", "-q", base_branch)

        insert_gap("review-with-branch", "review", "refine/unmerged")
        result = verify_op.approve_review(conn, "review-with-branch")
        assert result["ok"] is False, result
        assert result["stage"] == "not_merged", result
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = 'review-with-branch'"
        ).fetchone()
        assert row["status"] == "review", dict(row)

        result = verify_op.perform_verify(conn, "review-with-branch")
        assert result["ok"] is False, result
        assert result["stage"] == "lookup", result
        assert "not ready to merge" in result["message"], result

        insert_gap("ready-merge-gap", "ready-merge", None)
        result = verify_op.approve_review(conn, "ready-merge-gap")
        assert result["ok"] is False, result
        assert "not awaiting review" in result["message"], result
    finally:
        os.chdir(tempfile.gettempdir())
        shutil.rmtree(tmp, ignore_errors=True)

    print("verify approval tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
