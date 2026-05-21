"""SQLite projection for refine merge commits shown on the Changes screen."""
from __future__ import annotations

import sqlite3

from refine_server import db, git_ops, perf_metrics

_HEAD_KEY_PREFIX = "__refine_changes_index_head:"


def _head_key(branch: str) -> str:
    return f"{_HEAD_KEY_PREFIX}{branch}"


def effective_target_branch(conn: sqlite3.Connection) -> str | None:
    configured = (db.get_setting(conn, "merge_target_branch") or "").strip()
    if configured:
        return configured
    return git_ops.current_branch()


def rebuild_target_branch(conn: sqlite3.Connection) -> str | None:
    branch = effective_target_branch(conn)
    if not branch:
        return None
    rebuild_branch(conn, branch)
    return branch


def rebuild_branch(conn: sqlite3.Connection, branch: str) -> int:
    """Rebuild one branch's merge projection from first-parent Git history."""
    metric_start = perf_metrics.now()
    head = git_ops.rev_parse(branch) or ""
    rows = git_ops.list_all_refine_merges(branch)
    with db.transaction(conn):
        conn.execute("DELETE FROM refine_merges WHERE branch = ?", (branch,))
        for row in rows:
            _upsert_row(conn, row)
        conn.execute(
            "INSERT INTO settings(key, value) VALUES(?, ?) "
            "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (_head_key(branch), head),
        )
    perf_metrics.record(
        "changes_index_rebuild",
        conn=conn,
        elapsed_ms=perf_metrics.elapsed_ms(metric_start),
        success=True,
        rows_scanned=len(rows),
        rows_returned=len(rows),
        details={"branch": branch, "head": head},
    )
    return len(rows)


def ensure_branch_current(conn: sqlite3.Connection, branch: str) -> bool:
    """Refresh the projection only when the target branch HEAD changed."""
    head = git_ops.rev_parse(branch) or ""
    if not head:
        return False
    cached = db.get_setting(conn, _head_key(branch), "")
    if cached != head:
        rebuild_branch(conn, branch)
        return True
    return False


def upsert_head_merge(conn: sqlite3.Connection, branch: str) -> bool:
    row = git_ops.refine_merge_for_commit("HEAD", branch=branch)
    if not row:
        return False
    with db.transaction(conn):
        _upsert_row(conn, row)
        head = git_ops.rev_parse(branch) or ""
        if head:
            conn.execute(
                "INSERT INTO settings(key, value) VALUES(?, ?) "
                "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (_head_key(branch), head),
            )
    return True


def advance_branch_head(
    conn: sqlite3.Connection,
    branch: str,
    *,
    previous_head: str | None = None,
) -> None:
    """Mark an already-built branch projection current after a non-merge commit.

    Undo creates a revert commit but does not remove the original merge commit
    from first-parent history, so the merge projection itself remains valid.
    """
    key = _head_key(branch)
    cached = db.get_setting(conn, key)
    if cached is None:
        return
    if previous_head is not None and cached != previous_head:
        return
    head = git_ops.rev_parse(branch) or ""
    if not head:
        return
    with db.transaction(conn):
        conn.execute(
            "INSERT INTO settings(key, value) VALUES(?, ?) "
            "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (key, head),
        )


def count_for_gap(conn: sqlite3.Connection, gap_id: str, branch: str) -> int:
    ensure_branch_current(conn, branch)
    row = conn.execute(
        "SELECT COUNT(*) AS n FROM refine_merges "
        "WHERE branch = ? AND gap_id = ?",
        (branch, gap_id.strip().upper()),
    ).fetchone()
    return int(row["n"] if row else 0)


def list_changes(
    conn: sqlite3.Connection,
    branch: str,
    *,
    limit: int,
    offset: int,
    q: str = "",
    status: str = "",
    priority: str = "",
) -> list[dict]:
    where = ["m.branch = ?"]
    args: list[object] = [branch]
    if status:
        where.append("g.status = ?")
        args.append(status)
    if priority:
        where.append("g.priority = ?")
        args.append(priority)
    if q:
        like = f"%{q.lower()}%"
        where.append(
            "lower("
            "m.commit_sha || ' ' || m.gap_id || ' ' || m.subject || ' ' || "
            "coalesce(g.name, '') || ' ' || coalesce(g.status, '') || ' ' || "
            "coalesce(g.priority, '')"
            ") LIKE ?"
        )
        args.append(like)
    args.extend([limit, offset])
    rows = conn.execute(
        "SELECT m.commit_sha, m.committed, m.subject, m.gap_id, m.branch, "
        "g.name, g.status, g.priority "
        "FROM refine_merges m "
        "LEFT JOIN gaps_index g ON g.id = m.gap_id "
        f"WHERE {' AND '.join(where)} "
        "ORDER BY m.committed DESC, m.commit_sha DESC "
        "LIMIT ? OFFSET ?",
        args,
    ).fetchall()
    return [
        {
            "commit": row["commit_sha"],
            "committed": row["committed"],
            "subject": row["subject"],
            "gap_id": row["gap_id"],
            "branch": row["branch"],
            "name": row["name"],
            "status": row["status"],
            "priority": row["priority"],
        }
        for row in rows
    ]


def _upsert_row(conn: sqlite3.Connection, row: dict) -> None:
    conn.execute(
        "INSERT INTO refine_merges(commit_sha, branch, committed, subject, gap_id) "
        "VALUES (?, ?, ?, ?, ?) "
        "ON CONFLICT(commit_sha) DO UPDATE SET "
        "branch = excluded.branch, "
        "committed = excluded.committed, "
        "subject = excluded.subject, "
        "gap_id = excluded.gap_id",
        (
            row["commit"],
            row["branch"],
            row["committed"],
            row["subject"],
            row["gap_id"],
        ),
    )
