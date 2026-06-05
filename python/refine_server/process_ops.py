"""Shared process control operations."""
from __future__ import annotations

import os
import sqlite3
from datetime import datetime, timezone
from collections.abc import Callable
from typing import Any

from refine_runtime import resources as runtime_resources

from . import activity, db, project_state, target_app_ops
from .backend_protocol import M_BACKGROUND_PROCESSES_SET, M_ENFORCE_SCHEDULING


RunnerCall = Callable[[str, dict[str, object], float], dict[str, Any]]
CancelActiveJobs = Callable[[], list[dict[str, Any]]]
ProcessSummary = Callable[[], tuple[int, dict[str, Any]]]
ActiveBackgroundJob = Callable[[str], dict[str, Any] | None]


def set_background_processes(
    conn: sqlite3.Connection,
    body: dict[str, Any] | None,
    *,
    runner_call: RunnerCall,
    cancel_active_jobs: CancelActiveJobs | None = None,
    process_summary: ProcessSummary | None = None,
) -> tuple[int, dict[str, Any]]:
    body = body or {}
    current_stopped = (db.get_setting(conn, "paused") or "0") == "1"
    stopped = (
        not current_stopped
        if "stopped" not in body
        else _truthy(body.get("stopped"))
    )
    db.set_setting(conn, "paused", "1" if stopped else "0")
    project_state.set_setting("paused", "1" if stopped else "0")
    activity.append(
        conn,
        message="Background processes stopped" if stopped else "Background processes started",
        severity="warn" if stopped else "info",
        category="state",
        actor="refine",
    )

    cancelled_jobs = cancel_active_jobs() if stopped and cancel_active_jobs is not None else []
    runner_result = runner_call(
        M_BACKGROUND_PROCESSES_SET,
        {"stopped": stopped},
        30.0 if stopped else 10.0,
    )
    if stopped and not runner_result.get("ok", True):
        cleanup = runner_result.get("cleanup") or {}
        return error(
            409,
            cleanup.get("message") or (
                "background processes stopped but target worktree cleanup "
                "did not complete"
            ),
        )

    summary = _summary_payload(process_summary)
    return 200, {
        "stopped": stopped,
        "paused": stopped,
        "runner": runner_result,
        "cancelled_background_jobs": len(cancelled_jobs),
        "processes": summary.get("processes") or [],
        "runner_work": summary.get("runner_work") or [],
        "runner_reachable": summary.get("runner_reachable"),
    }


def set_agent_processes(
    conn: sqlite3.Connection,
    body: dict[str, Any] | None,
    *,
    runner_call: RunnerCall,
    process_summary: ProcessSummary | None = None,
) -> tuple[int, dict[str, Any]]:
    body = body or {}
    current_paused = (db.get_setting(conn, "agents_paused") or "0") == "1"
    paused = (
        not current_paused
        if "paused" not in body
        else _truthy(body.get("paused"))
    )
    db.set_setting(conn, "agents_paused", "1" if paused else "0")
    project_state.set_setting("agents_paused", "1" if paused else "0")
    activity.append(
        conn,
        message="Agents paused" if paused else "Agents unpaused",
        severity="warn" if paused else "info",
        category="state",
        actor="refine",
    )

    runner_result = runner_call(
        M_ENFORCE_SCHEDULING,
        {"settle_timeout_seconds": 8.0},
        30.0 if paused else 10.0,
    )
    if paused and not runner_result.get("ok", True):
        cleanup = runner_result.get("cleanup") or {}
        return error(
            409,
            cleanup.get("message") or (
                "agents paused but target worktree cleanup did not complete"
            ),
        )

    summary = _summary_payload(process_summary)
    return 200, {
        "paused": paused,
        "agents_paused": paused,
        "agent_processes_paused": summary.get("agent_processes_paused", paused),
        "background_processes_stopped": summary.get("background_processes_stopped"),
        "runner": runner_result,
        "processes": summary.get("processes") or [],
        "runner_work": summary.get("runner_work") or [],
        "runner_reachable": summary.get("runner_reachable"),
    }


def summary(
    conn: sqlite3.Connection,
    *,
    runner_snapshot: dict[str, Any],
    backend: dict[str, Any],
    ui_pid: int | None,
    supervisor_pid: int | None,
    active_background_job: ActiveBackgroundJob | None = None,
) -> dict[str, Any]:
    runner_reachable = bool(runner_snapshot.get("runner_reachable"))
    runner_pid = runner_snapshot.get("pid")
    settings = db.list_settings(conn)
    background_stopped = (db.get_setting(conn, "paused") or "0") == "1"
    agents_paused = (db.get_setting(conn, "agents_paused") or "0") == "1"
    agent_processes_paused = background_stopped or agents_paused
    target_app = target_app_ops.snapshot(conn)
    agent_runs = active_agent_run_snapshots(
        conn,
        runner_snapshot.get("running") or [],
    )
    resource_caps = process_resource_caps(settings)
    worker_caps = {
        "cpu_priority": resource_caps["worker_cpu_priority"],
        "max_memory": resource_caps["worker_max_memory"],
    }
    ui_caps = {
        "cpu_priority": resource_caps["ui_cpu_priority"],
        "max_memory": resource_caps["ui_max_memory"],
    }
    unmanaged_caps = {
        "cpu_priority": {"label": "unmanaged"},
        "max_memory": {"label": "unmanaged"},
    }
    no_caps = {
        "cpu_priority": {"label": "-"},
        "max_memory": {"label": "-"},
    }

    processes: list[dict[str, Any]] = []
    if backend.get("process_model") == "supervisor":
        processes.append({
            "id": "supervisor",
            "kind": "supervisor",
            "label": "Supervisor",
            "status": "running" if supervisor_pid else "unknown",
            "pid": supervisor_pid,
            "details": (
                "Supervises the UI and runner worker processes; shuts Refine "
                "down if either exits."
            ),
            "background_processes_stopped": background_stopped,
            "agents_paused": agents_paused,
            "actions": [
                "start_background_processes"
                if background_stopped
                else "stop_background_processes"
            ],
            **no_caps,
        })
    processes.extend([
        {
            "id": "ui",
            "kind": "ui",
            "label": "UI process",
            "status": "running" if ui_pid else "unknown",
            "pid": ui_pid,
            "details": "Serves the web UI, API routes, and SSE updates.",
            "actions": [],
            **ui_caps,
        },
        {
            "id": "runner",
            "kind": "runner",
            "label": (
                "Runner worker"
                if backend.get("process_model") == "supervisor"
                else "In-process runner"
            ),
            "status": "running" if runner_reachable else "unreachable",
            "pid": runner_pid,
            "actions": [],
            **worker_caps,
        },
        {
            "id": "target-app",
            "kind": "target_app",
            "label": "Target application",
            "status": target_app.get("state") or "unknown",
            "pid": None,
            "actions": ["start", "rebuild", "stop", "check"],
            "target_app": target_app,
            **unmanaged_caps,
        },
    ])

    merger = runner_snapshot.get("merger") or None
    governance = runner_snapshot.get("governance") or None
    target_app_rebuild = runner_snapshot.get("target_app_rebuild") or None
    runner_work = runner_work_summary(
        merger,
        governance,
        target_app_rebuild,
        paused=background_stopped,
        active_background_job=active_background_job,
    )

    for chat in runner_snapshot.get("chat") or []:
        session_id = str(chat.get("session_id") or "")
        processes.append({
            "id": f"chat:{session_id}",
            "kind": "chat",
            "label": "Chat",
            "status": chat.get("status") or "running",
            "session_id": session_id,
            "pid": chat.get("pid"),
            "provider": chat.get("provider"),
            "mode": chat.get("mode"),
            "gap_id": chat.get("gap_id"),
            "elapsed_seconds": chat.get("elapsed_seconds") or 0,
            "idle_seconds": chat.get("idle_seconds") or 0,
            "actions": ["stop"],
            **worker_caps,
        })

    for run in agent_runs:
        gap_id = str(run.get("gap_id") or "")
        run_kind = str(run.get("kind") or "implementation")
        processes.append({
            "id": f"agent:{gap_id}",
            "kind": "agent",
            "label": "Quality agent" if run_kind == "quality" else "Agent",
            "status": "running",
            "gap_id": gap_id,
            "round_idx": run.get("round_idx"),
            "run_kind": run_kind,
            "pid": run.get("pid"),
            "elapsed_seconds": run.get("elapsed_seconds") or 0,
            "idle_seconds": run.get("idle_seconds") or 0,
            "tracked_by_runner": run.get("tracked_by_runner", True),
            "actions": ["cancel"] if run.get("tracked_by_runner", True) else [],
            **worker_caps,
        })

    return {
        "paused": agent_processes_paused,
        "agents_paused": agents_paused,
        "agent_processes_paused": agent_processes_paused,
        "background_processes_stopped": background_stopped,
        "backend": backend,
        "runner_reachable": runner_reachable,
        "processes": processes,
        "running": agent_runs,
        "chat": runner_snapshot.get("chat") or [],
        "runner_work": runner_work,
        "merger": merger,
        "governance": governance,
        "target_app_rebuild": target_app_rebuild,
        "target_app": target_app,
        "resource_caps": resource_caps,
    }


def active_agent_run_snapshots(
    conn: sqlite3.Connection,
    running_snapshot: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    active_node = project_state.active_node_id()
    active_runs = running_snapshot_for_node(conn, active_node, running_snapshot)
    seen = {
        (str(run.get("gap_id") or ""), str(run.get("kind") or "implementation"))
        for run in active_runs
        if run.get("gap_id")
    }
    active_runs.extend(sqlite_running_agent_snapshots(conn, active_node, seen))
    return active_runs


def running_snapshot_for_node(
    conn: sqlite3.Connection,
    active_node: str,
    running_snapshot: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    scoped: list[dict[str, Any]] = []
    unknown_node: list[dict[str, Any]] = []
    for run in running_snapshot:
        node_id = str(run.get("node_id") or "")
        if node_id:
            if node_owner(node_id) == active_node:
                scoped.append(run)
            continue
        if run.get("gap_id"):
            unknown_node.append(run)
    if not unknown_node:
        return scoped
    gap_ids = [str(run["gap_id"]) for run in unknown_node if run.get("gap_id")]
    placeholders = ",".join("?" * len(gap_ids))
    rows = conn.execute(
        f"SELECT id, node_id FROM gaps_index WHERE id IN ({placeholders})",
        gap_ids,
    ).fetchall()
    node_by_gap = {str(row["id"]): node_owner(row["node_id"]) for row in rows}
    scoped.extend(
        run for run in unknown_node
        if node_by_gap.get(str(run.get("gap_id") or "")) == active_node
    )
    return scoped


def sqlite_running_agent_snapshots(
    conn: sqlite3.Connection,
    active_node: str,
    seen: set[tuple[str, str]],
) -> list[dict[str, Any]]:
    rows = conn.execute(
        "SELECT r.gap_id, r.round_idx, r.started_at, r.last_output_at, "
        "r.pid, r.kind, g.node_id "
        "FROM runs r "
        "JOIN gaps_index g ON g.id = r.gap_id "
        "WHERE r.status = 'running' AND r.finished_at IS NULL "
        "ORDER BY r.started_at ASC"
    ).fetchall()
    out: list[dict[str, Any]] = []
    for row in rows:
        if node_owner(row["node_id"]) != active_node:
            continue
        gap_id = str(row["gap_id"] or "")
        run_kind = str(row["kind"] or "implementation")
        if (gap_id, run_kind) in seen:
            continue
        pid = int_or_none(row["pid"])
        if pid is None or not pid_may_be_alive(pid):
            continue
        seen.add((gap_id, run_kind))
        out.append({
            "gap_id": gap_id,
            "node_id": node_owner(row["node_id"]),
            "round_idx": row["round_idx"],
            "pid": pid,
            "kind": run_kind,
            "elapsed_seconds": elapsed_since(row["started_at"]),
            "idle_seconds": elapsed_since(row["last_output_at"]),
            "tracked_by_runner": False,
        })
    return out


def runner_work_summary(
    merger: dict | None,
    governance_state: dict | None,
    target_app_rebuild: dict | None,
    *,
    paused: bool = False,
    active_background_job: ActiveBackgroundJob | None = None,
) -> list[dict[str, Any]]:
    merger = merger or {}
    governance_state = governance_state or {}
    target_app_rebuild = target_app_rebuild or {}
    target_status = "idle"
    if target_app_rebuild.get("running"):
        target_status = "running"
    elif target_app_rebuild.get("queued"):
        target_status = "queued"
    rows = [
        {
            "id": "merger",
            "kind": "merger",
            "label": "Merger",
            "status": merger.get("state") or "unknown",
            "gap_id": merger.get("gap_id"),
            "elapsed_seconds": merger.get("elapsed_seconds") or 0,
            "queued": merger.get("queued") or 0,
            "last_outcome": merger.get("last_outcome") or "",
            "details": "Merges ready Gap work into the target branch.",
        },
        {
            "id": "governance",
            "kind": "governance",
            "label": "Governance",
            "status": governance_state.get("state") or "unknown",
            "gap_id": governance_state.get("gap_id"),
            "elapsed_seconds": governance_state.get("elapsed_seconds") or 0,
            "queued": governance_state.get("queued") or 0,
            "last_outcome": governance_state.get("last_outcome") or "",
            "details": (
                "Reviews Gaps against configured governance rules."
                if governance_state.get("configured")
                else "Idle until governance rules are configured."
            ),
        },
        {
            "id": "target-app-rebuilder",
            "kind": "target_app_rebuilder",
            "label": "Target-app rebuilder",
            "status": target_status if target_app_rebuild else "unknown",
            "queued": 1 if target_app_rebuild.get("queued") else 0,
            "details": (
                target_app_rebuild.get("last_reason")
                or "Rebuilds the target application after merged work."
            ),
        },
        static_worker_row(
            "target-app-config-generator",
            "target_app_config_generator",
            "Target-app config generator",
            "Uses the AI provider to draft target-app commands from the codebase.",
        ),
        background_worker_row(
            "sqlite-cache-rebuilder",
            "sqlite_cache_rebuild",
            "SQLite cache rebuilder",
            "Rebuilds index.sqlite from canonical .refine JSON.",
            active_background_job=active_background_job,
        ),
        static_worker_row(
            "activity-log-cleanup",
            "activity_log_cleanup",
            "Activity log cleanup",
            "Deletes activity log entries older than the selected retention window.",
        ),
        background_worker_row(
            "import-preparer",
            "import_prepare",
            "Import preparer",
            "Parses and deduplicates imported Gap drafts before review.",
            active_background_job=active_background_job,
        ),
        background_worker_row(
            "import-persister",
            "import_persist",
            "Import persister",
            "Persists large imported Gap batches in the background.",
            active_background_job=active_background_job,
        ),
        background_worker_row(
            "bulk-gap-updater",
            "bulk_update_gaps",
            "Bulk Gap updater",
            "Applies large bulk Gap updates in the background.",
            active_background_job=active_background_job,
        ),
        background_worker_row(
            "bulk-gap-deleter",
            "bulk_delete_gaps",
            "Bulk Gap deleter",
            "Deletes large selected Gap batches in the background.",
            active_background_job=active_background_job,
        ),
    ]
    if paused:
        rows = [paused_runner_worker(row) for row in rows]
    return rows


def paused_runner_worker(row: dict[str, Any]) -> dict[str, Any]:
    paused = dict(row)
    paused["status"] = "paused"
    paused["queued"] = 0
    paused["paused"] = True
    return paused


def static_worker_row(
    worker_id: str,
    kind: str,
    label: str,
    details: str,
) -> dict[str, Any]:
    return {
        "id": worker_id,
        "kind": kind,
        "label": label,
        "status": "idle",
        "gap_id": None,
        "elapsed_seconds": 0,
        "queued": 0,
        "details": details,
    }


def background_worker_row(
    worker_id: str,
    job_kind: str,
    label: str,
    details: str,
    *,
    active_background_job: ActiveBackgroundJob | None = None,
) -> dict[str, Any]:
    job = active_background_job(job_kind) if active_background_job is not None else None
    if not job:
        return static_worker_row(worker_id, job_kind, label, details)
    progress = job.get("progress") or {}
    message = progress.get("message") or job.get("label") or details
    return {
        "id": worker_id,
        "kind": job_kind,
        "label": label,
        "status": job.get("status") or "idle",
        "gap_id": None,
        "elapsed_seconds": elapsed_since(job.get("started_at")),
        "queued": 1 if job.get("status") == "queued" else 0,
        "details": message,
        "job_id": job.get("id"),
        "progress": progress,
    }


def process_resource_caps(settings: dict[str, str]) -> dict[str, Any]:
    resource_settings = runtime_resources.ResourceSettings.from_settings(settings)
    worker_memory_mb = runtime_resources.memory_limit_mb(resource_settings, "agent")
    ui_memory_mb = runtime_resources.memory_limit_mb(resource_settings, "ui")
    worker_cpu_weight = runtime_resources.cpu_weight(resource_settings, "agent")
    return {
        "worker_cpu_priority": {
            "label": cpu_priority_label(
                resource_settings.worker_cpu_priority,
                worker_cpu_weight,
            ),
            "weight": worker_cpu_weight,
            "priority": resource_settings.worker_cpu_priority,
        },
        "ui_cpu_priority": {
            "label": "normal (weight 100)",
            "weight": 100,
            "priority": "normal",
        },
        "worker_max_memory": {
            "label": memory_limit_label(worker_memory_mb),
            "mb": worker_memory_mb,
            "configured_mb": resource_settings.worker_memory_limit_mb,
        },
        "ui_max_memory": {
            "label": memory_limit_label(ui_memory_mb),
            "mb": ui_memory_mb,
            "configured_mb": resource_settings.ui_memory_limit_mb,
        },
    }


def int_or_none(value: Any) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def pid_may_be_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return False


def elapsed_since(value: Any) -> int:
    text = str(value or "").strip()
    if not text:
        return 0
    try:
        started = datetime.fromisoformat(text.replace("Z", "+00:00"))
    except ValueError:
        return 0
    now = datetime.now(started.tzinfo or timezone.utc)
    return max(0, int((now - started).total_seconds()))


def memory_limit_label(memory_mb: int) -> str:
    return f"{memory_mb} MB" if memory_mb else "uncapped"


def cpu_priority_label(priority: str, weight: int) -> str:
    label = priority.replace("_", " ")
    return f"{label} (weight {weight})"


def node_owner(node_id: str | None) -> str:
    return str(node_id or project_state.DEFAULT_NODE_ID)


def error(code: int, message: str) -> tuple[int, dict[str, Any]]:
    return code, {"error": {"message": message}}


def _summary_payload(process_summary: ProcessSummary | None) -> dict[str, Any]:
    if process_summary is None:
        return {}
    status, summary = process_summary()
    return summary if status == 200 else {}


def _truthy(value: Any) -> bool:
    return str(value).strip().lower() in {"1", "true", "yes", "on"}
