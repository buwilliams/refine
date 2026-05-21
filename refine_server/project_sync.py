"""Synchronize the active target app with its upstream branch."""
from __future__ import annotations

import sqlite3

from refine_server import activity, config, db, perf_metrics, project_state

from . import git_ops

_PULSE_BRANCH_KEY = "__refine_project_pulse_branch"
_PULSE_HEAD_KEY = "__refine_project_pulse_head"
_PULSE_UPSTREAM_KEY = "__refine_project_pulse_upstream"


def sync_latest(conn: sqlite3.Connection, *, actor: str = "refine") -> dict:
    """Fetch/pull the active app branch and rebuild SQLite from JSON state."""
    metric_start = perf_metrics.now()
    metric_details: dict = {"actor": actor}

    def finish(result: dict, *, success: bool | None = None) -> dict:
        perf_metrics.record(
            "project_sync",
            conn=conn,
            elapsed_ms=perf_metrics.elapsed_ms(metric_start),
            success=bool(result.get("ok")) if success is None else success,
            query_mode=str(result.get("stage") or ""),
            details={**metric_details, **{
                "stage": result.get("stage") or "",
                "branch": result.get("branch") or "",
                "upstream": result.get("upstream") or "",
                "pulled": bool(result.get("pulled")),
                "committed_state": bool(result.get("committed_state")),
            }},
        )
        return result

    branch = git_ops.current_branch()
    if not branch:
        return finish({
            "ok": False,
            "stage": "precheck",
            "message": "Cannot sync while the target app is in detached HEAD.",
        })

    committed_state = False
    dirty_refine = git_ops.dirty_paths_under(".refine")
    metric_details["dirty_refine_count"] = len(dirty_refine)
    if dirty_refine:
        commit = git_ops.add_and_commit(dirty_refine, "refine: persist state")
        if not commit.ok:
            return finish({
                "ok": False,
                "stage": "commit",
                "message": "Could not commit local .refine state before sync.",
                "details": commit.stderr or commit.stdout,
            })
        committed_state = True

    upstream = git_ops.upstream_branch(branch)
    if upstream is None:
        rebuild_start = perf_metrics.now()
        project_state.rebuild_sqlite_cache(conn)
        metric_details["rebuild_ms"] = round(perf_metrics.elapsed_ms(rebuild_start), 2)
        msg = f"Branch `{branch}` has no upstream; rebuilt local cache without pulling."
        activity.append(
            conn, message=msg, severity="info", category="git", actor=actor,
        )
        return finish({
            "ok": True,
            "stage": "skipped",
            "branch": branch,
            "upstream": "",
            "committed_state": committed_state,
            "pulled": False,
            "message": msg,
        })

    fetch_start = perf_metrics.now()
    fetched = git_ops.fetch()
    metric_details["fetch_ms"] = round(perf_metrics.elapsed_ms(fetch_start), 2)
    if not fetched.ok:
        return finish({
            "ok": False,
            "stage": "fetch",
            "branch": branch,
            "upstream": upstream,
            "committed_state": committed_state,
            "message": "Could not fetch latest target-app updates.",
            "details": fetched.stderr or fetched.stdout,
        })

    pull_start = perf_metrics.now()
    pulled = git_ops.pull_ff_only()
    metric_details["pull_ms"] = round(perf_metrics.elapsed_ms(pull_start), 2)
    if not pulled.ok:
        return finish({
            "ok": False,
            "stage": "pull",
            "branch": branch,
            "upstream": upstream,
            "committed_state": committed_state,
            "message": "Could not fast-forward pull latest target-app updates.",
            "details": pulled.stderr or pulled.stdout,
        })

    config.get(reload=True)
    rebuild_start = perf_metrics.now()
    project_state.rebuild_sqlite_cache(conn)
    metric_details["rebuild_ms"] = round(perf_metrics.elapsed_ms(rebuild_start), 2)
    msg = f"Synced `{branch}` from `{upstream}` and rebuilt the cache."
    activity.append(
        conn, message=msg, severity="info", category="git", actor=actor,
    )
    return finish({
        "ok": True,
        "stage": "synced",
        "branch": branch,
        "upstream": upstream,
        "committed_state": committed_state,
        "pulled": True,
        "message": msg,
    })


def pulse(conn: sqlite3.Connection, *, actor: str = "runner") -> dict:
    """Refresh projected state when the target repo changed.

    This is intentionally cheaper and quieter than manual sync: it fetches to
    learn whether the upstream has commits this worktree lacks, only pulls when
    needed, and otherwise rebuilds SQLite only when the local branch HEAD moved
    since the last pulse.
    """
    metric_start = perf_metrics.now()
    metric_details: dict = {"actor": actor}

    def finish(result: dict, *, success: bool | None = None) -> dict:
        perf_metrics.record(
            "project_sync.pulse",
            conn=conn,
            elapsed_ms=perf_metrics.elapsed_ms(metric_start),
            success=bool(result.get("ok")) if success is None else success,
            query_mode=str(result.get("stage") or ""),
            details={**metric_details, **{
                "stage": result.get("stage") or "",
                "changed": bool(result.get("changed")),
                "branch": result.get("branch") or "",
                "upstream": result.get("upstream") or "",
                "pulled": bool(result.get("pulled")),
            }},
        )
        return result

    branch = git_ops.current_branch()
    if not branch:
        return finish({
            "ok": False,
            "stage": "precheck",
            "changed": False,
            "message": "Cannot pulse target repo while detached from a branch.",
        })

    upstream = git_ops.upstream_branch(branch) or ""
    head = git_ops.rev_parse("HEAD") or ""
    remote_head = ""
    if upstream:
        fetch_start = perf_metrics.now()
        fetched = git_ops.fetch()
        metric_details["fetch_ms"] = round(perf_metrics.elapsed_ms(fetch_start), 2)
        if not fetched.ok:
            return finish({
                "ok": False,
                "stage": "fetch",
                "changed": False,
                "branch": branch,
                "upstream": upstream,
                "message": "Could not fetch latest target repo updates.",
                "details": fetched.stderr or fetched.stdout,
            })
        remote_head = git_ops.rev_parse(upstream) or ""
        if git_ops.rev_list_count("HEAD", upstream) > 0:
            result = sync_latest(conn, actor=actor)
            result["pulse"] = True
            result["changed"] = bool(result.get("ok"))
            _remember_pulse_refs(
                conn,
                branch=git_ops.current_branch() or branch,
                head=git_ops.rev_parse("HEAD") or head,
                upstream=git_ops.rev_parse(upstream) or remote_head,
            )
            return finish(result)

    previous = _read_pulse_refs(conn)
    if not previous.get("branch"):
        rebuild_start = perf_metrics.now()
        project_state.rebuild_sqlite_cache(conn)
        metric_details["rebuild_ms"] = round(perf_metrics.elapsed_ms(rebuild_start), 2)
        _remember_pulse_refs(
            conn, branch=branch, head=head, upstream=remote_head,
        )
        return finish({
            "ok": True,
            "stage": "initialized",
            "changed": True,
            "branch": branch,
            "upstream": upstream,
            "message": "Initialized target repo update pulse.",
        })
    if (
        previous.get("branch") != branch
        or previous.get("head") != head
        or previous.get("upstream") != remote_head
    ):
        rebuild_start = perf_metrics.now()
        project_state.rebuild_sqlite_cache(conn)
        metric_details["rebuild_ms"] = round(perf_metrics.elapsed_ms(rebuild_start), 2)
        _remember_pulse_refs(
            conn, branch=branch, head=head, upstream=remote_head,
        )
        msg = "Detected target repo update and rebuilt projected state."
        activity.append(
            conn, message=msg, severity="info", category="git", actor=actor,
        )
        return finish({
            "ok": True,
            "stage": "refreshed",
            "changed": True,
            "branch": branch,
            "upstream": upstream,
            "message": msg,
        })

    _remember_pulse_refs(conn, branch=branch, head=head, upstream=remote_head)
    return finish({
        "ok": True,
        "stage": "unchanged",
        "changed": False,
        "branch": branch,
        "upstream": upstream,
        "message": "Target repo state is current.",
    })


def _read_pulse_refs(conn: sqlite3.Connection) -> dict[str, str]:
    rows = conn.execute(
        "SELECT key, value FROM settings WHERE key IN (?, ?, ?)",
        (_PULSE_BRANCH_KEY, _PULSE_HEAD_KEY, _PULSE_UPSTREAM_KEY),
    ).fetchall()
    values = {row["key"]: row["value"] for row in rows}
    return {
        "branch": values.get(_PULSE_BRANCH_KEY, ""),
        "head": values.get(_PULSE_HEAD_KEY, ""),
        "upstream": values.get(_PULSE_UPSTREAM_KEY, ""),
    }


def _remember_pulse_refs(
    conn: sqlite3.Connection,
    *,
    branch: str,
    head: str,
    upstream: str,
) -> None:
    with db.transaction(conn):
        for key, value in (
            (_PULSE_BRANCH_KEY, branch),
            (_PULSE_HEAD_KEY, head),
            (_PULSE_UPSTREAM_KEY, upstream),
        ):
            conn.execute(
                "INSERT INTO settings(key, value) VALUES(?, ?) "
                "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value),
            )
