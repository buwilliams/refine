"""Port-local registry of client applications Refine knows about."""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from . import config
from .gaps import now_iso

REGISTRY_FILENAME = "apps.json"
LEGACY_REGISTRY_FILENAME = ".refine-apps.json"


def registry_path(clone_dir: Path, *, port: int | str | None = None) -> Path:
    return config.local_run_dir(clone_dir, port=port) / REGISTRY_FILENAME


def list_apps(clone_dir: Path, *, port: int | str | None = None) -> list[dict[str, str]]:
    state = _load_state(clone_dir, port=port)
    apps = _normalize_apps(state.get("apps") if isinstance(state, dict) else [])
    if apps != state.get("apps"):
        _write_state(clone_dir, apps, str(state.get("active_app") or ""), port=port)
    return apps


def active_app(clone_dir: Path, *, port: int | str | None = None) -> Path | None:
    state = _load_state(clone_dir, port=port)
    raw = str(state.get("active_app") or "").strip() if isinstance(state, dict) else ""
    if not raw:
        return None
    path = Path(raw).expanduser().resolve()
    if not path.exists():
        _write_state(clone_dir, list_apps(clone_dir, port=port), "", port=port)
        return None
    return path


def active_config_path(clone_dir: Path, *, port: int | str | None = None) -> Path | None:
    app = active_app(clone_dir, port=port)
    if app is None:
        return None
    cfg = app / ".refine" / config.CONFIG_FILENAME
    return cfg if cfg.is_file() else None


def upsert_app(
    clone_dir: Path,
    client_repo: Path,
    *,
    make_current: bool = False,
    port: int | str | None = None,
) -> list[dict[str, str]]:
    state = _load_state(clone_dir, port=port)
    apps = _normalize_apps(state.get("apps") if isinstance(state, dict) else [])
    active = str(state.get("active_app") or "") if isinstance(state, dict) else ""
    path = str(client_repo.expanduser().resolve())
    now = now_iso()
    found = False
    for app in apps:
        if app["path"] == path:
            app["name"] = app["name"] or Path(path).name
            if make_current:
                app["last_used_at"] = now
            found = True
            break
    if not found:
        apps.append({
            "name": Path(path).name or path,
            "path": path,
            "added_at": now,
            "last_used_at": now if make_current else "",
        })
    if make_current:
        active = path
    _write_state(clone_dir, apps, active, port=port)
    return apps


def set_active_app(
    clone_dir: Path,
    client_repo: Path,
    *,
    port: int | str | None = None,
) -> list[dict[str, str]]:
    return upsert_app(clone_dir, client_repo, make_current=True, port=port)


def remove_app(
    clone_dir: Path,
    client_repo: Path,
    *,
    port: int | str | None = None,
) -> list[dict[str, str]]:
    target = str(client_repo.expanduser().resolve())
    state = _load_state(clone_dir, port=port)
    active = str(state.get("active_app") or "") if isinstance(state, dict) else ""
    apps = [app for app in _normalize_apps(state.get("apps") if isinstance(state, dict) else []) if app["path"] != target]
    if active == target:
        active = ""
    _write_state(clone_dir, apps, active, port=port)
    return apps


def detach_port(clone_dir: Path, *, port: int | str | None = None) -> list[dict[str, str]]:
    apps = list_apps(clone_dir, port=port)
    _write_state(clone_dir, apps, "", port=port)
    return apps


def _load_state(clone_dir: Path, *, port: int | str | None = None) -> dict[str, Any]:
    path = registry_path(clone_dir, port=port)
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        raw = None
    if isinstance(raw, dict):
        state = {
            "version": 1,
            "active_app": str(raw.get("active_app") or ""),
            "apps": _normalize_apps(raw.get("apps")),
        }
        if not state["active_app"]:
            legacy = _legacy_state(clone_dir)
            if legacy["active_app"]:
                apps_by_path = {app["path"]: app for app in state["apps"]}
                for app in legacy["apps"]:
                    apps_by_path.setdefault(app["path"], app)
                state = {
                    "version": 1,
                    "active_app": legacy["active_app"],
                    "apps": list(apps_by_path.values()),
                }
                _write_state(clone_dir, state["apps"], state["active_app"], port=port)
        return state
    migrated = _legacy_state(clone_dir)
    if migrated["active_app"] or migrated["apps"]:
        _write_state(clone_dir, migrated["apps"], migrated["active_app"], port=port)
    return migrated


def _legacy_state(clone_dir: Path) -> dict[str, Any]:
    apps: list[dict[str, str]] = []
    active = ""
    legacy_registry = clone_dir.resolve() / LEGACY_REGISTRY_FILENAME
    try:
        raw = json.loads(legacy_registry.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        raw = None
    if isinstance(raw, dict):
        apps = _normalize_apps(raw.get("apps"))
    elif isinstance(raw, list):
        apps = _normalize_apps(raw)
    legacy_binding = clone_dir.resolve() / config.BINDING_FILENAME
    if legacy_binding.is_file():
        try:
            active_path = config.read_binding(legacy_binding)
            active = str(active_path.resolve())
            if active_path.exists() and not any(app.get("path") == active for app in apps):
                apps.append({
                    "name": active_path.name or active,
                    "path": active,
                    "added_at": "",
                    "last_used_at": "",
                })
        except (OSError, config.ConfigError):
            pass
    return {"version": 1, "active_app": active, "apps": apps}


def _normalize_apps(raw_apps: Any) -> list[dict[str, str]]:
    if not isinstance(raw_apps, list):
        return []
    out: list[dict[str, str]] = []
    seen: set[str] = set()
    for app in raw_apps:
        if not isinstance(app, dict):
            continue
        raw_path = str(app.get("path") or "").strip()
        if not raw_path:
            continue
        resolved_path = Path(raw_path).expanduser().resolve()
        if not resolved_path.exists():
            continue
        resolved = str(resolved_path)
        if resolved in seen:
            continue
        seen.add(resolved)
        out.append({
            "name": str(app.get("name") or Path(resolved).name or resolved),
            "path": resolved,
            "added_at": str(app.get("added_at") or ""),
            "last_used_at": str(app.get("last_used_at") or ""),
        })
    return out


def _write_state(
    clone_dir: Path,
    apps: list[dict[str, Any]],
    active_app: str,
    *,
    port: int | str | None = None,
) -> None:
    path = registry_path(clone_dir, port=port)
    path.parent.mkdir(parents=True, exist_ok=True)
    state = {
        "version": 1,
        "active_app": active_app,
        "apps": apps,
    }
    path.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n", encoding="utf-8")
