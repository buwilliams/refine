"""JSON API endpoint handlers.

Returns (status_code, body_dict) tuples. The server module wraps these.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import sqlite3
import subprocess
from functools import wraps
from pathlib import Path
from typing import Any, Callable
from urllib.parse import urlparse

from refine_server import activity, chat_ops, cluster, cluster_ops, config, dashboard_ops, db, diagnostics_ops, feature_ops, file_ops, gap_ops, gap_writer, gaps as shared_gaps, import_ops, node_ops, observability_ops, process_ops, project_apps, project_config_ops, project_registry, project_state, project_sync as project_sync_mod, quality, reporter_ops, reporters, round_logs, search_index, settings_ops, target_app_ops, upgrade
from refine_server import perf_metrics
from refine_server.gaps import now_iso
from refine_server.backend_protocol import (
    M_APPEND_ROUND, M_BACKGROUND_PROCESSES_SET, M_CANCEL, M_CANCEL_ALL, M_CHAT_INPUT, M_CHAT_READ, M_CHAT_START,
    M_CHAT_RESET_ALL, M_CHAT_STOP, M_CREATE_GAP, M_DELETE_GAP, M_DIAGNOSTICS, M_EDIT_ROUND,
    M_BULK_DELETE_GAPS, M_BULK_UPDATE_GAPS, M_ENFORCE_SCHEDULING, M_EXTRACT_GAPS, M_LAUNCH, M_LIST_CHANGES, M_LOG_APPEND, M_PREFLIGHT,
    M_HARD_RESET_WORKTREE, M_PROJECT_SYNC,
    M_RETRY_MERGE, M_RETRY_QA, M_SET_NOTES,
    M_TARGET_APP_REBUILD_PENDING, M_UNDO_GAP, M_VERIFY,
)
from refine_server.ulid import new_ulid
from .backend_client import BackendError, get_client
from . import background_jobs, runtime, system_events


# --- error helpers ------------------------------------------------------------

def err(
    code: int,
    message: str,
    details: str | None = None,
    *,
    error_code: str | None = None,
) -> tuple[int, dict]:
    body: dict[str, Any] = {"error": {"message": message}}
    if details is not None:
        body["error"]["details"] = details
    if error_code is not None:
        body["error"]["code"] = error_code
    return code, body


IMPORT_BACKGROUND_THRESHOLD = 100
BULK_UPDATE_BACKGROUND_THRESHOLD = 100
FILE_PREVIEW_MAX_BYTES = file_ops.FILE_PREVIEW_MAX_BYTES
FILE_TEXT_CHUNK_BYTES = file_ops.FILE_TEXT_CHUNK_BYTES
IMAGE_PREVIEW_MAX_BYTES = file_ops.IMAGE_PREVIEW_MAX_BYTES
FILES_TREE_MAX_DEPTH = file_ops.FILES_TREE_MAX_DEPTH
FILES_TREE_MAX_ENTRIES = file_ops.FILES_TREE_MAX_ENTRIES
FILES_SEARCH_MAX_SCAN = file_ops.FILES_SEARCH_MAX_SCAN
FILE_BROWSER_IGNORE_DEFAULT = file_ops.FILE_BROWSER_IGNORE_DEFAULT
FILE_BROWSER_ALWAYS_IGNORE = file_ops.FILE_BROWSER_ALWAYS_IGNORE
IMAGE_MIME_BY_EXT = file_ops.IMAGE_MIME_BY_EXT


def _conn(*, ensure_cache: bool = True) -> sqlite3.Connection:
    conn = db.connect()
    if ensure_cache:
        project_state.ensure_sqlite_cache_current(conn)
    return conn


def _project_attached() -> bool:
    port = _current_port()
    if config.find_config(port=port) is None:
        return False
    try:
        config.get(reload=True, port=port)
    except config.ConfigError:
        return False
    return True


def _empty_page(limit: int, offset: int) -> dict[str, Any]:
    return gap_ops.empty_page(limit, offset)


def _schema_block_response(*, block_maintenance: bool = True) -> tuple[int, dict] | None:
    try:
        cfg = config.get(reload=True, port=_current_port())
    except config.ConfigError:
        return None
    maintenance = project_state.read_maintenance(root=cfg.volume_root)
    if block_maintenance and maintenance is not None:
        return err(
            409,
            "Project maintenance is active.",
            str(maintenance.get("operator_instructions")
                or maintenance.get("reason")
                or "Refine writes are paused while maintenance is active."),
        )
    schema = project_state.schema_status(cfg.volume_root)
    if schema.get("compatible"):
        return None
    if schema.get("migration_required"):
        details = (
            project_state.migration_block_details(schema)
            if project_state.migration_requires_manual(schema)
            else "Open this app from the browser and choose Migrate and open."
        )
        return err(
            409,
            project_state.migration_block_message(schema),
            details,
        )
    return err(
        409,
        "Project schema is not supported by this Refine version.",
        schema.get("reason") or "",
    )


def _background_processes_stopped() -> bool:
    try:
        conn = _conn()
    except (sqlite3.Error, config.ConfigError):
        return False
    try:
        return (db.get_setting(conn, "paused") or "0") == "1"
    finally:
        conn.close()


def _agents_paused(conn: sqlite3.Connection | None = None) -> bool:
    close_conn = False
    if conn is None:
        try:
            conn = _conn()
        except (sqlite3.Error, config.ConfigError):
            return False
        close_conn = True
    try:
        return (
            (db.get_setting(conn, "paused") or "0") == "1"
            or (db.get_setting(conn, "agents_paused") or "0") == "1"
        )
    finally:
        if close_conn:
            conn.close()


def _background_processes_stopped_response() -> tuple[int, dict] | None:
    if not _background_processes_stopped():
        return None
    return err(
        409,
        "Background processes are stopped.",
        "Start Background before running worker actions.",
        error_code="background_processes_stopped",
    )


def _background_job_conflict_response(
    conflict: background_jobs.BackgroundJobConflict,
) -> tuple[int, dict]:
    job = conflict.job
    return err(
        409,
        "A background job is already running.",
        details=f"{job.get('label') or job.get('kind')} ({job.get('status')})",
        error_code="background_job_active",
    )


def _exclusive_mutation(
    label: str,
    *,
    allow_active_kinds: set[str] | None = None,
    allow_busy_when: Callable[[dict[str, Any]], bool] | None = None,
    block_maintenance: bool = True,
) -> Callable:
    def decorator(fn: Callable) -> Callable:
        @wraps(fn)
        def wrapped(*args, **kwargs):
            blocked = _schema_block_response(block_maintenance=block_maintenance)
            if blocked is not None:
                return blocked
            try:
                with background_jobs.exclusive_operation(
                    label,
                    allow_active_kinds=allow_active_kinds,
                ):
                    return fn(*args, **kwargs)
            except background_jobs.BackgroundJobConflict as e:
                if allow_busy_when is not None and allow_busy_when(e.job):
                    return fn(*args, **kwargs)
                return _background_job_conflict_response(e)
        return wrapped
    return decorator


def _system_operation(label: str) -> Callable:
    def decorator(fn: Callable) -> Callable:
        @wraps(fn)
        def wrapped(*args, **kwargs):
            system_events.publish(f"{label} started", status="start", category="operation")
            try:
                code, body = fn(*args, **kwargs)
            except Exception as e:
                system_events.publish(
                    f"{label} failed: {e}",
                    status="error",
                    category="operation",
                )
                raise
            if int(code) >= 400:
                error = body.get("error") if isinstance(body, dict) else {}
                message = error.get("message") if isinstance(error, dict) else ""
                detail = f": {message}" if message else ""
                system_events.publish(
                    f"{label} failed{detail}",
                    status="error",
                    category="operation",
                    http_status=code,
                )
            elif isinstance(body, dict) and body.get("queued"):
                system_events.publish(
                    f"{label} queued",
                    status="queued",
                    category="operation",
                    http_status=code,
                )
            else:
                system_events.publish(
                    f"{label} completed",
                    status="complete",
                    category="operation",
                    http_status=code,
                )
            return code, body
        return wrapped
    return decorator


# --- Files --------------------------------------------------------------------

def _target_repo_root() -> Path:
    return config.get(reload=True, port=_current_port()).client_repo.resolve()


def files_tree(
    path: str | None = None,
    *,
    recursive: bool = False,
    max_depth: int = FILES_TREE_MAX_DEPTH,
    max_entries: int = FILES_TREE_MAX_ENTRIES,
) -> tuple[int, dict]:
    conn = _conn(ensure_cache=False)
    try:
        return file_ops.tree(
            _target_repo_root(),
            path,
            recursive=recursive,
            max_depth=max_depth,
            max_entries=max_entries,
            ignore_patterns=file_ops.file_browser_ignore_patterns(conn),
        )
    finally:
        conn.close()


def files_search(
    query: str | None = None,
    *,
    max_entries: int = FILES_TREE_MAX_ENTRIES,
) -> tuple[int, dict]:
    conn = _conn(ensure_cache=False)
    try:
        return file_ops.search(
            _target_repo_root(),
            query,
            max_entries=max_entries,
            ignore_patterns=file_ops.file_browser_ignore_patterns(conn),
        )
    finally:
        conn.close()


def files_read(
    path: str | None = None,
    *,
    offset: int = 0,
    limit: int = FILE_TEXT_CHUNK_BYTES,
) -> tuple[int, dict]:
    return file_ops.read(_target_repo_root(), path, offset=offset, limit=limit)


def _node_owner(node_id: str | None) -> str:
    return str(node_id or project_state.DEFAULT_NODE_ID)


def _ownership_error(
    owner_id: str | None,
    *,
    active_id: str | None = None,
    count: int = 1,
) -> tuple[int, dict]:
    owner = _node_owner(owner_id)
    active = active_id or project_state.active_node_id()
    owner_name = project_state.gap_node_display(owner)
    active_name = project_state.gap_node_display(active)
    subject = "Gap is" if count == 1 else f"{count} Gaps are"
    return err(
        409,
        (
            f"Action not allowed: {subject} owned by another node "
            f"({owner_name}). Transfer to {active_name} before making changes."
        ),
        error_code="node_ownership",
    )


def _require_active_gap(
    conn: sqlite3.Connection,
    gap_id: str,
    *,
    columns: str = "status, branch_name, node_id",
) -> tuple[sqlite3.Row | None, tuple[int, dict] | None]:
    row = conn.execute(
        f"SELECT {columns} FROM gaps_index WHERE id = ?",
        (gap_id,),
    ).fetchone()
    if not row:
        return None, err(404, "Gap not found")
    active = project_state.active_node_id()
    if _node_owner(row["node_id"]) != active:
        return None, _ownership_error(row["node_id"], active_id=active)
    return row, None


def _require_active_gap_ids(gap_ids: list[str]) -> tuple[bool, tuple[int, dict] | None]:
    if not gap_ids:
        return True, None
    conn = _conn()
    try:
        return gap_ops.require_active_gap_ids(conn, gap_ids)
    finally:
        conn.close()


def _id_chunks(values: list[str], size: int = 500) -> list[list[str]]:
    return gap_ops.id_chunks(values, size=size)


def _selected_gap_ids(body: dict[str, Any]) -> list[str] | None:
    return gap_ops.selected_gap_ids(body)


def _append_gap_workflow_log(
    gap_id: str,
    message: str,
    *,
    severity: str = "info",
    actor: str = "refine",
    details: str | None = None,
) -> None:
    try:
        gap_writer.append_latest_round_log(
            gap_id=gap_id,
            severity=severity,
            category="state",
            actor=actor,
            message=message,
            details=details,
        )
    except Exception:
        pass


# --- Project attach/setup -----------------------------------------------------

def _current_port() -> int:
    return config.runtime_port()


def project_status() -> tuple[int, dict]:
    """Return whether this UI process is attached to a refine project."""
    clone_dir = Path.cwd().resolve()
    port = _current_port()
    payload = project_apps.status(clone_dir, port=port, include_nodes=True)
    if payload.get("attached"):
        payload["scaffold_required"] = _project_needs_scaffold_template(
            Path(str(payload.get("client_repo") or "")),
        )
    return 200, payload


def project_list() -> tuple[int, dict]:
    clone_dir = Path.cwd().resolve()
    port = _current_port()
    return 200, project_apps.list_apps(clone_dir, port=port)


@_system_operation("Remove app")
def project_remove(body: dict[str, Any]) -> tuple[int, dict]:
    clone_dir = Path.cwd().resolve()
    port = _current_port()
    return project_apps.remove_project(
        body,
        clone_dir=clone_dir,
        port=port,
        attach_next=project_attach,
        detach_current=lambda c, t, p: _detach_current_project(c, t, port=p),
        project_status=project_status,
    )


@_system_operation("Sync app")
@_exclusive_mutation("Sync project", block_maintenance=False)
def project_sync(_: dict[str, Any] | None = None) -> tuple[int, dict]:
    block = _schema_block_response(block_maintenance=False)
    if block:
        return block
    try:
        result = get_client().call(M_PROJECT_SYNC, {}, timeout=120.0)
    except BackendError as e:
        return _backend_err(e)
    if not result.get("ok"):
        return err(
            409,
            result.get("message") or "Could not sync latest target-app updates.",
            result.get("details") or "",
        )
    return 200, result


# --- Nodes ---------------------------------------------------------------

def list_nodes() -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    conn = _conn()
    try:
        payload = node_ops.list_with_counts(conn)
    finally:
        conn.close()
    return 200, payload


@_exclusive_mutation("Create node")
def create_node(body: dict[str, Any]) -> tuple[int, dict]:
    name = (body.get("display_name") or body.get("name") or "").strip()
    try:
        entry = node_ops.create(name)
    except ValueError as e:
        return err(400, str(e))
    sync = _sync_refine_state_after_mutation("refine: create node")
    if not sync.get("ok"):
        return err(409, "Could not sync Refine node state.", sync.get("details") or sync.get("message"))
    return 201, {"node": entry, "sync": sync, **_node_summary()}


@_exclusive_mutation("Update node")
def update_node(node_id: str, body: dict[str, Any]) -> tuple[int, dict]:
    try:
        entry = node_ops.update(
            node_id,
            display_name=body.get("display_name") if "display_name" in body else body.get("name"),
            archived=body.get("archived") if "archived" in body else None,
        )
    except ValueError as e:
        return err(400, str(e))
    sync = _sync_refine_state_after_mutation("refine: update node")
    if not sync.get("ok"):
        return err(409, "Could not sync Refine node state.", sync.get("details") or sync.get("message"))
    return 200, {"node": entry, "sync": sync, **_node_summary()}


@_exclusive_mutation("Copy node settings")
def copy_node_settings(body: dict[str, Any]) -> tuple[int, dict]:
    source = (body.get("source_node_id") or body.get("node_id") or "").strip()
    section = (body.get("section") or "").strip()
    try:
        result = node_ops.copy_settings(source, section)
    except ValueError as e:
        return err(400, str(e))
    sync = _sync_refine_state_after_mutation("refine: copy node settings")
    if not sync.get("ok"):
        return err(409, "Could not sync Refine node settings.", sync.get("details") or sync.get("message"))
    conn = _conn()
    try:
        settings = node_ops.record_settings_copy(conn, source_node_id=source, section=section)
    finally:
        conn.close()
    return 200, {"ok": True, "settings": settings, "sync": sync, **result}


@_exclusive_mutation("Activate node")
def activate_node(body: dict[str, Any]) -> tuple[int, dict]:
    node_id = (body.get("node_id") or body.get("id") or "").strip()
    try:
        payload = node_ops.activate(node_id)
    except ValueError as e:
        return err(400, str(e))
    try:
        runtime.refresh_local_node(payload.get("active_node_id") or node_id)
        runtime.stop_runner()
    except Exception:
        pass
    try:
        get_client().call(
            M_CHAT_RESET_ALL,
            {"reason": "node activated"},
            timeout=10.0,
        )
    except BackendError:
        pass
    try:
        get_client().call(M_ENFORCE_SCHEDULING, {}, timeout=10.0)
    except BackendError:
        pass
    return 200, payload


@_system_operation("Transfer Gaps between nodes")
@_exclusive_mutation("Transfer Gaps between nodes")
def transfer_node_gaps(body: dict[str, Any]) -> tuple[int, dict]:
    target = (body.get("target_node_id") or "").strip()
    source = (body.get("source_node_id") or "").strip() or None
    if not target:
        return err(400, "target_node_id is required")
    target_node = project_state.node_by_id(target)
    if target_node is None:
        return err(400, f"unknown target node: {target}")
    if target_node.get("archived"):
        return err(400, f"archived target node: {target}")
    statuses = body.get("statuses")
    allowed = None
    if statuses is not None:
        if not isinstance(statuses, list):
            return err(400, "statuses must be a list")
        allowed = {str(s) for s in statuses if str(s) in _VALID_STATUSES}
    selected_ids = _selected_gap_ids(body)
    gap_ids = set(selected_ids) if selected_ids is not None else None
    if selected_ids == []:
        return 200, {
            "updated": 0,
            "ids": [],
            "skipped": 0,
            "skipped_details": [],
        }
    filt = body.get("filter")
    if gap_ids is None and isinstance(filt, dict):
        excluded = set(body.get("exclude_ids") or [])
        code, listing = list_gaps(
            status=filt.get("status") or None,
            q=filt.get("q") or None,
            severity=filt.get("severity") or None,
            category=filt.get("category") or None,
            actor=filt.get("actor") or None,
            reporter=filt.get("reporter") or None,
            rounds_gte=filt.get("rounds_gte"),
            rounds_lte=filt.get("rounds_lte"),
            node=filt.get("node") or None,
            limit=10_000,
        )
        if code != 200:
            return code, listing
        gap_ids = {
            g["id"] for g in (listing.get("gaps") or [])
            if g["id"] not in excluded
        }
        if not gap_ids:
            return 200, {
                "updated": 0,
                "ids": [],
                "skipped": 0,
                "skipped_details": [],
            }
    try:
        cancelled = (
            _cancel_active_transfer_gaps(source, gap_ids)
            if body.get("cancel_active")
            else {
                "paused": False,
                "stopped_processes": 0,
                "cancelled": 0,
                "cancelled_ids": [],
            }
        )
    except BackendError as e:
        return _backend_err(e)
    try:
        result = node_ops.transfer_gaps(
            source, target, statuses=allowed, gap_ids=gap_ids,
        )
    except ValueError as e:
        return err(400, str(e))
    sync = _sync_refine_state_after_mutation("refine: transfer node gaps")
    if not sync.get("ok"):
        return err(409, "Could not sync transferred Gaps.", sync.get("details") or sync.get("message"))
    if _should_enforce_after_node_transfer(result.get("ids") or []):
        try:
            get_client().call(M_ENFORCE_SCHEDULING, {}, timeout=10.0)
        except BackendError:
            pass
    result.update(cancelled)
    result["sync"] = sync
    return 200, result


def list_cluster() -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    return cluster_ops.list_cluster()


@_exclusive_mutation("Upsert cluster node")
def upsert_cluster_node(body: dict[str, Any]) -> tuple[int, dict]:
    return cluster_ops.upsert_node(body, _sync_refine_state_after_mutation)


@_exclusive_mutation("Update cluster node")
def update_cluster_node(node_id: str, body: dict[str, Any]) -> tuple[int, dict]:
    return cluster_ops.update_node(node_id, body, _sync_refine_state_after_mutation)


def run_cluster_node(node_id: str, body: dict[str, Any]) -> tuple[int, dict]:
    return cluster_ops.run_node(node_id, body)


@_exclusive_mutation("Bootstrap cluster node")
def bootstrap_cluster_node(node_id: str, _body: dict[str, Any] | None = None) -> tuple[int, dict]:
    return cluster_ops.bootstrap_node(node_id, _sync_refine_state_after_mutation)


def _should_enforce_after_node_transfer(gap_ids: list[str]) -> bool:
    if not gap_ids:
        return False
    active = project_state.active_node_id()
    placeholders = ",".join("?" * len(gap_ids))
    conn = _conn()
    try:
        if _agents_paused(conn):
            return False
        row = conn.execute(
            "SELECT COUNT(*) AS n FROM gaps_index "
            f"WHERE id IN ({placeholders}) "
            "AND node_id = ? AND status = 'todo'",
            [*gap_ids, active],
        ).fetchone()
        return bool(row and int(row["n"] or 0) > 0)
    finally:
        conn.close()


def list_guidance() -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    return 200, project_config_ops.list_guidance()


def update_guidance(body: dict[str, Any]) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    try:
        return 200, project_config_ops.update_guidance(body.get("guidance"))
    except ValueError as e:
        return err(400, str(e))


def _cancel_active_transfer_gaps(
    source_node_id: str | None,
    gap_ids: set[str] | None,
) -> dict[str, Any]:
    conn = _conn()
    try:
        project_state.set_setting("agents_paused", "1")
        db.set_setting(conn, "agents_paused", "1")
        where = ["status IN ('in-progress', 'qa', 'ready-merge', 'awaiting-rebuild')"]
        args: list[Any] = []
        if source_node_id:
            where.append("node_id = ?")
            args.append(source_node_id)
        if gap_ids is not None:
            if not gap_ids:
                return {
                    "paused": True,
                    "stopped_processes": 0,
                    "cancelled": 0,
                    "cancelled_ids": [],
                }
            where.append("id IN (" + ",".join("?" * len(gap_ids)) + ")")
            args.extend(sorted(gap_ids))
        rows = conn.execute(
            "SELECT id FROM gaps_index WHERE " + " AND ".join(where),
            args,
        ).fetchall()
        active_ids = [r["id"] for r in rows]
        activity.append(
            conn,
            message="Agents paused for node transfer",
            severity="warn",
            category="state",
            actor="refine",
        )
    finally:
        conn.close()

    stopped = 0
    result = get_client().call(M_CANCEL_ALL, {"reason": "paused"}, timeout=10.0)
    stopped = int(result.get("killed_subprocesses") or 0)
    cancelled_ids: list[str] = []
    for gid in active_ids:
        get_client().call(M_CANCEL, {"gap_id": gid}, timeout=30.0)
        cancelled_ids.append(gid)
    return {
        "paused": True,
        "stopped_processes": stopped,
        "cancelled": len(cancelled_ids),
        "cancelled_ids": cancelled_ids,
    }


def _sqlite_cache_files(sqlite_file: Path) -> list[Path]:
    return observability_ops.sqlite_cache_files(sqlite_file)


def _unlink_sqlite_cache_files(sqlite_file: Path) -> list[str]:
    return observability_ops.unlink_sqlite_cache_files(sqlite_file)


def _sqlite_cache_counts(conn: sqlite3.Connection) -> dict[str, int]:
    return observability_ops.sqlite_cache_counts(conn)


def background_job(job_id: str) -> tuple[int, dict]:
    try:
        return 200, observability_ops.background_job(job_id, background_jobs.snapshot)
    except LookupError:
        return err(404, "Background job not found")


@_system_operation("Cancel background job")
def cancel_background_job(job_id: str) -> tuple[int, dict]:
    try:
        return 200, observability_ops.cancel_background_job(job_id, background_jobs.cancel)
    except LookupError:
        return err(404, "Background job not found")


def _cancel_active_background_jobs() -> list[dict[str, Any]]:
    try:
        conn = _conn()
        try:
            rows = conn.execute(
                "SELECT id FROM background_jobs "
                "WHERE status IN ('queued', 'running') "
                "ORDER BY started_at DESC LIMIT 100",
            ).fetchall()
        finally:
            conn.close()
    except Exception:
        rows = []
    cancelled: list[dict[str, Any]] = []
    seen: set[str] = set()
    for row in rows:
        job_id = str(row["id"])
        if job_id in seen:
            continue
        seen.add(job_id)
        job = background_jobs.cancel(job_id)
        if job:
            cancelled.append(job)
    return cancelled


def performance_summary(*, operation: str | None = None,
                        success: str | None = None,
                        limit: int = 50,
                        offset: int = 0) -> tuple[int, dict]:
    conn = _conn()
    try:
        return 200, observability_ops.performance_summary(
            conn,
            operation=operation or None,
            success=success,
            limit=limit,
            offset=offset,
            backend=runtime.backend_info(),
        )
    finally:
        conn.close()


@_system_operation("Clean up performance data")
def performance_cleanup(body: dict | None = None) -> tuple[int, dict]:
    body = body or {}
    conn = _conn()
    try:
        return 200, observability_ops.performance_cleanup(
            conn,
            clear=bool(body.get("clear")),
        )
    finally:
        conn.close()


@_system_operation("Rebuild SQLite cache")
@_exclusive_mutation("Rebuild SQLite cache")
def rebuild_sqlite_cache(body: dict | None = None) -> tuple[int, dict]:
    """Operator recovery path for a stale or corrupted SQLite cache."""
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped

    body = body or {}
    if body.get("background"):
        restart_services = body.get("restart_services") is not False

        def run_job(progress=None) -> dict[str, Any]:
            def report(completed: int, total: int, message: str) -> None:
                if progress is not None:
                    progress(completed=completed, total=total, message=message)

            status, result = _rebuild_sqlite_cache_sync(
                {"restart_services": restart_services},
                progress=report,
            )
            return {"http_status": status, **result}

        try:
            job = background_jobs.start(
                "sqlite_cache_rebuild",
                "Rebuild SQLite cache",
                run_job,
            )
        except background_jobs.BackgroundJobConflict as e:
            return _background_job_conflict_response(e)
        return 202, {"queued": True, "job": job}

    return _rebuild_sqlite_cache_sync(body)


def _rebuild_sqlite_cache_sync(
    body: dict,
    *,
    progress: project_state.ProgressCallback | None = None,
) -> tuple[int, dict]:
    restart_services = body.get("restart_services") is not False
    cfg = config.get(reload=True, port=_current_port())
    backend = runtime.backend_info()
    controls_runner_lifecycle = bool(backend.get("ui_controls_runner_lifecycle"))
    return 200, observability_ops.rebuild_sqlite_cache(
        cfg.sqlite_path,
        backend=backend,
        restart_services=restart_services,
        controls_runner_lifecycle=controls_runner_lifecycle,
        progress=progress,
        stop_all=runtime.stop_all,
        stop_poller=runtime.stop_poller,
        ensure_poller=runtime.ensure_poller,
        ensure_runner=runtime.ensure_runner,
    )


@_system_operation("Attach or switch app")
def project_attach(body: dict[str, Any]) -> tuple[int, dict]:
    """Create or attach a target app path and make it active."""
    clone_dir = Path.cwd().resolve()
    port = _current_port()
    try:
        return runtime.attach_project_via_supervisor(body, clone_dir=clone_dir, port=port)
    except config.ConfigError as e:
        if os.environ.get("REFINE_TEST_INPROCESS_BACKEND") == "1":
            return project_apps.attach_project(
                body,
                clone_dir=clone_dir,
                port=port,
                load_configured=_load_project_attach_configured,
                current_client_repo=_current_client_repo,
                loaded_client_repo=_loaded_client_repo,
                prepare_current_project_for_switch=_prepare_current_project_for_switch,
                commit_refine_state=_commit_refine_state,
                node_summary=_node_summary,
            )
        return err(400, str(e))


def _resolve_project_setup_path(
    raw_path: str | None,
    *,
    kind: str = "app",
    remote: str | None = None,
) -> Path:
    clone_dir = Path.cwd().resolve()
    text = str(raw_path or "").strip()
    if kind == "clone" and not text and remote and project_apps.looks_like_git_remote(remote):
        return project_apps.default_project_clone_path(remote, clone_dir).resolve()
    if not text:
        return clone_dir.parent.resolve()
    target = Path(text).expanduser()
    if not target.is_absolute():
        base = clone_dir.parent if kind == "clone" else clone_dir
        target = base / target
    return target.resolve()


def project_setup_path(
    path: str | None = None,
    *,
    kind: str = "app",
    remote: str | None = None,
) -> tuple[int, dict]:
    """Resolve a user-facing app setup path the same way attach will use it."""
    if kind not in {"app", "clone"}:
        return err(400, "unknown project path kind")
    try:
        resolved = _resolve_project_setup_path(path, kind=kind, remote=remote)
    except OSError as e:
        return err(400, "could not resolve project path", str(e))
    return 200, {
        "path": str(resolved),
        "exists": resolved.exists(),
        "is_directory": resolved.is_dir(),
    }


def project_directories(
    path: str | None = None,
    *,
    kind: str = "app",
    remote: str | None = None,
    max_entries: int = 200,
) -> tuple[int, dict]:
    """List host directories for the project attach picker."""
    if kind not in {"app", "clone"}:
        return err(400, "unknown project path kind")
    try:
        selected = _resolve_project_setup_path(path, kind=kind, remote=remote)
    except OSError as e:
        return err(400, "could not resolve project path", str(e))
    browse_root = selected if selected.is_dir() else selected.parent
    while not browse_root.exists() and browse_root != browse_root.parent:
        browse_root = browse_root.parent
    if not browse_root.exists() or not browse_root.is_dir():
        browse_root = Path.cwd().resolve().parent
    max_entries = max(1, min(500, int(max_entries)))
    entries: list[dict[str, Any]] = []
    truncated = False
    try:
        children = sorted(
            browse_root.iterdir(),
            key=lambda item: (not item.is_dir(), item.name.lower()),
        )
        for child in children:
            if not child.is_dir():
                continue
            if len(entries) >= max_entries:
                truncated = True
                break
            entries.append({
                "name": child.name,
                "path": str(child.resolve()),
                "type": "directory",
            })
    except PermissionError as e:
        return err(403, "directory cannot be read", str(e))
    except OSError as e:
        return err(400, "directory cannot be read", str(e))
    return 200, {
        "path": str(browse_root.resolve()),
        "selected_path": str(selected),
        "exists": selected.exists(),
        "is_directory": selected.is_dir(),
        "parent": str(browse_root.parent.resolve()) if browse_root != browse_root.parent else "",
        "entries": entries,
        "truncated": truncated,
        "max_entries": max_entries,
    }


def _load_project_attach_configured(
    config_path: Path,
    start_poller: bool,
    start_runner: bool,
    migrate: bool,
    port: int,
) -> config.Config:
    return runtime.load_configured(
        config_path,
        port=port,
        start_poller=start_poller,
        start_runner=start_runner,
        migrate=migrate,
    )


PROJECT_TEMPLATE_DIR = Path(__file__).resolve().parent / "project_templates"
_PROJECT_TEMPLATE_ID_RE = project_apps.PROJECT_TEMPLATE_ID_RE
_APP_MARKER_FILES = project_apps.APP_MARKER_FILES
_APP_MARKER_DIRS = project_apps.APP_MARKER_DIRS
_APP_CODE_EXTS = project_apps.APP_CODE_EXTS
_APP_SCAN_IGNORED_DIRS = project_apps.APP_SCAN_IGNORED_DIRS


def _project_needs_scaffold_template(client_repo: Path) -> bool:
    return project_apps.project_needs_scaffold_template(client_repo)


def _target_has_existing_application(path: Path) -> bool:
    return project_apps.target_has_existing_application(path)


def _directory_has_visible_files(path: Path) -> bool:
    return project_apps.directory_has_visible_files(path)


def list_project_templates() -> tuple[int, dict]:
    return 200, project_apps.list_project_templates()


def _project_template_summary(template_id: str, content: str) -> dict[str, str]:
    return project_apps.project_template_summary(template_id, content)


def _load_project_template(template_id: str) -> tuple[dict[str, str], str] | None:
    return project_apps.load_project_template(template_id)


@_system_operation("Create scaffold Gap")
@_exclusive_mutation(
    "Create Scaffold Gap",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def create_project_scaffold_gap(body: dict[str, Any]) -> tuple[int, dict]:
    return project_apps.create_project_scaffold_gap(_conn, body)


def _create_indexed_gap(
    *,
    name: str,
    reporter: str,
    actual: str,
    target: str,
    priority: str,
) -> dict[str, Any]:
    return project_apps.create_indexed_gap(
        _conn,
        name=name,
        reporter=reporter,
        actual=actual,
        target=target,
        priority=priority,
    )


def _looks_like_git_remote(value: str) -> bool:
    return project_apps.looks_like_git_remote(value)


def _clone_project_remote(remote: str, clone_dir: Path) -> Path:
    return project_apps.clone_project_remote(remote, clone_dir)


def _default_project_clone_path(remote: str, clone_dir: Path) -> Path:
    return project_apps.default_project_clone_path(remote, clone_dir)


def _validate_target_schema_before_switch(client_repo: Path, body: dict[str, Any]) -> None:
    project_apps.validate_target_schema_before_switch(client_repo, body)


def _record_migration_candidate_app(clone_dir: Path, client_repo: Path, port: int) -> None:
    project_apps.record_migration_candidate_app(clone_dir, client_repo, port)


def _detach_current_project(clone_dir: Path, target: Path, *, port: int | None = None) -> None:
    project_registry.detach_port(clone_dir, port=port)
    runtime.detach_configured()


def _node_summary() -> dict[str, Any]:
    try:
        return node_ops.summary()
    except Exception:
        return {"nodes": [], "active_node_id": ""}


def _sync_refine_state_after_mutation(message: str) -> dict:
    conn = _conn(ensure_cache=False)
    try:
        result = project_sync_mod.commit_and_push_refine_state(
            conn,
            actor="refine",
            state_message=message,
            rebuild_cache=True,
        )
    finally:
        conn.close()
    return result


class _SwitchBlocked(project_apps.SwitchBlocked):
    pass


def _current_client_repo() -> Path | None:
    try:
        return config.get(reload=True, port=_current_port()).client_repo
    except config.ConfigError:
        return None


def _loaded_client_repo() -> Path | None:
    loaded_path = getattr(runtime, "_loaded_config_path", None)
    if loaded_path is None:
        return None
    try:
        return config.Config.load(loaded_path).client_repo
    except (OSError, config.ConfigError):
        return None


def _prepare_current_project_for_switch(current_repo: Path | None = None) -> dict[str, Any]:
    """Stop active agents and leave the current target app clean before switching."""
    warnings: list[str] = []
    if current_repo is not None:
        cfg = config.Config.load(current_repo / ".refine" / config.CONFIG_FILENAME)
    else:
        cfg = config.get(reload=True, port=_current_port())
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
    from refine_server import project_sync

    config.ensure_refine_gitignore(repo / ".refine")
    dirty_refine = _git_stdout(repo, ["status", "--porcelain", "--", ".refine"])
    if not dirty_refine.strip():
        return
    repo_cfg = config.Config.load(repo / ".refine" / config.CONFIG_FILENAME)
    db.init_db(repo_cfg.sqlite_path)
    conn = db.connect(repo_cfg.sqlite_path)
    try:
        result = project_sync.commit_and_push_refine_state(
            conn,
            actor="refine",
            cwd=repo,
            state_message="refine: sync project state before switch",
        )
    finally:
        conn.close()
    if result.get("ok"):
        return
    raise _SwitchBlocked(
        "Could not commit current app Refine state.",
        str(result.get("details") or result.get("message") or "git commit failed").strip(),
    )


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

_VALID_PRIORITIES = gap_ops.VALID_PRIORITIES
_VALID_STATUSES = gap_ops.VALID_STATUSES
_USER_STATUS_TRANSITIONS = gap_ops.USER_STATUS_TRANSITIONS
_BULK_STATUS_AUTOMATED_VALUES = gap_ops.BULK_STATUS_AUTOMATED_VALUES
_BULK_STATUS_VALUES = gap_ops.BULK_STATUS_VALUES
_BULK_STATUS_SOURCE_VALUES = gap_ops.BULK_STATUS_SOURCE_VALUES
_BULK_LAST_WORKFLOW_STATUS = gap_ops.BULK_LAST_WORKFLOW_STATUS
_DUPLICATE_DECISION_IGNORE = "duplicate"
_DUPLICATE_DECISION_IMPORT = "original"
_DUPLICATE_DECISION_MOVE_ORIGINAL = "move_original_to_backlog"
_DUPLICATE_UPDATE_PREFIX = "update_original_"
_DUPLICATE_UPDATE_FIELDS = {"actual", "target", "reporter", "priority"}
_DUPLICATE_BACKLOG_PROTECTED_STATUSES = {
    "todo",
    "in-progress",
    "qa",
    "ready-merge",
    "awaiting-rebuild",
    "awaiting-review",
}

# Map a public sort key to a SQL expression. Whitelisted to prevent SQL
# injection from the query string. `id` doubles as a chronological sort
# because we mint Gap ids as ULIDs.
_GAPS_SORT_EXPRESSIONS: dict[str, str] = {
    "name":     "name COLLATE NOCASE",
    "status":   "status",
    "priority": "CASE priority WHEN 'high' THEN 0 WHEN 'medium' THEN 1 ELSE 2 END",
    "reporter": "reporter COLLATE NOCASE",
    "node": "node_id COLLATE NOCASE",
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
    "node": "ASC",
    "updated":  "DESC",
    "created":  "DESC",
    "id":       "DESC",
}


def _gaps_order_clause(sort: str | None, direction: str | None) -> str:
    return gap_ops.gaps_order_clause(sort, direction)


def _validate_user_status_transition(previous: str | None,
                                     next_status: str) -> tuple[int, dict] | None:
    if previous == next_status:
        return None
    allowed = _USER_STATUS_TRANSITIONS.get(previous or "", set())
    if next_status in allowed:
        return None
    return err(
        409,
        (
            f"Invalid workflow transition: {previous or 'unknown'} → {next_status}. "
            "Use the dedicated workflow action for system-owned states."
        ),
    )


def _page_bounds(limit: int, offset: int = 0) -> tuple[int, int]:
    return gap_ops.page_bounds(limit, offset)


def list_gaps(*, status: str | None = None, q: str | None = None,
              severity: str | None = None,
              category: str | None = None,
              actor: str | None = None,
              reporter: str | None = None,
              feature: str | None = None,
              rounds_gte: object | None = None,
              rounds_lte: object | None = None,
              node: str | None = None,
              limit: int = 50,
              offset: int = 0,
              sort: str | None = None,
              direction: str | None = None,
              include_facets: bool = False) -> tuple[int, dict]:
    """List Gaps. `severity` / `category` / `actor` filter to Gaps that
    have at least one activity entry matching. `reporter` filters by
    the indexed `gaps_index.reporter` column, which the runner keeps in
    sync with the latest round's reporter on every write.
    """
    attached = _project_attached()
    if not attached:
        return gap_ops.list_gaps(
            attached=False,
            limit=limit,
            offset=offset,
            include_facets=include_facets,
        )
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    return gap_ops.list_gaps(
        attached=True,
        status=status,
        q=q,
        severity=severity,
        category=category,
        actor=actor,
        reporter=reporter,
        feature=feature,
        rounds_gte=rounds_gte,
        rounds_lte=rounds_lte,
        node=node,
        limit=limit,
        offset=offset,
        sort=sort,
        direction=direction,
        include_facets=include_facets,
    )


def _select_bulk_update_candidates(
    filt: dict[str, Any],
    excluded: set[str],
    *,
    skip_automated: bool,
    selected_ids: list[str] | None = None,
) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    conn = _conn()
    try:
        return gap_ops.select_bulk_update_candidates(
            conn,
            filt,
            excluded,
            skip_automated=skip_automated,
            selected_ids=selected_ids,
        )
    finally:
        conn.close()


def _filter_bulk_candidate_rows(
    rows: list[dict[str, Any]],
    *,
    skip_automated: bool,
) -> tuple[int, dict]:
    return gap_ops.filter_bulk_candidate_rows(rows, skip_automated=skip_automated)


def get_gap(gap_id: str) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    return gap_ops.get_gap(gap_id)


def _compact_log(log: dict[str, Any] | None) -> dict[str, Any] | None:
    return gap_ops.compact_log(log)


def _round_metadata(round_obj: dict[str, Any]) -> dict[str, Any]:
    return gap_ops.round_metadata(round_obj)


def get_gap_logs(
    gap_id: str,
    *,
    round_idx: int,
    limit: int = 50,
    offset: int = 0,
) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    return gap_ops.get_gap_logs(
        gap_id,
        round_idx=round_idx,
        limit=limit,
        offset=offset,
    )


# --- Feature endpoints --------------------------------------------------------

def list_features(*, status: str | None = None, q: str | None = None,
                  reporter: str | None = None, node: str | None = None,
                  limit: int = 50, offset: int = 0,
                  sort: str | None = None,
                  direction: str | None = None) -> tuple[int, dict]:
    if not _project_attached():
        return 200, {
            "features": [],
            "page": _empty_page(limit, offset),
            "attached": False,
        }
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    return feature_ops.list_features(
        status=status,
        q=q,
        reporter=reporter,
        node=node,
        limit=limit,
        offset=offset,
        sort=sort,
        direction=direction,
    )


def get_feature(feature_id: str) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    return feature_ops.get_feature(feature_id.upper())


@_exclusive_mutation("Create Feature")
def create_feature(body: dict) -> tuple[int, dict]:
    status, payload = feature_ops.create_feature(body or {})
    _sync_feature_mutation(payload, status, "refine: create feature")
    return status, payload


@_exclusive_mutation("Update Feature")
def update_feature(feature_id: str, body: dict) -> tuple[int, dict]:
    status, payload = feature_ops.update_feature(feature_id.upper(), body or {})
    _sync_feature_mutation(payload, status, "refine: update feature")
    return status, payload


@_system_operation("Cancel Feature")
@_exclusive_mutation("Cancel Feature")
def cancel_feature(feature_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        status, payload = feature_ops.cancel_feature(
            conn,
            _backend_runner_call,
            feature_id.upper(),
        )
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()
    _sync_feature_mutation(payload, status, "refine: cancel feature")
    return status, payload


@_system_operation("Delete Feature")
@_exclusive_mutation("Delete Feature")
def delete_feature(feature_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        status, payload = feature_ops.delete_feature(
            conn,
            _backend_runner_call,
            feature_id.upper(),
        )
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()
    _sync_feature_mutation(payload, status, "refine: delete feature")
    return status, payload


@_exclusive_mutation("Assign Gap to Feature")
def assign_feature_gap(feature_id: str, gap_id: str) -> tuple[int, dict]:
    status, payload = feature_ops.assign_gap(feature_id.upper(), gap_id.upper())
    _sync_feature_mutation(payload, status, "refine: assign gap to feature")
    return status, payload


@_system_operation("Bulk assign Gaps to Feature")
@_exclusive_mutation("Bulk assign Gaps to Feature")
def bulk_assign_feature_gaps(feature_id: str, body: dict) -> tuple[int, dict]:
    status, payload = feature_ops.bulk_assign_gaps(feature_id.upper(), body or {})
    _sync_feature_mutation(payload, status, "refine: bulk assign gaps to feature")
    return status, payload


@_exclusive_mutation("Remove Gap from Feature")
def remove_feature_gap(feature_id: str, gap_id: str) -> tuple[int, dict]:
    status, payload = feature_ops.remove_gap(feature_id.upper(), gap_id.upper())
    _sync_feature_mutation(payload, status, "refine: remove gap from feature")
    return status, payload


@_exclusive_mutation("Reorder Feature Gaps")
def reorder_feature_gap(feature_id: str, gap_id: str, body: dict) -> tuple[int, dict]:
    status, payload = feature_ops.reorder_gap(
        feature_id.upper(),
        gap_id.upper(),
        before=str((body or {}).get("before") or "").strip() or None,
        after=str((body or {}).get("after") or "").strip() or None,
    )
    _sync_feature_mutation(payload, status, "refine: reorder feature gaps")
    return status, payload


def list_feature_candidate_gaps(feature_id: str, *,
                                limit: int = 50,
                                offset: int = 0) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    return feature_ops.candidate_gaps(feature_id.upper(), limit=limit, offset=offset)


def _sync_feature_mutation(payload: dict, status: int, message: str) -> None:
    if status >= 400:
        return
    payload["sync"] = _sync_refine_state_after_mutation(message)


def _mark_log_source(logs: list[dict[str, Any]], source: str) -> list[dict[str, Any]]:
    return gap_ops.mark_log_source(logs, source)


def _activity_for_round(
    conn: sqlite3.Connection,
    gap_id: str,
    rounds: list[dict[str, Any]],
    round_idx: int,
) -> list[dict[str, Any]]:
    return gap_ops.activity_for_round(conn, gap_id, rounds, round_idx)


def _enrich_gap_row(row: dict[str, Any]) -> dict[str, Any]:
    return gap_ops.enrich_gap_row(row)


_VALID_REPORTER = gap_ops.VALID_REPORTER


@_exclusive_mutation(
    "Create Gap",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def create_gap(body: dict) -> tuple[int, dict]:
    try:
        return import_ops.create_gap(_backend_runner_call, body)
    except BackendError as e:
        return _backend_err(e)


def _autoname(actual: str, target: str) -> str:
    return import_ops.autoname(actual, target)


@_exclusive_mutation(
    "Update Gap",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def update_gap_name(gap_id: str, body: dict) -> tuple[int, dict]:
    conn = _conn()
    try:
        return gap_ops.update_gap(
            conn,
            _backend_runner_call,
            gap_id,
            body,
            background_processes_stopped=_agents_paused(conn),
        )
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


@_exclusive_mutation(
    "Delete Gap",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def delete_gap(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        return gap_ops.delete_gap(conn, _backend_runner_call, gap_id)
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


@_system_operation("Bulk update Gaps")
def bulk_update_gaps(body: dict) -> tuple[int, dict]:
    allow_active_kinds = {"target_app_rebuild"}
    if _is_last_workflow_bulk_update(body):
        allow_active_kinds.add("merge_agent")
    try:
        with background_jobs.exclusive_operation(
            "Bulk update Gaps",
            allow_active_kinds=allow_active_kinds,
        ):
            return _bulk_update_gaps_impl(body)
    except background_jobs.BackgroundJobConflict as e:
        return _background_job_conflict_response(e)


def _bulk_update_gaps_impl(body: dict) -> tuple[int, dict]:
    allow_active_kinds = {"target_app_rebuild"}
    if gap_ops.is_last_workflow_bulk_update(body):
        allow_active_kinds.add("merge_agent")
    conn = _conn()
    try:
        code, plan_or_payload = gap_ops.prepare_bulk_update(conn, body)
    finally:
        conn.close()
    if code != 200 or not isinstance(plan_or_payload, gap_ops.BulkUpdatePlan):
        return code, plan_or_payload
    plan = plan_or_payload
    gap_ids = plan.gap_ids

    if (
        len(gap_ids) >= BULK_UPDATE_BACKGROUND_THRESHOLD
        and body.get("background") is not False
    ):
        stopped = _background_processes_stopped_response()
        if stopped is not None:
            return stopped
        job_data = json.loads(json.dumps({
            "field": plan.field,
            "value": plan.value,
            "gaps": plan.selected_gaps,
            "skipped_details": plan.skipped_details,
        }))

        def run_job(progress=None) -> dict[str, Any]:
            if progress is not None:
                progress(
                    completed=0,
                    total=len(job_data["gaps"]),
                    message="Bulk update queued",
                )
            result = gap_ops.run_bulk_update(
                _backend_runner_call,
                gap_ops.BulkUpdatePlan(
                    field=job_data["field"],
                    value=job_data["value"],
                    selected_gaps=job_data["gaps"],
                    skipped_details=job_data["skipped_details"],
                ),
            )
            if progress is not None:
                runner_progress = result.get("progress") or {}
                progress(
                    completed=runner_progress.get("completed", result["updated"]),
                    total=runner_progress.get("total", len(job_data["gaps"])),
                    message="Bulk update complete",
                )
            return {"http_status": 200, **result}

        try:
            job = background_jobs.start(
                "bulk_update_gaps",
                f"Bulk update {len(gap_ids)} Gaps",
                run_job,
                allow_active_kinds=allow_active_kinds,
            )
        except background_jobs.BackgroundJobConflict as e:
            return _background_job_conflict_response(e)
        return 202, {
            "queued": True,
            "job": job,
            "matched": len(gap_ids),
            "skipped": len(plan.skipped_details),
            "skipped_details": plan.skipped_details,
        }

    result = gap_ops.run_bulk_update(_backend_runner_call, plan)
    if int(result.get("http_status") or 200) >= 400:
        return int(result["http_status"]), result
    return 200, result


def _is_last_workflow_bulk_update(body: dict) -> bool:
    return gap_ops.is_last_workflow_bulk_update(body)


def _bulk_update_selected_gaps(
    field: str,
    value: str,
    selected_gaps: list[dict[str, Any]],
    skipped_status_ids: list[dict[str, str]],
) -> dict:
    return gap_ops.run_bulk_update(
        _backend_runner_call,
        gap_ops.BulkUpdatePlan(
            field=field,
            value=value,
            selected_gaps=selected_gaps,
            skipped_details=skipped_status_ids,
        ),
    )


@_system_operation("Bulk delete Gaps")
@_exclusive_mutation("Bulk delete Gaps")
def bulk_delete_gaps(body: dict) -> tuple[int, dict]:
    """Delete every Gap matching the supplied filter.

    One runner request carries the ordered id list. The runner then cancels
    any running subprocess, tears down worktrees + branches for non-done
    gaps, erases gap.json, and cleans the index in runner-owned order.
    Per-Gap failures don't abort the run — we collect them in the response.
    """
    conn = _conn()
    try:
        return gap_ops.bulk_delete_gaps(conn, _backend_runner_call, body)
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


@_exclusive_mutation(
    "Append Gap round",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def append_round(gap_id: str, body: dict) -> tuple[int, dict]:
    conn = _conn()
    try:
        return gap_ops.append_round(conn, _backend_runner_call, gap_id, body)
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


@_exclusive_mutation(
    "Edit Gap round",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def edit_latest_round(gap_id: str, body: dict) -> tuple[int, dict]:
    conn = _conn()
    try:
        return gap_ops.edit_latest_round(conn, _backend_runner_call, gap_id, body)
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


@_exclusive_mutation("Verify Gap")
def verify(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        return gap_ops.verify(conn, _backend_runner_call, gap_id)
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


def list_changes(*, limit: int = 50, offset: int = 0,
                 q: str | None = None, status: str | None = None,
                 priority: str | None = None) -> tuple[int, dict]:
    """List refine merge commits on the target branch (plus the Gap
    metadata for each). Used by the Changes screen."""
    attached = _project_attached()
    if not attached:
        return gap_ops.list_changes(
            _backend_runner_call,
            attached=False,
            limit=limit,
            offset=offset,
            q=q,
            status=status,
            priority=priority,
        )
    try:
        return gap_ops.list_changes(
            _backend_runner_call,
            attached=True,
            limit=limit,
            offset=offset,
            q=q,
            status=status,
            priority=priority,
        )
    except BackendError as e:
        return _backend_err(e)


def undo_change(body: dict) -> tuple[int, dict]:
    conn = _conn()
    try:
        return gap_ops.undo_change(conn, _backend_runner_call, body)
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


def retry(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        return gap_ops.retry(conn, _backend_runner_call, gap_id)
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


@_system_operation("Retry Gap merge")
@_exclusive_mutation("Retry Merge", allow_active_kinds={"merge_agent"})
def retry_merge(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        return gap_ops.retry_merge(conn, _backend_runner_call, gap_id)
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


@_exclusive_mutation("Retry QA")
def retry_qa(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        return gap_ops.retry_qa(conn, _backend_runner_call, gap_id)
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


def cancel(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        return gap_ops.cancel(conn, _backend_runner_call, gap_id)
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


# --- Reporters ----------------------------------------------------------------

def list_reporters() -> tuple[int, dict]:
    conn = _conn()
    try:
        return 200, reporter_ops.list_reporters(conn)
    finally:
        conn.close()


def create_reporter(body: dict) -> tuple[int, dict]:
    conn = _conn()
    try:
        payload = reporter_ops.create_reporter(conn, body.get("name"))
    except ValueError as e:
        return err(400, str(e))
    finally:
        conn.close()
    return 201, payload


def rename_reporter(rid: int, body: dict) -> tuple[int, dict]:
    try:
        payload = reporter_ops.rename_reporter(
            _backend_runner_call,
            rid,
            body.get("name"),
        )
    except ValueError as e:
        return err(400, str(e))
    except BackendError as e:
        return _backend_err(e)
    return 200, payload


@_system_operation("Merge reporters")
def merge_reporter(rid: int, body: dict) -> tuple[int, dict]:
    try:
        target_rid = int(body.get("target_id"))
    except (TypeError, ValueError):
        return err(400, "target_id is required")
    try:
        payload = reporter_ops.merge_reporter(_backend_runner_call, rid, target_rid)
    except ValueError as e:
        return err(400, str(e))
    except BackendError as e:
        return _backend_err(e)
    return 200, payload


def delete_reporter(rid: int) -> tuple[int, dict]:
    conn = _conn()
    try:
        payload = reporter_ops.delete_reporter(conn, rid)
    finally:
        conn.close()
    return 200, payload


# --- Settings -----------------------------------------------------------------

def list_settings() -> tuple[int, dict]:
    conn = _conn()
    try:
        return 200, settings_ops.list_settings(conn)
    finally:
        conn.close()


def upgrade_status() -> tuple[int, dict]:
    return 200, {"upgrade": upgrade.status(Path.cwd()).as_dict()}


@_exclusive_mutation(
    "Update settings",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def update_settings(body: dict) -> tuple[int, dict]:
    conn = _conn()
    try:
        return settings_ops.update_settings(
            conn,
            body,
            runner_call=_backend_runner_call,
            cancel_active_jobs=_cancel_active_background_jobs,
        )
    except ValueError as e:
        return err(400, str(e))
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


def governance_get() -> tuple[int, dict]:
    conn = _conn()
    try:
        return 200, project_config_ops.governance_get(conn)
    finally:
        conn.close()


def governance_save(body: dict) -> tuple[int, dict]:
    conn = _conn()
    try:
        result = project_config_ops.governance_save(
            conn,
            body,
            runner_call=_best_effort_backend_runner_call,
        )
    except ValueError as e:
        return err(400, str(e))
    finally:
        conn.close()
    return 200, result


def quality_get() -> tuple[int, dict]:
    conn = _conn()
    try:
        return 200, project_config_ops.quality_get(conn)
    finally:
        conn.close()


def quality_save(body: dict) -> tuple[int, dict]:
    conn = _conn()
    try:
        result = project_config_ops.quality_save(
            conn,
            body,
            runner_call=_best_effort_backend_runner_call,
        )
    except ValueError as e:
        return err(400, str(e))
    finally:
        conn.close()
    return 200, result


def quality_regression_create(body: dict) -> tuple[int, dict]:
    conn = _conn()
    try:
        payload = project_config_ops.regression_create(conn, body)
    finally:
        conn.close()
    return 201, payload


def quality_regression_update(regression_id: str, body: dict) -> tuple[int, dict]:
    try:
        payload = project_config_ops.regression_update(regression_id, body)
    except LookupError:
        return err(404, "Regression not found")
    return 200, payload


def quality_regression_delete(regression_id: str) -> tuple[int, dict]:
    try:
        payload = project_config_ops.regression_delete(regression_id)
    except LookupError:
        return err(404, "Regression not found")
    return 200, payload


@_system_operation("Run quality regressions")
def quality_regression_run(_body: dict | None = None) -> tuple[int, dict]:
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    try:
        result = project_config_ops.regression_run(_backend_runner_call)
    except BackendError as e:
        return _backend_err(e)
    return 200, result


@_system_operation("Generate governance rules")
def governance_generate_rules(body: dict) -> tuple[int, dict]:
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    try:
        result = project_config_ops.governance_generate_rules(_backend_runner_call, body)
    except ValueError as e:
        return err(400, str(e))
    except BackendError as e:
        return _backend_err(e)
    return 200, result


@_system_operation("Recheck provider auth")
def recheck_auth() -> tuple[int, dict]:
    try:
        result = get_client().call(M_PREFLIGHT, {}, timeout=30.0)
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def backend_diagnostics() -> tuple[int, dict]:
    backend = runtime.backend_info()
    try:
        result = diagnostics_ops.backend_diagnostics(
            _backend_runner_call,
            backend=backend,
        )
    except BackendError as e:
        return 200, diagnostics_ops.unreachable(
            backend=backend,
            message=e.message,
            code=e.code,
        )
    return 200, result


# --- Activity / Dashboard -----------------------------------------------------

def list_activity(*, limit: int = 50, gap_id: str | None = None,
                  since_id: int | None = None,
                  severity: str | None = None,
                  category: str | None = None,
                  actor: str | None = None,
                  q: str | None = None,
                  offset: int = 0,
                  sort: str | None = None,
                  direction: str | None = None,
                  include_facets: bool = False) -> tuple[int, dict]:
    if not _project_attached():
        return 200, observability_ops.empty_activity(
            limit=limit,
            offset=offset,
            include_facets=include_facets,
        )
    conn = _conn()
    try:
        body = observability_ops.list_activity(
            conn,
            limit=limit,
            gap_id=gap_id,
            since_id=since_id,
            severity=severity,
            category=category,
            actor=actor,
            q=q,
            offset=offset,
            sort=sort,
            direction=direction,
            include_facets=include_facets,
        )
    finally:
        conn.close()
    return 200, body


def record_ui_error(body: dict) -> tuple[int, dict]:
    try:
        conn = _conn(ensure_cache=False)
        try:
            return 200, observability_ops.record_ui_error(conn, body)
        finally:
            conn.close()
    except Exception:
        return 200, {"ok": False}


_LOG_RETENTION_OPTIONS = observability_ops.LOG_RETENTION_OPTIONS


@_system_operation("Clean up logs")
def cleanup_logs(body: dict) -> tuple[int, dict]:
    """Delete activity entries older than `days` days.

    `days == 0` deletes the whole activity table (operator chose
    "don't keep any"). Anything else uses an ISO-timestamp cutoff
    computed against `now`. Returns the number of rows deleted.
    """
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    conn = _conn()
    try:
        return 200, observability_ops.cleanup_logs(conn, body.get("days"))
    except ValueError as e:
        return err(400, str(e))
    finally:
        conn.close()


def dashboard_summary(*, node: str | None = None) -> tuple[int, dict]:
    if not _project_attached():
        return 200, dashboard_ops.empty_dashboard(node=node)
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    runner_snap = runtime.runner_status_snapshot()
    conn = _conn()
    try:
        return 200, dashboard_ops.summary(
            conn,
            node=node,
            runner_snapshot=runner_snap,
        )
    finally:
        conn.close()


def process_summary() -> tuple[int, dict]:
    """Return managed process state for System > Processes."""
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked

    runner_snap = runtime.runner_status_snapshot()
    backend = runner_snap.get("backend") or runtime.backend_info()
    supervisor_pid = process_ops.int_or_none(os.environ.get("REFINE_SUPERVISOR_PID"))
    conn = _conn()
    try:
        return 200, process_ops.summary(
            conn,
            runner_snapshot=runner_snap,
            backend=backend,
            ui_pid=os.getpid(),
            supervisor_pid=supervisor_pid,
            active_background_job=_active_background_job,
        )
    finally:
        conn.close()


def _active_agent_run_snapshots(
    conn: sqlite3.Connection,
    running_snapshot: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    return process_ops.active_agent_run_snapshots(conn, running_snapshot)


def _running_snapshot_for_node(
    conn: sqlite3.Connection,
    active_node: str,
    running_snapshot: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    return process_ops.running_snapshot_for_node(conn, active_node, running_snapshot)


def _sqlite_running_agent_snapshots(
    conn: sqlite3.Connection,
    active_node: str,
    seen: set[tuple[str, str]],
) -> list[dict[str, Any]]:
    return process_ops.sqlite_running_agent_snapshots(conn, active_node, seen)


def _pid_may_be_alive(pid: int) -> bool:
    return process_ops.pid_may_be_alive(pid)


@_system_operation("Toggle background processes")
def set_background_processes(body: dict | None = None) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    conn = _conn()
    try:
        return process_ops.set_background_processes(
            conn,
            body,
            runner_call=_backend_runner_call,
            cancel_active_jobs=_cancel_active_background_jobs,
            process_summary=process_summary,
        )
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


@_system_operation("Pause or unpause agents")
def set_agent_processes(body: dict | None = None) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    conn = _conn()
    try:
        return process_ops.set_agent_processes(
            conn,
            body,
            runner_call=_backend_runner_call,
            process_summary=process_summary,
        )
    except BackendError as e:
        return _backend_err(e)
    finally:
        conn.close()


def _int_or_none(value: Any) -> int | None:
    return process_ops.int_or_none(value)


def _runner_work_summary(
    merger: dict | None,
    governance_state: dict | None,
    target_app_rebuild: dict | None,
    *,
    paused: bool = False,
) -> list[dict[str, Any]]:
    return process_ops.runner_work_summary(
        merger,
        governance_state,
        target_app_rebuild,
        paused=paused,
        active_background_job=_active_background_job,
    )


def _paused_runner_worker(row: dict[str, Any]) -> dict[str, Any]:
    return process_ops.paused_runner_worker(row)


def _static_worker_row(
    worker_id: str,
    kind: str,
    label: str,
    details: str,
) -> dict[str, Any]:
    return process_ops.static_worker_row(worker_id, kind, label, details)


def _background_worker_row(
    worker_id: str,
    job_kind: str,
    label: str,
    details: str,
) -> dict[str, Any]:
    return process_ops.background_worker_row(
        worker_id,
        job_kind,
        label,
        details,
        active_background_job=_active_background_job,
    )


def _active_background_job(kind: str) -> dict[str, Any] | None:
    try:
        conn = _conn()
        try:
            rows = conn.execute(
                "SELECT id FROM background_jobs "
                "WHERE kind = ? AND status IN ('queued', 'running') "
                "ORDER BY started_at DESC LIMIT 5",
                (kind,),
            ).fetchall()
        finally:
            conn.close()
    except Exception:
        return None
    for row in rows:
        snap = background_jobs.snapshot(str(row["id"]))
        if snap and snap.get("status") in {"queued", "running"}:
            return snap
    return None


def _elapsed_since(value: Any) -> int:
    return process_ops.elapsed_since(value)


def _process_resource_caps(settings: dict[str, str]) -> dict[str, Any]:
    return process_ops.process_resource_caps(settings)


def _memory_limit_label(memory_mb: int) -> str:
    return process_ops.memory_limit_label(memory_mb)


def _cpu_priority_label(priority: str, weight: int) -> str:
    return process_ops.cpu_priority_label(priority, weight)


# --- Import (LLM extraction) --------------------------------------------------

IMPORT_DEDUP_THRESHOLD = import_ops.IMPORT_DEDUP_THRESHOLD


@_system_operation("Extract Gaps from import text")
def import_extract(body: dict) -> tuple[int, dict]:
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    try:
        return import_ops.extract(_backend_runner_call, body)
    except BackendError as e:
        return _backend_err(e)


@_system_operation("Prepare CSV import")
def import_parse_csv(body: dict) -> tuple[int, dict]:
    raw = str(body.get("text") or "")
    if not raw.strip():
        return err(400, "CSV text is required")
    if body.get("background") is True:
        stopped = _background_processes_stopped_response()
        if stopped is not None:
            return stopped
        job_body = {
            "text": raw,
            "background": False,
            "dedup": bool(body.get("dedup")),
            "distribute": bool(body.get("distribute")),
        }

        def run_job() -> dict[str, Any]:
            status, result = import_parse_csv(job_body)
            return {"http_status": status, **result}

        job = background_jobs.start(
            "import_prepare",
            "Prepare CSV import",
            run_job,
        )
        return 202, {"queued": True, "job": job}
    return import_ops.parse_csv(
        body,
        progress=_import_prepare_progress,
        cancel_requested=_import_prepare_cancel_if_requested,
    )


def _import_prepare_cancel_if_requested() -> None:
    if background_jobs.current_cancelled():
        raise background_jobs.CancellationRequested({"cancelled": True})


def _import_prepare_progress(completed: int, total: int, message: str) -> None:
    job_id = background_jobs.current_job_id()
    if not job_id:
        return
    background_jobs.progress(
        job_id,
        completed=completed,
        total=total,
        message=message,
    )


@_system_operation("Deduplicate import drafts")
def import_dedup(body: dict) -> tuple[int, dict]:
    return import_ops.dedup(
        body,
        progress=_import_prepare_progress,
        cancel_requested=_import_prepare_cancel_if_requested,
    )


@_system_operation("Import Gaps")
@_exclusive_mutation("Import Gaps")
def import_persist(body: dict) -> tuple[int, dict]:
    drafts = body.get("drafts") or []
    if (
        isinstance(drafts, list)
        and (
            body.get("background") is True
            or (
                len(drafts) >= IMPORT_BACKGROUND_THRESHOLD
                and body.get("background") is not False
            )
        )
    ):
        stopped = _background_processes_stopped_response()
        if stopped is not None:
            return stopped
        job_body = {
            "reporter": (body.get("reporter") or "").strip(),
            "drafts": drafts,
            "background": False,
        }
        for key in (
            "feature_id",
            "feature",
            "new_feature_name",
            "new_feature_description",
            "feature_description",
            "feature_reporter",
        ):
            if key in body:
                job_body[key] = body.get(key)
        job_body = json.loads(json.dumps(job_body))

        def run_job() -> dict[str, Any]:
            status, result = _import_persist_sync(job_body)
            return {"http_status": status, **result}

        job = background_jobs.start(
            "import_persist",
            f"Import {len(drafts)} Gaps",
            run_job,
        )
        return 202, {"queued": True, "job": job, "drafts": len(drafts)}
    return _import_persist_sync(body)


def _cancel_import_if_requested(
    created: list[str],
    duplicate_moves: list[dict[str, Any]] | None = None,
    duplicate_updates: list[dict[str, Any]] | None = None,
) -> None:
    if not background_jobs.current_cancelled():
        return
    rolled_back = import_ops.rollback_import_created_gaps(_backend_runner_call, created)
    restored = import_ops.rollback_import_duplicate_moves(duplicate_moves or [])
    restored_updates = import_ops.rollback_import_duplicate_updates(duplicate_updates or [])
    raise background_jobs.CancellationRequested({
        "cancelled": True,
        "created": created,
        "rolled_back": rolled_back,
        "restored_duplicates": restored,
        "restored_original_updates": restored_updates,
        "count": 0,
        "failures": [],
        "failed": 0,
    })


def _import_persist_progress(completed: int, total: int, message: str) -> None:
    job_id = background_jobs.current_job_id()
    if not job_id:
        return
    background_jobs.progress(
        job_id,
        completed=completed,
        total=total,
        message=message,
    )


def _import_persist_sync(body: dict) -> tuple[int, dict]:
    try:
        return import_ops.persist(
            _backend_runner_call,
            body,
            progress=_import_persist_progress,
            cancel=_cancel_import_if_requested,
        )
    except BackendError as e:
        return _backend_err(e)


_import_parse_csv_drafts = import_ops.import_parse_csv_drafts
_assign_import_nodes = import_ops.assign_import_nodes
_import_dedup_matches = import_ops.import_dedup_matches
_annotate_import_duplicate_drafts = import_ops.annotate_import_duplicate_drafts
_duplicate_move_to_backlog_status = import_ops.duplicate_move_to_backlog_status
_move_duplicate_original_to_backlog = import_ops.move_duplicate_original_to_backlog
_import_dedup_candidates = import_ops.import_dedup_candidates
_find_import_duplicate = import_ops.find_import_duplicate
_best_import_duplicate = import_ops.best_import_duplicate
_import_dedup_score = import_ops.import_dedup_score
_import_dedup_normalize = import_ops.import_dedup_normalize
_import_token_jaccard = import_ops.import_token_jaccard
_import_token_cosine = import_ops.import_token_cosine
_import_token_counts = import_ops.import_token_counts
_import_stem_token = import_ops.import_stem_token
_import_trigram_cosine = import_ops.import_trigram_cosine
_import_char_ngrams = import_ops.import_char_ngrams
_rollback_import_created_gaps = (
    lambda created: import_ops.rollback_import_created_gaps(_backend_runner_call, created)
)
_rollback_import_duplicate_moves = import_ops.rollback_import_duplicate_moves
_rollback_import_duplicate_updates = import_ops.rollback_import_duplicate_updates
_duplicate_update_field = import_ops.duplicate_update_field
_latest_round_snapshot = import_ops.latest_round_snapshot
_update_gap_priority_no_ownership = import_ops.update_gap_priority_no_ownership
_update_gap_reporter_index_no_ownership = import_ops.update_gap_reporter_index_no_ownership
_upsert_gap_search_no_ownership = import_ops.upsert_gap_search_no_ownership
_update_duplicate_original_from_draft = import_ops.update_duplicate_original_from_draft


# --- Chat ---------------------------------------------------------------------

def chat_start(body: dict) -> tuple[int, dict]:
    try:
        return chat_ops.start(_backend_runner_call, body)
    except BackendError as e:
        return _backend_err(e)


def chat_input(sid: str, body: dict) -> tuple[int, dict]:
    try:
        return chat_ops.input(_backend_runner_call, sid, body)
    except BackendError as e:
        return _backend_err(e)


def chat_read(sid: str) -> tuple[int, dict]:
    try:
        return chat_ops.read(_backend_runner_call, sid)
    except BackendError as e:
        return _backend_err(e)


def chat_stop(sid: str) -> tuple[int, dict]:
    try:
        return chat_ops.stop(_backend_runner_call, sid)
    except BackendError as e:
        return _backend_err(e)


# --- Target application -------------------------------------------------------
#
# The operator writes plain-language start/stop prompts in Settings (or
# generates them via /api/target-app/generate-instructions). Clicking the
# nav toggle hits /start or /stop, which routes through the runner to a
# Standalone agent. State transitions are recorded in SQLite settings so
# every browser tab sees the same status.

def target_app_status() -> tuple[int, dict]:
    """Return the current target-app state + last health-check snapshot."""
    conn = _conn()
    try:
        snap = _target_app_snapshot(conn)
    finally:
        conn.close()
    return 200, snap


def _target_app_snapshot(conn: sqlite3.Connection) -> dict:
    return target_app_ops.snapshot(conn)


def _cleanup_legacy_target_app_settings(conn: sqlite3.Connection) -> bool:
    return target_app_ops.cleanup_legacy_settings(conn)


def _target_app_config(settings: dict[str, str]) -> dict[str, Any]:
    return target_app_ops.config_from_settings(settings)


def _has_status_checks(cfg: dict[str, Any]) -> bool:
    return target_app_ops.has_status_checks(cfg)


@_system_operation("Start app")
@_exclusive_mutation("Start target app")
def target_app_start(_body: dict | None = None) -> tuple[int, dict]:
    """Run the configured start command via the host runner."""
    return _target_app_run("start")


@_system_operation("Stop app")
@_exclusive_mutation("Stop target app")
def target_app_stop(_body: dict | None = None) -> tuple[int, dict]:
    """Run the configured stop command via the host runner."""
    return _target_app_run("stop")


@_exclusive_mutation("Rebuild target app")
def target_app_rebuild(_body: dict | None = None) -> tuple[int, dict]:
    """Queue the standard stop/rebuild/start target-app rebuild sequence."""
    return target_app_rebuild_queue(_body)


@_system_operation("Rebuild app")
def target_app_rebuild_queue(_body: dict | None = None) -> tuple[int, dict]:
    """Queue the persistent target-app rebuilder worker."""
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    try:
        return target_app_ops.queue_rebuild(_target_app_runner_call)
    except BackendError as e:
        return _backend_err(e)


@_system_operation("Hard reset worktree")
def hard_reset_worktree(_body: dict | None = None) -> tuple[int, dict]:
    """Destructively reset the host target worktree through the runner."""
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    try:
        return target_app_ops.hard_reset(_target_app_runner_call)
    except BackendError as e:
        return _backend_err(e)


def _target_app_run(kind: str) -> tuple[int, dict]:
    try:
        return target_app_ops.run(_conn, _target_app_runner_call, kind)
    except BackendError as e:
        return _backend_err(e)


def _target_app_runner_call(method: str, params: dict[str, Any], timeout: float) -> dict:
    return get_client().call(method, params, timeout=timeout)


def _backend_runner_call(method: str, params: dict[str, Any], timeout: float) -> dict:
    return get_client().call(method, params, timeout=timeout)


def _best_effort_backend_runner_call(
    method: str,
    params: dict[str, Any],
    timeout: float,
) -> dict:
    try:
        return _backend_runner_call(method, params, timeout)
    except BackendError:
        return {}


def target_app_check(_body: dict | None = None) -> tuple[int, dict]:
    """Force an immediate deterministic status check."""
    quiet = bool((_body or {}).get("quiet"))
    try:
        return target_app_ops.check(_conn, _target_app_runner_call, quiet=quiet)
    except BackendError as e:
        return _backend_err(e)


@_system_operation("Check app")
def target_app_health(_body: dict | None = None) -> tuple[int, dict]:
    """Back-compatible route name for a target-app status check."""
    return target_app_check(_body)


def _target_app_run_health_check() -> dict:
    """Back-compatible poller hook for deterministic target-app status."""
    status, snap = target_app_check({"quiet": True})
    return snap if status == 200 else {"state": "unknown", "last_check_ok": False}


@_system_operation("Generate app configuration")
def target_app_generate(body: dict) -> tuple[int, dict]:
    """Use the agent to draft structured target-app config for this codebase."""
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    try:
        status, payload = target_app_ops.generate(_target_app_runner_call, body)
    except BackendError as e:
        return _backend_err(e)
    if status == 400:
        return err(400, payload["error"]["message"])
    return status, payload


# --- helpers ------------------------------------------------------------------

def _backend_err(e: BackendError) -> tuple[int, dict]:
    if e.code == "backend_unavailable":
        code = 502
    elif e.code == "node_ownership":
        code = 409
    elif e.code == "bad_request":
        code = 400
    else:
        code = 500
    return code, {"error": {"code": e.code, "message": e.message,
                            "details": e.details}}
