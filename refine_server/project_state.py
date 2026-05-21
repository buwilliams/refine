"""Canonical per-application Refine state.

SQLite is a rebuildable cache. This module owns the JSON files under
``<app>/.refine`` that carry durable project, instance, reporter, settings,
and Gap ownership state.
"""
from __future__ import annotations

import hashlib
import json
import os
import sqlite3
import tempfile
from pathlib import Path
from typing import Any

from . import config
from .gaps import now_iso


CURRENT_SCHEMA_VERSION = 1
MIN_SUPPORTED_SCHEMA_VERSION = 1
DEFAULT_INSTANCE_ID = "default"
CACHE_ACTIVE_INSTANCE_KEY = "__refine_cache_active_instance_id"
CACHE_STATE_FINGERPRINT_KEY = "__refine_cache_state_fingerprint"

PROJECT_SETTING_KEYS = {
    "governance_product",
    "governance_constitution",
    "governance_rules_json",
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
    "chat_idle_timeout_seconds",
    "backlog_promote_after_seconds",
    "project_update_pulse_interval_seconds",
    "paused",
    "agent_cli",
}

TARGET_APP_SETTING_KEYS = {
    "target_app_start_instructions",
    "target_app_stop_instructions",
    "target_app_health_url",
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


def volume_root() -> Path:
    return config.get().volume_root


def config_json_path(root: Path | None = None) -> Path:
    return (root or volume_root()) / "config.json"


def instances_json_path(root: Path | None = None) -> Path:
    return (root or volume_root()) / "instances.json"


def guidance_json_path(root: Path | None = None) -> Path:
    return (root or volume_root()) / "guidance.json"


def instances_dir(root: Path | None = None) -> Path:
    return (root or volume_root()) / "instances"


def run_dir(root: Path | None = None) -> Path:
    return config.local_run_dir()


def legacy_run_dir(root: Path | None = None) -> Path:
    return (root or volume_root()) / "run"


def active_instances_path() -> Path:
    return run_dir() / "active-instances.json"


def active_instance_path(root: Path | None = None) -> Path:
    return legacy_run_dir(root) / "active-instance.json"


def instance_dir(instance_id: str, root: Path | None = None) -> Path:
    return instances_dir(root) / instance_id


def schema_status(root: Path | None = None) -> dict[str, Any]:
    """Return schema compatibility for an application's .refine directory."""
    root = root or volume_root()
    cfg_path = config_json_path(root)
    if not cfg_path.exists():
        return {
            "compatible": False,
            "migration_required": True,
            "schema_version": None,
            "current_schema_version": CURRENT_SCHEMA_VERSION,
            "reason": "legacy_project",
        }
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
        return {
            "compatible": False,
            "migration_required": True,
            "schema_version": version,
            "current_schema_version": CURRENT_SCHEMA_VERSION,
            "reason": "outdated_schema",
        }
    return {
        "compatible": True,
        "migration_required": False,
        "schema_version": version,
        "current_schema_version": CURRENT_SCHEMA_VERSION,
        "reason": "",
    }


def empty_refine_state(root: Path | None = None) -> bool:
    root = root or volume_root()
    gaps = root / "gaps"
    has_gaps = gaps.exists() and any(gaps.glob("**/gap.json"))
    return not has_gaps and not (root / "index.sqlite").exists()


def ensure_initialized(conn: sqlite3.Connection | None = None, *,
                       migrate: bool = True) -> dict[str, Any]:
    """Ensure canonical JSON exists and is compatible for the active app."""
    root = volume_root()
    root.mkdir(parents=True, exist_ok=True)
    (root / "gaps").mkdir(exist_ok=True)
    instances_dir(root).mkdir(exist_ok=True)
    config.ensure_refine_gitignore(root)
    status = schema_status(root)
    if status["compatible"]:
        ensure_default_instance(root=root)
        ensure_active_instance(root=root)
        ensure_guidance_file(root=root)
        return status
    if not migrate or not status.get("migration_required"):
        return status
    migrate_legacy(conn, root=root)
    return schema_status(root)


def migrate_legacy(conn: sqlite3.Connection | None = None, *,
                   root: Path | None = None) -> None:
    """Create v1 JSON files from an existing SQLite-backed project."""
    root = root or volume_root()
    root.mkdir(parents=True, exist_ok=True)
    (root / "gaps").mkdir(exist_ok=True)
    instances_dir(root).mkdir(exist_ok=True)
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
    _write_json(instances_json_path(root), {
        "instances": [{
            "id": DEFAULT_INSTANCE_ID,
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
    _write_instance_files(
        DEFAULT_INSTANCE_ID,
        settings=legacy_settings,
        reporters=reporters,
        root=root,
    )
    set_active_instance(DEFAULT_INSTANCE_ID, root=root)
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
            "instance_id": DEFAULT_INSTANCE_ID,
        }
        for key, value in defaults.items():
            if key not in gap:
                gap[key] = value
                changed = True
        if changed:
            gap["updated"] = gap.get("updated") or now_iso()
            _write_json(path, gap)


def ensure_default_instance(*, root: Path | None = None) -> dict[str, Any]:
    root = root or volume_root()
    registry = read_instances(root=root)
    entries = registry.get("instances") or []
    if not entries:
        entries = [{
            "id": DEFAULT_INSTANCE_ID,
            "display_name": "Default",
            "created_at": now_iso(),
            "updated_at": now_iso(),
            "archived": False,
        }]
        registry["instances"] = entries
        _write_json(instances_json_path(root), registry)
    for entry in entries:
        if entry.get("id"):
            _ensure_instance_files(str(entry["id"]), root=root)
    return entries[0]


def read_project_config(*, root: Path | None = None) -> dict[str, Any]:
    return _read_json(config_json_path(root), {})


def write_project_config(data: dict[str, Any], *, root: Path | None = None) -> None:
    data["updated_at"] = now_iso()
    _write_json(config_json_path(root), data)


def read_instances(*, root: Path | None = None) -> dict[str, Any]:
    root = root or volume_root()
    data = _read_json(instances_json_path(root), {"instances": []})
    if not isinstance(data.get("instances"), list):
        data["instances"] = []
    return data


def write_instances(data: dict[str, Any], *, root: Path | None = None) -> None:
    _write_json(instances_json_path(root), data)


def ensure_guidance_file(*, root: Path | None = None) -> None:
    root = root or volume_root()
    if not guidance_json_path(root).exists():
        _write_json(guidance_json_path(root), {
            "guidance": [],
            "updated_at": now_iso(),
        })


def list_instances(*, root: Path | None = None) -> list[dict[str, Any]]:
    return list(read_instances(root=root).get("instances") or [])


def instance_by_id(instance_id: str, *, root: Path | None = None) -> dict[str, Any] | None:
    for entry in list_instances(root=root):
        if entry.get("id") == instance_id:
            return entry
    return None


def ensure_active_instance(*, root: Path | None = None) -> str:
    root = root or volume_root()
    registry = read_instances(root=root)
    entries = registry.get("instances") or []
    active, legacy = _read_active_instance_selection(root)
    if active and any(e.get("id") == active and not e.get("archived") for e in entries):
        _ensure_instance_files(str(active), root=root)
        if legacy:
            _write_active_instance_selection(root, str(active))
        _cleanup_legacy_run_state(root)
        return str(active)
    fallback = next((e for e in entries if not e.get("archived")), None)
    if fallback is None:
        fallback = ensure_default_instance(root=root)
    active_id = str(fallback["id"])
    set_active_instance(active_id, root=root)
    return active_id


def active_instance_id(*, root: Path | None = None) -> str:
    return ensure_active_instance(root=root)


def _active_instance_selection_key(root: Path) -> str:
    base = str(root.resolve())
    scope = config.runtime_scope()
    return f"{base}#scope={scope}" if scope else base


def _legacy_active_instance_selection_key(root: Path) -> str:
    return str(root.resolve())


def _read_active_instance_selection(root: Path) -> tuple[str | None, bool]:
    key = _active_instance_selection_key(root)
    data = _read_json(active_instances_path(), {"selections": {}})
    selections = data.get("selections") or {}
    selection = selections.get(key) or {}
    active = selection.get("active_instance_id")
    if active:
        return str(active), False
    legacy_selection = selections.get(_legacy_active_instance_selection_key(root)) or {}
    active = legacy_selection.get("active_instance_id")
    if active:
        return str(active), False
    legacy = _read_json(active_instance_path(root), {}).get("active_instance_id")
    if legacy:
        return str(legacy), True
    return None, False


def _write_active_instance_selection(root: Path, instance_id: str) -> None:
    path = active_instances_path()
    data = _read_json(path, {"selections": {}})
    selections = data.setdefault("selections", {})
    selections[_active_instance_selection_key(root)] = {
        "active_instance_id": instance_id,
        "volume_root": str(root.resolve()),
        "updated_at": now_iso(),
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    _write_json(path, data)


def set_active_instance(instance_id: str, *, root: Path | None = None) -> None:
    root = root or volume_root()
    entry = instance_by_id(instance_id, root=root)
    if entry is None:
        raise ValueError(f"unknown instance_id: {instance_id}")
    if entry.get("archived"):
        raise ValueError(f"archived instance cannot be activated: {instance_id}")
    _write_active_instance_selection(root, instance_id)
    _ensure_instance_files(instance_id, root=root)


def create_instance(display_name: str, *, root: Path | None = None) -> dict[str, Any]:
    root = root or volume_root()
    name = display_name.strip() or "New instance"
    instance_id = _slug_instance_id(name)
    existing = {str(e.get("id")) for e in list_instances(root=root)}
    base = instance_id
    i = 2
    while instance_id in existing:
        instance_id = f"{base}-{i}"
        i += 1
    entry = {
        "id": instance_id,
        "display_name": name,
        "created_at": now_iso(),
        "updated_at": now_iso(),
        "archived": False,
    }
    registry = read_instances(root=root)
    registry.setdefault("instances", []).append(entry)
    write_instances(registry, root=root)
    _ensure_instance_files(instance_id, root=root)
    return entry


def update_instance(instance_id: str, *, display_name: str | None = None,
                    archived: bool | None = None,
                    root: Path | None = None) -> dict[str, Any]:
    root = root or volume_root()
    registry = read_instances(root=root)
    for entry in registry.get("instances") or []:
        if entry.get("id") != instance_id:
            continue
        if display_name is not None:
            name = display_name.strip()
            if not name:
                raise ValueError("display_name is required")
            entry["display_name"] = name
        if archived is not None:
            entry["archived"] = bool(archived)
        entry["updated_at"] = now_iso()
        write_instances(registry, root=root)
        if archived and active_instance_id(root=root) == instance_id:
            ensure_active_instance(root=root)
        return entry
    raise ValueError(f"unknown instance_id: {instance_id}")


def list_settings() -> dict[str, str]:
    from . import db

    root = volume_root()
    ensure_initialized(migrate=True)
    active = active_instance_id(root=root)
    settings = dict(db.DEFAULT_SETTINGS)
    cfg = read_project_config(root=root)
    settings.update(_string_map(cfg.get("settings") or {}))
    settings.update(_string_map(_read_json(instance_dir(active, root) / "application.json", {})))
    settings.update(_string_map(_read_json(instance_dir(active, root) / "runtime.json", {})))
    settings.update(_string_map(_read_json(instance_dir(active, root) / "target-app.json", {})))
    return settings


def set_setting(key: str, value: str) -> None:
    root = volume_root()
    ensure_initialized(migrate=True)
    if key in PROJECT_SETTING_KEYS:
        cfg = read_project_config(root=root)
        settings = cfg.setdefault("settings", {})
        settings[key] = value
        write_project_config(cfg, root=root)
    elif key in APPLICATION_SETTING_KEYS:
        _update_instance_file("application.json", {key: value}, root=root)
    elif key in RUNTIME_SETTING_KEYS or key.startswith("feature_"):
        _update_instance_file("runtime.json", {key: value}, root=root)
    elif key in TARGET_APP_SETTING_KEYS:
        _update_instance_file("target-app.json", {key: value}, root=root)


def list_reporters(*, root: Path | None = None) -> list[dict[str, Any]]:
    root = root or volume_root()
    active = active_instance_id(root=root)
    data = _read_json(instance_dir(active, root) / "reporters.json", {"reporters": []})
    return [r for r in data.get("reporters") or [] if isinstance(r, dict)]


def write_reporters(reporters: list[dict[str, Any]], *,
                    root: Path | None = None) -> None:
    root = root or volume_root()
    active = active_instance_id(root=root)
    _write_json(instance_dir(active, root) / "reporters.json", {
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


def gap_instance_display(instance_id: str | None) -> str:
    if not instance_id:
        return "Unknown"
    entry = instance_by_id(instance_id)
    if entry is None:
        return "Unknown"
    return str(entry.get("display_name") or entry.get("id") or "Unknown")


def transfer_gaps(source_instance_id: str | None, target_instance_id: str,
                  *, statuses: set[str] | None = None,
                  gap_ids: set[str] | None = None) -> dict[str, Any]:
    target = instance_by_id(target_instance_id)
    if target is None:
        raise ValueError(f"unknown target instance: {target_instance_id}")
    if target.get("archived"):
        raise ValueError(f"archived target instance: {target_instance_id}")
    allowed = statuses or {
        "backlog", "todo", "failed", "awaiting-rebuild",
        "review", "done", "cancelled",
    }
    skipped: list[dict[str, str]] = []
    updated: list[str] = []
    root = volume_root()
    for path in sorted((root / "gaps").glob("**/gap.json")):
        gap = _read_json(path, {})
        gid = str(gap.get("id") or "")
        if not gid:
            continue
        if gap_ids is not None and gid not in gap_ids:
            continue
        current = str(gap.get("instance_id") or "")
        if source_instance_id and current != source_instance_id:
            continue
        status = str(gap.get("status") or "backlog")
        if status not in allowed:
            skipped.append({"id": gid, "reason": f"status:{status}"})
            continue
        if current == target_instance_id:
            skipped.append({"id": gid, "reason": "already_target"})
            continue
        gap["instance_id"] = target_instance_id
        gap["updated"] = now_iso()
        _write_json(path, gap)
        updated.append(gid)
    return {"updated": len(updated), "ids": updated,
            "skipped": len(skipped), "skipped_details": skipped}


def rebuild_sqlite_cache(conn: sqlite3.Connection) -> None:
    """Refresh SQLite projection tables from canonical JSON.

    Gap projection is incremental: unchanged gap.json files are identified by
    cached mtime/size metadata and are not read or parsed.
    """
    from . import changes_index
    from . import db
    from . import perf_metrics
    total_start = perf_metrics.now()
    phase_ms: dict[str, float] = {}
    rows_updated = 0
    ensure_initialized(conn, migrate=True)
    active = active_instance_id()
    settings = list_settings()
    reps = list_reporters()
    root = volume_root()
    phase_start = perf_metrics.now()
    fingerprint = state_fingerprint(root=root)
    phase_ms["fingerprint_ms"] = perf_metrics.elapsed_ms(phase_start)
    phase_start = perf_metrics.now()
    gap_refresh = _plan_gap_cache_refresh(conn, root)
    phase_ms["gap_scan_ms"] = perf_metrics.elapsed_ms(phase_start)
    with db.transaction(conn):
        phase_start = perf_metrics.now()
        conn.execute("DELETE FROM settings")
        conn.execute("DELETE FROM reporters")
        phase_ms["delete_ms"] = perf_metrics.elapsed_ms(phase_start)
        phase_start = perf_metrics.now()
        for key, value in settings.items():
            conn.execute(
                "INSERT INTO settings(key, value) VALUES(?, ?)",
                (key, str(value)),
            )
        conn.execute(
            "INSERT INTO settings(key, value) VALUES(?, ?)",
            (CACHE_ACTIVE_INSTANCE_KEY, active),
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
            **gap_refresh["stats"],
        },
    )


def ensure_sqlite_cache_current(conn: sqlite3.Connection) -> str:
    """Ensure SQLite projections are scoped to the active instance.

    Routine reads must stay O(1) with respect to the number of Gap JSON files.
    Normal Refine writes update SQLite and canonical JSON together; incremental
    projection refreshes are reserved for startup, project sync, app/instance
    switches, and the explicit System > Runtime rebuild action.
    """
    active = active_instance_id()
    try:
        row = conn.execute(
            "SELECT value FROM settings WHERE key = ?",
            (CACHE_ACTIVE_INSTANCE_KEY,),
        ).fetchone()
        cached = str(row["value"]) if row is not None else ""
    except sqlite3.Error:
        cached = ""
    if cached != active:
        rebuild_sqlite_cache(conn)
    return active


def state_fingerprint(*, root: Path | None = None) -> str:
    """Cheap fingerprint for non-Gap project state projected into SQLite."""
    root = root or volume_root()
    paths: list[Path] = [
        config_json_path(root),
        instances_json_path(root),
        guidance_json_path(root),
    ]
    paths.extend(sorted(instances_dir(root).glob("**/*.json")))
    parts: list[str] = []
    for path in paths:
        try:
            st = path.stat()
        except OSError:
            continue
        rel = path.relative_to(root).as_posix()
        parts.append(f"{rel}:{st.st_mtime_ns}:{st.st_size}")
    return "|".join(parts)


def _plan_gap_cache_refresh(conn: sqlite3.Connection,
                            root: Path) -> dict[str, Any]:
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
    for path in sorted((root / "gaps").glob("**/gap.json")):
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
            prior is not None
            and index_has_prior
            and int(prior.get("mtime_ns") or -1) == mtime_ns
            and int(prior.get("size") or -1) == size
        ):
            stats["files_unchanged"] += 1
            continue
        raw = _read_gap_cache_bytes(path)
        stats["bytes_read"] += len(raw)
        stats["files_hashed"] += 1
        digest = hashlib.sha256(raw).hexdigest()
        if (
            prior is not None
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
            continue
        upserts.append({
            "json_path": rel,
            "old_gap_id": old_gap_id,
            "gap": gap,
            "mtime_ns": mtime_ns,
            "size": size,
            "sha256": digest,
        })
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
        _upsert_gap_index_row(conn, gap, rel)
        search_index.upsert_gap(conn, gap)
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
        "branch_name, instance_id, json_path) "
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
            str(gap.get("instance_id") or DEFAULT_INSTANCE_ID),
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


def _write_instance_files(instance_id: str, *, settings: dict[str, str],
                          reporters: list[dict[str, Any]],
                          root: Path) -> None:
    d = instance_dir(instance_id, root)
    d.mkdir(parents=True, exist_ok=True)
    files = {
        "application.json": APPLICATION_SETTING_KEYS,
        "runtime.json": RUNTIME_SETTING_KEYS,
        "target-app.json": TARGET_APP_SETTING_KEYS,
    }
    for name, keys in files.items():
        _write_json(d / name, {k: v for k, v in settings.items() if k in keys})
    _write_json(d / "reporters.json", {
        "reporters": reporters,
        "updated_at": now_iso(),
    })


def _ensure_instance_files(instance_id: str, *, root: Path) -> None:
    from . import db

    d = instance_dir(instance_id, root)
    d.mkdir(parents=True, exist_ok=True)
    defaults = db.DEFAULT_SETTINGS
    for name, keys in {
        "application.json": APPLICATION_SETTING_KEYS,
        "runtime.json": RUNTIME_SETTING_KEYS,
        "target-app.json": TARGET_APP_SETTING_KEYS,
    }.items():
        p = d / name
        if not p.exists():
            _write_json(p, {k: defaults[k] for k in keys if k in defaults})
    reps = d / "reporters.json"
    if not reps.exists():
        _write_json(reps, {"reporters": [], "updated_at": now_iso()})


def _update_instance_file(filename: str, updates: dict[str, str], *,
                          root: Path | None = None) -> None:
    root = root or volume_root()
    active = active_instance_id(root=root)
    p = instance_dir(active, root) / filename
    data = _read_json(p, {})
    data.update({k: str(v) for k, v in updates.items()})
    data["updated_at"] = now_iso()
    _write_json(p, data)


def _string_map(value: dict[str, Any]) -> dict[str, str]:
    metadata = {"created_at", "updated_at", "schema_version", "refine"}
    return {str(k): str(v) for k, v in value.items() if str(k) not in metadata}


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
    _unlink_quietly(active_instance_path(root))
    try:
        legacy_run_dir(root).rmdir()
    except OSError:
        pass


def _slug_instance_id(name: str) -> str:
    import re

    slug = re.sub(r"[^a-z0-9_-]+", "-", name.lower()).strip("-")
    return slug[:40] or "instance"


def _refine_version() -> str:
    try:
        import importlib.metadata
        return importlib.metadata.version("refine")
    except Exception:
        return "unknown"
