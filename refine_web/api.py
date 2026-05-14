"""JSON API endpoint handlers.

Returns (status_code, body_dict) tuples. The server module wraps these.
"""
from __future__ import annotations

import json
import re
import sqlite3
from datetime import datetime, timedelta, timezone
from typing import Any

from refine_shared import activity, db, gaps as shared_gaps, reporters
from refine_shared.gaps import now_iso
from refine_shared.ipc_protocol import (
    M_APPEND_ROUND, M_CANCEL, M_CHAT_INPUT, M_CHAT_READ, M_CHAT_START,
    M_CHAT_STOP, M_CREATE_GAP, M_DELETE_GAP, M_DIAGNOSTICS, M_EDIT_ROUND,
    M_EXTRACT_GAPS, M_LAUNCH, M_LIST_CHANGES, M_LOG_APPEND, M_PREFLIGHT,
    M_RENAME_REPORTER, M_RUNNING, M_SET_NOTES, M_UNDO_GAP, M_VERIFY,
)
from refine_shared.ulid import new_ulid

from .ipc_client import IpcError, get_client


# --- error helpers ------------------------------------------------------------

def err(code: int, message: str, details: str | None = None) -> tuple[int, dict]:
    body: dict[str, Any] = {"error": {"message": message}}
    if details is not None:
        body["error"]["details"] = details
    return code, body


def _conn() -> sqlite3.Connection:
    return db.connect()


# --- Gap endpoints ------------------------------------------------------------

_VALID_PRIORITIES = ("low", "medium", "high")
_VALID_STATUSES = (
    "backlog", "todo", "in-progress", "ready-merge",
    "review", "done", "failed", "cancelled",
)

# Map a public sort key to a SQL expression. Whitelisted to prevent SQL
# injection from the query string. `id` doubles as a chronological sort
# because we mint Gap ids as ULIDs.
_GAPS_SORT_EXPRESSIONS: dict[str, str] = {
    "name":     "name COLLATE NOCASE",
    "status":   "status",
    "priority": "CASE priority WHEN 'high' THEN 0 WHEN 'medium' THEN 1 ELSE 2 END",
    "reporter": "reporter COLLATE NOCASE",
    "updated":  "updated",
    "created":  "created",
    "id":       "id",
}
# Default direction per column when one isn't supplied.
_GAPS_DEFAULT_DIR: dict[str, str] = {
    "name":     "ASC",
    "status":   "ASC",
    "priority": "ASC",   # CASE maps high=0, so ASC = high first
    "reporter": "ASC",
    "updated":  "DESC",
    "created":  "DESC",
    "id":       "DESC",
}


def _gaps_order_clause(sort: str | None, direction: str | None) -> str:
    key = (sort or "updated").lower()
    if key not in _GAPS_SORT_EXPRESSIONS:
        key = "updated"
    expr = _GAPS_SORT_EXPRESSIONS[key]
    d = (direction or "").upper()
    if d not in ("ASC", "DESC"):
        d = _GAPS_DEFAULT_DIR[key]
    # Tiebreaker by updated so the order is deterministic when the primary
    # key is equal across rows.
    tiebreaker = "" if key == "updated" else ", updated DESC"
    return f"{expr} {d}{tiebreaker}"


def list_gaps(*, status: str | None = None, q: str | None = None,
              severity: str | None = None,
              category: str | None = None,
              actor: str | None = None,
              reporter: str | None = None,
              limit: int = 200,
              sort: str | None = None,
              direction: str | None = None,
              include_facets: bool = False) -> tuple[int, dict]:
    """List Gaps. `severity` / `category` / `actor` filter to Gaps that
    have at least one activity entry matching. `reporter` filters by
    the indexed `gaps_index.reporter` column, which the runner keeps in
    sync with the latest round's reporter on every write.
    """
    sql = [
        "SELECT id, name, status, priority, reporter, "
        "created, updated, branch_name "
        "FROM gaps_index"
    ]
    args: list[Any] = []
    where: list[str] = []
    if status:
        where.append("status = ?")
        args.append(status)
    if q:
        where.append("(name LIKE ? OR id LIKE ?)")
        like = f"%{q}%"
        args.extend([like, like])
    if reporter:
        where.append("reporter = ?")
        args.append(reporter)
    # Activity-derived filters: gap must have at least one matching entry.
    if severity or category or actor:
        sub_where = ["gap_id IS NOT NULL"]
        sub_args: list[Any] = []
        if severity:
            sub_where.append("severity = ?")
            sub_args.append(severity)
        if category:
            sub_where.append("category = ?")
            sub_args.append(category)
        if actor:
            sub_where.append("actor = ?")
            sub_args.append(actor)
        where.append(
            "id IN (SELECT DISTINCT gap_id FROM activity WHERE "
            + " AND ".join(sub_where) + ")"
        )
        args.extend(sub_args)
    if where:
        sql.append("WHERE " + " AND ".join(where))
    sql.append("ORDER BY " + _gaps_order_clause(sort, direction) + " LIMIT ?")
    args.append(limit)
    conn = _conn()
    try:
        rows = [dict(r) for r in conn.execute(" ".join(sql), args)]
        facets: dict | None = None
        if include_facets:
            facets = {
                "categories": activity.distinct_categories(conn),
                "actors": activity.distinct_actors(conn),
            }
    finally:
        conn.close()
    # Round-content search only kicks in when the only non-q filters are
    # the existing ones — the activity-side subquery already constrains
    # the candidate id set.
    if q and len(rows) < limit and not (severity or category or actor or reporter):
        rows = _augment_with_round_search(rows, q, limit)
    body: dict = {"gaps": rows}
    if facets is not None:
        body["facets"] = facets
    return 200, body


def _augment_with_round_search(initial: list[dict], q: str,
                                limit: int) -> list[dict]:
    seen = {r["id"] for r in initial}
    needle = q.lower()
    conn = _conn()
    try:
        rows = conn.execute(
            "SELECT id, name, status, priority, created, updated, branch_name "
            "FROM gaps_index ORDER BY updated DESC LIMIT 1000"
        ).fetchall()
    finally:
        conn.close()
    extras: list[dict] = []
    for r in rows:
        if r["id"] in seen:
            continue
        gap = shared_gaps.read_gap_json(r["id"])
        if not gap:
            continue
        for round_obj in gap.get("rounds", []):
            blob = " ".join([
                round_obj.get("reporter", "") or "",
                round_obj.get("actual", "") or "",
                round_obj.get("target", "") or "",
            ]).lower()
            if needle in blob:
                extras.append(dict(r))
                break
        if len(initial) + len(extras) >= limit:
            break
    return initial + extras


def get_gap(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        row = conn.execute(
            "SELECT id, name, status, priority, created, updated, branch_name "
            "FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
        if not row:
            return err(404, "Gap not found")
        # Gap-scoped activity entries (lifecycle events, dispatcher errors,
        # subprocess flush nudges). These are merged into the round view so
        # users see real progress even when the round's logs[] is empty.
        gap_activity = activity.recent(conn, limit=500, gap_id=gap_id)
    finally:
        conn.close()
    gap = shared_gaps.read_gap_json(gap_id) or {
        "id": gap_id, "name": row["name"], "rounds": [],
        "created": row["created"], "updated": row["updated"],
    }
    # SQLite is the source of truth for `status` and `priority` — overlay
    # them onto the response.
    gap = dict(gap)
    gap["status"] = row["status"]
    gap["priority"] = row["priority"] or "low"
    gap["branch_name"] = row["branch_name"]
    gap["activity"] = gap_activity
    return 200, {"gap": gap}


_VALID_REPORTER = re.compile(r"^[^\x00-\x1f]{1,80}$")


def create_gap(body: dict) -> tuple[int, dict]:
    reporter = (body.get("reporter") or "").strip()
    actual = (body.get("actual") or "").strip()
    target = (body.get("target") or "").strip()
    name = (body.get("name") or "").strip() or _autoname(actual, target)
    priority = (body.get("priority") or "low").strip().lower()
    if priority not in _VALID_PRIORITIES:
        return err(400, "priority must be one of low/medium/high")
    if not reporter:
        return err(400, "reporter is required")
    if not actual and not target:
        return err(400, "actual or target must be non-empty")
    if not _VALID_REPORTER.match(reporter):
        return err(400, "invalid reporter name")
    gap_id = new_ulid()
    try:
        result = get_client().call(M_CREATE_GAP, {
            "gap_id": gap_id, "name": name, "priority": priority,
            "reporter": reporter, "actual": actual, "target": target,
        })
    except IpcError as e:
        return _ipc_err(e)
    return 201, result


def _autoname(actual: str, target: str) -> str:
    """Cheap, deterministic name from the first sentence of target (or actual)."""
    text = (target or actual or "Untitled Gap").strip()
    text = text.split("\n", 1)[0]
    # first sentence-ish
    m = re.split(r"[.!?]", text, maxsplit=1)
    short = (m[0] if m else text).strip()
    if len(short) > 80:
        short = short[:77].rstrip() + "..."
    return short or "Untitled Gap"


def update_gap_name(gap_id: str, body: dict) -> tuple[int, dict]:
    """PATCH handler: accepts `name`, `priority`, and/or `notes`.

    Name and priority are SQLite-only fields — we write the index row
    directly and nudge gap.json so its mtime matches. Notes live in
    gap.json (gap-level metadata that should travel with the file), so
    we route those writes through the runner via M_SET_NOTES.
    """
    sql_fields: dict[str, str] = {}
    if "name" in body:
        new_name = (body.get("name") or "").strip()
        if not new_name:
            return err(400, "name is required")
        sql_fields["name"] = new_name
    if "priority" in body:
        p = (body.get("priority") or "").strip().lower()
        if p not in _VALID_PRIORITIES:
            return err(400, "priority must be one of low/medium/high")
        sql_fields["priority"] = p
    if "status" in body:
        # Per-Gap status updates power the workflow back/forward buttons
        # on the detail page. The transitions are bookkeeping-only — they
        # don't kick off agent runs, cancel running subprocesses, or
        # touch the worktree. (The agent picks up Gaps in `todo`; the
        # `verify` endpoint still owns the merge+push that lands a Gap
        # in `done`.)
        s = (body.get("status") or "").strip().lower()
        if s not in _VALID_STATUSES:
            return err(400, "invalid status")
        sql_fields["status"] = s
    notes_change = "notes" in body
    if not sql_fields and not notes_change:
        return err(400, "expected `name`, `priority`, `status`, and/or `notes`")
    if sql_fields:
        set_clause = ", ".join(f"{k} = ?" for k in sql_fields) + ", updated = ?"
        args = list(sql_fields.values()) + [now_iso(), gap_id]
        conn = _conn()
        try:
            with db.transaction(conn):
                conn.execute(
                    f"UPDATE gaps_index SET {set_clause} WHERE id = ?", args,
                )
        finally:
            conn.close()
    if notes_change:
        notes = body.get("notes")
        if not isinstance(notes, list):
            return err(400, "notes must be a list of {id, author, body, ...} objects")
        try:
            get_client().call(M_SET_NOTES, {"gap_id": gap_id, "notes": notes})
        except IpcError as e:
            return _ipc_err(e)
    elif sql_fields:
        # nudge gap.json's mtime to match the index update.
        try:
            get_client().call(M_EDIT_ROUND, {
                "gap_id": gap_id, "actual": None, "target": None, "reporter": None,
            })
        except IpcError:
            pass
    return 200, {"ok": True}


def delete_gap(gap_id: str) -> tuple[int, dict]:
    try:
        result = get_client().call(M_DELETE_GAP, {"gap_id": gap_id})
    except IpcError as e:
        return _ipc_err(e)
    return 200, result


def bulk_update_gaps(body: dict) -> tuple[int, dict]:
    """Apply a single field update to every Gap matching the supplied filter.

    Body shape:

        {"filter": {"status": "...", "q": "..."},
         "update": {"priority": "high"} | {"status": "cancelled"} | {"reporter": "alice"}}

    Exactly one update key is honored per call so the action is unambiguous
    to confirm in the UI. `priority` and `status` are SQL-index fields and
    are updated in a single transaction; `reporter` rewrites each Gap's
    latest round via the runner-owned `M_EDIT_ROUND` path, one Gap at a
    time. Status changes here are bookkeeping-only — they don't trigger
    workflow side effects like killing a running subprocess or cleaning
    up a worktree; for those, use the per-Gap action on the detail page.
    """
    update = body.get("update") or {}
    update = {k: v for k, v in update.items()
              if k in ("priority", "status", "reporter")}
    if len(update) != 1:
        return err(400,
                   "update must contain exactly one of "
                   "`priority`, `status`, or `reporter`")
    field, raw = next(iter(update.items()))
    value = (raw or "").strip()
    if field == "priority":
        value = value.lower()
        if value not in _VALID_PRIORITIES:
            return err(400, "priority must be one of low/medium/high")
    elif field == "status":
        value = value.lower()
        if value not in _VALID_STATUSES:
            return err(400, "invalid status")
    else:  # reporter
        if not value or not _VALID_REPORTER.match(value):
            return err(400, "invalid reporter name")

    filt = body.get("filter") or {}
    excluded = set(body.get("exclude_ids") or [])
    code, listing = list_gaps(
        status=filt.get("status") or None,
        q=filt.get("q") or None,
        severity=filt.get("severity") or None,
        category=filt.get("category") or None,
        actor=filt.get("actor") or None,
        reporter=filt.get("reporter") or None,
        limit=10_000,
    )
    if code != 200:
        return code, listing
    gap_ids = [g["id"] for g in (listing.get("gaps") or [])
               if g["id"] not in excluded]
    if not gap_ids:
        return 200, {"updated": 0, "ids": []}

    updated_ids: list[str] = []

    if field in ("priority", "status"):
        conn = _conn()
        try:
            with db.transaction(conn):
                placeholders = ",".join("?" * len(gap_ids))
                conn.execute(
                    f"UPDATE gaps_index SET {field} = ?, updated = ? "
                    f"WHERE id IN ({placeholders})",
                    [value, now_iso(), *gap_ids],
                )
        finally:
            conn.close()
        # Nudge each gap.json's mtime via the runner so listings sort
        # consistently and any file-watchers see the touch.
        for gid in gap_ids:
            try:
                get_client().call(M_EDIT_ROUND, {
                    "gap_id": gid,
                    "actual": None, "target": None, "reporter": None,
                })
                updated_ids.append(gid)
            except IpcError:
                updated_ids.append(gid)
    else:  # reporter — rewrite the latest round's reporter on each gap
        for gid in gap_ids:
            try:
                get_client().call(M_EDIT_ROUND, {
                    "gap_id": gid,
                    "actual": None, "target": None,
                    "reporter": value,
                })
                updated_ids.append(gid)
            except IpcError:
                # Skip gaps the runner refused (no rounds yet, etc.) and
                # keep going. The response count reflects what stuck.
                continue

    return 200, {"updated": len(updated_ids), "ids": updated_ids,
                 "field": field, "value": value}


def bulk_delete_gaps(body: dict) -> tuple[int, dict]:
    """Delete every Gap matching the supplied filter.

    Each Gap is dispatched through the same `M_DELETE_GAP` path a
    single-Gap delete uses, so the runner cancels any running
    subprocess, tears down the worktree + branch for non-done gaps,
    erases gap.json, and cleans the index. Per-Gap failures don't
    abort the run — we collect them in the response.
    """
    filt = body.get("filter") or {}
    excluded = set(body.get("exclude_ids") or [])
    code, listing = list_gaps(
        status=filt.get("status") or None,
        q=filt.get("q") or None,
        severity=filt.get("severity") or None,
        category=filt.get("category") or None,
        actor=filt.get("actor") or None,
        reporter=filt.get("reporter") or None,
        limit=10_000,
    )
    if code != 200:
        return code, listing
    gap_ids = [g["id"] for g in (listing.get("gaps") or [])
               if g["id"] not in excluded]
    if not gap_ids:
        return 200, {"deleted": 0, "ids": [], "failures": []}

    deleted_ids: list[str] = []
    failures: list[dict] = []
    for gid in gap_ids:
        try:
            res = get_client().call(
                M_DELETE_GAP, {"gap_id": gid}, timeout=60.0,
            )
            if res.get("deleted"):
                deleted_ids.append(gid)
        except IpcError as e:
            failures.append({"id": gid, "error": str(e)})
    return 200, {
        "deleted": len(deleted_ids),
        "ids": deleted_ids,
        "failures": failures,
    }


def append_round(gap_id: str, body: dict) -> tuple[int, dict]:
    reporter = (body.get("reporter") or "").strip()
    actual = (body.get("actual") or "").strip()
    target = (body.get("target") or "").strip()
    if not reporter:
        return err(400, "reporter is required")
    if not actual and not target:
        return err(400, "actual or target must be non-empty")
    # Guard: only allowed from review or failed (or todo, treated as edit of latest).
    conn = _conn()
    try:
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
    finally:
        conn.close()
    if not row:
        return err(404, "Gap not found")
    if row["status"] != "review":
        return err(
            409,
            "New rounds may only be appended from `review` "
            f"(status={row['status']}). From `todo` or `failed`, edit the "
            "latest round instead."
        )
    try:
        result = get_client().call(M_APPEND_ROUND, {
            "gap_id": gap_id, "reporter": reporter,
            "actual": actual, "target": target,
        })
    except IpcError as e:
        return _ipc_err(e)
    return 201, result


def edit_latest_round(gap_id: str, body: dict) -> tuple[int, dict]:
    conn = _conn()
    try:
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
    finally:
        conn.close()
    if not row:
        return err(404, "Gap not found")
    if row["status"] not in ("backlog", "todo", "failed"):
        return err(409, "Only the latest unaddressed round can be edited "
                        f"(status={row['status']})")
    try:
        result = get_client().call(M_EDIT_ROUND, {
            "gap_id": gap_id,
            "actual": body.get("actual"),
            "target": body.get("target"),
            "reporter": body.get("reporter"),
        })
    except IpcError as e:
        return _ipc_err(e)
    return 200, result


def verify(gap_id: str) -> tuple[int, dict]:
    try:
        result = get_client().call(M_VERIFY, {"gap_id": gap_id}, timeout=120.0)
    except IpcError as e:
        return _ipc_err(e)
    return 200, result


def list_changes(*, limit: int = 50) -> tuple[int, dict]:
    """List refine merge commits on the target branch (plus the Gap
    metadata for each). Used by the Changes screen."""
    try:
        result = get_client().call(
            M_LIST_CHANGES, {"limit": int(limit)}, timeout=15.0,
        )
    except IpcError as e:
        return _ipc_err(e)
    return 200, result


def undo_change(body: dict) -> tuple[int, dict]:
    """Revert a refine merge commit. The runner derives the Gap id from
    the commit's `Refine Gap:` trailer, switches branches if needed,
    runs `git revert -m 1`, pushes when an upstream exists, and moves
    the Gap to `cancelled` with a log entry."""
    commit = (body.get("commit") or "").strip()
    if not commit:
        return err(400, "commit is required")
    try:
        result = get_client().call(
            M_UNDO_GAP, {"commit": commit}, timeout=120.0,
        )
    except IpcError as e:
        return _ipc_err(e)
    code = 200 if result.get("ok") else 409
    return code, result


def retry(gap_id: str) -> tuple[int, dict]:
    """Reopen a terminal Gap by transitioning it back to `todo` so the
    dispatcher picks it up again. Allowed from `failed`, `done`, or
    `cancelled`. (Webapp writes `status=todo` directly per the write-
    ownership split.)
    """
    conn = _conn()
    try:
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
        if not row:
            return err(404, "Gap not found")
        prev_status = row["status"]
        if prev_status not in ("failed", "done", "cancelled"):
            return err(
                409,
                f"Reopen only valid from failed/done/cancelled (status={prev_status})",
            )
        # If the most recent failure was an auth issue, re-run pre-flight
        # before reopening so we don't immediately fail again.
        last = conn.execute(
            "SELECT failure_category FROM runs WHERE gap_id = ? "
            "ORDER BY id DESC LIMIT 1", (gap_id,),
        ).fetchone()
        if last and last["failure_category"] == "auth":
            try:
                pf = get_client().call(M_PREFLIGHT, {})
                if not pf.get("ok"):
                    return err(409, "Auth pre-flight still failing — Reopen blocked",
                               pf.get("message"))
            except IpcError as e:
                return _ipc_err(e)
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'todo', updated = ? WHERE id = ?",
                (now_iso(), gap_id),
            )
        activity.append(
            conn, message=f"Reopened from {prev_status} → todo",
            severity="info", category="state",
            gap_id=gap_id, actor="refine",
        )
    finally:
        conn.close()
    return 200, {"ok": True}


def cancel(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
    finally:
        conn.close()
    if not row:
        return err(404, "Gap not found")
    if row["status"] in ("done", "cancelled"):
        return err(409, f"Already terminal (status={row['status']})")
    try:
        result = get_client().call(M_CANCEL, {"gap_id": gap_id})
    except IpcError as e:
        return _ipc_err(e)
    return 200, result


# --- Reporters ----------------------------------------------------------------

def list_reporters() -> tuple[int, dict]:
    conn = _conn()
    try:
        return 200, {"reporters": reporters.list_all(conn)}
    finally:
        conn.close()


def create_reporter(body: dict) -> tuple[int, dict]:
    name = (body.get("name") or "").strip()
    if not name:
        return err(400, "name is required")
    conn = _conn()
    try:
        rep = reporters.add(conn, name)
    finally:
        conn.close()
    return 201, {"reporter": rep}


def rename_reporter(rid: int, body: dict) -> tuple[int, dict]:
    name = (body.get("name") or "").strip()
    if not name:
        return err(400, "name is required")
    if not _VALID_REPORTER.match(name):
        return err(400, "invalid reporter name")
    # Route through the runner so the rename cascades through every Gap's
    # `rounds[].reporter` strings — keeping the dropdown and historical
    # data in sync. (Deletes deliberately don't cascade; see delete_reporter.)
    try:
        result = get_client().call(
            M_RENAME_REPORTER, {"rid": rid, "new_name": name}, timeout=60.0,
        )
    except IpcError as e:
        return _ipc_err(e)
    return 200, {"ok": True, **result}


def delete_reporter(rid: int) -> tuple[int, dict]:
    conn = _conn()
    try:
        reporters.remove(conn, rid)
    finally:
        conn.close()
    return 200, {"ok": True}


# --- Settings -----------------------------------------------------------------

def list_settings() -> tuple[int, dict]:
    conn = _conn()
    try:
        return 200, {"settings": db.list_settings(conn)}
    finally:
        conn.close()


def update_settings(body: dict) -> tuple[int, dict]:
    if not isinstance(body, dict) or not body:
        return err(400, "expected an object of {key: value}")
    allowed = {
        "parallel_run_cap", "branch_name_pattern",
        "agent_idle_timeout_seconds", "agent_hard_cap_seconds",
        "chat_idle_timeout_seconds",
        "agent_subpath", "merge_target_branch",
        "paused",
    }
    normalized: dict[str, str] = {}
    for k, v in body.items():
        if k not in allowed:
            return err(400, f"unknown setting: {k}")
        if k == "merge_target_branch":
            br = str(v or "").strip()
            # Empty means "follow host's current branch". Validate format
            # only — existence is checked at the time it's used so the
            # operator can pre-configure before the branch exists.
            if br:
                if any(c.isspace() for c in br):
                    return err(400, "merge_target_branch may not contain whitespace")
                if br.startswith("-") or "\0" in br:
                    return err(400, "merge_target_branch contains an invalid character")
            normalized[k] = br
        elif k == "agent_subpath":
            sub = str(v or "").strip()
            # Reject absolute paths, `..` traversal, and any embedded NUL.
            if sub:
                if sub.startswith("/") or sub.startswith("~"):
                    return err(400, "agent_subpath must be relative to the repo root")
                if "\0" in sub:
                    return err(400, "agent_subpath contains an invalid character")
                parts = [p for p in sub.replace("\\", "/").split("/") if p]
                if any(p == ".." for p in parts):
                    return err(400, "agent_subpath must not contain `..` components")
                sub = "/".join(parts)
            normalized[k] = sub
        else:
            normalized[k] = str(v)
    conn = _conn()
    try:
        for k, v in normalized.items():
            db.set_setting(conn, k, v)
        activity.append(
            conn, message=f"Settings updated: {', '.join(normalized.keys())}",
            severity="info", category="user", actor="refine",
        )
    finally:
        conn.close()
    return 200, {"ok": True}


def recheck_auth() -> tuple[int, dict]:
    try:
        result = get_client().call(M_PREFLIGHT, {}, timeout=30.0)
    except IpcError as e:
        return _ipc_err(e)
    return 200, result


def ipc_diagnostics() -> tuple[int, dict]:
    try:
        result = get_client().call(M_DIAGNOSTICS, {}, timeout=5.0)
    except IpcError as e:
        return 200, {"reachable": False, "error": {"message": e.message,
                                                    "code": e.code}}
    result["reachable"] = True
    return 200, result


# --- Activity / Dashboard -----------------------------------------------------

def list_activity(*, limit: int = 100, gap_id: str | None = None,
                  since_id: int | None = None,
                  severity: str | None = None,
                  category: str | None = None,
                  actor: str | None = None,
                  q: str | None = None,
                  include_facets: bool = False) -> tuple[int, dict]:
    conn = _conn()
    try:
        entries = activity.recent(
            conn, limit=limit, gap_id=gap_id, since_id=since_id,
            severity=severity, category=category, actor=actor, q=q,
        )
        body: dict = {"activity": entries}
        if include_facets:
            body["facets"] = {
                "categories": activity.distinct_categories(conn),
                "actors": activity.distinct_actors(conn),
                "severities": ["info", "warn", "error"],
            }
    finally:
        conn.close()
    return 200, body


_LOG_RETENTION_OPTIONS = (0, 7, 30, 60, 90, 365)


def cleanup_logs(body: dict) -> tuple[int, dict]:
    """Delete activity entries older than `days` days.

    `days == 0` deletes the whole activity table (operator chose
    "don't keep any"). Anything else uses an ISO-timestamp cutoff
    computed against `now`. Returns the number of rows deleted.
    """
    raw = body.get("days")
    try:
        days = int(raw)
    except (TypeError, ValueError):
        return err(400, "days must be an integer")
    if days not in _LOG_RETENTION_OPTIONS:
        return err(
            400,
            f"days must be one of {sorted(_LOG_RETENTION_OPTIONS)}",
        )
    conn = _conn()
    try:
        if days == 0:
            cur = conn.execute("DELETE FROM activity")
        else:
            cutoff = (
                datetime.now(timezone.utc) - timedelta(days=days)
            ).strftime("%Y-%m-%dT%H:%M:%SZ")
            cur = conn.execute(
                "DELETE FROM activity WHERE datetime < ?", (cutoff,),
            )
        deleted = cur.rowcount or 0
        conn.commit()
    finally:
        conn.close()
    return 200, {"deleted": deleted, "days_kept": days}


def dashboard_summary() -> tuple[int, dict]:
    conn = _conn()
    try:
        counts = {}
        for row in conn.execute(
            "SELECT status, COUNT(*) AS n FROM gaps_index GROUP BY status"
        ):
            counts[row["status"]] = row["n"]
        merger_snap: dict | None = None
        try:
            r = get_client().call(M_RUNNING, {}, timeout=5.0)
            running = r.get("running", [])
            merger_snap = r.get("merger")
        except IpcError:
            running = []
        pf = conn.execute(
            "SELECT ok, checked_at, message FROM preflight WHERE id = 1"
        ).fetchone()
        preflight = ({
            "ok": bool(pf["ok"]), "checked_at": pf["checked_at"],
            "message": pf["message"],
        } if pf else None)
        # latest activity (top of feed)
        feed = activity.recent(conn, limit=50)
        # Per-reporter stats: the runner mirrors the latest round's
        # reporter onto `gaps_index.reporter`, so the SQL aggregation
        # gives us exact counts without reading every gap.json.
        stat_rows = conn.execute(
            "SELECT reporter, status, COUNT(*) AS n "
            "FROM gaps_index "
            "WHERE reporter != '' "
            "GROUP BY reporter, status"
        ).fetchall()
        known_reporters = [r["name"] for r in reporters.list_all(conn)]
    finally:
        conn.close()
    reporter_stats = _compute_reporter_stats(stat_rows, known_reporters)
    runner_reachable = get_client().is_reachable()
    return 200, {
        "counts": counts,
        "running": running,
        "merger": merger_snap,
        "preflight": preflight,
        "activity": feed,
        "runner_reachable": runner_reachable,
        "reporter_stats": reporter_stats,
        "needs_attention": _compute_needs_attention(counts, preflight,
                                                    runner_reachable),
    }


_ACTIVE_STATUSES = ("todo", "in-progress", "ready-merge", "review")


def _compute_reporter_stats(stat_rows, known_reporters: list[str]) -> list[dict]:
    """Build `reporter_stats` from the pre-aggregated (reporter, status,
    count) rows produced by the dashboard query. Seeds every known
    reporter (so inactive ones show as zeroes), then folds in any
    historical reporters that appear on Gaps but aren't in the table."""
    def _empty(name: str) -> dict:
        return {"reporter": name, "active": 0, "done": 0,
                "reported": 0, "completion_rate": 0.0}

    by_reporter: dict[str, dict] = {n: _empty(n) for n in known_reporters}
    for row in stat_rows:
        reporter = row["reporter"]
        bucket = by_reporter.setdefault(reporter, _empty(reporter))
        n = row["n"]
        bucket["reported"] += n
        status = row["status"]
        if status in _ACTIVE_STATUSES:
            bucket["active"] += n
        elif status == "done":
            bucket["done"] += n
    out = list(by_reporter.values())
    for b in out:
        b["completion_rate"] = (
            round(100.0 * b["done"] / b["reported"], 1) if b["reported"] else 0.0
        )
    out.sort(key=lambda b: (-b["done"], b["reporter"].lower()))
    return out


def _compute_needs_attention(counts: dict, preflight: dict | None,
                              runner_reachable: bool) -> list[dict]:
    items: list[dict] = []
    if not runner_reachable:
        items.append({
            "kind": "banner", "severity": "error",
            "message": "Host runner unreachable",
        })
    if preflight and not preflight.get("ok"):
        items.append({
            "kind": "banner", "severity": "error",
            "message": "Refine cannot reach Claude — run `claude login` on the host",
        })
    if counts.get("failed", 0):
        items.append({
            "kind": "filter", "severity": "warn",
            "message": f"{counts['failed']} failed Gaps",
            "filter": {"status": "failed"},
        })
    return items


# --- Import (LLM extraction) --------------------------------------------------

def import_extract(body: dict) -> tuple[int, dict]:
    """LLM-driven extraction: hand the raw text to the host claude CLI
    via the runner and return the parsed `{name, actual, target}` drafts
    for the user to review before persisting. Times out generously since
    the model call can take 30–90s for longer pastes.
    """
    raw = (body.get("text") or "").strip()
    if not raw:
        return err(400, "text is required")
    try:
        result = get_client().call(
            M_EXTRACT_GAPS, {"text": raw}, timeout=200.0,
        )
    except IpcError as e:
        return _ipc_err(e)
    return 200, {"drafts": result.get("drafts") or []}


def import_persist(body: dict) -> tuple[int, dict]:
    """Persist user-confirmed extracted Gaps."""
    reporter = (body.get("reporter") or "").strip()
    drafts = body.get("drafts") or []
    if not reporter:
        return err(400, "reporter is required")
    if not isinstance(drafts, list) or not drafts:
        return err(400, "drafts must be a non-empty list")
    created = []
    for d in drafts:
        actual = (d.get("actual") or "").strip()
        target = (d.get("target") or "").strip()
        name = (d.get("name") or "").strip() or _autoname(actual, target)
        if not actual and not target:
            continue
        gap_id = new_ulid()
        try:
            get_client().call(M_CREATE_GAP, {
                "gap_id": gap_id, "name": name, "reporter": reporter,
                "actual": actual, "target": target,
            })
            created.append(gap_id)
        except IpcError as e:
            return _ipc_err(e)
    return 201, {"created": created, "count": len(created)}


# --- Chat ---------------------------------------------------------------------

def chat_start(body: dict) -> tuple[int, dict]:
    try:
        result = get_client().call(M_CHAT_START, {"gap_id": body.get("gap_id")})
    except IpcError as e:
        return _ipc_err(e)
    return 201, result


def chat_input(sid: str, body: dict) -> tuple[int, dict]:
    text = body.get("text", "")
    try:
        result = get_client().call(M_CHAT_INPUT, {"session_id": sid, "text": text})
    except IpcError as e:
        return _ipc_err(e)
    return 200, result


def chat_read(sid: str) -> tuple[int, dict]:
    try:
        result = get_client().call(M_CHAT_READ, {"session_id": sid})
    except IpcError as e:
        return _ipc_err(e)
    return 200, result


def chat_stop(sid: str) -> tuple[int, dict]:
    try:
        result = get_client().call(M_CHAT_STOP, {"session_id": sid})
    except IpcError as e:
        return _ipc_err(e)
    return 200, result


# --- helpers ------------------------------------------------------------------

def _ipc_err(e: IpcError) -> tuple[int, dict]:
    code = 502 if e.code == "runner_unreachable" else 500
    return code, {"error": {"code": e.code, "message": e.message,
                            "details": e.details}}
