"""Shared helpers for script-style tests."""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def run(cwd: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*args],
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    )


def git(cwd: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return run(cwd, "git", *args)


def reset_refine_imports() -> None:
    for mod in list(sys.modules):
        if mod.startswith("refine"):
            del sys.modules[mod]


def make_client_repo(prefix: str, *, with_remote: bool = False) -> tuple[Path, Path]:
    tmp = Path(tempfile.mkdtemp(prefix=prefix))
    if with_remote:
        origin = tmp / "origin.git"
        git(tmp, "init", "--bare", str(origin))
        git(tmp, "clone", str(origin), "client")
        client = tmp / "client"
    else:
        client = tmp / "client"
        client.mkdir()
        git(client, "init", "-q")
    git(client, "config", "user.email", "t@x")
    git(client, "config", "user.name", "t")
    git(client, "checkout", "-B", "main")
    (client / "app.txt").write_text("base\n", encoding="utf-8")
    git(client, "add", "app.txt")
    git(client, "commit", "-m", "init")
    if with_remote:
        git(client, "push", "-u", "origin", "main")
    return tmp, client


def cleanup_tmp(tmp: Path) -> None:
    os.chdir(tempfile.gettempdir())
    shutil.rmtree(tmp, ignore_errors=True)


def init_refine(client: Path):
    os.chdir(client)
    reset_refine_imports()
    from refine_server import config, db

    config.write_defaults(client / ".refine")
    config.get(reload=True)
    db.init_db()
    return db.connect()


def create_indexed_gap(conn, gap_id: str, *, status: str = "todo",
                       branch: str | None = None, priority: str = "medium") -> None:
    from refine_server import gap_writer, gaps
    from refine_server.paths import relative_gap_path

    gap = gap_writer.create_gap(
        gap_id=gap_id,
        name=gap_id,
        initial_round=gaps.new_round(
            "Reporter",
            f"Current behavior for {gap_id}",
            f"Target behavior for {gap_id}",
        ),
    )
    conn.execute(
        "INSERT INTO gaps_index "
        "(id, name, status, priority, reporter, created, updated, branch_name, json_path) "
        "VALUES (?, ?, ?, ?, 'Reporter', ?, ?, ?, ?)",
        (
            gap_id,
            gap_id,
            status,
            priority,
            gap["created"],
            gap["updated"],
            branch,
            relative_gap_path(gap_id),
        ),
    )
