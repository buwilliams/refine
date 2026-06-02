"""Shared target-app attachment and registry operations."""
from __future__ import annotations

import os
import re
import shutil
import subprocess
from pathlib import Path
from typing import Any, Callable
from urllib.parse import urlparse

from . import activity, config, db, gap_writer, gaps, project_registry, project_state
from . import reporters, search_index
from .ulid import new_ulid


class InitError(Exception):
    """Surface a clean error message from app setup helpers."""


class SwitchBlocked(Exception):
    """Surface a clean conflict message from app switching helpers."""

    def __init__(self, message: str, details: str | None = None) -> None:
        super().__init__(message)
        self.details = details


PrepareClone = Callable[[Path], None]
InstallUnit = Callable[[Path, Path | None], Path]
ConnFactory = Callable[[], Any]
LoadConfigured = Callable[[Path, bool, bool, bool, int], Any]
PathCallback = Callable[[Path], None]
OptionalPathCallback = Callable[[], Path | None]
PrepareSwitch = Callable[[Path | None], dict[str, Any]]
NodeSummary = Callable[[], dict[str, Any]]
AttachNext = Callable[[dict[str, Any]], tuple[int, dict[str, Any]]]
DetachCurrent = Callable[[Path, Path, int | None], None]
ProjectStatus = Callable[[], tuple[int, dict[str, Any]]]

PROJECT_TEMPLATE_DIR = Path(__file__).resolve().parents[1] / "refine_ui" / "project_templates"
PROJECT_TEMPLATE_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$")
VALID_REPORTER = re.compile(r"^[^\x00-\x1f]{1,80}$")
APP_MARKER_FILES = {
    "package.json",
    "pnpm-lock.yaml",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "bun.lock",
    "deno.json",
    "vite.config.js",
    "vite.config.ts",
    "next.config.js",
    "next.config.ts",
    "astro.config.mjs",
    "svelte.config.js",
    "pyproject.toml",
    "requirements.txt",
    "Pipfile",
    "poetry.lock",
    "uv.lock",
    "manage.py",
    "main.py",
    "app.py",
    "Dockerfile",
    "docker-compose.yml",
    "compose.yml",
    "index.html",
}
APP_MARKER_DIRS = {
    "src",
    "app",
    "pages",
    "components",
    "public",
    "static",
    "templates",
    "backend",
    "frontend",
}
APP_CODE_EXTS = {
    ".py",
    ".js",
    ".jsx",
    ".ts",
    ".tsx",
    ".mjs",
    ".cjs",
    ".css",
    ".html",
    ".vue",
    ".svelte",
    ".go",
    ".rs",
    ".java",
    ".kt",
    ".cs",
    ".php",
    ".rb",
}
APP_SCAN_IGNORED_DIRS = {
    ".git",
    ".refine",
    ".github",
    ".vscode",
    ".idea",
    "__pycache__",
    "node_modules",
    "dist",
    "build",
    ".venv",
    "venv",
}


def is_refine_source_dir(path: Path) -> bool:
    """Return whether `path` looks like a Refine source checkout."""
    return (
        (path / "pyproject.toml").is_file()
        and (path / "refine_cli" / "cli.py").is_file()
    )


def ensure_current_app(
    apps: list[dict[str, Any]],
    client_repo: Path,
) -> list[dict[str, Any]]:
    """Always include the active app, even without a clone-local registry."""
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


def list_apps(
    clone_dir: Path,
    *,
    port: int | None = None,
) -> dict[str, Any]:
    clone_dir = clone_dir.resolve()
    registry_enabled = is_refine_source_dir(clone_dir)
    apps = (
        project_registry.list_apps(clone_dir, port=port)
        if registry_enabled else []
    )
    current = ""
    try:
        cfg = config.get(reload=True, port=port)
        current = str(cfg.client_repo)
        apps = ensure_current_app(apps, cfg.client_repo)
    except config.ConfigError:
        pass
    return {
        "apps": apps,
        "current": current,
        "registry_enabled": registry_enabled,
    }


def status(
    clone_dir: Path,
    *,
    port: int | None = None,
    include_nodes: bool = False,
) -> dict[str, Any]:
    clone_dir = clone_dir.resolve()
    registry_enabled = is_refine_source_dir(clone_dir)
    apps = (
        project_registry.list_apps(clone_dir, port=port)
        if registry_enabled else []
    )
    cfg_path = config.find_config(port=port)
    if cfg_path is None:
        return {
            "attached": False,
            "apps": apps,
            "registry_enabled": registry_enabled,
            "message": "No refine project is attached.",
        }
    try:
        cfg = config.get(reload=True, port=port)
    except config.ConfigError as e:
        return {
            "attached": False,
            "apps": apps,
            "registry_enabled": registry_enabled,
            "config_path": str(cfg_path),
            "message": str(e),
        }
    if registry_enabled:
        apps = project_registry.upsert_app(
            clone_dir, cfg.client_repo, make_current=True, port=port,
        )
    else:
        apps = ensure_current_app(apps, cfg.client_repo)
    schema = project_state.schema_status(cfg.volume_root)
    payload: dict[str, Any] = {
        "attached": True,
        "apps": apps,
        "registry_enabled": registry_enabled,
        "client_repo": str(cfg.client_repo),
        "volume_root": str(cfg.volume_root),
        "config_path": str(cfg.config_path),
        "schema": schema,
        "maintenance": project_state.read_maintenance(root=cfg.volume_root),
    }
    if include_nodes and schema.get("compatible"):
        nodes = project_state.list_nodes()
        active = project_state.active_node_id()
        payload.update({
            "nodes": nodes,
            "active_node_id": active,
            "active_node": next(
                (node for node in nodes if node.get("id") == active),
                None,
            ),
        })
    elif include_nodes:
        payload.update({"nodes": [], "active_node_id": ""})
    return payload


def attach_project(
    body: dict[str, Any],
    *,
    clone_dir: Path,
    port: int,
    load_configured: LoadConfigured,
    current_client_repo: OptionalPathCallback,
    loaded_client_repo: OptionalPathCallback,
    prepare_current_project_for_switch: PrepareSwitch,
    commit_refine_state: PathCallback,
    node_summary: NodeSummary,
    force: bool = True,
    create: bool = True,
    init_git: bool = True,
    reuse_existing_config: bool = True,
    install_unit: bool = False,
    prepare_clone: PrepareClone | None = None,
    install_ui_unit: InstallUnit | None = None,
) -> tuple[int, dict[str, Any]]:
    """Create or attach a target app path and make it active."""
    raw_path = str(body.get("path") or "").strip()
    if not raw_path:
        return _err(400, "Enter a project path or Git remote.")

    clone_dir = clone_dir.resolve()
    try:
        if not is_refine_source_dir(clone_dir):
            return _err(
                409,
                "Project setup must run from the host refine source directory.",
                (
                    f"The process is running in {clone_dir}. Start refine from "
                    "the source checkout with `uv run refine start` so it can "
                    "create host directories and manage port-local app state."
                ),
            )
        if body.get("install_unit") is True and not install_unit:
            return _err(
                400,
                "Persistent service installation is only available from the CLI.",
                "Run `uv run refine install` from the Refine checkout.",
            )

        client_repo = (
            clone_project_remote(raw_path, clone_dir)
            if looks_like_git_remote(raw_path)
            else Path(raw_path).expanduser()
        )
        client_repo = client_repo.resolve()
        scaffold_required = project_needs_scaffold_template(client_repo)
        current_before = loaded_client_repo() or current_client_repo()
        switching = current_before is not None and current_before != client_repo
        record_migration_candidate_app(clone_dir, client_repo, port)
        validate_target_schema_before_switch(client_repo, body)
        prep = (
            prepare_current_project_for_switch(current_before)
            if switching else {"warnings": []}
        )

        result = bootstrap_client_repo(
            client_repo,
            clone_dir=clone_dir,
            port=port,
            force=force,
            create=create,
            init_git=init_git,
            reuse_existing_config=reuse_existing_config,
            install_unit=install_unit,
            prepare_clone=prepare_clone,
            install_ui_unit=install_ui_unit,
        )
        cfg = load_configured(
            Path(str(result["config_path"])),
            body.get("start_poller") is not False,
            body.get("start_runner") is not False,
            bool(body.get("migrate")),
            port,
        )
        if body.get("migrate"):
            commit_refine_state(cfg.client_repo)
    except (config.ConfigError, InitError, OSError, TimeoutError) as e:
        return _err(400, str(e))
    except SwitchBlocked as e:
        return _err(409, str(e), e.details)

    runner = {"started": False, "message": ""}
    if body.get("start_runner") is not False:
        runner = {"started": True, "message": "Backend runner started."}

    return 200, {
        "attached": True,
        "client_repo": str(cfg.client_repo),
        "volume_root": str(cfg.volume_root),
        "config_path": str(cfg.config_path),
        "binding_path": str(result["binding_path"]) if result.get("binding_path") else "",
        "registry_path": str(result["registry_path"]) if result.get("registry_path") else "",
        "unit_path": str(result["unit_path"]) if result.get("unit_path") else "",
        "ui_unit_path": str(result["ui_unit_path"]) if result.get("ui_unit_path") else "",
        "git_initialized": bool(result.get("git_initialized")),
        "config_created": bool(result.get("config_created")),
        "apps": project_registry.list_apps(clone_dir, port=port),
        "registry_enabled": True,
        "schema": project_state.schema_status(cfg.volume_root),
        **node_summary(),
        "switch_warnings": prep.get("warnings", []),
        "scaffold_required": scaffold_required,
        "scaffold_templates": list_project_templates()["templates"],
        "runner": runner,
    }


def validate_target_schema_before_switch(client_repo: Path, body: dict[str, Any]) -> None:
    existing_refine = client_repo / ".refine"
    existing_cfg = existing_refine / config.CONFIG_FILENAME
    if not existing_cfg.exists():
        return
    schema = project_state.schema_status(existing_refine)
    migrate_requested = bool(body.get("migrate"))
    if schema.get("compatible"):
        return
    if (
        schema.get("migration_required")
        and project_state.migration_requires_manual(schema)
    ):
        raise SwitchBlocked(
            project_state.migration_block_message(schema),
            project_state.migration_block_details(schema),
        )
    if (
        schema.get("migration_required")
        and not migrate_requested
        and not project_state.empty_refine_state(existing_refine)
    ):
        raise SwitchBlocked(
            project_state.migration_block_message(schema),
            "Open this app with migrate=true to upgrade .refine state before switching.",
        )
    if not schema.get("migration_required"):
        raise SwitchBlocked(
            "Project schema is not supported by this Refine version.",
            schema.get("reason") or "",
        )


def record_migration_candidate_app(clone_dir: Path, client_repo: Path, port: int) -> None:
    existing_refine = client_repo / ".refine"
    existing_cfg = existing_refine / config.CONFIG_FILENAME
    if not existing_cfg.exists():
        return
    schema = project_state.schema_status(existing_refine)
    if not schema.get("migration_required"):
        return
    project_registry.upsert_app(clone_dir, client_repo, make_current=True, port=port)


def project_needs_scaffold_template(client_repo: Path) -> bool:
    """True when the target has no detectable application code yet."""
    try:
        path = client_repo.expanduser()
    except RuntimeError:
        return False
    if not path.exists():
        return True
    if not path.is_dir():
        return False
    return not target_has_existing_application(path)


def target_has_existing_application(path: Path) -> bool:
    for marker in APP_MARKER_FILES:
        if (path / marker).is_file():
            return True
    for dirname in APP_MARKER_DIRS:
        child = path / dirname
        if child.is_dir() and directory_has_visible_files(child):
            return True

    seen = 0
    for root, dirs, files in os.walk(path):
        dirs[:] = [d for d in dirs if d not in APP_SCAN_IGNORED_DIRS]
        rel_root = Path(root).relative_to(path)
        if rel_root.parts and rel_root.parts[0] in APP_SCAN_IGNORED_DIRS:
            continue
        for filename in files:
            seen += 1
            if seen > 500:
                return False
            if Path(filename).suffix.lower() in APP_CODE_EXTS:
                return True
    return False


def directory_has_visible_files(path: Path) -> bool:
    try:
        for child in path.rglob("*"):
            if any(part in APP_SCAN_IGNORED_DIRS for part in child.relative_to(path).parts):
                continue
            if child.is_file():
                return True
    except OSError:
        return False
    return False


def list_project_templates() -> dict[str, Any]:
    templates: list[dict[str, str]] = []
    if not PROJECT_TEMPLATE_DIR.is_dir():
        return {"templates": templates}
    for path in sorted(PROJECT_TEMPLATE_DIR.glob("*.md")):
        if not PROJECT_TEMPLATE_ID_RE.match(path.stem):
            continue
        try:
            content = path.read_text(encoding="utf-8")
        except OSError:
            continue
        templates.append(project_template_summary(path.stem, content))
    return {"templates": templates}


def project_template_summary(template_id: str, content: str) -> dict[str, str]:
    title = ""
    summary = ""
    for raw in content.splitlines():
        line = raw.strip()
        if not line:
            continue
        if line.startswith("# "):
            title = line[2:].strip()
            continue
        if not summary and not line.startswith("#"):
            summary = line
        if title and summary:
            break
    if not title:
        title = template_id.replace("-", " ").replace("_", " ").title()
    return {"id": template_id, "name": title, "summary": summary}


def load_project_template(template_id: str) -> tuple[dict[str, str], str] | None:
    if not PROJECT_TEMPLATE_ID_RE.match(template_id):
        return None
    path = PROJECT_TEMPLATE_DIR / f"{template_id}.md"
    if not path.is_file():
        return None
    try:
        content = path.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return project_template_summary(template_id, content), content


def create_project_scaffold_gap(
    conn_factory: ConnFactory,
    body: dict[str, Any],
) -> tuple[int, dict[str, Any]]:
    template_id = str(body.get("template") or "").strip()
    loaded = load_project_template(template_id)
    if loaded is None:
        return _err(404, "Unknown project template.")
    summary, content = loaded
    reporter = str(body.get("reporter") or "Refine").strip()
    if not reporter or not VALID_REPORTER.match(reporter):
        return _err(400, "invalid reporter name")

    name = f"Scaffold {summary['name']}"
    actual = (
        "The attached project has no detectable application scaffold yet. "
        f"Implement the selected project template: {summary['name']}."
    )
    gap = create_indexed_gap(
        conn_factory,
        name=name,
        reporter=reporter,
        actual=actual,
        target=content,
        priority="high",
    )
    return 201, {"ok": True, "gap": gap, "template": summary}


def create_indexed_gap(
    conn_factory: ConnFactory,
    *,
    name: str,
    reporter: str,
    actual: str,
    target: str,
    priority: str,
) -> dict[str, Any]:
    gap_id = new_ulid()
    node_id = project_state.active_node_id()
    round_obj = gaps.new_round(
        reporter=reporter,
        actual=actual,
        target=target,
    )
    gap = gap_writer.create_gap(
        gap_id=gap_id,
        name=name,
        initial_round=round_obj,
        status="backlog",
        priority=priority,
        node_id=node_id,
    )

    from refine_server.paths import relative_gap_path

    conn = conn_factory()
    try:
        with db.transaction(conn):
            conn.execute(
                "INSERT INTO gaps_index "
                "(id, name, status, priority, reporter, created, updated, node_id, json_path) "
                "VALUES (?, ?, 'backlog', ?, ?, ?, ?, ?, ?)",
                (
                    gap_id,
                    name,
                    priority,
                    reporter,
                    gap["created"],
                    gap["updated"],
                    node_id,
                    relative_gap_path(gap_id),
                ),
            )
            search_index.upsert_gap(conn, gap)
            reporters.add(conn, reporter)
            activity.append(
                conn,
                message=f"Gap created: {name}",
                severity="info",
                category="state",
                gap_id=gap_id,
                actor=reporter,
            )
    finally:
        conn.close()
    return gap


def looks_like_git_remote(value: str) -> bool:
    text = str(value or "").strip()
    if text.startswith(("git@", "ssh://", "git://")):
        return True
    parsed = urlparse(text)
    return parsed.scheme in {"http", "https", "file"} and bool(parsed.netloc or parsed.scheme == "file")


def default_project_clone_path(remote: str, clone_dir: Path) -> Path:
    parsed = urlparse(remote)
    raw_name = Path(parsed.path or remote.rstrip("/")).name or "target-app"
    if raw_name.endswith(".git"):
        raw_name = raw_name[:-4]
    name = re.sub(r"[^A-Za-z0-9._-]+", "-", raw_name).strip(".-") or "target-app"
    base = clone_dir.parent
    candidate = base / name
    if not candidate.exists() or (candidate / ".git").exists():
        return candidate
    i = 2
    while True:
        numbered = base / f"{name}-{i}"
        if not numbered.exists() or (numbered / ".git").exists():
            return numbered
        i += 1


def clone_project_remote(remote: str, clone_dir: Path) -> Path:
    git = shutil.which("git")
    if git is None:
        raise config.ConfigError("could not find `git` on PATH; install git or use a local app path")
    target = default_project_clone_path(remote, clone_dir)
    if target.exists():
        if (target / ".git").exists():
            return target
        try:
            has_entries = any(target.iterdir())
        except OSError as e:
            raise config.ConfigError(f"cannot inspect clone target {target}: {e}") from e
        if has_entries:
            raise config.ConfigError(
                f"clone target already exists and is not empty: {target}"
            )
    target.parent.mkdir(parents=True, exist_ok=True)
    result = subprocess.run(
        [git, "clone", remote, str(target)],
        cwd=str(clone_dir),
        capture_output=True,
        text=True,
        timeout=600,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "git clone failed").strip()
        raise config.ConfigError(
            "could not clone target app. Check the Git remote and host credentials: "
            + detail
        )
    return target


def remove_app(
    clone_dir: Path,
    target: Path,
    *,
    port: int | None = None,
) -> dict[str, Any]:
    clone_dir = clone_dir.resolve()
    target = target.expanduser().resolve()
    if not is_refine_source_dir(clone_dir):
        raise config.ConfigError(
            "Known-apps list is only available from the host refine source checkout."
        )
    try:
        current = config.get(reload=True, port=port).client_repo
    except config.ConfigError:
        current = None
    apps = project_registry.remove_app(clone_dir, target, port=port)
    detached = False
    if current is not None and current == target:
        project_registry.detach_port(clone_dir, port=port)
        detached = True
    return {
        "apps": apps,
        "removed_path": str(target),
        "detached": detached,
    }


def remove_project(
    body: dict[str, Any],
    *,
    clone_dir: Path,
    port: int | None,
    attach_next: AttachNext,
    detach_current: DetachCurrent,
    project_status: ProjectStatus,
) -> tuple[int, dict[str, Any]]:
    raw_path = str(body.get("path") or "").strip()
    if not raw_path:
        return _err(400, "Choose an app to remove.")
    clone_dir = clone_dir.resolve()
    if not is_refine_source_dir(clone_dir):
        return _err(409, "Known-apps list is only available from the host refine source checkout.")
    target = Path(raw_path).expanduser().resolve()
    apps_before = project_registry.list_apps(clone_dir, port=port)
    try:
        current = config.get(reload=True, port=port).client_repo
    except config.ConfigError:
        current = None
    if current is not None and current == target:
        remaining = [app for app in apps_before if app.get("path") != str(target)]
        if remaining:
            removed_index = next(
                (
                    i for i, app in enumerate(apps_before)
                    if app.get("path") == str(target)
                ),
                0,
            )
            next_app = remaining[removed_index % len(remaining)]
            attach_body = {
                "path": next_app["path"],
                "install_unit": body.get("install_unit") is True,
            }
            if "start_runner" in body:
                attach_body["start_runner"] = body.get("start_runner")
            if "start_poller" in body:
                attach_body["start_poller"] = body.get("start_poller")
            status, attached = attach_next(attach_body)
            if status != 200:
                return status, attached
            apps = project_registry.remove_app(clone_dir, target, port=port)
            attached["apps"] = apps
            attached["removed_path"] = str(target)
            attached["auto_attached"] = True
            return status, attached
        removed = remove_app(clone_dir, target, port=port)
        detach_current(clone_dir, target, port)
        status, payload = project_status()
        payload["removed_path"] = str(target)
        payload["detached"] = bool(removed.get("detached"))
        return status, payload
    removed = remove_app(clone_dir, target, port=port)
    return 200, {
        "apps": removed["apps"],
        "removed_path": removed["removed_path"],
        "detached": bool(removed.get("detached")),
    }


def bootstrap_client_repo(
    client_repo: Path,
    *,
    clone_dir: Path,
    port: int | None = None,
    force: bool,
    create: bool,
    init_git: bool,
    reuse_existing_config: bool,
    install_unit: bool,
    prepare_clone: PrepareClone | None = None,
    install_ui_unit: InstallUnit | None = None,
) -> dict[str, Path | bool | None]:
    """Create or attach a target app using Refine's shared app files."""
    clone_dir = clone_dir.resolve()
    client_repo = client_repo.expanduser().resolve()

    if client_repo.exists() and not client_repo.is_dir():
        raise config.ConfigError(f"not a directory: {client_repo}")
    if not client_repo.exists():
        if not create:
            raise config.ConfigError(f"not a directory: {client_repo}")
        client_repo.mkdir(parents=True)

    git_dir = client_repo / ".git"
    git_initialized = False
    if not git_dir.exists():
        if not init_git:
            raise config.ConfigError(
                f"not a git repository: {client_repo}\n"
                "  Run `git init` inside it first, or pass a path to an existing git repo."
            )
        git = shutil.which("git")
        if git is None:
            raise InitError("could not find `git` on PATH; install git or choose an existing git repo")
        out = subprocess.run(
            [git, "init", "-q"], cwd=str(client_repo),
            capture_output=True, text=True, timeout=30,
        )
        if out.returncode != 0:
            raise InitError((out.stderr or out.stdout or "git init failed").strip())
        git_initialized = True

    target = client_repo / ".refine"
    cfg_path = target / config.CONFIG_FILENAME
    config_created = False
    if cfg_path.exists() and reuse_existing_config:
        (target / "gaps").mkdir(parents=True, exist_ok=True)
        config.ensure_refine_gitignore(target)
    else:
        cfg_path = config.write_defaults(target, force=force, port=port)
        config_created = True

    registry_path = None
    ui_unit_path = None
    if is_refine_source_dir(clone_dir):
        if prepare_clone is not None:
            prepare_clone(clone_dir)
        if install_unit:
            if install_ui_unit is None:
                raise InitError("install_unit requires a systemd unit installer")
            ui_unit_path = install_ui_unit(clone_dir, None)
        project_registry.upsert_app(
            clone_dir, client_repo, make_current=True, port=port,
        )
        registry_path = project_registry.registry_path(clone_dir, port=port)

    return {
        "client_repo": client_repo,
        "volume_root": target,
        "config_path": cfg_path,
        "binding_path": None,
        "registry_path": registry_path,
        "ui_unit_path": ui_unit_path,
        "git_initialized": git_initialized,
        "config_created": config_created,
    }


def _err(
    code: int,
    message: str,
    details: str | None = None,
) -> tuple[int, dict[str, Any]]:
    body: dict[str, Any] = {"error": {"message": message}}
    if details is not None:
        body["error"]["details"] = details
    return code, body
