"""JSON API endpoint handlers.

Returns (status_code, body_dict) tuples. The server module wraps these.
"""
from __future__ import annotations

import json
import re
import sqlite3
import subprocess
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from refine_shared import activity, config, db, gaps as shared_gaps, project_registry, reporters
from refine_shared.gaps import now_iso
from refine_shared.backend_protocol import (
    M_APPEND_ROUND, M_CANCEL, M_CHAT_INPUT, M_CHAT_READ, M_CHAT_START,
    M_CHAT_STOP, M_CREATE_GAP, M_DELETE_GAP, M_DIAGNOSTICS, M_EDIT_ROUND,
    M_EXTRACT_GAPS, M_LAUNCH, M_LIST_CHANGES, M_LOG_APPEND, M_PREFLIGHT,
    M_RENAME_REPORTER, M_RUNNING, M_SET_NOTES, M_TARGET_APP_GENERATE,
    M_TARGET_APP_HEALTH, M_TARGET_APP_RUN, M_UNDO_GAP, M_VERIFY,
)
from refine_shared.ulid import new_ulid

from .backend_client import BackendError, get_client
from . import runtime


# --- error helpers ------------------------------------------------------------

def err(code: int, message: str, details: str | None = None) -> tuple[int, dict]:
    body: dict[str, Any] = {"error": {"message": message}}
    if details is not None:
        body["error"]["details"] = details
    return code, body


def _conn() -> sqlite3.Connection:
    return db.connect()


# --- Project attach/setup -----------------------------------------------------

def project_status() -> tuple[int, dict]:
    """Return whether this UI process is attached to a refine project."""
    clone_dir = Path.cwd().resolve()
    registry_enabled = _project_registry_enabled(clone_dir)
    apps = project_registry.list_apps(clone_dir) if registry_enabled else []
    cfg_path = config.find_config()
    if cfg_path is None:
        return 200, {
            "attached": False,
            "apps": apps,
            "registry_enabled": registry_enabled,
            "message": "No refine project is attached.",
        }
    try:
        cfg = config.get(reload=True)
    except config.ConfigError as e:
        return 200, {
            "attached": False,
            "apps": apps,
            "registry_enabled": registry_enabled,
            "config_path": str(cfg_path),
            "message": str(e),
        }
    if registry_enabled:
        apps = project_registry.upsert_app(clone_dir, cfg.client_repo, make_current=True)
    else:
        apps = _ensure_current_app(apps, cfg.client_repo)
    return 200, {
        "attached": True,
        "apps": apps,
        "registry_enabled": registry_enabled,
        "client_repo": str(cfg.client_repo),
        "volume_root": str(cfg.volume_root),
        "config_path": str(cfg.config_path),
    }


def project_list() -> tuple[int, dict]:
    clone_dir = Path.cwd().resolve()
    current = ""
    apps = project_registry.list_apps(clone_dir) if _project_registry_enabled(clone_dir) else []
    try:
        current_repo = config.get(reload=True).client_repo
        current = str(current_repo)
        apps = _ensure_current_app(apps, current_repo)
    except config.ConfigError:
        pass
    return 200, {
        "apps": apps,
        "current": current,
    }


def project_remove(body: dict[str, Any]) -> tuple[int, dict]:
    raw_path = (body.get("path") or "").strip()
    if not raw_path:
        return err(400, "Choose an app to remove.")
    clone_dir = Path.cwd().resolve()
    if not _project_registry_enabled(clone_dir):
        return err(409, "Known-apps list is only available from the host refine source checkout.")
    target = Path(raw_path).expanduser().resolve()
    try:
        current = config.get(reload=True).client_repo
    except config.ConfigError:
        current = None
    if current is not None and current == target:
        return err(409, "Cannot remove the currently attached app. Switch to another app first.")
    apps = project_registry.remove_app(clone_dir, target)
    return 200, {"apps": apps}


def project_attach(body: dict[str, Any]) -> tuple[int, dict]:
    """Create or attach a target app path and make it active."""
    raw_path = (body.get("path") or "").strip()
    if not raw_path:
        return err(400, "Enter a project path.")

    clone_dir = Path.cwd().resolve()
    client_repo = Path(raw_path).expanduser()

    try:
        from refine_cli.cli import (
            _InitError, _is_refine_source_dir, bootstrap_client_repo,
        )

        if not _is_refine_source_dir(clone_dir):
            return err(
                409,
                "Project setup must run from the host refine source directory.",
                (
                    f"The UI process is running in {clone_dir}. Start refine from "
                    "the source checkout with `uv run refine start` so it can "
                    "create host directories and write the binding."
                ),
            )

        current_before = _current_client_repo()
        switching = current_before is not None and current_before != client_repo.resolve()
        prep = _prepare_current_project_for_switch(clone_dir) if switching else {"warnings": []}

        install_unit = body.get("install_unit") is not False
        result = bootstrap_client_repo(
            client_repo,
            clone_dir=clone_dir,
            force=True,
            create=True,
            init_git=True,
            reuse_existing_config=True,
            install_unit=install_unit,
        )
        cfg = runtime.load_configured(
            result["config_path"],
            start_poller=body.get("start_poller") is not False,
            start_runner=body.get("start_runner") is not False,
        )
    except (config.ConfigError, _InitError, OSError, TimeoutError) as e:
        return err(400, str(e))
    except _SwitchBlocked as e:
        return err(409, str(e), e.details)

    runner = {"started": False, "message": ""}
    if body.get("start_runner") is not False:
        runner = {"started": True, "message": "Backend runner started in the UI process."}

    return 200, {
        "attached": True,
        "client_repo": str(cfg.client_repo),
        "volume_root": str(cfg.volume_root),
        "config_path": str(cfg.config_path),
        "binding_path": str(result["binding_path"]) if result.get("binding_path") else "",
        "unit_path": str(result["unit_path"]) if result.get("unit_path") else "",
        "ui_unit_path": str(result["ui_unit_path"]) if result.get("ui_unit_path") else "",
        "git_initialized": bool(result.get("git_initialized")),
        "config_created": bool(result.get("config_created")),
        "apps": project_registry.list_apps(clone_dir),
        "registry_enabled": True,
        "switch_warnings": prep.get("warnings", []),
        "runner": runner,
    }


def _project_registry_enabled(clone_dir: Path) -> bool:
    return (clone_dir / "pyproject.toml").is_file() and (clone_dir / "refine_cli" / "cli.py").is_file()


def _ensure_current_app(apps: list[dict[str, str]], client_repo: Path) -> list[dict[str, str]]:
    """Always include the active app, even when the clone-local registry is unavailable."""
    current = str(client_repo.resolve())
    if any(app.get("path") == current for app in apps):
        return apps
    return [
        *apps,
        {
            "name": client_repo.name or current,
            "path": current,
            "added_at": "",
            "last_used_at": "",
        },
    ]


class _SwitchBlocked(Exception):
    def __init__(self, message: str, details: str | None = None) -> None:
        super().__init__(message)
        self.details = details


def _current_client_repo() -> Path | None:
    try:
        return config.get(reload=True).client_repo
    except config.ConfigError:
        return None


def _prepare_current_project_for_switch(clone_dir: Path) -> dict[str, Any]:
    """Stop active agents and leave the current target app clean before switching."""
    warnings: list[str] = []
    cfg = config.get(reload=True)
    runtime.stop_runner()

    _commit_refine_state(cfg.client_repo)
    dirty = _git_stdout(cfg.client_repo, ["status", "--porcelain"])
    if dirty.strip():
        raise _SwitchBlocked(
            "Current app has uncommitted changes.",
            (
                "Commit, stash, or discard changes in the current app before switching:\n"
                + dirty.strip()
            ),
        )
    return {"warnings": warnings}


def _commit_refine_state(repo: Path) -> None:
    dirty_refine = _git_stdout(repo, ["status", "--porcelain", "--", ".refine"])
    if not dirty_refine.strip():
        return
    _git_checked(repo, ["add", ".refine"])
    staged = subprocess.run(
        ["git", "diff", "--cached", "--quiet"],
        cwd=str(repo), capture_output=True, text=True, timeout=30,
    )
    if staged.returncode == 0:
        return
    _git_checked(repo, [
        "-c", "user.email=refine@localhost",
        "-c", "user.name=refine",
        "commit", "-m", "refine: persist state before switch",
    ])


def _git_stdout(repo: Path, args: list[str]) -> str:
    out = subprocess.run(
        ["git", *args], cwd=str(repo), capture_output=True, text=True, timeout=30,
    )
    if out.returncode != 0:
        raise _SwitchBlocked(
            "Could not inspect current app git state.",
            (out.stderr or out.stdout or f"git {' '.join(args)} failed").strip(),
        )
    return out.stdout


def _git_checked(repo: Path, args: list[str]) -> None:
    out = subprocess.run(
        ["git", *args], cwd=str(repo), capture_output=True, text=True, timeout=30,
    )
    if out.returncode != 0:
        raise _SwitchBlocked(
            "Could not prepare current app for switching.",
            (out.stderr or out.stdout or f"git {' '.join(args)} failed").strip(),
        )


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
    except BackendError as e:
        return _backend_err(e)
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
        except BackendError as e:
            return _backend_err(e)
    elif sql_fields:
        # nudge gap.json's mtime to match the index update.
        try:
            get_client().call(M_EDIT_ROUND, {
                "gap_id": gap_id, "actual": None, "target": None, "reporter": None,
            })
        except BackendError:
            pass
    return 200, {"ok": True}


def delete_gap(gap_id: str) -> tuple[int, dict]:
    try:
        result = get_client().call(M_DELETE_GAP, {"gap_id": gap_id})
    except BackendError as e:
        return _backend_err(e)
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
            except BackendError:
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
            except BackendError:
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
        except BackendError as e:
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
    except BackendError as e:
        return _backend_err(e)
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
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def verify(gap_id: str) -> tuple[int, dict]:
    try:
        result = get_client().call(M_VERIFY, {"gap_id": gap_id}, timeout=120.0)
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def list_changes(*, limit: int = 50) -> tuple[int, dict]:
    """List refine merge commits on the target branch (plus the Gap
    metadata for each). Used by the Changes screen."""
    try:
        result = get_client().call(
            M_LIST_CHANGES, {"limit": int(limit)}, timeout=15.0,
        )
    except BackendError as e:
        return _backend_err(e)
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
    except BackendError as e:
        return _backend_err(e)
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
            except BackendError as e:
                return _backend_err(e)
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
    except BackendError as e:
        return _backend_err(e)
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
    except BackendError as e:
        return _backend_err(e)
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


def list_features() -> tuple[int, dict]:
    """Provider-scoped feature flag matrix. Used by the Settings UI
    to render the Feature flags card and by client-side gating of
    Chat / Import affordances."""
    from refine_shared import features
    conn = _conn()
    try:
        return 200, features.get_matrix(conn)
    finally:
        conn.close()


def set_feature_override(body: dict) -> tuple[int, dict]:
    """Operator override for a (provider, feature) cell. Body:
        {"provider": "codex", "feature": "chat", "enabled": true|false|null}
    `enabled=null` clears the override so the code-defined default
    re-applies."""
    from refine_shared import features
    if not isinstance(body, dict):
        return err(400, "expected an object")
    provider = (body.get("provider") or "").strip().lower()
    feature = (body.get("feature") or "").strip()
    if provider not in features.PROVIDERS:
        return err(400, f"unknown provider: {provider}")
    if feature not in features.FEATURES:
        return err(400, f"unknown feature: {feature}")
    enabled = body.get("enabled")
    if enabled is not None and not isinstance(enabled, bool):
        return err(400, "enabled must be true, false, or null")
    conn = _conn()
    try:
        features.set_override(conn, provider, feature, enabled)
        activity.append(
            conn,
            message=(f"Feature flag `{provider}.{feature}` "
                     + (f"overridden to {enabled}"
                        if enabled is not None else "override cleared")),
            severity="info", category="user", actor="refine",
        )
    finally:
        conn.close()
    return 200, {"ok": True}


def update_settings(body: dict) -> tuple[int, dict]:
    if not isinstance(body, dict) or not body:
        return err(400, "expected an object of {key: value}")
    allowed = {
        "parallel_run_cap", "branch_name_pattern",
        "agent_idle_timeout_seconds", "agent_hard_cap_seconds",
        "chat_idle_timeout_seconds",
        "backlog_promote_after_seconds",
        "agent_subpath", "merge_target_branch",
        "agent_cli",
        "paused",
        # Target-app configuration. The state fields
        # (target_app_state etc.) are owned by the system and are
        # mutated via the /api/target-app/* endpoints, not Settings.
        "target_app_start_instructions",
        "target_app_stop_instructions",
        "target_app_health_url",
        "target_app_start_command",
        "target_app_stop_command",
        "target_app_status_command",
        "target_app_cwd",
        "target_app_env_json",
        "target_app_start_timeout_seconds",
        "target_app_stop_timeout_seconds",
        "target_app_status_timeout_seconds",
        "target_app_log_path",
        "target_app_http_check_url",
        "target_app_tcp_check_host",
        "target_app_tcp_check_port",
        "target_app_process_check_command",
    }
    valid_agent_clis = ("claude", "codex", "gemini")
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
        elif k == "agent_cli":
            choice = str(v or "").strip().lower()
            if choice not in valid_agent_clis:
                return err(400,
                    f"agent_cli must be one of {', '.join(valid_agent_clis)}")
            normalized[k] = choice
        elif k == "target_app_cwd":
            cwd = str(v or "").strip()
            if cwd and "\0" in cwd:
                return err(400, "target_app_cwd contains an invalid character")
            if cwd.startswith("~"):
                return err(400, "target_app_cwd must be absolute or relative to the repo root")
            if cwd and not cwd.startswith("/"):
                parts = [p for p in cwd.replace("\\", "/").split("/") if p]
                if any(p == ".." for p in parts):
                    return err(400, "target_app_cwd must not contain `..` components")
                cwd = "/".join(parts)
            normalized[k] = cwd
        elif k == "target_app_env_json":
            raw = str(v or "{}").strip() or "{}"
            try:
                env_obj = json.loads(raw)
            except json.JSONDecodeError:
                return err(400, "target_app_env_json must be a JSON object")
            if not isinstance(env_obj, dict):
                return err(400, "target_app_env_json must be a JSON object")
            normalized[k] = json.dumps({str(ek): str(ev) for ek, ev in env_obj.items()})
        elif k in {
            "target_app_start_timeout_seconds",
            "target_app_stop_timeout_seconds",
            "target_app_status_timeout_seconds",
        }:
            try:
                n = int(v)
            except (TypeError, ValueError):
                return err(400, f"{k} must be an integer")
            if n < 1 or n > 3600:
                return err(400, f"{k} must be between 1 and 3600")
            normalized[k] = str(n)
        elif k == "target_app_tcp_check_port":
            port = str(v or "").strip()
            if port:
                try:
                    n = int(port)
                except ValueError:
                    return err(400, "target_app_tcp_check_port must be an integer")
                if n < 1 or n > 65535:
                    return err(400, "target_app_tcp_check_port must be between 1 and 65535")
                port = str(n)
            normalized[k] = port
        elif k == "backlog_promote_after_seconds":
            # -1 = never, 0 = instant, otherwise seconds. Restrict to the
            # canonical set shown in the UI so a stale client can't smuggle
            # in something weird.
            try:
                n = int(v)
            except (TypeError, ValueError):
                return err(400, "backlog_promote_after_seconds must be an integer")
            allowed_intervals = {-1, 0, 300, 1800, 3600, 10800, 21600, 86400}
            if n not in allowed_intervals:
                return err(400,
                    "backlog_promote_after_seconds must be one of "
                    "-1 (never), 0 (instant), 300, 1800, 3600, 10800, 21600, 86400")
            normalized[k] = str(n)
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
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def backend_diagnostics() -> tuple[int, dict]:
    try:
        result = get_client().call(M_DIAGNOSTICS, {}, timeout=5.0)
    except BackendError as e:
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
        except BackendError:
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
        provider = (db.get_setting(conn, "agent_cli") or "claude").strip().lower()
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
        "needs_attention": _compute_needs_attention(
            counts, preflight, runner_reachable, provider,
        ),
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
                              runner_reachable: bool,
                              provider: str = "claude") -> list[dict]:
    items: list[dict] = []
    if not runner_reachable:
        items.append({
            "kind": "banner", "severity": "error",
            "message": "Backend runner unavailable",
        })
    if preflight and not preflight.get("ok"):
        login_hint = {
            "claude": "claude login",
            "codex": "codex login",
            "gemini": "gemini auth login",
        }.get(provider, f"{provider} login")
        items.append({
            "kind": "banner", "severity": "error",
            "message": f"Refine cannot reach {provider} — run `{login_hint}` on the host",
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
    """LLM-driven extraction: hand the raw text to the host agent CLI
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
    except BackendError as e:
        return _backend_err(e)
    if result.get("ok") is False and result.get("code") == "feature_disabled":
        return 409, result
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
        except BackendError as e:
            return _backend_err(e)
    return 201, {"created": created, "count": len(created)}


# --- Chat ---------------------------------------------------------------------

def chat_start(body: dict) -> tuple[int, dict]:
    try:
        result = get_client().call(M_CHAT_START, {"gap_id": body.get("gap_id")})
    except BackendError as e:
        return _backend_err(e)
    if result.get("ok") is False and result.get("code") == "feature_disabled":
        return 409, result
    return 201, result


def chat_input(sid: str, body: dict) -> tuple[int, dict]:
    text = body.get("text", "")
    try:
        result = get_client().call(M_CHAT_INPUT, {"session_id": sid, "text": text})
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def chat_read(sid: str) -> tuple[int, dict]:
    try:
        result = get_client().call(M_CHAT_READ, {"session_id": sid})
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def chat_stop(sid: str) -> tuple[int, dict]:
    try:
        result = get_client().call(M_CHAT_STOP, {"session_id": sid})
    except BackendError as e:
        return _backend_err(e)
    return 200, result


# --- Target application -------------------------------------------------------
#
# The operator writes plain-language start/stop prompts in Settings (or
# generates them via /api/target-app/generate-instructions). Clicking the
# nav toggle hits /start or /stop, which routes through the runner to a
# Standalone agent. State transitions are recorded in SQLite settings so
# every browser tab sees the same status.

_TARGET_APP_STATES = (
    "unknown", "starting", "running", "degraded",
    "stopping", "stopped", "failed",
)


def target_app_status() -> tuple[int, dict]:
    """Return the current target-app state + last health-check snapshot."""
    conn = _conn()
    try:
        snap = _target_app_snapshot(conn)
    finally:
        conn.close()
    return 200, snap


def _target_app_snapshot(conn: sqlite3.Connection) -> dict:
    state = db.get_setting(conn, "target_app_state") or "unknown"
    settings = db.list_settings(conn)
    cfg = _target_app_config(settings)
    last_op = conn.execute(
        "SELECT id, kind, state, started_at, finished_at, exit_code, message "
        "FROM target_app_operations ORDER BY id DESC LIMIT 1"
    ).fetchone()
    legacy_start = (settings.get("target_app_start_instructions") or "").strip()
    legacy_stop = (settings.get("target_app_stop_instructions") or "").strip()
    return {
        "state": state if state in _TARGET_APP_STATES else "unknown",
        "health_url": cfg.get("http_check_url") or "",
        "has_start_command": bool(cfg.get("start_command")),
        "has_stop_command": bool(cfg.get("stop_command")),
        "has_status_checks": _has_status_checks(cfg),
        # Back-compat names for older JS during upgrades.
        "has_start_instructions": bool(cfg.get("start_command") or legacy_start),
        "has_stop_instructions": bool(cfg.get("stop_command") or legacy_stop),
        "last_check_at": settings.get("target_app_last_check_at") or "",
        "last_check_ok": (settings.get("target_app_last_check_ok") or "0") == "1",
        "last_check_message": settings.get("target_app_last_check_message") or "",
        "last_health_at": settings.get("target_app_last_check_at") or settings.get("target_app_last_health_at") or "",
        "last_health_ok": (settings.get("target_app_last_check_ok") or settings.get("target_app_last_health_ok") or "0") == "1",
        "last_health_message": settings.get("target_app_last_check_message") or settings.get("target_app_last_health_message") or "",
        "last_error": settings.get("target_app_last_error") or "",
        "last_operation_id": settings.get("target_app_last_operation_id") or "",
        "last_operation": dict(last_op) if last_op else None,
        "legacy_config_present": bool(legacy_start or legacy_stop or (settings.get("target_app_health_url") or "").strip()),
    }


def _target_app_config(settings: dict[str, str]) -> dict[str, Any]:
    from refine_server import target_app as target_app_runtime
    return target_app_runtime.config_from_settings(settings)


def _has_status_checks(cfg: dict[str, Any]) -> bool:
    return any((
        (cfg.get("status_command") or "").strip(),
        (cfg.get("http_check_url") or "").strip(),
        (cfg.get("tcp_check_host") or "").strip() and (cfg.get("tcp_check_port") or "").strip(),
        (cfg.get("process_check_command") or "").strip(),
    ))


def target_app_start(_body: dict | None = None) -> tuple[int, dict]:
    """Run the configured start command via the host runner."""
    return _target_app_run("start")


def target_app_stop(_body: dict | None = None) -> tuple[int, dict]:
    """Run the configured stop command via the host runner."""
    return _target_app_run("stop")


def _target_app_run(kind: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        settings = db.list_settings(conn)
        cfg = _target_app_config(settings)
        command = (cfg.get(f"{kind}_command") or "").strip()
        if not command:
            return err(400,
                f"No {kind} command configured. "
                f"Generate or enter target-app configuration in Settings first.")
        # Optimistic transition. The command run is synchronous but may be long;
        # SSE listeners see the in-flight state via /api/target-app/status.
        next_pending = "starting" if kind == "start" else "stopping"
        db.set_setting(conn, "target_app_state", next_pending)
        db.set_setting(conn, "target_app_last_error", "")
        activity.append(
            conn,
            message=f"target-app: {kind} requested via UI",
            severity="info", category="target_app", actor="refine",
        )
    finally:
        conn.close()

    try:
        result = get_client().call(
            M_TARGET_APP_RUN,
            {"kind": kind, "config": cfg},
            timeout=900.0,
        )
    except BackendError as e:
        _target_app_record_failure(kind, e.message)
        return _backend_err(e)

    ok = bool(result.get("ok"))
    final_state = result.get("state") or ("running" if kind == "start" else "stopped")
    if final_state not in _TARGET_APP_STATES:
        final_state = "failed" if not ok else "unknown"
    err_msg = "" if ok else (result.get("message") or "target-app operation failed")
    conn = _conn()
    try:
        db.set_setting(conn, "target_app_state", final_state)
        db.set_setting(conn, "target_app_last_error", err_msg)
        op_id = _record_target_app_operation(conn, kind, result, final_state)
        db.set_setting(conn, "target_app_last_operation_id", str(op_id))
        if result.get("checks_configured"):
            _persist_check_settings(conn, result.get("checks") or [], result.get("message") or "")
        snap = _target_app_snapshot(conn)
    finally:
        conn.close()

    status = 200 if ok else 502
    snap.update({
        "ok": ok,
        "state": final_state,
        "message": result.get("message") or "",
        "details": (
            result.get("stderr_tail")
            or result.get("stdout_tail")
            or json.dumps(result.get("checks") or [])
        )[:8000],
    })
    return status, snap


def _record_target_app_operation(conn: sqlite3.Connection, kind: str,
                                 result: dict, state: str) -> int:
    cur = conn.execute(
        "INSERT INTO target_app_operations "
        "(kind, state, started_at, finished_at, command, cwd, exit_code, "
        "message, stdout_tail, stderr_tail, checks_json) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (
            kind, state,
            result.get("started_at") or now_iso(),
            result.get("finished_at") or now_iso(),
            result.get("command") or "",
            result.get("cwd") or "",
            result.get("exit_code"),
            result.get("message") or "",
            result.get("stdout_tail") or "",
            result.get("stderr_tail") or "",
            json.dumps(result.get("checks") or []),
        ),
    )
    return int(cur.lastrowid)


def _persist_check_settings(conn: sqlite3.Connection, checks: list[dict],
                            message: str) -> None:
    ok = bool(checks) and all(bool(c.get("ok")) for c in checks)
    db.set_setting(conn, "target_app_last_check_at", now_iso())
    db.set_setting(conn, "target_app_last_check_ok", "1" if ok else "0")
    db.set_setting(conn, "target_app_last_check_message", message or "")
    # Back-compat mirrors.
    db.set_setting(conn, "target_app_last_health_at", db.get_setting(conn, "target_app_last_check_at") or "")
    db.set_setting(conn, "target_app_last_health_ok", "1" if ok else "0")
    db.set_setting(conn, "target_app_last_health_message", message or "")


def target_app_check(_body: dict | None = None) -> tuple[int, dict]:
    """Force an immediate deterministic status check."""
    quiet = bool((_body or {}).get("quiet"))
    conn = _conn()
    try:
        settings = db.list_settings(conn)
        cfg = _target_app_config(settings)
    finally:
        conn.close()
    try:
        result = get_client().call(
            M_TARGET_APP_RUN,
            {"kind": "status", "config": cfg, "quiet": quiet},
            timeout=60.0,
        )
    except BackendError as e:
        _target_app_record_failure("status", e.message)
        return _backend_err(e)
    if result.get("busy") and quiet:
        conn = _conn()
        try:
            return 200, _target_app_snapshot(conn)
        finally:
            conn.close()
    final_state = result.get("state") if result.get("state") in _TARGET_APP_STATES else "unknown"
    conn = _conn()
    try:
        db.set_setting(conn, "target_app_state", final_state)
        db.set_setting(conn, "target_app_last_error", "" if result.get("ok") else (result.get("message") or "status check failed"))
        if not quiet:
            op_id = _record_target_app_operation(conn, "status", result, final_state)
            db.set_setting(conn, "target_app_last_operation_id", str(op_id))
        _persist_check_settings(conn, result.get("checks") or [], result.get("message") or "")
        snap = _target_app_snapshot(conn)
    finally:
        conn.close()
    snap.update({"ok": bool(result.get("ok")), "probe_message": result.get("message") or ""})
    return 200, snap


def target_app_health(_body: dict | None = None) -> tuple[int, dict]:
    """Back-compatible route name for a target-app status check."""
    return target_app_check(_body)


def _target_app_run_health_check() -> dict:
    """Back-compatible poller hook for deterministic target-app status."""
    status, snap = target_app_check({"quiet": True})
    return snap if status == 200 else {"state": "unknown", "last_check_ok": False}


def _target_app_record_failure(kind: str, message: str) -> None:
    conn = _conn()
    try:
        rollback = "stopped" if kind == "start" else (
            "running" if kind == "stop" else "unknown"
        )
        db.set_setting(conn, "target_app_state", rollback)
        db.set_setting(conn, "target_app_last_error", message)
        activity.append(
            conn,
            message=f"target-app: {kind} failed — {message}",
            severity="error", category="target_app", actor="refine",
        )
    finally:
        conn.close()


def target_app_generate(body: dict) -> tuple[int, dict]:
    """Use the agent to draft structured target-app config for this codebase."""
    kind = (body.get("kind") or "all").strip().lower()
    if kind not in ("all", "start", "stop", "status"):
        return err(400, "kind must be 'all', 'start', 'stop', or 'status'")
    try:
        result = get_client().call(
            M_TARGET_APP_GENERATE, {"kind": kind}, timeout=600.0,
        )
    except BackendError as e:
        return _backend_err(e)
    if not result.get("ok"):
        return 502, {"error": {"message": result.get("message") or "generation failed"}}
    return 200, {
        "ok": True,
        "config": result.get("config") or {},
        "notes": (result.get("config") or {}).get("notes") or "",
        "raw": result.get("raw") or "",
    }


# --- helpers ------------------------------------------------------------------

def _backend_err(e: BackendError) -> tuple[int, dict]:
    code = 502 if e.code == "backend_unavailable" else 500
    return code, {"error": {"code": e.code, "message": e.message,
                            "details": e.details}}
