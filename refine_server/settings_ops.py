"""Shared project settings operations."""
from __future__ import annotations

import json
import sqlite3
from collections.abc import Callable
from typing import Any
from urllib.parse import urlparse

from refine_runtime import resources as runtime_resources

from . import activity, db, quality, target_app_ops
from .backend_protocol import (
    M_BACKGROUND_PROCESSES_SET,
    M_ENFORCE_SCHEDULING,
    M_TARGET_APP_REBUILD_PENDING,
)


RunnerCall = Callable[[str, dict[str, object], float], dict[str, Any]]
CancelActiveJobs = Callable[[], list[dict[str, Any]]]


ALLOWED_SETTINGS = {
    "parallel_run_cap", "branch_name_pattern",
    "agent_idle_timeout_seconds", "agent_hard_cap_seconds",
    "agent_limit_pause_seconds",
    "worker_memory_limit_mb", "ui_memory_limit_mb",
    "worker_cpu_priority", "resource_isolation_mode",
    "chat_idle_timeout_seconds",
    "backlog_promote_after_seconds",
    "project_update_pulse_interval_seconds",
    "file_browser_ignore_patterns",
    "agent_subpath", "merge_target_branch",
    "quality_enabled",
    "quality_timing",
    "quality_regressions_enabled",
    "agent_cli",
    "paused",
    "target_app_start_instructions",
    "target_app_stop_instructions",
    "target_app_health_url",
    "target_app_url",
    "target_app_start_command",
    "target_app_stop_command",
    "target_app_rebuild_command",
    "target_app_status_command",
    "target_app_cwd",
    "target_app_env_json",
    "target_app_start_timeout_seconds",
    "target_app_stop_timeout_seconds",
    "target_app_rebuild_timeout_seconds",
    "target_app_status_timeout_seconds",
    "target_app_log_path",
    "target_app_http_check_url",
    "target_app_tcp_check_host",
    "target_app_tcp_check_port",
    "target_app_process_check_command",
    "target_app_auto_rebuild",
}
VALID_AGENT_CLIS = ("claude", "codex", "gemini", "copilot", "smoke-ai")


def list_settings(conn: sqlite3.Connection) -> dict[str, dict[str, str]]:
    target_app_ops.cleanup_legacy_settings(conn)
    return {"settings": db.list_settings(conn)}


def update_settings(
    conn: sqlite3.Connection,
    body: dict[str, Any],
    *,
    runner_call: RunnerCall | None = None,
    cancel_active_jobs: CancelActiveJobs | None = None,
) -> tuple[int, dict[str, Any]]:
    normalized = normalize_settings(body)
    for key, value in normalized.items():
        db.set_setting(conn, key, value)
    target_app_ops.cleanup_legacy_settings(conn)
    activity.append(
        conn,
        message=f"Settings updated: {', '.join(normalized.keys())}",
        severity="info",
        category="user",
        actor="refine",
    )
    if "paused" in normalized:
        if runner_call is None:
            raise RuntimeError("settings update requires a backend runner")
        stopped = normalized.get("paused") == "1"
        if stopped and cancel_active_jobs is not None:
            cancel_active_jobs()
        result = runner_call(
            M_BACKGROUND_PROCESSES_SET,
            {"stopped": stopped, "settle_timeout_seconds": 8.0},
            30.0 if stopped else 10.0,
        )
        if stopped and not result.get("ok", True):
            cleanup = result.get("cleanup") or {}
            return error(
                409,
                cleanup.get("message") or (
                    "background processes stopped but target worktree "
                    "cleanup did not complete"
                ),
            )
    if runner_call is not None and (
        "quality_enabled" in normalized or "quality_timing" in normalized
    ):
        try:
            runner_call(M_ENFORCE_SCHEDULING, {}, 10.0)
        except Exception:
            pass
    if runner_call is not None and (
        normalized.get("target_app_auto_rebuild") == "on_worktree_merge"
        or "target_app_rebuild_command" in normalized
    ):
        try:
            runner_call(M_TARGET_APP_REBUILD_PENDING, {}, 10.0)
        except Exception:
            pass
    return 200, {"ok": True}


def normalize_settings(body: dict[str, Any]) -> dict[str, str]:
    if not isinstance(body, dict) or not body:
        raise ValueError("expected an object of {key: value}")
    normalized: dict[str, str] = {}
    for key, value in body.items():
        if key not in ALLOWED_SETTINGS:
            raise ValueError(f"unknown setting: {key}")
        normalized[key] = _normalize_setting(key, value)
    return normalized


def error(code: int, message: str) -> tuple[int, dict[str, Any]]:
    return code, {"error": {"message": message}}


def _normalize_setting(key: str, value: Any) -> str:
    if key == "merge_target_branch":
        branch = str(value or "").strip()
        if branch:
            if any(c.isspace() for c in branch):
                raise ValueError("merge_target_branch may not contain whitespace")
            if branch.startswith("-") or "\0" in branch:
                raise ValueError("merge_target_branch contains an invalid character")
        return branch
    if key == "agent_subpath":
        subpath = str(value or "").strip()
        if subpath:
            if subpath.startswith("/") or subpath.startswith("~"):
                raise ValueError("agent_subpath must be relative to the repo root")
            if "\0" in subpath:
                raise ValueError("agent_subpath contains an invalid character")
            parts = [part for part in subpath.replace("\\", "/").split("/") if part]
            if any(part == ".." for part in parts):
                raise ValueError("agent_subpath must not contain `..` components")
            subpath = "/".join(parts)
        return subpath
    if key == "agent_cli":
        choice = str(value or "").strip().lower()
        if choice not in VALID_AGENT_CLIS:
            raise ValueError(f"agent_cli must be one of {', '.join(VALID_AGENT_CLIS)}")
        return choice
    if key in {"quality_enabled", "quality_regressions_enabled"}:
        return "1" if str(value).strip().lower() in {"1", "true", "yes", "on"} else "0"
    if key == "quality_timing":
        choice = quality.normalize_timing(value)
        if str(value or "").strip() not in quality.QUALITY_TIMING_VALUES:
            raise ValueError("quality_timing must be one of pre_merge, post_rebuild")
        return choice
    if key == "parallel_run_cap":
        number = _int_setting(value, "parallel_run_cap")
        if number < 1 or number > 100:
            raise ValueError("parallel_run_cap must be between 1 and 100")
        return str(number)
    if key in {
        "worker_memory_limit_mb",
        "ui_memory_limit_mb",
        "worker_cpu_priority",
        "resource_isolation_mode",
    }:
        return runtime_resources.validate_setting(key, value)
    if key == "target_app_cwd":
        cwd = str(value or "").strip()
        if cwd and "\0" in cwd:
            raise ValueError("target_app_cwd contains an invalid character")
        if cwd.startswith("~"):
            raise ValueError("target_app_cwd must be absolute or relative to the repo root")
        if cwd and not cwd.startswith("/"):
            parts = [part for part in cwd.replace("\\", "/").split("/") if part]
            if any(part == ".." for part in parts):
                raise ValueError("target_app_cwd must not contain `..` components")
            cwd = "/".join(parts)
        return cwd
    if key == "target_app_env_json":
        raw = str(value or "{}").strip() or "{}"
        try:
            env_obj = json.loads(raw)
        except json.JSONDecodeError as e:
            raise ValueError("target_app_env_json must be a JSON object") from e
        if not isinstance(env_obj, dict):
            raise ValueError("target_app_env_json must be a JSON object")
        return json.dumps({str(env_key): str(env_value) for env_key, env_value in env_obj.items()})
    if key == "target_app_url":
        app_url = str(value or "").strip()
        if app_url:
            parsed = urlparse(app_url)
            if parsed.scheme not in {"http", "https"} or not parsed.netloc:
                raise ValueError("target_app_url must be an http:// or https:// URL")
        return app_url
    if key in {
        "target_app_start_timeout_seconds",
        "target_app_stop_timeout_seconds",
        "target_app_rebuild_timeout_seconds",
        "target_app_status_timeout_seconds",
    }:
        number = _int_setting(value, key)
        if number < 1 or number > 3600:
            raise ValueError(f"{key} must be between 1 and 3600")
        return str(number)
    if key == "target_app_auto_rebuild":
        choice = str(value or "").strip()
        allowed_modes = {"never", "on_worktree_merge", "hourly", "nightly"}
        if choice not in allowed_modes:
            raise ValueError(
                "target_app_auto_rebuild must be one of never, "
                "on_worktree_merge, hourly, nightly"
            )
        return choice
    if key == "target_app_tcp_check_port":
        port = str(value or "").strip()
        if port:
            try:
                number = int(port)
            except ValueError as e:
                raise ValueError("target_app_tcp_check_port must be an integer") from e
            if number < 1 or number > 65535:
                raise ValueError("target_app_tcp_check_port must be between 1 and 65535")
            port = str(number)
        return port
    if key == "backlog_promote_after_seconds":
        number = _int_setting(value, "backlog_promote_after_seconds")
        allowed_intervals = {-1, 0, 300, 1800, 3600, 10800, 21600, 86400}
        if number not in allowed_intervals:
            raise ValueError(
                "backlog_promote_after_seconds must be one of "
                "-1 (never), 0 (instant), 300, 1800, 3600, 10800, 21600, 86400"
            )
        return str(number)
    if key == "project_update_pulse_interval_seconds":
        number = _int_setting(value, "project_update_pulse_interval_seconds")
        allowed_intervals = {-1, 30, 60, 300, 900, 1800, 3600}
        if number not in allowed_intervals:
            raise ValueError(
                "project_update_pulse_interval_seconds must be one of "
                "-1 (never), 30, 60, 300, 900, 1800, 3600"
            )
        return str(number)
    if key == "file_browser_ignore_patterns":
        raw = str(value or "")
        if "\0" in raw:
            raise ValueError("file_browser_ignore_patterns contains an invalid character")
        patterns = [
            item.strip().replace("\\", "/").strip("/")
            for item in raw.split(",")
            if item.strip().strip("/")
        ]
        return ", ".join(patterns)
    if key == "agent_limit_pause_seconds":
        number = _int_setting(value, "agent_limit_pause_seconds")
        allowed_intervals = {30, 60, 3600, 10800}
        if number not in allowed_intervals:
            raise ValueError("agent_limit_pause_seconds must be one of 30, 60, 3600, 10800")
        return str(number)
    return str(value)


def _int_setting(value: Any, key: str) -> int:
    try:
        return int(value)
    except (TypeError, ValueError) as e:
        raise ValueError(f"{key} must be an integer") from e
