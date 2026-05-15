"""Clone-local registry of client applications refine knows about."""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from .gaps import now_iso

REGISTRY_FILENAME = ".refine-apps.json"


def registry_path(clone_dir: Path) -> Path:
    return clone_dir.resolve() / REGISTRY_FILENAME


def list_apps(clone_dir: Path) -> list[dict[str, str]]:
    path = registry_path(clone_dir)
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return []
    apps = raw.get("apps") if isinstance(raw, dict) else raw
    if not isinstance(apps, list):
        return []
    out: list[dict[str, str]] = []
    seen: set[str] = set()
    for app in apps:
        if not isinstance(app, dict):
            continue
        raw_path = str(app.get("path") or "").strip()
        if not raw_path:
            continue
        resolved = str(Path(raw_path).expanduser().resolve())
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


def upsert_app(clone_dir: Path, client_repo: Path, *, make_current: bool = False) -> list[dict[str, str]]:
    apps = list_apps(clone_dir)
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
    _write(clone_dir, apps)
    return apps


def remove_app(clone_dir: Path, client_repo: Path) -> list[dict[str, str]]:
    path = str(client_repo.expanduser().resolve())
    apps = [app for app in list_apps(clone_dir) if app["path"] != path]
    _write(clone_dir, apps)
    return apps


def _write(clone_dir: Path, apps: list[dict[str, Any]]) -> None:
    path = registry_path(clone_dir)
    path.write_text(
        json.dumps({"apps": apps}, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
