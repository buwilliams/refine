"""Synchronize the active target app with its upstream branch."""
from __future__ import annotations

import sqlite3

from refine_server import activity, config, project_state

from . import git_ops


def sync_latest(conn: sqlite3.Connection, *, actor: str = "refine") -> dict:
    """Fetch/pull the active app branch and rebuild SQLite from JSON state."""
    branch = git_ops.current_branch()
    if not branch:
        return {
            "ok": False,
            "stage": "precheck",
            "message": "Cannot sync while the target app is in detached HEAD.",
        }

    committed_state = False
    dirty_refine = git_ops.dirty_paths_under(".refine")
    if dirty_refine:
        commit = git_ops.add_and_commit(dirty_refine, "refine: persist state")
        if not commit.ok:
            return {
                "ok": False,
                "stage": "commit",
                "message": "Could not commit local .refine state before sync.",
                "details": commit.stderr or commit.stdout,
            }
        committed_state = True

    upstream = git_ops.upstream_branch(branch)
    if upstream is None:
        project_state.rebuild_sqlite_cache(conn)
        msg = f"Branch `{branch}` has no upstream; rebuilt local cache without pulling."
        activity.append(
            conn, message=msg, severity="info", category="git", actor=actor,
        )
        return {
            "ok": True,
            "stage": "skipped",
            "branch": branch,
            "upstream": "",
            "committed_state": committed_state,
            "pulled": False,
            "message": msg,
        }

    fetched = git_ops.fetch()
    if not fetched.ok:
        return {
            "ok": False,
            "stage": "fetch",
            "branch": branch,
            "upstream": upstream,
            "committed_state": committed_state,
            "message": "Could not fetch latest target-app updates.",
            "details": fetched.stderr or fetched.stdout,
        }

    pulled = git_ops.pull_ff_only()
    if not pulled.ok:
        return {
            "ok": False,
            "stage": "pull",
            "branch": branch,
            "upstream": upstream,
            "committed_state": committed_state,
            "message": "Could not fast-forward pull latest target-app updates.",
            "details": pulled.stderr or pulled.stdout,
        }

    config.get(reload=True)
    project_state.rebuild_sqlite_cache(conn)
    msg = f"Synced `{branch}` from `{upstream}` and rebuilt the cache."
    activity.append(
        conn, message=msg, severity="info", category="git", actor=actor,
    )
    return {
        "ok": True,
        "stage": "synced",
        "branch": branch,
        "upstream": upstream,
        "committed_state": committed_state,
        "pulled": True,
        "message": msg,
    }
