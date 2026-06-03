"""Shared target-application lifecycle operations."""
from __future__ import annotations

import json
import sqlite3
from typing import Any, Callable

from . import activity, db, gap_writer, project_state, project_sync, quality, target_app
from .backend_protocol import (
    M_HARD_RESET_WORKTREE,
    M_TARGET_APP_GENERATE,
    M_TARGET_APP_REBUILD_QUEUE,
    M_TARGET_APP_RUN,
)
from .gaps import now_iso


RunnerCall = Callable[[str, dict[str, Any], float], dict[str, Any]]

TARGET_APP_STATES = (
    "unknown", "starting", "rebuilding", "running", "degraded",
    "stopping", "stopped", "failed",
)


def snapshot(conn: sqlite3.Connection) -> dict[str, Any]:
    """Return the current target-app state and last health-check snapshot."""
    state = db.get_setting(conn, "target_app_state") or "unknown"
    cleanup_legacy_settings(conn)
    settings = db.list_settings(conn)
    cfg = target_app.config_from_settings(settings)
    last_op = conn.execute(
        "SELECT id, kind, state, started_at, finished_at, exit_code, message "
        "FROM target_app_operations ORDER BY id DESC LIMIT 1"
    ).fetchone()
    legacy_start = (settings.get("target_app_start_instructions") or "").strip()
    legacy_stop = (settings.get("target_app_stop_instructions") or "").strip()
    return {
        "state": state if state in TARGET_APP_STATES else "unknown",
        "health_url": cfg.get("http_check_url") or "",
        "app_url": settings.get("target_app_url") or "",
        "has_start_command": bool(cfg.get("start_command")),
        "has_stop_command": bool(cfg.get("stop_command")),
        "has_rebuild_command": bool(cfg.get("rebuild_command")),
        "has_status_checks": has_status_checks(cfg),
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
        "auto_rebuild": settings.get("target_app_auto_rebuild") or "on_worktree_merge",
        "auto_rebuild_last_started_at": settings.get("target_app_auto_rebuild_last_started_at") or "",
        "auto_rebuild_last_finished_at": settings.get("target_app_auto_rebuild_last_finished_at") or "",
        "auto_rebuild_last_ok": (settings.get("target_app_auto_rebuild_last_ok") or "0") == "1",
        "auto_rebuild_last_message": settings.get("target_app_auto_rebuild_last_message") or "",
        "legacy_config_present": bool(
            legacy_start
            or legacy_stop
            or (settings.get("target_app_health_url") or "").strip()
        ),
    }


def cleanup_legacy_settings(conn: sqlite3.Connection) -> bool:
    settings = db.list_settings(conn)
    updates: dict[str, str] = {}
    legacy_health = (settings.get("target_app_health_url") or "").strip()
    if legacy_health:
        if not (settings.get("target_app_http_check_url") or "").strip():
            updates["target_app_http_check_url"] = legacy_health
        updates["target_app_health_url"] = ""
    if (
        (settings.get("target_app_start_instructions") or "").strip()
        and (settings.get("target_app_start_command") or "").strip()
    ):
        updates["target_app_start_instructions"] = ""
    if (
        (settings.get("target_app_stop_instructions") or "").strip()
        and (settings.get("target_app_stop_command") or "").strip()
    ):
        updates["target_app_stop_instructions"] = ""
    for key, value in updates.items():
        db.set_setting(conn, key, value)
    return bool(updates)


def config_from_settings(settings: dict[str, str]) -> dict[str, Any]:
    return target_app.config_from_settings(settings)


def has_status_checks(cfg: dict[str, Any]) -> bool:
    return any((
        (cfg.get("status_command") or "").strip(),
        (cfg.get("http_check_url") or "").strip(),
        (cfg.get("tcp_check_host") or "").strip()
        and (cfg.get("tcp_check_port") or "").strip(),
        (cfg.get("process_check_command") or "").strip(),
    ))


def run(
    conn_factory: Callable[[], sqlite3.Connection],
    runner_call: RunnerCall,
    kind: str,
) -> tuple[int, dict[str, Any]]:
    conn = conn_factory()
    try:
        settings = db.list_settings(conn)
        cfg = config_from_settings(settings)
        command = (cfg.get(f"{kind}_command") or "").strip()
        if not command:
            msg = f"No {kind} command configured; {kind} is a no-op."
            db.set_setting(conn, "target_app_last_error", "")
            promoted = promote_rebuilt_gaps(conn) if kind == "rebuild" else 0
            activity.append(
                conn,
                message=f"target-app: {kind} skipped; no {kind} command configured",
                severity="info", category="target_app", actor="refine",
            )
            snap = snapshot(conn)
            snap.update({
                "ok": True,
                "noop": True,
                "state": snap.get("state") or "unknown",
                "message": msg,
                "details": "",
                "promoted_gaps": promoted,
            })
            return 200, snap
        next_pending = {
            "start": "starting",
            "stop": "stopping",
            "rebuild": "rebuilding",
        }.get(kind, "unknown")
        db.set_setting(conn, "target_app_state", next_pending)
        db.set_setting(conn, "target_app_last_error", "")
        activity.append(
            conn,
            message=f"target-app: {kind} requested",
            severity="info", category="target_app", actor="refine",
        )
    finally:
        conn.close()

    try:
        result = runner_call(M_TARGET_APP_RUN, {"kind": kind, "config": cfg}, 900.0)
    except Exception as e:
        record_failure(conn_factory, kind, str(e))
        raise

    ok = bool(result.get("ok"))
    final_state = result.get("state") or ("running" if kind == "start" else "stopped")
    if final_state not in TARGET_APP_STATES:
        final_state = "failed" if not ok else "unknown"
    err_msg = "" if ok else (result.get("message") or "target-app operation failed")
    conn = conn_factory()
    try:
        db.set_setting(conn, "target_app_state", final_state)
        db.set_setting(conn, "target_app_last_error", err_msg)
        op_id = record_operation(conn, kind, result, final_state)
        db.set_setting(conn, "target_app_last_operation_id", str(op_id))
        if result.get("checks_configured"):
            persist_check_settings(conn, result.get("checks") or [], result.get("message") or "")
        promoted = promote_rebuilt_gaps(conn) if kind == "rebuild" and ok else 0
        snap = snapshot(conn)
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
        "promoted_gaps": promoted,
    })
    return status, snap


def queue_rebuild(runner_call: RunnerCall) -> tuple[int, dict[str, Any]]:
    result = runner_call(M_TARGET_APP_REBUILD_QUEUE, {}, 10.0)
    return 202, result


def hard_reset(runner_call: RunnerCall) -> tuple[int, dict[str, Any]]:
    result = runner_call(M_HARD_RESET_WORKTREE, {}, 300.0)
    return (200 if result.get("ok") else 409), result


def check(
    conn_factory: Callable[[], sqlite3.Connection],
    runner_call: RunnerCall,
    *,
    quiet: bool = False,
) -> tuple[int, dict[str, Any]]:
    conn = conn_factory()
    try:
        settings = db.list_settings(conn)
        cfg = config_from_settings(settings)
    finally:
        conn.close()
    try:
        result = runner_call(
            M_TARGET_APP_RUN,
            {"kind": "status", "config": cfg, "quiet": quiet},
            60.0,
        )
    except Exception as e:
        record_failure(conn_factory, "status", str(e))
        raise
    if result.get("busy") and quiet:
        conn = conn_factory()
        try:
            return 200, snapshot(conn)
        finally:
            conn.close()
    final_state = (
        result.get("state")
        if result.get("state") in TARGET_APP_STATES else "unknown"
    )
    conn = conn_factory()
    try:
        persist_status = not quiet
        db.set_setting(conn, "target_app_state", final_state, persist=persist_status)
        db.set_setting(
            conn,
            "target_app_last_error",
            "" if result.get("ok") else (result.get("message") or "status check failed"),
            persist=persist_status,
        )
        if not quiet:
            op_id = record_operation(conn, "status", result, final_state)
            db.set_setting(conn, "target_app_last_operation_id", str(op_id))
        persist_check_settings(
            conn,
            result.get("checks") or [],
            result.get("message") or "",
            persist=persist_status,
        )
        snap = snapshot(conn)
    finally:
        conn.close()
    snap.update({"ok": bool(result.get("ok")), "probe_message": result.get("message") or ""})
    return 200, snap


def generate(runner_call: RunnerCall, body: dict[str, Any]) -> tuple[int, dict[str, Any]]:
    kind = (body.get("kind") or "all").strip().lower()
    if kind not in ("all", "start", "stop", "rebuild", "status"):
        return 400, {"error": {"message": "kind must be 'all', 'start', 'stop', 'rebuild', or 'status'"}}
    result = runner_call(M_TARGET_APP_GENERATE, {"kind": kind}, 600.0)
    if not result.get("ok"):
        return 502, {"error": {"message": result.get("message") or "generation failed"}}
    return 200, {
        "ok": True,
        "config": result.get("config") or {},
        "notes": (result.get("config") or {}).get("notes") or "",
        "raw": result.get("raw") or "",
        "script_path": result.get("script_path") or "",
    }


def record_operation(
    conn: sqlite3.Connection,
    kind: str,
    result: dict[str, Any],
    state: str,
) -> int:
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


def promote_rebuilt_gaps(conn: sqlite3.Connection) -> int:
    active_node = project_state.active_node_id()
    rows = conn.execute(
        "SELECT id FROM gaps_index WHERE status = 'awaiting-rebuild' "
        "AND node_id = ? ORDER BY updated ASC",
        (active_node,),
    ).fetchall()
    if not rows:
        return 0
    post_rebuild_quality = quality.enabled(conn) and quality.post_rebuild(conn)
    next_status = "qa" if post_rebuild_quality else "review"
    message = (
        "Target application rebuilt; Gap queued for QA"
        if post_rebuild_quality
        else "Target application rebuilt; Gap is ready for review"
    )
    with db.transaction(conn):
        conn.execute(
            "UPDATE gaps_index SET status = ?, updated = ? "
            "WHERE status = 'awaiting-rebuild' AND node_id = ?",
            (next_status, now_iso(), active_node),
        )
    for row in rows:
        gap_id = row["id"]
        try:
            gap_writer.update_fields(gap_id, status=next_status, branch_name=None)
            append_gap_workflow_log(gap_id, message, actor="refine")
        except Exception:
            pass
        activity.append(
            conn,
            message=message,
            severity="info", category="state", gap_id=gap_id, actor="refine",
        )
    project_sync.commit_refine_transition_state(
        conn,
        actor="refine",
        state_message="refine: persist rebuilt Gap state",
    )
    return len(rows)


def append_gap_workflow_log(
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


def persist_check_settings(
    conn: sqlite3.Connection,
    checks: list[dict[str, Any]],
    message: str,
    *,
    persist: bool = True,
) -> None:
    ok = bool(checks) and all(bool(c.get("ok")) for c in checks)
    checked_at = now_iso()
    db.set_setting(conn, "target_app_last_check_at", checked_at, persist=persist)
    db.set_setting(conn, "target_app_last_check_ok", "1" if ok else "0", persist=persist)
    db.set_setting(conn, "target_app_last_check_message", message or "", persist=persist)
    db.set_setting(conn, "target_app_last_health_at", checked_at, persist=persist)
    db.set_setting(conn, "target_app_last_health_ok", "1" if ok else "0", persist=persist)
    db.set_setting(conn, "target_app_last_health_message", message or "", persist=persist)


def record_failure(
    conn_factory: Callable[[], sqlite3.Connection],
    kind: str,
    message: str,
) -> None:
    conn = conn_factory()
    try:
        rollback = "stopped" if kind == "start" else (
            "running" if kind == "stop" else "unknown"
        )
        db.set_setting(conn, "target_app_state", rollback)
        db.set_setting(conn, "target_app_last_error", message)
        activity.append(
            conn,
            message=f"target-app: {kind} failed - {message}",
            severity="error", category="target_app", actor="refine",
        )
    finally:
        conn.close()
