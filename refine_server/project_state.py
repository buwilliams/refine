"""Canonical per-application Refine state.

SQLite is a rebuildable cache. This module owns the JSON files under
``<app>/.refine`` that carry durable project, node, reporter, settings,
and Gap ownership state.
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import tempfile
from pathlib import Path
from typing import Any, Callable

from . import config
from .gaps import now_iso


CURRENT_SCHEMA_VERSION = 2
MIN_SUPPORTED_SCHEMA_VERSION = 1
DEFAULT_NODE_ID = "default"
CACHE_ACTIVE_NODE_KEY = "__refine_cache_active_node_id"
LEGACY_CACHE_ACTIVE_INSTANCE_KEY = "__refine_cache_active_instance_id"
CACHE_STATE_FINGERPRINT_KEY = "__refine_cache_state_fingerprint"
INSTANCE_TO_NODE_MIGRATION_ID = "instance_to_node_v2"
LEGACY_PROJECT_MIGRATION_ID = "legacy_project_to_json_v2"
MANUAL_SCHEMA_MIGRATION_INSTRUCTIONS = (
    "Manual cluster migration is required. Stop every old Refine node for this "
    "target app, run `refine migrate run` from one upgraded checkout, commit "
    "and push the migrated .refine state, then pull and start the upgraded "
    "nodes."
)

PROJECT_SETTING_KEYS = {
    "governance_product",
    "governance_constitution",
    "governance_rules_json",
    "quality_enabled",
    "quality_timing",
    "quality_regressions_enabled",
    "quality_business_requirements",
    "quality_instructions",
}

APPLICATION_SETTING_KEYS = {
    "agent_subpath",
    "merge_target_branch",
}

RUNTIME_SETTING_KEYS = {
    "parallel_run_cap",
    "branch_name_pattern",
    "agent_idle_timeout_seconds",
    "agent_hard_cap_seconds",
    "agent_limit_pause_seconds",
    "worker_memory_limit_mb",
    "ui_memory_limit_mb",
    "worker_cpu_priority",
    "resource_isolation_mode",
    "chat_idle_timeout_seconds",
    "backlog_promote_after_seconds",
    "project_update_pulse_interval_seconds",
    "file_browser_ignore_patterns",
    "agents_paused",
    "paused",
    "agent_cli",
}

TARGET_APP_CONFIG_SETTING_KEYS = {
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

TARGET_APP_RUNTIME_SETTING_KEYS = {
    "target_app_auto_rebuild_last_started_at",
    "target_app_auto_rebuild_last_finished_at",
    "target_app_auto_rebuild_last_ok",
    "target_app_auto_rebuild_last_message",
    "target_app_state",
    "target_app_last_check_at",
    "target_app_last_check_ok",
    "target_app_last_check_message",
    "target_app_last_health_at",
    "target_app_last_health_ok",
    "target_app_last_health_message",
    "target_app_last_operation_id",
    "target_app_last_error",
}

TARGET_APP_SETTING_KEYS = TARGET_APP_CONFIG_SETTING_KEYS | TARGET_APP_RUNTIME_SETTING_KEYS

APPLICATION_COPY_SETTING_KEYS = APPLICATION_SETTING_KEYS | TARGET_APP_CONFIG_SETTING_KEYS
RUNTIME_COPY_SETTING_KEYS = RUNTIME_SETTING_KEYS - {
    "agent_cli", "agents_paused", "paused",
}


def volume_root() -> Path:
    return config.get().volume_root


def config_json_path(root: Path | None = None) -> Path:
    return (root or volume_root()) / "config.json"


def nodes_json_path(root: Path | None = None) -> Path:
    return (root or volume_root()) / "nodes.json"


def legacy_instances_json_path(root: Path | None = None) -> Path:
    return (root or volume_root()) / "instances.json"


def guidance_json_path(root: Path | None = None) -> Path:
    return (root or volume_root()) / "guidance.json"


def cluster_json_path(root: Path | None = None) -> Path:
    return (root or volume_root()) / "cluster.json"


def maintenance_json_path(root: Path | None = None) -> Path:
    return (root or volume_root()) / "maintenance.json"


def nodes_dir(root: Path | None = None) -> Path:
    return (root or volume_root()) / "nodes"


def legacy_instances_dir(root: Path | None = None) -> Path:
    return (root or volume_root()) / "instances"


def run_dir(root: Path | None = None) -> Path:
    if root is not None:
        return config.local_run_dir(Path.cwd())
    return config.local_run_dir()


def legacy_run_dir(root: Path | None = None) -> Path:
    return (root or volume_root()) / "run"


def active_nodes_path(root: Path | None = None) -> Path:
    return run_dir(root) / "active-nodes.json"


def legacy_active_instances_path(root: Path | None = None) -> Path:
    return run_dir(root) / "active-instances.json"


def active_node_path(root: Path | None = None) -> Path:
    return legacy_run_dir(root) / "active-node.json"


def legacy_active_instance_path(root: Path | None = None) -> Path:
    return legacy_run_dir(root) / "active-instance.json"


def node_dir(node_id: str, root: Path | None = None) -> Path:
    return nodes_dir(root) / node_id


def schema_status(root: Path | None = None) -> dict[str, Any]:
    """Return schema compatibility for an application's .refine directory."""
    root = root or volume_root()
    cfg_path = config_json_path(root)
    if not cfg_path.exists():
        return _with_migration_metadata({
            "compatible": False,
            "migration_required": True,
            "schema_version": None,
            "current_schema_version": CURRENT_SCHEMA_VERSION,
            "reason": "legacy_project",
        })
    try:
        cfg = _read_json(cfg_path, {})
        version = int(cfg.get("schema_version") or 0)
    except Exception:
        return {
            "compatible": False,
            "migration_required": False,
            "schema_version": None,
            "current_schema_version": CURRENT_SCHEMA_VERSION,
            "reason": "invalid_config",
        }
    if version > CURRENT_SCHEMA_VERSION:
        return {
            "compatible": False,
            "migration_required": False,
            "schema_version": version,
            "current_schema_version": CURRENT_SCHEMA_VERSION,
            "reason": "newer_schema",
        }
    if version < MIN_SUPPORTED_SCHEMA_VERSION:
        return _with_migration_metadata({
            "compatible": False,
            "migration_required": True,
            "schema_version": version,
            "current_schema_version": CURRENT_SCHEMA_VERSION,
            "reason": "outdated_schema",
        })
    if version < CURRENT_SCHEMA_VERSION:
        return _with_migration_metadata({
            "compatible": False,
            "migration_required": True,
            "schema_version": version,
            "current_schema_version": CURRENT_SCHEMA_VERSION,
            "reason": "schema_upgrade",
        })
    return {
        "compatible": True,
        "migration_required": False,
        "schema_version": version,
        "current_schema_version": CURRENT_SCHEMA_VERSION,
        "reason": "",
    }


def _with_migration_metadata(status: dict[str, Any]) -> dict[str, Any]:
    reason = status.get("reason")
    schema_version = status.get("schema_version")
    if reason == "legacy_project":
        status.update({
            "migration_id": LEGACY_PROJECT_MIGRATION_ID,
            "migration_description": (
                "Create canonical .refine JSON state from an older SQLite-backed "
                "project."
            ),
            "safe_auto": True,
            "requires_cluster_quiescence": False,
            "operator_instructions": (
                "This initialization is safe to run automatically for empty or "
                "legacy single-node project state."
            ),
        })
    elif reason == "schema_upgrade" and schema_version == 1:
        status.update({
            "migration_id": INSTANCE_TO_NODE_MIGRATION_ID,
            "migration_description": (
                "Rename Refine instance state to node state for the distributed "
                "cluster model."
            ),
            "safe_auto": False,
            "requires_cluster_quiescence": True,
            "operator_instructions": MANUAL_SCHEMA_MIGRATION_INSTRUCTIONS,
        })
    else:
        status.update({
            "migration_id": "",
            "migration_description": "Unsupported schema migration.",
            "safe_auto": False,
            "requires_cluster_quiescence": True,
            "operator_instructions": (
                "Upgrade Refine incrementally or migrate this project with a "
                "version that supports its current schema."
            ),
        })
    return status


def migration_requires_manual(status: dict[str, Any] | None) -> bool:
    return bool(
        status
        and status.get("migration_required")
        and status.get("safe_auto") is False
    )


def migration_block_message(status: dict[str, Any] | None) -> str:
    if not status:
        return "Project schema migration required."
    migration_id = status.get("migration_id") or "unknown"
    return f"Project schema migration required: {migration_id}."


def migration_block_details(status: dict[str, Any] | None) -> str:
    if not status:
        return MANUAL_SCHEMA_MIGRATION_INSTRUCTIONS
    return str(status.get("operator_instructions") or MANUAL_SCHEMA_MIGRATION_INSTRUCTIONS)


def _require_current_schema(root: Path) -> None:
    status = schema_status(root)
    if not status.get("compatible"):
        raise RuntimeError(migration_block_details(status))


def read_maintenance(*, root: Path | None = None) -> dict[str, Any] | None:
    path = maintenance_json_path(root)
    if not path.exists():
        return None
    data = _read_json(path, {})
    return data if isinstance(data, dict) else {"reason": "maintenance"}


def write_maintenance(data: dict[str, Any], *, root: Path | None = None) -> dict[str, Any]:
    payload = {
        "active": True,
        "created_at": now_iso(),
        **data,
        "updated_at": now_iso(),
    }
    _write_json(maintenance_json_path(root), payload)
    return payload


def clear_maintenance(*, root: Path | None = None) -> None:
    try:
        maintenance_json_path(root).unlink()
    except FileNotFoundError:
        pass


def empty_refine_state(root: Path | None = None) -> bool:
    root = root or volume_root()
    gaps = root / "gaps"
    has_gaps = gaps.exists() and any(gaps.glob("**/gap.json"))
    return not has_gaps and not (root / "index.sqlite").exists()


def ensure_initialized(conn: sqlite3.Connection | None = None, *,
                       migrate: bool = True,
                       allow_manual_migrations: bool = False,
                       root: Path | None = None) -> dict[str, Any]:
    """Ensure canonical JSON exists and is compatible for the active app."""
    root = root or volume_root()
    root.mkdir(parents=True, exist_ok=True)
    (root / "gaps").mkdir(exist_ok=True)
    config.ensure_refine_gitignore(root)
    config.ensure_runtime_gitignore(root.parent)
    status = schema_status(root)
    if status["compatible"]:
        ensure_default_node(root=root)
        ensure_active_node(root=root)
        ensure_guidance_file(root=root)
        ensure_project_quality_settings(conn, root=root)
        ensure_active_node_runtime_settings(conn, root=root)
        return status
    if not migrate or not status.get("migration_required"):
        return status
    if migration_requires_manual(status) and not allow_manual_migrations:
        return status
    if status.get("reason") == "legacy_project":
        migrate_legacy(conn, root=root)
    elif status.get("migration_id") == INSTANCE_TO_NODE_MIGRATION_ID:
        migrate_project_state(root=root)
    else:
        return status
    return schema_status(root)


def migrate_project_state(*, root: Path | None = None) -> None:
    """Upgrade v1 project state from instance naming to node naming."""
    root = root or volume_root()
    _rename_legacy_node_paths(root)
    _rewrite_nodes_registry(root)
    _assert_legacy_node_paths_removed(root)
    _rewrite_gap_node_ownership(root)
    cfg = read_project_config(root=root)
    cfg["schema_version"] = CURRENT_SCHEMA_VERSION
    cfg.setdefault("refine", {})["version"] = _refine_version()
    write_project_config(cfg, root=root)
    ensure_default_node(root=root)
    ensure_active_node(root=root)


def _rename_legacy_node_paths(root: Path) -> None:
    old_registry = legacy_instances_json_path(root)
    new_registry = nodes_json_path(root)
    if old_registry.exists() and not new_registry.exists():
        _git_mv_or_rename(root, old_registry, new_registry)
    old_dir = legacy_instances_dir(root)
    new_dir = nodes_dir(root)
    if old_dir.exists() and not new_dir.exists():
        _git_mv_or_rename(root, old_dir, new_dir)
    elif old_dir.exists() and new_dir.exists() and not any(new_dir.iterdir()):
        new_dir.rmdir()
        _git_mv_or_rename(root, old_dir, new_dir)
    elif old_dir.exists() and new_dir.exists():
        for src in sorted(p for p in old_dir.iterdir() if p.is_dir()):
            dst = new_dir / src.name
            if not dst.exists():
                _git_mv_or_rename(root, src, dst)
            elif _node_dir_can_be_replaced(dst):
                shutil.rmtree(dst)
                src.rename(dst)
        try:
            old_dir.rmdir()
        except OSError:
            pass
    new_dir.mkdir(parents=True, exist_ok=True)


def _git_mv_or_rename(root: Path, src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    repo = root.parent
    rel_src = src.relative_to(repo).as_posix()
    rel_dst = dst.relative_to(repo).as_posix()
    tracked = subprocess.run(
        ["git", "ls-files", "--error-unmatch", rel_src],
        cwd=repo,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if tracked.returncode == 0:
        moved = subprocess.run(
            ["git", "mv", rel_src, rel_dst],
            cwd=repo,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        if moved.returncode == 0:
            return
    src.rename(dst)


def _rewrite_nodes_registry(root: Path) -> None:
    path = nodes_json_path(root)
    data = _read_json(path, {"nodes": []})
    nodes = _registry_entries(data)
    legacy_path = legacy_instances_json_path(root)
    legacy_data = _read_json(legacy_path, {}) if legacy_path.exists() else {}
    legacy_nodes = _registry_entries(legacy_data)
    if legacy_nodes:
        merged: dict[str, dict[str, Any]] = {}
        order: list[str] = []
        for entry in nodes:
            node_id = str(entry.get("id") or "")
            if not node_id:
                continue
            merged[node_id] = entry
            order.append(node_id)
        for entry in legacy_nodes:
            node_id = str(entry.get("id") or "")
            if not node_id:
                continue
            if node_id not in merged:
                order.append(node_id)
            merged[node_id] = entry
        data["nodes"] = [merged[node_id] for node_id in order if node_id in merged]
    elif "nodes" not in data and isinstance(data.get("instances"), list):
        data["nodes"] = data.get("instances") or []
    data.pop("instances", None)
    if not isinstance(data.get("nodes"), list):
        data["nodes"] = []
    _write_json(path, data)
    if legacy_path.exists():
        legacy_path.unlink()


def _registry_entries(data: Any) -> list[dict[str, Any]]:
    if not isinstance(data, dict):
        return []
    raw = data.get("nodes")
    if raw is None:
        raw = data.get("instances")
    if not isinstance(raw, list):
        return []
    return [entry for entry in raw if isinstance(entry, dict)]


def _node_dir_can_be_replaced(path: Path) -> bool:
    if not path.exists():
        return True
    if not path.is_dir():
        return False
    from . import db

    expected = {
        "application.json": APPLICATION_SETTING_KEYS,
        "runtime.json": RUNTIME_SETTING_KEYS,
        "target-app.json": TARGET_APP_CONFIG_SETTING_KEYS,
    }
    metadata = {"created_at", "updated_at", "schema_version", "refine"}
    allowed_files = set(expected) | {"reporters.json"}
    if any(child.is_dir() or child.name not in allowed_files for child in path.iterdir()):
        return False
    for name, keys in expected.items():
        child = path / name
        if not child.exists():
            continue
        data = _read_json(child, None)
        if not isinstance(data, dict):
            return False
        for key, value in data.items():
            key = str(key)
            if key in metadata:
                continue
            if key not in keys:
                return False
            if str(value) != str(db.DEFAULT_SETTINGS.get(key, "")):
                return False
    reporters = path / "reporters.json"
    if reporters.exists():
        data = _read_json(reporters, None)
        if not isinstance(data, dict):
            return False
        reporters_list = data.get("reporters") or []
        if reporters_list:
            return False
        if any(str(key) not in {"reporters", *metadata} for key in data):
            return False
    return True


def _assert_legacy_node_paths_removed(root: Path) -> None:
    legacy_registry = legacy_instances_json_path(root)
    legacy_dir = legacy_instances_dir(root)
    if legacy_dir.exists():
        try:
            legacy_dir.rmdir()
        except OSError:
            pass
    remaining = [p for p in (legacy_registry, legacy_dir) if p.exists()]
    if remaining:
        rels = ", ".join(p.relative_to(root).as_posix() for p in remaining)
        raise RuntimeError(
            "Instance-to-node migration did not finish; legacy state remains: "
            f"{rels}"
        )


def _rewrite_gap_node_ownership(root: Path) -> None:
    for path in sorted((root / "gaps").glob("**/gap.json")):
        gap = _read_json(path, {})
        if not isinstance(gap, dict) or not gap.get("id"):
            continue
        changed = False
        if "node_id" not in gap and "instance_id" in gap:
            gap["node_id"] = gap.get("instance_id") or DEFAULT_NODE_ID
            changed = True
        elif "node_id" not in gap:
            gap["node_id"] = DEFAULT_NODE_ID
            changed = True
        if "instance_id" in gap:
            gap.pop("instance_id", None)
            changed = True
        if changed:
            gap["updated"] = gap.get("updated") or now_iso()
            _write_json(path, gap)


def migrate_legacy(conn: sqlite3.Connection | None = None, *,
                   root: Path | None = None) -> None:
    """Create v1 JSON files from an existing SQLite-backed project."""
    root = root or volume_root()
    root.mkdir(parents=True, exist_ok=True)
    (root / "gaps").mkdir(exist_ok=True)
    nodes_dir(root).mkdir(exist_ok=True)
    config.ensure_refine_gitignore(root)

    from . import db

    close_conn = False
    if conn is None:
        conn = db.connect(root / "index.sqlite")
        close_conn = True
    try:
        legacy_settings = dict(db.DEFAULT_SETTINGS)
        try:
            legacy_settings.update(db.list_settings(conn))
        except sqlite3.Error:
            pass
        reporters = _legacy_reporters(conn)
        gap_rows = _legacy_gap_rows(conn)
    finally:
        if close_conn:
            conn.close()

    _write_json(config_json_path(root), {
        "schema_version": CURRENT_SCHEMA_VERSION,
        "refine": {"version": _refine_version()},
        "created_at": now_iso(),
        "updated_at": now_iso(),
        "settings": {
            k: v for k, v in legacy_settings.items()
            if k in PROJECT_SETTING_KEYS
        },
    })
    _write_json(nodes_json_path(root), {
        "nodes": [{
            "id": DEFAULT_NODE_ID,
            "display_name": "Default",
            "created_at": now_iso(),
            "updated_at": now_iso(),
            "archived": False,
        }],
    })
    _write_json(guidance_json_path(root), {
        "guidance": [],
        "updated_at": now_iso(),
    })
    _write_node_files(
        DEFAULT_NODE_ID,
        settings=legacy_settings,
        reporters=reporters,
        root=root,
    )
    set_active_node(DEFAULT_NODE_ID, root=root)
    _migrate_gap_files(gap_rows, root=root)


def _legacy_reporters(conn: sqlite3.Connection) -> list[dict[str, Any]]:
    try:
        return [
            {"id": int(r["id"]), "name": r["name"], "created": r["created"]}
            for r in conn.execute(
                "SELECT id, name, created FROM reporters ORDER BY id"
            )
        ]
    except sqlite3.Error:
        return []


def _legacy_gap_rows(conn: sqlite3.Connection) -> dict[str, dict[str, Any]]:
    try:
        return {
            r["id"]: dict(r)
            for r in conn.execute(
                "SELECT id, name, status, priority, reporter, created, updated, "
                "branch_name, json_path FROM gaps_index"
            )
        }
    except sqlite3.Error:
        return {}


def _migrate_gap_files(rows: dict[str, dict[str, Any]], *, root: Path) -> None:
    for path in sorted((root / "gaps").glob("**/gap.json")):
        gap = _read_json(path, {})
        if not isinstance(gap, dict) or not gap.get("id"):
            continue
        row = rows.get(str(gap["id"]))
        changed = False
        defaults = {
            "status": row.get("status") if row else "backlog",
            "priority": row.get("priority") if row else "low",
            "branch_name": row.get("branch_name") if row else None,
            "node_id": DEFAULT_NODE_ID,
        }
        if "node_id" not in gap and "instance_id" in gap:
            defaults["node_id"] = gap.get("instance_id") or DEFAULT_NODE_ID
        for key, value in defaults.items():
            if key not in gap:
                gap[key] = value
                changed = True
        if "instance_id" in gap:
            gap.pop("instance_id", None)
            changed = True
        if changed:
            gap["updated"] = gap.get("updated") or now_iso()
            _write_json(path, gap)


def ensure_default_node(*, root: Path | None = None) -> dict[str, Any]:
    root = root or volume_root()
    _require_current_schema(root)
    registry = read_nodes(root=root)
    entries = registry.get("nodes") or []
    if not entries:
        entries = [{
            "id": DEFAULT_NODE_ID,
            "display_name": "Default",
            "created_at": now_iso(),
            "updated_at": now_iso(),
            "archived": False,
        }]
        registry["nodes"] = entries
        _write_json(nodes_json_path(root), registry)
    for entry in entries:
        if entry.get("id"):
            _ensure_node_files(str(entry["id"]), root=root)
    return entries[0]


def read_project_config(*, root: Path | None = None) -> dict[str, Any]:
    return _read_json(config_json_path(root), {})


def write_project_config(data: dict[str, Any], *, root: Path | None = None) -> None:
    data["updated_at"] = now_iso()
    _write_json(config_json_path(root), data)


def read_nodes(*, root: Path | None = None) -> dict[str, Any]:
    root = root or volume_root()
    data = _read_json(nodes_json_path(root), {"nodes": []})
    if not isinstance(data.get("nodes"), list):
        data["nodes"] = []
    return data


def write_nodes(data: dict[str, Any], *, root: Path | None = None) -> None:
    root = root or volume_root()
    _require_current_schema(root)
    _write_json(nodes_json_path(root), data)


def ensure_guidance_file(*, root: Path | None = None) -> None:
    root = root or volume_root()
    _require_current_schema(root)
    if not guidance_json_path(root).exists():
        _write_json(guidance_json_path(root), {
            "guidance": [],
            "updated_at": now_iso(),
        })


def ensure_project_quality_settings(
    conn: sqlite3.Connection | None = None,
    *,
    root: Path | None = None,
) -> None:
    """Lift legacy per-node quality flags into project settings."""
    from . import db

    root = root or volume_root()
    cfg = read_project_config(root=root)
    settings = cfg.setdefault("settings", {})
    changed = False
    for key in ("quality_timing",):
        if key in settings:
            continue
        value = db.DEFAULT_SETTINGS.get(key, "")
        if conn is not None:
            try:
                value = db.get_setting(conn, key, value) or value
            except sqlite3.Error:
                pass
        settings[key] = str(value)
        changed = True
    for key in ("quality_enabled", "quality_regressions_enabled"):
        if key in settings:
            continue
        for entry in list_nodes(root=root):
            node_id = str(entry.get("id") or "")
            if not node_id:
                continue
            values = _read_json(node_dir(node_id, root) / "application.json", {})
            if key in values:
                settings[key] = str(values.get(key) or "0")
                changed = True
                break
    if changed:
        write_project_config(cfg, root=root)


def ensure_active_node_runtime_settings(
    conn: sqlite3.Connection | None = None,
    *,
    root: Path | None = None,
) -> None:
    """Backfill runtime settings added after canonical JSON migration."""
    from . import db

    root = root or volume_root()
    active = active_node_id(root=root)
    runtime_path = node_dir(active, root) / "runtime.json"
    data = _read_json(runtime_path, {})
    changed = False
    cached_active = _cached_active_node_id(conn) if conn is not None else ""
    for key in ("backlog_promote_after_seconds",):
        if key in data:
            continue
        value = db.DEFAULT_SETTINGS.get(key, "")
        if conn is not None and cached_active in {"", active}:
            try:
                value = db.get_setting(conn, key, value) or value
            except sqlite3.Error:
                pass
        data[key] = str(value)
        changed = True
    if changed:
        _write_json(runtime_path, data)


def list_nodes(*, root: Path | None = None) -> list[dict[str, Any]]:
    return list(read_nodes(root=root).get("nodes") or [])


def node_by_id(node_id: str, *, root: Path | None = None) -> dict[str, Any] | None:
    for entry in list_nodes(root=root):
        if entry.get("id") == node_id:
            return entry
    return None


def ensure_active_node(*, root: Path | None = None) -> str:
    root = root or volume_root()
    _require_current_schema(root)
    registry = read_nodes(root=root)
    entries = registry.get("nodes") or []
    active, legacy = _read_active_node_selection(root)
    if active and any(e.get("id") == active and not e.get("archived") for e in entries):
        _ensure_node_files(str(active), root=root)
        if legacy:
            _write_active_node_selection(root, str(active))
        _cleanup_legacy_run_state(root)
        return str(active)
    fallback = next((e for e in entries if not e.get("archived")), None)
    if fallback is None:
        fallback = ensure_default_node(root=root)
    active_id = str(fallback["id"])
    set_active_node(active_id, root=root)
    return active_id


def active_node_id(*, root: Path | None = None) -> str:
    return ensure_active_node(root=root)


def local_node_id(*, root: Path | None = None) -> str:
    """Stable node identity for this local supervisor process.

    UI state may browse or activate another node, but runtime automation owns
    exactly one local node. Supervisor workers receive this value in the
    environment at launch and freeze it for their lifetime.
    """
    root = root or volume_root()
    env_node = os.environ.get(config.ENV_LOCAL_NODE_ID, "").strip()
    if env_node:
        entry = node_by_id(env_node, root=root)
        if entry is not None and not entry.get("archived"):
            return env_node
    return active_node_id(root=root)


def _active_node_selection_key(root: Path) -> str:
    base = str(root.resolve())
    scope = config.runtime_scope()
    return f"{base}#scope={scope}" if scope else base


def _legacy_active_node_selection_key(root: Path) -> str:
    return str(root.resolve())


def _read_active_node_selection(root: Path) -> tuple[str | None, bool]:
    key = _active_node_selection_key(root)
    data = _read_json(active_nodes_path(root), {"selections": {}})
    selections = data.get("selections") or {}
    selection = selections.get(key) or {}
    active = selection.get("active_node_id")
    if active:
        return str(active), False
    legacy_selection = selections.get(_legacy_active_node_selection_key(root)) or {}
    active = legacy_selection.get("active_node_id")
    if active:
        return str(active), False
    legacy_data = _read_json(legacy_active_instances_path(root), {"selections": {}})
    legacy_selections = legacy_data.get("selections") or {}
    legacy_selection = legacy_selections.get(key) or {}
    active = legacy_selection.get("active_instance_id")
    if active:
        return str(active), True
    legacy_selection = legacy_selections.get(_legacy_active_node_selection_key(root)) or {}
    active = legacy_selection.get("active_instance_id")
    if active:
        return str(active), True
    legacy = _read_json(active_node_path(root), {}).get("active_node_id")
    if legacy:
        return str(legacy), True
    legacy = _read_json(legacy_active_instance_path(root), {}).get("active_instance_id")
    if legacy:
        return str(legacy), True
    return None, False


def _write_active_node_selection(root: Path, node_id: str) -> None:
    path = active_nodes_path(root)
    data = _read_json(path, {"selections": {}})
    selections = data.setdefault("selections", {})
    selections[_active_node_selection_key(root)] = {
        "active_node_id": node_id,
        "volume_root": str(root.resolve()),
        "updated_at": now_iso(),
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    _write_json(path, data)


def set_active_node(node_id: str, *, root: Path | None = None) -> None:
    root = root or volume_root()
    _require_current_schema(root)
    entry = node_by_id(node_id, root=root)
    if entry is None:
        raise ValueError(f"unknown node_id: {node_id}")
    if entry.get("archived"):
        raise ValueError(f"archived node cannot be activated: {node_id}")
    _write_active_node_selection(root, node_id)
    _ensure_node_files(node_id, root=root)


def create_node(display_name: str, *, node_id: str | None = None,
                root: Path | None = None) -> dict[str, Any]:
    root = root or volume_root()
    _require_current_schema(root)
    name = display_name.strip() or "New node"
    node_id = node_id or _slug_node_id(name)
    existing = {str(e.get("id")) for e in list_nodes(root=root)}
    base = node_id
    i = 2
    while node_id in existing:
        node_id = f"{base}-{i}"
        i += 1
    entry = {
        "id": node_id,
        "display_name": name,
        "created_at": now_iso(),
        "updated_at": now_iso(),
        "archived": False,
    }
    registry = read_nodes(root=root)
    registry.setdefault("nodes", []).append(entry)
    write_nodes(registry, root=root)
    _ensure_node_files(node_id, root=root)
    return entry


def update_node(node_id: str, *, display_name: str | None = None,
                    archived: bool | None = None,
                    root: Path | None = None) -> dict[str, Any]:
    root = root or volume_root()
    _require_current_schema(root)
    registry = read_nodes(root=root)
    for entry in registry.get("nodes") or []:
        if entry.get("id") != node_id:
            continue
        if display_name is not None:
            name = display_name.strip()
            if not name:
                raise ValueError("display_name is required")
            entry["display_name"] = name
        if archived is not None:
            entry["archived"] = bool(archived)
        entry["updated_at"] = now_iso()
        write_nodes(registry, root=root)
        if archived and active_node_id(root=root) == node_id:
            ensure_active_node(root=root)
        return entry
    raise ValueError(f"unknown node_id: {node_id}")


def list_settings(*, node_id: str | None = None) -> dict[str, str]:
    from . import db

    root = volume_root()
    ensure_initialized(migrate=True)
    selected = node_id or active_node_id(root=root)
    return node_settings(selected, root=root)


def node_settings(node_id: str, *, root: Path | None = None) -> dict[str, str]:
    from . import db

    root = root or volume_root()
    ensure_initialized(migrate=True)
    settings = dict(db.DEFAULT_SETTINGS)
    cfg = read_project_config(root=root)
    settings.update(_string_map(cfg.get("settings") or {}))
    settings.update(_string_map(
        _read_json(node_dir(node_id, root) / "application.json", {}),
        allowed=APPLICATION_SETTING_KEYS,
    ))
    settings.update(_string_map(
        _read_json(node_dir(node_id, root) / "runtime.json", {}),
        allowed=RUNTIME_SETTING_KEYS,
    ))
    settings.update(_string_map(
        _read_json(node_dir(node_id, root) / "target-app.json", {}),
        allowed=TARGET_APP_CONFIG_SETTING_KEYS,
    ))
    return settings


def copy_node_settings(source_node_id: str, section: str,
                           *, root: Path | None = None) -> dict[str, Any]:
    root = root or volume_root()
    ensure_initialized(migrate=True)
    source = node_by_id(source_node_id, root=root)
    if source is None:
        raise ValueError(f"unknown source node: {source_node_id}")
    target = active_node_id(root=root)
    if source_node_id == target:
        raise ValueError("source node must be different from the active node")

    source_settings = node_settings(source_node_id, root=root)
    if section == "application":
        app_values = {
            k: source_settings.get(k, "")
            for k in APPLICATION_SETTING_KEYS
            if k in APPLICATION_COPY_SETTING_KEYS
        }
        target_values = {
            k: source_settings.get(k, "")
            for k in TARGET_APP_CONFIG_SETTING_KEYS
            if k in APPLICATION_COPY_SETTING_KEYS
        }
        _update_node_file(
            "application.json", app_values, node_id=target, root=root,
        )
        _update_node_file(
            "target-app.json", target_values, node_id=target, root=root,
        )
        copied = {**app_values, **target_values}
    elif section == "runtime":
        values = {
            k: source_settings.get(k, "")
            for k in RUNTIME_COPY_SETTING_KEYS
        }
        _update_node_file(
            "runtime.json", values, node_id=target, root=root,
        )
        copied = values
    else:
        raise ValueError("section must be application or runtime")
    return {
        "source_node_id": source_node_id,
        "target_node_id": target,
        "section": section,
        "copied": copied,
        "copied_count": len(copied),
    }


def set_setting(key: str, value: str) -> None:
    if not (
        key in PROJECT_SETTING_KEYS
        or key in APPLICATION_SETTING_KEYS
        or key in RUNTIME_SETTING_KEYS
        or key in TARGET_APP_CONFIG_SETTING_KEYS
    ):
        return
    root = volume_root()
    ensure_initialized(migrate=True)
    _require_current_schema(root)
    if key in PROJECT_SETTING_KEYS:
        cfg = read_project_config(root=root)
        settings = cfg.setdefault("settings", {})
        settings[key] = value
        write_project_config(cfg, root=root)
    elif key in APPLICATION_SETTING_KEYS:
        _update_node_file("application.json", {key: value}, root=root)
    elif key in RUNTIME_SETTING_KEYS:
        _update_node_file("runtime.json", {key: value}, root=root)
    elif key in TARGET_APP_CONFIG_SETTING_KEYS:
        _update_node_file("target-app.json", {key: value}, root=root)


def resume_agents_for_startup(conn: sqlite3.Connection | None = None) -> bool:
    """Clear the pause flag when Refine starts a configured application."""
    from . import db

    root = volume_root()
    status = schema_status(root)
    if not status.get("compatible"):
        raise RuntimeError(migration_block_details(status))

    close_conn = False
    if conn is None:
        conn = db.connect()
        close_conn = True
    try:
        ensure_sqlite_cache_current(conn)
        was_paused = (db.get_setting(conn, "paused") or "0") == "1"
        agents_were_paused = (db.get_setting(conn, "agents_paused") or "0") == "1"
        if was_paused:
            db.set_setting(conn, "paused", "0")
        if agents_were_paused:
            db.set_setting(conn, "agents_paused", "0")
        return was_paused or agents_were_paused
    finally:
        if close_conn:
            conn.close()


def list_reporters(
    *,
    root: Path | None = None,
    node_id: str | None = None,
) -> list[dict[str, Any]]:
    root = root or volume_root()
    selected = node_id or active_node_id(root=root)
    data = _read_json(node_dir(selected, root) / "reporters.json", {"reporters": []})
    return [r for r in data.get("reporters") or [] if isinstance(r, dict)]


def write_reporters(reporters: list[dict[str, Any]], *,
                    root: Path | None = None) -> None:
    root = root or volume_root()
    _require_current_schema(root)
    active = active_node_id(root=root)
    _write_json(node_dir(active, root) / "reporters.json", {
        "reporters": reporters,
        "updated_at": now_iso(),
    })


def list_guidance(*, root: Path | None = None) -> list[dict[str, Any]]:
    root = root or volume_root()
    ensure_guidance_file(root=root)
    data = _read_json(guidance_json_path(root), {"guidance": []})
    items = data.get("guidance") or []
    return [normalize_guidance_item(item) for item in items if isinstance(item, dict)]


def write_guidance(items: list[dict[str, Any]], *,
                   root: Path | None = None) -> list[dict[str, Any]]:
    root = root or volume_root()
    _require_current_schema(root)
    normalized = [
        item for item in (normalize_guidance_item(raw) for raw in items)
        if item["name"] or item["rule"] or item["instructions"]
    ]
    _write_json(guidance_json_path(root), {
        "guidance": normalized,
        "updated_at": now_iso(),
    })
    return normalized


def normalize_guidance_item(item: dict[str, Any]) -> dict[str, Any]:
    return {
        "name": str(item.get("name") or "").strip(),
        "rule": str(item.get("rule") or "").strip(),
        "instructions": str(item.get("instructions") or "").strip(),
        "enabled": _coerce_guidance_enabled(item.get("enabled", True)),
    }


def _coerce_guidance_enabled(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if value is None:
        return True
    if isinstance(value, (int, float)):
        return value != 0
    text = str(value).strip().lower()
    if text in {"0", "false", "no", "off", "disabled"}:
        return False
    return True


def gap_node_display(node_id: str | None) -> str:
    if not node_id:
        return "Unknown"
    entry = node_by_id(node_id)
    if entry is None:
        return "Unknown"
    return str(entry.get("display_name") or entry.get("id") or "Unknown")


def transfer_gaps(source_node_id: str | None, target_node_id: str,
                  *, statuses: set[str] | None = None,
                  gap_ids: set[str] | None = None) -> dict[str, Any]:
    root = volume_root()
    _require_current_schema(root)
    target = node_by_id(target_node_id)
    if target is None:
        raise ValueError(f"unknown target node: {target_node_id}")
    if target.get("archived"):
        raise ValueError(f"archived target node: {target_node_id}")
    allowed = statuses or {
        "backlog", "todo", "failed", "qa", "awaiting-rebuild",
        "review", "done", "cancelled",
    }
    skipped: list[dict[str, str]] = []
    updated: list[str] = []
    for path in sorted((root / "gaps").glob("**/gap.json")):
        gap = _read_json(path, {})
        gid = str(gap.get("id") or "")
        if not gid:
            continue
        if gap_ids is not None and gid not in gap_ids:
            continue
        current = str(gap.get("node_id") or "")
        if source_node_id and current != source_node_id:
            continue
        status = str(gap.get("status") or "backlog")
        if status not in allowed:
            skipped.append({"id": gid, "reason": f"status:{status}"})
            continue
        if current == target_node_id:
            skipped.append({"id": gid, "reason": "already_target"})
            continue
        gap["node_id"] = target_node_id
        gap["updated"] = now_iso()
        _write_json(path, gap)
        updated.append(gid)
    return {"updated": len(updated), "ids": updated,
            "skipped": len(skipped), "skipped_details": skipped}


ProgressCallback = Callable[[int, int, str], None]


def rebuild_sqlite_cache(
    conn: sqlite3.Connection,
    *,
    force: bool = False,
    node_id: str | None = None,
    progress: ProgressCallback | None = None,
) -> None:
    """Refresh SQLite projection tables from canonical JSON.

    By default, Gap projection is incremental: unchanged gap.json files are
    identified by cached mtime/size metadata and are not read or parsed. A
    forced rebuild reparses every Gap file and replaces rebuildable projection
    tables from canonical .refine JSON.
    """
    from . import changes_index
    from . import db
    from . import perf_metrics
    total_start = perf_metrics.now()
    phase_ms: dict[str, float] = {}
    rows_updated = 0
    status = ensure_initialized(conn, migrate=True)
    if not status.get("compatible"):
        raise RuntimeError(migration_block_details(status))
    active = node_id or active_node_id()
    settings = list_settings(node_id=active)
    reps = list_reporters(node_id=active)
    root = volume_root()
    phase_start = perf_metrics.now()
    fingerprint = state_fingerprint(root=root)
    phase_ms["fingerprint_ms"] = perf_metrics.elapsed_ms(phase_start)
    phase_start = perf_metrics.now()
    gap_refresh = _plan_gap_cache_refresh(
        conn,
        root,
        force=force,
        progress=progress,
    )
    phase_ms["gap_scan_ms"] = perf_metrics.elapsed_ms(phase_start)
    with db.transaction(conn):
        phase_start = perf_metrics.now()
        conn.execute("DELETE FROM settings")
        conn.execute("DELETE FROM reporters")
        if force:
            conn.execute("DELETE FROM guidance_decisions")
            conn.execute("DELETE FROM gap_search_docs")
            conn.execute("DELETE FROM gaps_index")
            conn.execute("DELETE FROM gap_cache_meta")
        phase_ms["delete_ms"] = perf_metrics.elapsed_ms(phase_start)
        phase_start = perf_metrics.now()
        for key, value in settings.items():
            conn.execute(
                "INSERT INTO settings(key, value) VALUES(?, ?)",
                (key, str(value)),
            )
        conn.execute(
            "INSERT INTO settings(key, value) VALUES(?, ?)",
            (CACHE_ACTIVE_NODE_KEY, active),
        )
        conn.execute(
            "INSERT INTO settings(key, value) VALUES(?, ?)",
            (CACHE_STATE_FINGERPRINT_KEY, fingerprint),
        )
        for rep in reps:
            conn.execute(
                "INSERT OR IGNORE INTO reporters(id, name, created) VALUES(?, ?, ?)",
                (
                    int(rep.get("id") or 0) or None,
                    str(rep.get("name") or ""),
                    str(rep.get("created") or now_iso()),
                ),
            )
        phase_ms["settings_reporters_ms"] = perf_metrics.elapsed_ms(phase_start)
        phase_start = perf_metrics.now()
        rows_updated = _apply_gap_cache_refresh(conn, gap_refresh)
        phase_ms["gap_index_refresh_ms"] = perf_metrics.elapsed_ms(phase_start)
    if force:
        from . import search_index

        search_index.rebuild_fts(conn, "gap_search_fts")
    phase_start = perf_metrics.now()
    indexed_branch = changes_index.rebuild_target_branch(conn)
    phase_ms["changes_index_ms"] = perf_metrics.elapsed_ms(phase_start)
    perf_metrics.record(
        "sqlite_cache_rebuild",
        conn=conn,
        elapsed_ms=perf_metrics.elapsed_ms(total_start),
        success=True,
        rows_scanned=gap_refresh["files_seen"],
        rows_returned=rows_updated,
        bytes_in=gap_refresh["bytes_read"],
        details={
            **phase_ms,
            "changes_index_branch": indexed_branch,
            "force": force,
            **gap_refresh["stats"],
        },
    )


def ensure_sqlite_cache_current(
    conn: sqlite3.Connection,
    *,
    node_id: str | None = None,
) -> str:
    """Ensure SQLite projections are scoped to the selected node.

    Routine reads must stay O(1) with respect to the number of Gap JSON files.
    Normal Refine writes update SQLite and canonical JSON together; incremental
    projection refreshes are reserved for startup, project sync, and
    app/node switches. The explicit System > Runtime rebuild action uses
    a forced rebuild instead.
    """
    active = node_id or active_node_id()
    cached = _cached_active_node_id(conn)
    if cached != active:
        rebuild_sqlite_cache(conn, node_id=active)
    return active


def _cached_active_node_id(conn: sqlite3.Connection | None) -> str:
    if conn is None:
        return ""
    try:
        row = conn.execute(
            "SELECT value FROM settings WHERE key = ?",
            (CACHE_ACTIVE_NODE_KEY,),
        ).fetchone()
        if row is None:
            row = conn.execute(
                "SELECT value FROM settings WHERE key = ?",
                (LEGACY_CACHE_ACTIVE_INSTANCE_KEY,),
            ).fetchone()
        if row is None:
            return ""
        try:
            return str(row["value"])
        except (IndexError, TypeError):
            return str(row[0])
    except sqlite3.Error:
        return ""


def state_fingerprint(*, root: Path | None = None) -> str:
    """Cheap fingerprint for non-Gap project state projected into SQLite."""
    root = root or volume_root()
    paths: list[Path] = [
        config_json_path(root),
        nodes_json_path(root),
        cluster_json_path(root),
        guidance_json_path(root),
    ]
    paths.extend(sorted(nodes_dir(root).glob("**/*.json")))
    parts: list[str] = []
    for path in paths:
        try:
            st = path.stat()
        except OSError:
            continue
        rel = path.relative_to(root).as_posix()
        parts.append(f"{rel}:{st.st_mtime_ns}:{st.st_size}")
    return "|".join(parts)


def _plan_gap_cache_refresh(
    conn: sqlite3.Connection,
    root: Path,
    *,
    force: bool = False,
    progress: ProgressCallback | None = None,
) -> dict[str, Any]:
    existing = {
        str(r["json_path"]): dict(r)
        for r in conn.execute(
            "SELECT json_path, gap_id, mtime_ns, size, sha256 FROM gap_cache_meta"
        )
    }
    indexed = {
        str(r["json_path"]): str(r["id"] or "")
        for r in conn.execute("SELECT id, json_path FROM gaps_index")
    }
    indexed_search_docs = {
        str(r["gap_id"] or "")
        for r in conn.execute("SELECT gap_id FROM gap_search_docs")
    }
    seen: set[str] = set()
    upserts: list[dict[str, Any]] = []
    deletes: list[dict[str, str]] = []
    meta_only: list[dict[str, Any]] = []
    stats = {
        "files_seen": 0,
        "files_unchanged": 0,
        "files_hashed": 0,
        "files_parsed": 0,
        "files_deleted": 0,
        "files_invalid": 0,
        "bytes_read": 0,
    }
    gap_paths = sorted((root / "gaps").glob("**/gap.json"))
    total = len(gap_paths)
    if progress is not None:
        progress(0, total, f"Processing 0 of {total} Gaps")
    for path in gap_paths:
        try:
            st = path.stat()
        except OSError:
            continue
        rel = path.relative_to(root).as_posix()
        seen.add(rel)
        stats["files_seen"] += 1
        prior = existing.get(rel)
        prior_gap_id = str(prior.get("gap_id") or "") if prior is not None else ""
        index_has_prior = (
            not prior_gap_id
            or (
                indexed.get(rel) == prior_gap_id
                and prior_gap_id in indexed_search_docs
            )
        )
        mtime_ns = int(st.st_mtime_ns)
        size = int(st.st_size)
        if (
            not force
            and prior is not None
            and index_has_prior
            and int(prior.get("mtime_ns") or -1) == mtime_ns
            and int(prior.get("size") or -1) == size
        ):
            stats["files_unchanged"] += 1
            if progress is not None:
                progress(stats["files_seen"], total,
                         f"Processing {stats['files_seen']} of {total} Gaps")
            continue
        raw = _read_gap_cache_bytes(path)
        stats["bytes_read"] += len(raw)
        stats["files_hashed"] += 1
        digest = hashlib.sha256(raw).hexdigest()
        if (
            not force
            and prior is not None
            and index_has_prior
            and str(prior.get("sha256") or "") == digest
        ):
            meta_only.append({
                "json_path": rel,
                "gap_id": prior_gap_id,
                "mtime_ns": mtime_ns,
                "size": size,
                "sha256": digest,
            })
            if progress is not None:
                progress(stats["files_seen"], total,
                         f"Processing {stats['files_seen']} of {total} Gaps")
            continue
        old_gap_id = prior_gap_id
        gap = _decode_gap_cache_json(raw)
        stats["files_parsed"] += 1
        if not isinstance(gap, dict) or not gap.get("id"):
            if old_gap_id:
                deletes.append({"json_path": rel, "gap_id": old_gap_id})
            meta_only.append({
                "json_path": rel,
                "gap_id": "",
                "mtime_ns": mtime_ns,
                "size": size,
                "sha256": digest,
            })
            stats["files_invalid"] += 1
            if progress is not None:
                progress(stats["files_seen"], total,
                         f"Processing {stats['files_seen']} of {total} Gaps")
            continue
        upserts.append({
            "json_path": rel,
            "old_gap_id": old_gap_id,
            "gap": gap,
            "mtime_ns": mtime_ns,
            "size": size,
            "sha256": digest,
        })
        if progress is not None:
            progress(stats["files_seen"], total,
                     f"Processing {stats['files_seen']} of {total} Gaps")
    if force:
        return {
            "upserts": upserts,
            "deletes": [],
            "meta_only": meta_only,
            "files_seen": stats["files_seen"],
            "bytes_read": stats["bytes_read"],
            "stats": stats,
        }
    for rel, prior in existing.items():
        if rel in seen:
            continue
        old_gap_id = str(prior.get("gap_id") or "")
        deletes.append({"json_path": rel, "gap_id": old_gap_id})
        stats["files_deleted"] += 1
    for rel, gap_id in indexed.items():
        if rel in seen or rel in existing:
            continue
        deletes.append({"json_path": rel, "gap_id": gap_id})
        stats["files_deleted"] += 1
    return {
        "upserts": upserts,
        "deletes": deletes,
        "meta_only": meta_only,
        "files_seen": stats["files_seen"],
        "bytes_read": stats["bytes_read"],
        "stats": stats,
    }


def _apply_gap_cache_refresh(conn: sqlite3.Connection,
                             refresh: dict[str, Any]) -> int:
    from . import guidance
    from . import search_index

    changed_rows = 0
    now = now_iso()
    for item in refresh["deletes"]:
        gap_id = str(item.get("gap_id") or "")
        rel = str(item.get("json_path") or "")
        if gap_id:
            conn.execute(
                "DELETE FROM gaps_index WHERE id = ? AND json_path = ?",
                (gap_id, rel),
            )
            search_index.delete_gap(conn, gap_id)
            guidance.delete_gap_guidance_decisions(conn, gap_id)
        conn.execute("DELETE FROM gap_cache_meta WHERE json_path = ?", (rel,))
        changed_rows += 1
    for item in refresh["upserts"]:
        rel = str(item["json_path"])
        old_gap_id = str(item.get("old_gap_id") or "")
        gap = item["gap"]
        gap_id = str(gap.get("id") or "")
        if old_gap_id and old_gap_id != gap_id:
            conn.execute(
                "DELETE FROM gaps_index WHERE id = ? AND json_path = ?",
                (old_gap_id, rel),
            )
            search_index.delete_gap(conn, old_gap_id)
            guidance.delete_gap_guidance_decisions(conn, old_gap_id)
        _upsert_gap_index_row(conn, gap, rel)
        search_index.upsert_gap(conn, gap)
        guidance.delete_gap_guidance_decisions(conn, gap_id)
        guidance.project_gap_guidance_decisions(conn, gap, use_transaction=False)
        _upsert_gap_cache_meta(conn, item, gap_id=gap_id, now=now)
        changed_rows += 1
    for item in refresh["meta_only"]:
        _upsert_gap_cache_meta(conn, item, gap_id=str(item.get("gap_id") or ""),
                               now=now)
    return changed_rows


def _upsert_gap_cache_meta(conn: sqlite3.Connection, item: dict[str, Any], *,
                           gap_id: str, now: str) -> None:
    conn.execute(
        "INSERT OR REPLACE INTO gap_cache_meta "
        "(json_path, gap_id, mtime_ns, size, sha256, updated_at) "
        "VALUES (?, ?, ?, ?, ?, ?)",
        (
            str(item["json_path"]),
            gap_id,
            int(item["mtime_ns"]),
            int(item["size"]),
            str(item["sha256"]),
            now,
        ),
    )


def _upsert_gap_index_row(conn: sqlite3.Connection,
                          gap: dict[str, Any],
                          rel_path: str) -> None:
    conn.execute(
        "INSERT OR REPLACE INTO gaps_index "
        "(id, name, status, priority, reporter, created, updated, "
        "branch_name, node_id, json_path) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (
            str(gap.get("id") or ""),
            str(gap.get("name") or "Untitled Gap"),
            str(gap.get("status") or "backlog"),
            str(gap.get("priority") or "low"),
            _latest_reporter(gap),
            str(gap.get("created") or now_iso()),
            str(gap.get("updated") or gap.get("created") or now_iso()),
            gap.get("branch_name"),
            str(gap.get("node_id") or DEFAULT_NODE_ID),
            rel_path,
        ),
    )


def _read_gap_cache_bytes(path: Path) -> bytes:
    try:
        return path.read_bytes()
    except OSError:
        return b""


def _decode_gap_cache_json(raw: bytes) -> dict[str, Any]:
    try:
        data = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return {}
    return data if isinstance(data, dict) else {}


def _latest_reporter(gap: dict[str, Any]) -> str:
    rounds = gap.get("rounds") or []
    if rounds and isinstance(rounds[-1], dict):
        return str(rounds[-1].get("reporter") or "")
    return ""


def _write_node_files(node_id: str, *, settings: dict[str, str],
                          reporters: list[dict[str, Any]],
                          root: Path) -> None:
    _require_current_schema(root)
    d = node_dir(node_id, root)
    d.mkdir(parents=True, exist_ok=True)
    files = {
        "application.json": APPLICATION_SETTING_KEYS,
        "runtime.json": RUNTIME_SETTING_KEYS,
        "target-app.json": TARGET_APP_CONFIG_SETTING_KEYS,
    }
    for name, keys in files.items():
        _write_json(d / name, {k: v for k, v in settings.items() if k in keys})
    _write_json(d / "reporters.json", {
        "reporters": reporters,
        "updated_at": now_iso(),
    })


def _ensure_node_files(node_id: str, *, root: Path) -> None:
    from . import db

    _require_current_schema(root)
    d = node_dir(node_id, root)
    d.mkdir(parents=True, exist_ok=True)
    defaults = db.DEFAULT_SETTINGS
    for name, keys in {
        "application.json": APPLICATION_SETTING_KEYS,
        "runtime.json": RUNTIME_SETTING_KEYS,
        "target-app.json": TARGET_APP_CONFIG_SETTING_KEYS,
    }.items():
        p = d / name
        if not p.exists():
            _write_json(p, {k: defaults[k] for k in keys if k in defaults})
        else:
            _prune_node_file(p, keys)
        if name == "target-app.json":
            _cleanup_legacy_target_app_config(p)
    reps = d / "reporters.json"
    if not reps.exists():
        _write_json(reps, {"reporters": [], "updated_at": now_iso()})


def _update_node_file(filename: str, updates: dict[str, str], *,
                          node_id: str | None = None,
                          root: Path | None = None) -> None:
    root = root or volume_root()
    _require_current_schema(root)
    target = node_id or active_node_id(root=root)
    p = node_dir(target, root) / filename
    data = _read_json(p, {})
    normalized = {k: str(v) for k, v in updates.items()}
    if all(data.get(k) == v for k, v in normalized.items()):
        return
    data.update(normalized)
    data["updated_at"] = now_iso()
    _write_json(p, data)


def _prune_node_file(path: Path, allowed_keys: set[str]) -> None:
    data = _read_json(path, {})
    if not isinstance(data, dict):
        return
    metadata = {"created_at", "updated_at", "schema_version", "refine"}
    pruned = {
        str(k): v
        for k, v in data.items()
        if str(k) in metadata or str(k) in allowed_keys
    }
    if pruned == data:
        return
    pruned["updated_at"] = now_iso()
    _write_json(path, pruned)


def _cleanup_legacy_target_app_config(path: Path) -> None:
    data = _read_json(path, {})
    if not isinstance(data, dict):
        return
    changed = False
    legacy_health = str(data.get("target_app_health_url") or "").strip()
    if legacy_health:
        if not str(data.get("target_app_http_check_url") or "").strip():
            data["target_app_http_check_url"] = legacy_health
        data["target_app_health_url"] = ""
        changed = True
    if (
        str(data.get("target_app_start_instructions") or "").strip()
        and str(data.get("target_app_start_command") or "").strip()
    ):
        data["target_app_start_instructions"] = ""
        changed = True
    if (
        str(data.get("target_app_stop_instructions") or "").strip()
        and str(data.get("target_app_stop_command") or "").strip()
    ):
        data["target_app_stop_instructions"] = ""
        changed = True
    if not changed:
        return
    data["updated_at"] = now_iso()
    _write_json(path, data)


def _string_map(value: dict[str, Any], *,
                allowed: set[str] | None = None) -> dict[str, str]:
    metadata = {"created_at", "updated_at", "schema_version", "refine"}
    out: dict[str, str] = {}
    for k, v in value.items():
        key = str(k)
        if key in metadata:
            continue
        if allowed is not None and key not in allowed:
            continue
        out[key] = str(v)
    return out


def _read_json(path: Path, default: Any) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return default


def _write_json(path: Path, data: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    raw = json.dumps(data, ensure_ascii=False, indent=2).encode("utf-8")
    fd, tmp = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=str(path.parent))
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(raw)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    except Exception:
        try:
            os.unlink(tmp)
        except FileNotFoundError:
            pass
        raise


def _unlink_quietly(path: Path) -> None:
    try:
        path.unlink()
    except OSError:
        pass


def _cleanup_legacy_run_state(root: Path) -> None:
    _unlink_quietly(active_node_path(root))
    _unlink_quietly(legacy_active_instance_path(root))
    try:
        legacy_run_dir(root).rmdir()
    except OSError:
        pass


def _slug_node_id(name: str) -> str:
    import re

    slug = re.sub(r"[^a-z0-9_-]+", "-", name.lower()).strip("-")
    return slug[:40] or "node"


def _refine_version() -> str:
    try:
        import importlib.metadata
        return importlib.metadata.version("refine")
    except Exception:
        return "unknown"
