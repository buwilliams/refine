"""State committer keeps runtime noise out of project-state commits."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, git, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-state-committer-", with_remote=True)
    conn = init_refine(client)
    try:
        from refine_server import gap_writer, gaps, state_committer

        gid = "01STATECOMMITTERGAPAAAAAA"
        gap_writer.create_gap(
            gap_id=gid,
            name="State committer",
            initial_round=gaps.new_round("Reporter", "Actual", "Target"),
            status="todo",
        )
        git(client, "add", ".refine")
        git(client, "commit", "-m", "init refine state")
        git(client, "push")

        runtime_paths = [
            ".refine/app.pid",
            ".refine/app.log",
            ".refine/gaps/01/STATECOMMITTERLOG/logs.jsonl",
        ]
        for rel in runtime_paths:
            p = client / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text("tracked runtime\n", encoding="utf-8")
        git(client, "add", "-f", *runtime_paths)
        git(client, "commit", "-m", "track runtime noise")
        git(client, "push")

        for rel in runtime_paths:
            (client / rel).write_text("changed runtime\n", encoding="utf-8")
        committer = state_committer.StateCommitter(lambda: conn, interval=999)
        assert committer.commit_now() is True
        assert git(client, "log", "-1", "--format=%s").stdout.strip() == (
            "refine: stop tracking runtime state"
        )
        assert git(client, "rev-parse", "HEAD").stdout == git(
            client, "rev-parse", "origin/main",
        ).stdout
        assert git(client, "ls-files", *runtime_paths).stdout.strip() == ""

        head = git(client, "rev-parse", "HEAD").stdout.strip()
        for rel in runtime_paths:
            (client / rel).write_text("later runtime noise\n", encoding="utf-8")
        assert committer.commit_now() is False
        assert git(client, "rev-parse", "HEAD").stdout.strip() == head

        gap_writer.update_fields(gid, priority="high")
        assert committer.commit_now() is True
        assert git(client, "log", "-1", "--format=%s").stdout.strip() == (
            "refine: sync project state"
        )
        assert git(client, "rev-parse", "HEAD").stdout == git(
            client, "rev-parse", "origin/main",
        ).stdout
        assert "gap.json" in git(client, "show", "--name-only", "--format=", "HEAD").stdout
        assert "logs.jsonl" not in git(client, "show", "--name-only", "--format=", "HEAD").stdout
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("state committer tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
