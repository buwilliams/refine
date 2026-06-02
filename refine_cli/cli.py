"""Typer-based CLI for refine.

Subcommands:
- target  — attach a target app and make it active for this checkout.
- install — install + start a persistent systemd refine service.
- uninstall — stop + remove a persistent systemd refine service.
- reset   — remove the active target binding and persistent units;
            optional --purge also deletes the active app's .refine/ data so
            you can attach a different app.
- start   — start refine in a detached supervisor process.
- stop    — stop refine.
- restart — stop then start (handy for picking up source changes).
- status  — show what's running (read-only).
- update  — update Refine to the latest version.
- ps      — show host process CPU/memory usage for refine.
- test    — run the repository's script-style test suite.
- server  — start the server component in the foreground for debugging.
- ui      — start the UI foreground process (supervised in normal use).
- doctor  — deeper diagnostic snapshot (config, agent CLI, git).
"""
from __future__ import annotations

import getpass
import json
import os
import re
import signal
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
from types import SimpleNamespace
from typing import Annotated, Callable

import click
import typer
from refine_server import chat_ops, cluster, cluster_ops, config, dashboard_ops, db, diagnostics_ops, file_ops, gap_ops, import_ops, node_ops, observability_ops, process_ops, project_apps, project_config_ops, project_registry, project_state, project_sync, reporter_ops, settings_ops, target_app_ops, upgrade
from refine_server.backend_protocol import M_PREFLIGHT, M_PROJECT_SYNC


SYSTEMD_SYSTEM_DIR = Path("/etc/systemd/system")
SYSTEMD_USER_DIR = Path.home() / ".config" / "systemd" / "user"
SETUP_UI_HOST = "0.0.0.0"
_LOGIN_PATH_CACHE: str | None = None
_LOGIN_PATH_RESOLVED = False
_Args = SimpleNamespace
_Command = Callable[[_Args], int]
_CONTEXT_SETTINGS = {"help_option_names": ["-h", "--help"]}
README_INSTALL_COMMAND = (
    "curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/"
    "scripts/install.sh | bash"
)


app = typer.Typer(
    name="refine",
    help="Manage refine.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
node_app = typer.Typer(
    name="node",
    help="Manage work-owning Refine nodes.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
cluster_app = typer.Typer(
    name="cluster",
    help="Manage distributed Refine cluster nodes.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
apps_app = typer.Typer(
    name="app",
    help="Manage known target apps for this Refine checkout.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
reporter_app = typer.Typer(
    name="reporter",
    help="Manage Gap reporters.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
guidance_app = typer.Typer(
    name="guidance",
    help="Manage project guidance.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
governance_app = typer.Typer(
    name="governance",
    help="Manage Governance settings and rules.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
quality_app = typer.Typer(
    name="quality",
    help="Manage Quality settings and regressions.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
quality_regression_app = typer.Typer(
    name="regression",
    help="Manage Quality regression checks.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
quality_app.add_typer(quality_regression_app, name="regression")
activity_app = typer.Typer(
    name="activity",
    help="Inspect and clean activity logs.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
performance_app = typer.Typer(
    name="performance",
    help="Inspect and clean performance telemetry.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
upgrade_app = typer.Typer(
    name="upgrade",
    help="Inspect Refine upgrade availability.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
job_app = typer.Typer(
    name="job",
    help="Inspect and cancel background jobs.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
files_app = typer.Typer(
    name="files",
    help="Inspect target-app files.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
gaps_app = typer.Typer(
    name="gaps",
    help="Inspect Gaps.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
changes_app = typer.Typer(
    name="changes",
    help="Inspect merged Refine changes.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
chat_app = typer.Typer(
    name="chat",
    help="Manage chat sessions.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
settings_app = typer.Typer(
    name="settings",
    help="Manage project settings.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
diagnostics_app = typer.Typer(
    name="diagnostics",
    help="Inspect machine-readable diagnostics.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
processes_app = typer.Typer(
    name="processes",
    help="Control managed processes.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
dashboard_app = typer.Typer(
    name="dashboard",
    help="Inspect dashboard data.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
import_app = typer.Typer(
    name="import",
    help="Prepare and persist Gap imports.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
migrate_app = typer.Typer(
    name="migrate",
    help="Manage Refine project-state migrations.",
    add_completion=False,
    context_settings=_CONTEXT_SETTINGS,
    no_args_is_help=True,
)
app.add_typer(node_app, name="node")
app.add_typer(cluster_app, name="cluster")
app.add_typer(apps_app, name="app")
app.add_typer(reporter_app, name="reporter")
app.add_typer(guidance_app, name="guidance")
app.add_typer(governance_app, name="governance")
app.add_typer(quality_app, name="quality")
app.add_typer(activity_app, name="activity")
app.add_typer(performance_app, name="performance")
app.add_typer(upgrade_app, name="upgrade")
app.add_typer(job_app, name="job")
app.add_typer(files_app, name="files")
app.add_typer(gaps_app, name="gaps")
app.add_typer(changes_app, name="changes")
app.add_typer(chat_app, name="chat")
app.add_typer(settings_app, name="settings")
app.add_typer(diagnostics_app, name="diagnostics")
app.add_typer(processes_app, name="processes")
app.add_typer(dashboard_app, name="dashboard")
app.add_typer(import_app, name="import")
app.add_typer(migrate_app, name="migrate")


def main(argv: list[str] | None = None) -> int:
    config.load_dotenv()
    raw_args = list(argv) if argv is not None else sys.argv[1:]
    normalized = _normalize_argv(raw_args)
    try:
        result = app(args=normalized, prog_name="refine", standalone_mode=False)
    except click.ClickException as e:
        e.show()
        return e.exit_code
    except click.Abort:
        typer.echo("Aborted!", err=True)
        return 1
    except click.exceptions.Exit as e:
        return int(e.exit_code or 0)
    return result if isinstance(result, int) else 0


def _normalize_argv(argv: list[str] | None) -> list[str] | None:
    """Preserve argparse-era `refine ps --watch` shorthand under Click/Typer."""
    if argv is None:
        return None
    ps_index = _command_index(argv)
    if ps_index is None or argv[ps_index] != "ps":
        return argv
    normalized: list[str] = []
    idx = 0
    while idx < len(argv):
        token = argv[idx]
        normalized.append(token)
        if idx > ps_index and token == "--watch":
            next_token = argv[idx + 1] if idx + 1 < len(argv) else None
            if next_token is None or next_token.startswith("-"):
                normalized.append("2.0")
        idx += 1
    return normalized


def _command_index(argv: list[str]) -> int | None:
    idx = 0
    while idx < len(argv):
        token = argv[idx]
        if token in {"--config", "-c"}:
            idx += 2
            continue
        if token.startswith("--config="):
            idx += 1
            continue
        if token.startswith("-"):
            idx += 1
            continue
        return idx
    return None


@app.callback()
def _cli(
    ctx: typer.Context,
    config_path: Annotated[
        str | None,
        typer.Option(
            "--config",
            "-c",
            help="Path to refine.toml (defaults to walking up from cwd).",
        ),
    ] = None,
) -> None:
    ctx.obj = {"config": config_path}


def _ctx_config(ctx: typer.Context) -> str | None:
    obj = ctx.obj if isinstance(ctx.obj, dict) else {}
    value = obj.get("config")
    return value if isinstance(value, str) else None


def _run_command(command: _Command, ctx: typer.Context, **kwargs: object) -> int:
    return command(_Args(config=_ctx_config(ctx), **kwargs))


def _ensure_cli_project(config_path: str | None = None, *, port: int | None = None) -> None:
    if config_path:
        config.get(path=config_path, reload=True, port=port)
    else:
        config.get(reload=True, port=port)
    conn = db.connect()
    try:
        status = project_state.ensure_initialized(conn, migrate=True)
        if not status.get("compatible"):
            raise click.ClickException(project_state.migration_block_details(status))
        project_state.rebuild_sqlite_cache(conn)
    finally:
        conn.close()


def _migration_config(ctx: typer.Context, port: int | None = None) -> config.Config:
    cfg_path = _ctx_config(ctx)
    if cfg_path:
        return config.get(path=cfg_path, reload=True)
    try:
        return config.get(reload=True, port=port)
    except config.ConfigError as e:
        if port is not None:
            raise click.ClickException(
                f"refine migrate: no app is attached on port {port}. "
                f"Run `refine target --port {port} <target-app>` first."
            ) from e
        raise click.ClickException(str(e)) from e


def _migration_operator(port: int | None) -> str:
    return f"refine migrate run {port}" if port is not None else "refine migrate run [port]"


def _sync_cli_refine_state(
    cfg: config.Config,
    *,
    message: str,
    rebuild_cache: bool = True,
) -> dict:
    db.init_db(cfg.sqlite_path)
    conn = db.connect(cfg.sqlite_path)
    try:
        result = project_sync.commit_and_push_refine_state(
            conn,
            actor="cli",
            cwd=cfg.client_repo,
            state_message=message,
            rebuild_cache=rebuild_cache,
        )
    finally:
        conn.close()
    if not result.get("ok"):
        raise click.ClickException(
            str(result.get("details") or result.get("message") or "Refine state sync failed")
        )
    return result


def _cli_project_config(ctx: typer.Context, *, port: int | None = None) -> config.Config:
    _ensure_cli_project(_ctx_config(ctx), port=port)
    cfg_path = _ctx_config(ctx)
    if cfg_path:
        return config.get(path=cfg_path, reload=True, port=port)
    return config.get(reload=True, port=port)


def _load_json_value(value: str) -> object:
    source = str(value or "").strip()
    if source == "-":
        text = sys.stdin.read()
    else:
        path = Path(source).expanduser()
        if path.is_file():
            text = path.read_text(encoding="utf-8")
        else:
            text = source
    try:
        return json.loads(text)
    except json.JSONDecodeError as e:
        raise click.ClickException(f"invalid JSON: {e}") from e


def _load_text_value(value: str) -> str:
    source = str(value or "")
    if source.strip() == "-":
        return sys.stdin.read()
    path = Path(source).expanduser()
    if path.is_file():
        return path.read_text(encoding="utf-8")
    return source


def _runner_call_for_cli(
    ctx: typer.Context,
    port: int | None,
) -> tuple[config.Config, project_config_ops.RunnerCall]:
    cfg, effective_port = _target_app_cli_config(ctx, port)
    return cfg, _target_app_cli_runner_call(cfg, effective_port)


def _backend_runner_for_cli(
    ctx: typer.Context,
    port: int | None,
) -> tuple[config.Config, chat_ops.RunnerCall]:
    cfg = _cli_project_config(ctx, port=port)
    effective_port = _effective_port(_Args(port=port), cfg)
    return cfg, _backend_cli_runner_call(cfg, effective_port)


def _backend_cli_runner_call(
    cfg: config.Config,
    port: int,
) -> chat_ops.RunnerCall:
    from refine_runtime import ipc
    from refine_runtime.supervisor_protocol import (
        M_ENSURE_WORKER,
        WORKER_STARTUP_TIMEOUT_SECONDS,
    )

    def call(method: str, params: dict[str, object], timeout: float) -> dict:
        supervisor_socket = ipc.supervisor_socket_path(port)
        worker = ipc.request(
            supervisor_socket,
            M_ENSURE_WORKER,
            {"config_path": str(cfg.config_path)},
            timeout=WORKER_STARTUP_TIMEOUT_SECONDS + 15.0,
        )
        worker_socket = str(
            worker.get("worker_socket")
            or worker.get("socket_path")
            or ipc.runner_socket_path(port=port, config_path=cfg.config_path),
        )
        return ipc.request(worker_socket, method, params, timeout=timeout)

    return call


def _print_json(payload: object) -> None:
    typer.echo(json.dumps(payload, indent=2))


@migrate_app.command("status", help="Show project-state migration status.")
def migrate_status_command(
    ctx: typer.Context,
    port: Annotated[
        int | None,
        typer.Argument(help="Refine port whose attached app should be inspected."),
    ] = None,
) -> int:
    cfg = _migration_config(ctx, port)
    payload = {
        "client_repo": str(cfg.client_repo),
        "volume_root": str(cfg.volume_root),
        "port": int(port if port is not None else cfg.web_port),
        "schema": project_state.schema_status(cfg.volume_root),
        "maintenance": project_state.read_maintenance(root=cfg.volume_root),
    }
    typer.echo(json.dumps(payload, indent=2))
    return 0


@migrate_app.command("run", help="Run the pending Refine project-state migration.")
def migrate_run_command(
    ctx: typer.Context,
    port: Annotated[
        int | None,
        typer.Argument(help="Refine port whose attached app should be migrated."),
    ] = None,
) -> int:
    cfg = _migration_config(ctx, port)
    operator = _migration_operator(int(port if port is not None else cfg.web_port))
    status = project_state.schema_status(cfg.volume_root)
    if status.get("compatible"):
        if project_state.read_maintenance(root=cfg.volume_root) is not None:
            project_state.clear_maintenance(root=cfg.volume_root)
            result = _sync_cli_refine_state(
                cfg,
                message="refine: clear project migration maintenance",
                rebuild_cache=True,
            )
            typer.echo(json.dumps({"ok": True, "schema": status, "maintenance_cleared": True, "sync": result}, indent=2))
            return 0
        typer.echo("No migration required.")
        return 0
    if not status.get("migration_required"):
        raise click.ClickException(
            "Project schema is not supported by this Refine version."
        )
    if status.get("migration_id") not in {
        project_state.INSTANCE_TO_NODE_MIGRATION_ID,
        project_state.LEGACY_PROJECT_MIGRATION_ID,
    }:
        raise click.ClickException(project_state.migration_block_details(status))

    from refine_server import git_ops

    stuck = git_ops.in_progress_op(cwd=cfg.client_repo)
    if stuck is not None:
        op, hint = stuck
        raise click.ClickException(
            f"Cannot migrate while a git {op} is in progress. {hint}"
        )
    dirty = git_ops.dirty_paths(cwd=cfg.client_repo)
    non_refine_dirty = [
        path for path in dirty
        if path != ".refine" and not path.startswith(".refine/")
    ]
    if non_refine_dirty:
        raise click.ClickException(
            "Cannot migrate with non-Refine worktree changes: "
            + ", ".join(non_refine_dirty[:20])
        )

    db.init_db(cfg.sqlite_path)
    conn = db.connect(cfg.sqlite_path)
    try:
        project_state.write_maintenance(
            {
                "migration_id": status.get("migration_id") or "",
                "reason": "project_state_migration",
                "operator": operator,
                "operator_instructions": project_state.migration_block_details(status),
            },
            root=cfg.volume_root,
        )
        lock_sync = project_sync.commit_and_push_refine_state(
            conn,
            actor="cli",
            cwd=cfg.client_repo,
            state_message="refine: enter maintenance for project migration",
            rebuild_cache=False,
        )
        if not lock_sync.get("ok"):
            raise click.ClickException(
                str(lock_sync.get("details") or lock_sync.get("message") or "Maintenance lock sync failed")
            )
        status = project_state.ensure_initialized(
            conn,
            migrate=True,
            allow_manual_migrations=True,
            root=cfg.volume_root,
        )
        if not status.get("compatible"):
            raise click.ClickException(project_state.migration_block_details(status))
        project_state.rebuild_sqlite_cache(conn, force=True)
        project_state.clear_maintenance(root=cfg.volume_root)
        migration_sync = project_sync.commit_and_push_refine_state(
            conn,
            actor="cli",
            cwd=cfg.client_repo,
            state_message="refine: migrate project state",
            rebuild_cache=True,
        )
        if not migration_sync.get("ok"):
            project_state.write_maintenance(
                {
                    "migration_id": status.get("migration_id") or "",
                    "reason": "project_state_migration_push_failed",
                    "operator": operator,
                    "operator_instructions": (
                        "Migration completed locally, but pushing the migrated "
                        f"state failed. Resolve Git sync, then rerun `{operator}` "
                        "or push the migration commit before restarting nodes."
                    ),
                },
                root=cfg.volume_root,
            )
            raise click.ClickException(
                str(migration_sync.get("details") or migration_sync.get("message") or "Migration sync failed")
            )
    finally:
        conn.close()
    payload = {
        "ok": bool(migration_sync.get("ok", True)),
        "schema": project_state.schema_status(cfg.volume_root),
        "lock_sync": lock_sync,
        "migration_sync": migration_sync,
    }
    typer.echo(json.dumps(payload, indent=2))
    return 0 if payload["ok"] else 1


@node_app.command("list", help="List nodes.")
def node_list_command(ctx: typer.Context) -> int:
    _ensure_cli_project(_ctx_config(ctx))
    typer.echo(json.dumps(node_ops.summary(), indent=2))
    return 0


@node_app.command("create", help="Create a node.")
def node_create_command(
    ctx: typer.Context,
    name: Annotated[str, typer.Argument(help="Node display name.")],
) -> int:
    _ensure_cli_project(_ctx_config(ctx))
    cfg = config.get(reload=True)
    node = node_ops.create(name)
    sync = _sync_cli_refine_state(cfg, message="refine: create node")
    typer.echo(json.dumps({"node": node, "sync": sync}, indent=2))
    return 0


@node_app.command("activate", help="Activate a node.")
def node_activate_command(
    ctx: typer.Context,
    node_id: Annotated[str, typer.Argument(help="Node ID.")],
) -> int:
    _ensure_cli_project(_ctx_config(ctx))
    node_ops.activate(node_id)
    typer.echo(f"Activated node {node_id}.")
    return 0


@node_app.command("rename", help="Rename a node.")
def node_rename_command(
    ctx: typer.Context,
    node_id: Annotated[str, typer.Argument(help="Node ID.")],
    name: Annotated[str, typer.Argument(help="New display name.")],
) -> int:
    _ensure_cli_project(_ctx_config(ctx))
    cfg = config.get(reload=True)
    node = node_ops.update(node_id, display_name=name)
    sync = _sync_cli_refine_state(cfg, message="refine: update node")
    typer.echo(json.dumps({"node": node, "sync": sync}, indent=2))
    return 0


@node_app.command("archive", help="Archive a node.")
def node_archive_command(
    ctx: typer.Context,
    node_id: Annotated[str, typer.Argument(help="Node ID.")],
) -> int:
    _ensure_cli_project(_ctx_config(ctx))
    cfg = config.get(reload=True)
    node = node_ops.update(node_id, archived=True)
    sync = _sync_cli_refine_state(cfg, message="refine: update node")
    typer.echo(json.dumps({"node": node, "sync": sync}, indent=2))
    return 0


@node_app.command("transfer-gaps", help="Transfer Gaps to another node.")
def node_transfer_gaps_command(
    ctx: typer.Context,
    target_node_id: Annotated[str, typer.Argument(help="Target node ID.")],
    source_node_id: Annotated[
        str | None,
        typer.Option("--source", help="Only transfer Gaps owned by this node."),
    ] = None,
) -> int:
    _ensure_cli_project(_ctx_config(ctx))
    cfg = config.get(reload=True)
    result = node_ops.transfer_gaps(source_node_id, target_node_id)
    sync = _sync_cli_refine_state(cfg, message="refine: transfer node gaps")
    result["sync"] = sync
    typer.echo(json.dumps(result, indent=2))
    return 0


@node_app.command("copy-settings", help="Copy Application or Runtime settings from another node.")
def node_copy_settings_command(
    ctx: typer.Context,
    source_node_id: Annotated[str, typer.Argument(help="Source node ID.")],
    section: Annotated[str, typer.Argument(help="Settings section: application or runtime.")],
) -> int:
    _ensure_cli_project(_ctx_config(ctx))
    cfg = config.get(reload=True)
    result = node_ops.copy_settings(source_node_id, section)
    sync = _sync_cli_refine_state(cfg, message="refine: copy node settings")
    result["sync"] = sync
    typer.echo(json.dumps(result, indent=2))
    return 0


@cluster_app.command("list", help="List cluster nodes.")
def cluster_list_command(ctx: typer.Context) -> int:
    _ensure_cli_project(_ctx_config(ctx))
    status, payload = cluster_ops.list_cluster()
    typer.echo(json.dumps(payload, indent=2))
    return 0 if status < 400 else 1


@cluster_app.command("register", help="Register or update a cluster node.")
def cluster_register_command(
    ctx: typer.Context,
    node_id: Annotated[str, typer.Argument(help="Cluster node ID.")],
    ssh_host: Annotated[str, typer.Argument(help="SSH host. Current user is assumed.")],
    display_name: Annotated[
        str | None,
        typer.Option("--name", help="Display name."),
    ] = None,
    ssh_port: Annotated[int, typer.Option("--ssh-port", help="SSH port.")] = 22,
    refine_checkout: Annotated[
        str,
        typer.Option("--refine-checkout", help="Remote Refine checkout path."),
    ] = "~/refine",
    target_app_path: Annotated[
        str,
        typer.Option("--target-app", help="Remote target app path."),
    ] = "",
    refine_port: Annotated[int, typer.Option("--refine-port", help="Remote Refine UI port.")] = 8080,
) -> int:
    _ensure_cli_project(_ctx_config(ctx))
    cfg = config.get(reload=True)
    status, payload = cluster_ops.upsert_node(
        {
            "id": node_id,
            "display_name": display_name or node_id,
            "ssh_host": ssh_host,
            "ssh_port": ssh_port,
            "refine_checkout": refine_checkout,
            "target_app_path": target_app_path,
            "refine_port": refine_port,
        },
        lambda message: _sync_cli_refine_state(cfg, message=message),
    )
    if status >= 400:
        raise click.ClickException(payload["error"]["message"])
    typer.echo(json.dumps(payload, indent=2))
    return 0


@cluster_app.command("update", help="Update a registered cluster node.")
def cluster_update_command(
    ctx: typer.Context,
    node_id: Annotated[str, typer.Argument(help="Cluster node ID.")],
    display_name: Annotated[
        str | None,
        typer.Option("--name", help="Display name."),
    ] = None,
    ssh_host: Annotated[
        str | None,
        typer.Option("--ssh-host", help="SSH host. Current user is assumed."),
    ] = None,
    ssh_port: Annotated[int | None, typer.Option("--ssh-port", help="SSH port.")] = None,
    refine_checkout: Annotated[
        str | None,
        typer.Option("--refine-checkout", help="Remote Refine checkout path."),
    ] = None,
    target_app_path: Annotated[
        str | None,
        typer.Option("--target-app", help="Remote target app path."),
    ] = None,
    refine_port: Annotated[int | None, typer.Option("--refine-port", help="Remote Refine UI port.")] = None,
    enabled: Annotated[
        bool | None,
        typer.Option("--enabled/--disabled", help="Enable or disable the cluster node."),
    ] = None,
) -> int:
    _ensure_cli_project(_ctx_config(ctx))
    cfg = config.get(reload=True)
    body: dict[str, object] = {}
    if display_name is not None:
        body["display_name"] = display_name
    if ssh_host is not None:
        body["ssh_host"] = ssh_host
    if ssh_port is not None:
        body["ssh_port"] = ssh_port
    if refine_checkout is not None:
        body["refine_checkout"] = refine_checkout
    if target_app_path is not None:
        body["target_app_path"] = target_app_path
    if refine_port is not None:
        body["refine_port"] = refine_port
    if enabled is not None:
        body["enabled"] = enabled
    status, payload = cluster_ops.update_node(
        node_id,
        body,
        lambda message: _sync_cli_refine_state(cfg, message=message),
    )
    if status >= 400:
        raise click.ClickException(payload["error"]["message"])
    typer.echo(json.dumps(payload, indent=2))
    return 0


@cluster_app.command("bootstrap", help="Bootstrap a cluster node over SSH.")
def cluster_bootstrap_command(
    ctx: typer.Context,
    node_id: Annotated[str, typer.Argument(help="Cluster node ID.")],
) -> int:
    _ensure_cli_project(_ctx_config(ctx))
    cfg = config.get(reload=True)
    status, payload = cluster_ops.bootstrap_node(
        node_id,
        lambda message: _sync_cli_refine_state(cfg, message=message),
    )
    if status == 400:
        raise click.ClickException(payload["error"]["message"])
    typer.echo(payload.get("stdout") or "", nl=False)
    if payload.get("stderr"):
        typer.echo(payload["stderr"], err=True, nl=False)
    return 0 if status < 400 and payload.get("ok") else int(payload.get("exit_code") or 1)


@cluster_app.command(
    "run",
    help="Run a Refine command on a cluster node over SSH.",
    context_settings={"allow_extra_args": True, "ignore_unknown_options": True},
)
def cluster_run_command(
    ctx: typer.Context,
    node_id: Annotated[str, typer.Argument(help="Cluster node ID.")],
) -> int:
    _ensure_cli_project(_ctx_config(ctx))
    args = list(ctx.args)
    if args and args[0] == "--":
        args = args[1:]
    status, payload = cluster_ops.run_node(node_id, {"args": args})
    if status == 400:
        raise click.ClickException(payload["error"]["message"])
    typer.echo(payload.get("stdout") or "", nl=False)
    if payload.get("stderr"):
        typer.echo(payload["stderr"], err=True, nl=False)
    return 0 if status < 400 and payload.get("ok") else int(payload.get("exit_code") or 1)


@reporter_app.command("list", help="List reporters.")
def reporter_list_command(ctx: typer.Context) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        _print_json(reporter_ops.list_reporters(conn))
    finally:
        conn.close()
    return 0


@reporter_app.command("add", help="Add a reporter.")
def reporter_add_command(
    ctx: typer.Context,
    name: Annotated[str, typer.Argument(help="Reporter name.")],
) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        payload = reporter_ops.create_reporter(conn, name)
    except ValueError as e:
        raise click.ClickException(str(e)) from e
    finally:
        conn.close()
    payload["sync"] = _sync_cli_refine_state(cfg, message="refine: update reporters")
    _print_json(payload)
    return 0


@reporter_app.command("delete", help="Delete a reporter from the dropdown.")
def reporter_delete_command(
    ctx: typer.Context,
    reporter_id: Annotated[int, typer.Argument(help="Reporter ID.")],
) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        payload = reporter_ops.delete_reporter(conn, reporter_id)
    finally:
        conn.close()
    payload["sync"] = _sync_cli_refine_state(cfg, message="refine: update reporters")
    _print_json(payload)
    return 0


@reporter_app.command("rename", help="Rename a reporter and cascade Gap rounds.")
def reporter_rename_command(
    ctx: typer.Context,
    reporter_id: Annotated[int, typer.Argument(help="Reporter ID.")],
    name: Annotated[str, typer.Argument(help="New reporter name.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port. Defaults to 8080.")] = None,
) -> int:
    cfg, runner_call = _runner_call_for_cli(ctx, port)
    try:
        payload = reporter_ops.rename_reporter(runner_call, reporter_id, name)
    except ValueError as e:
        raise click.ClickException(str(e)) from e
    payload["sync"] = _sync_cli_refine_state(cfg, message="refine: rename reporter")
    _print_json(payload)
    return 0


@reporter_app.command("merge", help="Merge one reporter into another and cascade Gap rounds.")
def reporter_merge_command(
    ctx: typer.Context,
    reporter_id: Annotated[int, typer.Argument(help="Source reporter ID.")],
    target_id: Annotated[int, typer.Argument(help="Target reporter ID.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port. Defaults to 8080.")] = None,
) -> int:
    cfg, runner_call = _runner_call_for_cli(ctx, port)
    try:
        payload = reporter_ops.merge_reporter(runner_call, reporter_id, target_id)
    except ValueError as e:
        raise click.ClickException(str(e)) from e
    payload["sync"] = _sync_cli_refine_state(cfg, message="refine: merge reporter")
    _print_json(payload)
    return 0


@guidance_app.command("list", help="List project guidance.")
def guidance_list_command(ctx: typer.Context) -> int:
    _cli_project_config(ctx)
    _print_json(project_config_ops.list_guidance())
    return 0


@guidance_app.command("replace", help="Replace guidance from a JSON array, file, or stdin.")
def guidance_replace_command(
    ctx: typer.Context,
    source: Annotated[str, typer.Argument(help="JSON array, JSON file path, or '-' for stdin.")],
) -> int:
    cfg = _cli_project_config(ctx)
    raw = _load_json_value(source)
    items = raw.get("guidance") if isinstance(raw, dict) else raw
    try:
        payload = project_config_ops.update_guidance(items)
    except ValueError as e:
        raise click.ClickException(str(e)) from e
    payload["sync"] = _sync_cli_refine_state(cfg, message="refine: update guidance")
    _print_json(payload)
    return 0


@settings_app.command("get", help="Show project settings.")
def settings_get_command(ctx: typer.Context) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        payload = settings_ops.list_settings(conn)
    finally:
        conn.close()
    _print_json(payload)
    return 0


@settings_app.command("save", help="Update project settings from a JSON object, file, or stdin.")
def settings_save_command(
    ctx: typer.Context,
    source: Annotated[str, typer.Argument(help="JSON object, JSON file path, or '-' for stdin.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port for backend side effects.")] = None,
) -> int:
    raw = _load_json_value(source)
    if not isinstance(raw, dict):
        raise click.ClickException("settings payload must be a JSON object")
    return _settings_save_payload(ctx, raw, port=port)


@settings_app.command("set", help="Update one project setting.")
def settings_set_command(
    ctx: typer.Context,
    key: Annotated[str, typer.Argument(help="Setting key.")],
    value: Annotated[str, typer.Argument(help="Setting value.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port for backend side effects.")] = None,
) -> int:
    return _settings_save_payload(ctx, {key: value}, port=port)


@settings_app.command("recheck-auth", help="Run the backend provider auth preflight check.")
def settings_recheck_auth_command(
    ctx: typer.Context,
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    _cfg, runner_call = _backend_runner_for_cli(ctx, port)
    try:
        payload = runner_call(M_PREFLIGHT, {}, 30.0)
    except Exception as e:
        raise click.ClickException(str(e)) from e
    _print_json(payload)
    return 0 if payload.get("ok") else 1


def _settings_save_payload(
    ctx: typer.Context,
    body: dict[str, object],
    *,
    port: int | None,
) -> int:
    cfg, runner_call = _backend_runner_for_cli(ctx, port)
    conn = db.connect(cfg.sqlite_path)
    try:
        try:
            status, payload = settings_ops.update_settings(
                conn,
                body,
                runner_call=runner_call,
            )
        except ValueError as e:
            raise click.ClickException(str(e)) from e
        except Exception as e:
            raise click.ClickException(str(e)) from e
    finally:
        conn.close()
    if status < 400 and payload.get("ok", True) is not False:
        payload["sync"] = _sync_cli_refine_state(cfg, message="refine: update settings")
    _print_json(payload)
    return 0 if status < 400 and payload.get("ok", True) is not False else 1


@diagnostics_app.command("backend", help="Show backend runner diagnostics.")
def diagnostics_backend_command(
    ctx: typer.Context,
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    cfg, runner_call = _backend_runner_for_cli(ctx, port)
    effective_port = _effective_port(_Args(port=port), cfg)
    backend = {
        "process_model": "supervisor",
        "config_path": str(cfg.config_path),
        "web_port": effective_port,
    }
    try:
        payload = diagnostics_ops.backend_diagnostics(
            runner_call,
            backend=backend,
        )
    except Exception as e:
        payload = diagnostics_ops.unreachable(
            backend=backend,
            message=str(e),
            code=e.__class__.__name__,
        )
    _print_json(payload)
    return 0 if payload.get("reachable") else 1


@processes_app.command("list", help="List managed processes and runner work.")
def processes_list_command(
    ctx: typer.Context,
) -> int:
    cfg = _cli_project_config(ctx)
    from refine_ui import runtime

    runner_snapshot = runtime.runner_status_snapshot()
    backend = runner_snapshot.get("backend") or runtime.backend_info()
    conn = db.connect(cfg.sqlite_path)
    try:
        payload = process_ops.summary(
            conn,
            runner_snapshot=runner_snapshot,
            backend=backend,
            ui_pid=None,
            supervisor_pid=_int_or_none(os.environ.get("REFINE_SUPERVISOR_PID")),
            active_background_job=_active_background_job_for_cli,
        )
    finally:
        conn.close()
    _print_json(payload)
    return 0


def _active_background_job_for_cli(kind: str) -> dict[str, object] | None:
    try:
        conn = db.connect()
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
    from refine_ui import background_jobs

    for row in rows:
        snap = background_jobs.snapshot(str(row["id"]))
        if snap and snap.get("status") in {"queued", "running"}:
            return snap
    return None


def _int_or_none(value: object) -> int | None:
    return process_ops.int_or_none(value)


@processes_app.command("background", help="Start, stop, or toggle background processes.")
def processes_background_command(
    ctx: typer.Context,
    stopped: Annotated[
        bool | None,
        typer.Option("--stopped/--running", help="Stop or start background processes."),
    ] = None,
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    cfg, runner_call = _backend_runner_for_cli(ctx, port)
    body: dict[str, object] = {}
    if stopped is not None:
        body["stopped"] = stopped
    conn = db.connect(cfg.sqlite_path)
    try:
        try:
            status, payload = process_ops.set_background_processes(
                conn,
                body,
                runner_call=runner_call,
            )
        except Exception as e:
            raise click.ClickException(str(e)) from e
    finally:
        conn.close()
    _print_json(payload)
    return 0 if status < 400 and payload.get("runner", {}).get("ok", True) is not False else 1


@processes_app.command("agents", help="Pause, unpause, or toggle agent processes.")
def processes_agents_command(
    ctx: typer.Context,
    paused: Annotated[
        bool | None,
        typer.Option("--paused/--running", help="Pause or unpause agent processes."),
    ] = None,
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    cfg, runner_call = _backend_runner_for_cli(ctx, port)
    body: dict[str, object] = {}
    if paused is not None:
        body["paused"] = paused
    conn = db.connect(cfg.sqlite_path)
    try:
        try:
            status, payload = process_ops.set_agent_processes(
                conn,
                body,
                runner_call=runner_call,
            )
        except Exception as e:
            raise click.ClickException(str(e)) from e
    finally:
        conn.close()
    _print_json(payload)
    return 0 if status < 400 and payload.get("runner", {}).get("ok", True) is not False else 1


@dashboard_app.command("summary", help="Show dashboard summary data.")
def dashboard_summary_command(
    ctx: typer.Context,
    node: Annotated[
        str,
        typer.Option("--node", help="Node scope: current or all."),
    ] = "current",
) -> int:
    cfg = _cli_project_config(ctx)
    from refine_ui import runtime

    conn = db.connect(cfg.sqlite_path)
    try:
        payload = dashboard_ops.summary(
            conn,
            node=node,
            runner_snapshot=runtime.runner_status_snapshot(),
        )
    finally:
        conn.close()
    _print_json(payload)
    return 0


@governance_app.command("get", help="Show Governance settings.")
def governance_get_command(ctx: typer.Context) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        _print_json(project_config_ops.governance_get(conn))
    finally:
        conn.close()
    return 0


@governance_app.command("save", help="Update Governance settings.")
def governance_save_command(
    ctx: typer.Context,
    product: Annotated[str | None, typer.Option("--product", help="Product description.")] = None,
    constitution: Annotated[str | None, typer.Option("--constitution", help="Constitution text.")] = None,
    rules: Annotated[str | None, typer.Option("--rules", help="Rules JSON array, file path, or '-' for stdin.")] = None,
) -> int:
    cfg = _cli_project_config(ctx)
    body: dict[str, object] = {}
    if product is not None:
        body["product"] = product
    if constitution is not None:
        body["constitution"] = constitution
    if rules is not None:
        raw_rules = _load_json_value(rules)
        body["rules"] = raw_rules.get("rules") if isinstance(raw_rules, dict) else raw_rules
    conn = db.connect(cfg.sqlite_path)
    try:
        payload = project_config_ops.governance_save(conn, body)
    except ValueError as e:
        raise click.ClickException(str(e)) from e
    finally:
        conn.close()
    payload["sync"] = _sync_cli_refine_state(cfg, message="refine: update governance")
    _print_json(payload)
    return 0


@governance_app.command("generate-rules", help="Generate Governance rules through the runner.")
def governance_generate_rules_command(
    ctx: typer.Context,
    product: Annotated[str, typer.Argument(help="Product description.")],
    constitution: Annotated[str, typer.Argument(help="Constitution text.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port. Defaults to 8080.")] = None,
) -> int:
    _cfg, runner_call = _runner_call_for_cli(ctx, port)
    try:
        payload = project_config_ops.governance_generate_rules(
            runner_call,
            {"product": product, "constitution": constitution},
        )
    except ValueError as e:
        raise click.ClickException(str(e)) from e
    _print_json(payload)
    return 0 if payload.get("ok", True) is not False else 1


@quality_app.command("get", help="Show Quality settings and regressions.")
def quality_get_command(ctx: typer.Context) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        _print_json(project_config_ops.quality_get(conn))
    finally:
        conn.close()
    return 0


@quality_app.command("save", help="Update Quality settings.")
def quality_save_command(
    ctx: typer.Context,
    enabled: Annotated[
        bool | None,
        typer.Option("--enabled/--disabled", help="Enable or disable Quality."),
    ] = None,
    timing: Annotated[
        str | None,
        typer.Option("--timing", help="Quality timing: pre_merge or post_rebuild."),
    ] = None,
    business_requirements: Annotated[
        str | None,
        typer.Option("--business-requirements", help="Business requirements text."),
    ] = None,
    instructions: Annotated[
        str | None,
        typer.Option("--instructions", help="Quality instructions text."),
    ] = None,
    regressions_enabled: Annotated[
        bool | None,
        typer.Option("--regressions-enabled/--regressions-disabled", help="Enable managed regressions."),
    ] = None,
) -> int:
    cfg = _cli_project_config(ctx)
    body: dict[str, object] = {}
    if enabled is not None:
        body["enabled"] = enabled
    if timing is not None:
        body["timing"] = timing
    if business_requirements is not None:
        body["business_requirements"] = business_requirements
    if instructions is not None:
        body["instructions"] = instructions
    if regressions_enabled is not None:
        body["regressions_enabled"] = regressions_enabled
    conn = db.connect(cfg.sqlite_path)
    try:
        payload = project_config_ops.quality_save(conn, body)
    except ValueError as e:
        raise click.ClickException(str(e)) from e
    finally:
        conn.close()
    payload["sync"] = _sync_cli_refine_state(cfg, message="refine: update quality")
    _print_json(payload)
    return 0


@quality_regression_app.command("list", help="List Quality regression checks.")
def quality_regression_list_command(ctx: typer.Context) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        payload = {"regressions": project_config_ops.quality_get(conn)["regressions"]}
    finally:
        conn.close()
    _print_json(payload)
    return 0


@quality_regression_app.command("create", help="Create a Quality regression check.")
def quality_regression_create_command(
    ctx: typer.Context,
    title: Annotated[str | None, typer.Option("--title", help="Regression title.")] = None,
    description: Annotated[str, typer.Option("--description", help="Regression description.")] = "",
    prompt: Annotated[str, typer.Option("--prompt", help="Prompt used to seed the spec.")] = "",
) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        payload = project_config_ops.regression_create(
            conn,
            {"title": title, "description": description, "prompt": prompt},
        )
    finally:
        conn.close()
    payload["sync"] = _sync_cli_refine_state(cfg, message="refine: update quality regressions")
    _print_json(payload)
    return 0


@quality_regression_app.command("update", help="Update a Quality regression check.")
def quality_regression_update_command(
    ctx: typer.Context,
    regression_id: Annotated[str, typer.Argument(help="Regression ID.")],
    title: Annotated[str | None, typer.Option("--title", help="Regression title.")] = None,
    description: Annotated[str | None, typer.Option("--description", help="Regression description.")] = None,
    enabled: Annotated[
        bool | None,
        typer.Option("--enabled/--disabled", help="Enable or disable this regression."),
    ] = None,
    timeout_seconds: Annotated[int | None, typer.Option("--timeout", help="Timeout seconds.")] = None,
    wait_until: Annotated[str | None, typer.Option("--wait-until", help="Playwright wait state.")] = None,
    viewport: Annotated[
        str | None,
        typer.Option("--viewport", help="Viewport JSON object, file path, or '-' for stdin."),
    ] = None,
) -> int:
    cfg = _cli_project_config(ctx)
    body: dict[str, object] = {}
    if title is not None:
        body["title"] = title
    if description is not None:
        body["description"] = description
    if enabled is not None:
        body["enabled"] = enabled
    if timeout_seconds is not None:
        body["timeout_seconds"] = timeout_seconds
    if wait_until is not None:
        body["wait_until"] = wait_until
    if viewport is not None:
        body["viewport"] = _load_json_value(viewport)
    try:
        payload = project_config_ops.regression_update(regression_id, body)
    except LookupError as e:
        raise click.ClickException(str(e)) from e
    payload["sync"] = _sync_cli_refine_state(cfg, message="refine: update quality regressions")
    _print_json(payload)
    return 0


@quality_regression_app.command("delete", help="Delete a Quality regression check.")
def quality_regression_delete_command(
    ctx: typer.Context,
    regression_id: Annotated[str, typer.Argument(help="Regression ID.")],
) -> int:
    cfg = _cli_project_config(ctx)
    try:
        payload = project_config_ops.regression_delete(regression_id)
    except LookupError as e:
        raise click.ClickException(str(e)) from e
    payload["sync"] = _sync_cli_refine_state(cfg, message="refine: update quality regressions")
    _print_json(payload)
    return 0


@quality_regression_app.command("run", help="Run enabled Quality regression checks through the runner.")
def quality_regression_run_command(
    ctx: typer.Context,
    port: Annotated[int | None, typer.Option("--port", help="Refine port. Defaults to 8080.")] = None,
) -> int:
    _cfg, runner_call = _runner_call_for_cli(ctx, port)
    payload = project_config_ops.regression_run(runner_call)
    _print_json(payload)
    return 0 if payload.get("ok", True) is not False else 1


@activity_app.command("list", help="List activity entries.")
def activity_list_command(
    ctx: typer.Context,
    limit: Annotated[int, typer.Option("--limit", help="Maximum rows.")] = 50,
    offset: Annotated[int, typer.Option("--offset", help="Rows to skip.")] = 0,
    gap_id: Annotated[str | None, typer.Option("--gap-id", help="Filter by Gap ID.")] = None,
    since_id: Annotated[int | None, typer.Option("--since-id", help="Only entries after this activity ID.")] = None,
    severity: Annotated[str | None, typer.Option("--severity", help="Filter severity.")] = None,
    category: Annotated[str | None, typer.Option("--category", help="Filter category.")] = None,
    actor: Annotated[str | None, typer.Option("--actor", help="Filter actor.")] = None,
    q: Annotated[str | None, typer.Option("--query", "-q", help="Search message/details.")] = None,
    sort: Annotated[str | None, typer.Option("--sort", help="Sort key.")] = None,
    direction: Annotated[str | None, typer.Option("--direction", help="asc or desc.")] = None,
    facets: Annotated[bool, typer.Option("--facets", help="Include facets.")] = False,
) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        payload = observability_ops.list_activity(
            conn,
            limit=limit,
            offset=offset,
            gap_id=gap_id,
            since_id=since_id,
            severity=severity,
            category=category,
            actor=actor,
            q=q,
            sort=sort,
            direction=direction,
            include_facets=facets,
            metric_operation="cli.list_activity",
        )
    finally:
        conn.close()
    _print_json(payload)
    return 0


@activity_app.command("cleanup", help="Delete old activity entries.")
def activity_cleanup_command(
    ctx: typer.Context,
    days: Annotated[int, typer.Option("--days", help="Days to keep; 0 clears all.")] = 30,
) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        payload = observability_ops.cleanup_logs(conn, days)
    except ValueError as e:
        raise click.ClickException(str(e)) from e
    finally:
        conn.close()
    _print_json(payload)
    return 0


@performance_app.command("summary", help="Show performance telemetry.")
def performance_summary_command(
    ctx: typer.Context,
    operation: Annotated[str | None, typer.Option("--operation", help="Filter operation.")] = None,
    success: Annotated[str | None, typer.Option("--success", help="Filter success/failure.")] = None,
    limit: Annotated[int, typer.Option("--limit", help="Maximum recent rows.")] = 50,
    offset: Annotated[int, typer.Option("--offset", help="Rows to skip.")] = 0,
) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        payload = observability_ops.performance_summary(
            conn,
            operation=operation,
            success=success,
            limit=limit,
            offset=offset,
        )
    finally:
        conn.close()
    _print_json(payload)
    return 0


@performance_app.command("cleanup", help="Prune or clear performance telemetry.")
def performance_cleanup_command(
    ctx: typer.Context,
    clear: Annotated[bool, typer.Option("--clear", help="Clear all telemetry.")] = False,
) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        payload = observability_ops.performance_cleanup(conn, clear=clear)
    finally:
        conn.close()
    _print_json(payload)
    return 0


@performance_app.command("rebuild-cache", help="Rebuild the SQLite cache from canonical Refine JSON.")
def performance_rebuild_cache_command(
    ctx: typer.Context,
) -> int:
    cfg = _cli_project_config(ctx)
    payload = observability_ops.rebuild_sqlite_cache(
        cfg.sqlite_path,
        restart_services=False,
    )
    _print_json(payload)
    return 0


@upgrade_app.command("status", help="Show Refine upgrade availability.")
def upgrade_status_command() -> int:
    _print_json({"upgrade": upgrade.status(Path.cwd()).as_dict()})
    return 0


@job_app.command("get", help="Show a background job snapshot.")
def job_get_command(
    _ctx: typer.Context,
    job_id: Annotated[str, typer.Argument(help="Job ID.")],
) -> int:
    from refine_ui import background_jobs

    try:
        payload = observability_ops.background_job(job_id, background_jobs.snapshot)
    except LookupError as e:
        raise click.ClickException(str(e)) from e
    _print_json(payload)
    return 0


@job_app.command("cancel", help="Cancel a background job.")
def job_cancel_command(
    _ctx: typer.Context,
    job_id: Annotated[str, typer.Argument(help="Job ID.")],
) -> int:
    from refine_ui import background_jobs

    try:
        payload = observability_ops.cancel_background_job(job_id, background_jobs.cancel)
    except LookupError as e:
        raise click.ClickException(str(e)) from e
    _print_json(payload)
    return 0


@files_app.command("tree", help="List target-app directory entries.")
def files_tree_command(
    ctx: typer.Context,
    path: Annotated[str, typer.Argument(help="Target-app relative path.")] = "",
    recursive: Annotated[bool, typer.Option("--recursive", help="Include nested directories.")] = False,
    max_depth: Annotated[int, typer.Option("--max-depth", help="Maximum recursive depth.")] = file_ops.FILES_TREE_MAX_DEPTH,
    max_entries: Annotated[int, typer.Option("--max-entries", help="Maximum entries.")] = file_ops.FILES_TREE_MAX_ENTRIES,
) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        status, payload = file_ops.tree(
            cfg.client_repo,
            path,
            recursive=recursive,
            max_depth=max_depth,
            max_entries=max_entries,
            ignore_patterns=file_ops.file_browser_ignore_patterns(conn),
        )
    finally:
        conn.close()
    _print_json(payload)
    return 0 if status < 400 else 1


@files_app.command("search", help="Search target-app paths.")
def files_search_command(
    ctx: typer.Context,
    query: Annotated[str, typer.Argument(help="Path search query.")],
    max_entries: Annotated[int, typer.Option("--max-entries", help="Maximum matches.")] = file_ops.FILES_TREE_MAX_ENTRIES,
) -> int:
    cfg = _cli_project_config(ctx)
    conn = db.connect(cfg.sqlite_path)
    try:
        status, payload = file_ops.search(
            cfg.client_repo,
            query,
            max_entries=max_entries,
            ignore_patterns=file_ops.file_browser_ignore_patterns(conn),
        )
    finally:
        conn.close()
    _print_json(payload)
    return 0 if status < 400 else 1


@files_app.command("read", help="Read a target-app file preview.")
def files_read_command(
    ctx: typer.Context,
    path: Annotated[str, typer.Argument(help="Target-app relative file path.")],
    offset: Annotated[int, typer.Option("--offset", help="Byte offset.")] = 0,
    limit: Annotated[int, typer.Option("--limit", help="Maximum bytes.")] = file_ops.FILE_TEXT_CHUNK_BYTES,
) -> int:
    cfg = _cli_project_config(ctx)
    status, payload = file_ops.read(
        cfg.client_repo,
        path,
        offset=offset,
        limit=limit,
    )
    _print_json(payload)
    return 0 if status < 400 else 1


@gaps_app.command("list", help="List Gaps.")
def gaps_list_command(
    ctx: typer.Context,
    status: Annotated[str | None, typer.Option("--status", help="Filter by workflow status.")] = None,
    q: Annotated[str | None, typer.Option("--q", help="Search query.")] = None,
    severity: Annotated[str | None, typer.Option("--severity", help="Filter by activity severity.")] = None,
    category: Annotated[str | None, typer.Option("--category", help="Filter by activity category.")] = None,
    actor: Annotated[str | None, typer.Option("--actor", help="Filter by activity actor.")] = None,
    reporter: Annotated[str | None, typer.Option("--reporter", help="Filter by reporter.")] = None,
    node: Annotated[str | None, typer.Option("--node", help="Node id, current, unknown, or all.")] = None,
    limit: Annotated[int, typer.Option("--limit", help="Maximum rows.")] = 50,
    offset: Annotated[int, typer.Option("--offset", help="Rows to skip.")] = 0,
    sort: Annotated[str | None, typer.Option("--sort", help="Sort key.")] = None,
    direction: Annotated[str | None, typer.Option("--dir", help="Sort direction.")] = None,
    facets: Annotated[bool, typer.Option("--facets", help="Include filter facets.")] = False,
) -> int:
    _cli_project_config(ctx)
    status_code, payload = gap_ops.list_gaps(
        status=status,
        q=q,
        severity=severity,
        category=category,
        actor=actor,
        reporter=reporter,
        node=node,
        limit=limit,
        offset=offset,
        sort=sort,
        direction=direction,
        include_facets=facets,
    )
    _print_json(payload)
    return 0 if status_code < 400 else 1


@gaps_app.command("get", help="Show a Gap detail payload.")
def gaps_get_command(
    ctx: typer.Context,
    gap_id: Annotated[str, typer.Argument(help="Gap ID.")],
) -> int:
    _cli_project_config(ctx)
    status_code, payload = gap_ops.get_gap(gap_id.upper())
    _print_json(payload)
    return 0 if status_code < 400 else 1


@gaps_app.command("logs", help="List logs for a Gap round.")
def gaps_logs_command(
    ctx: typer.Context,
    gap_id: Annotated[str, typer.Argument(help="Gap ID.")],
    round_idx: Annotated[int, typer.Option("--round-idx", help="Round index.")] = 0,
    limit: Annotated[int, typer.Option("--limit", help="Maximum rows.")] = 50,
    offset: Annotated[int, typer.Option("--offset", help="Rows to skip.")] = 0,
) -> int:
    _cli_project_config(ctx)
    status_code, payload = gap_ops.get_gap_logs(
        gap_id.upper(),
        round_idx=round_idx,
        limit=limit,
        offset=offset,
    )
    _print_json(payload)
    return 0 if status_code < 400 else 1


@gaps_app.command("create", help="Create a Gap.")
def gaps_create_command(
    ctx: typer.Context,
    reporter: Annotated[str, typer.Option("--reporter", help="Reporter name.")],
    actual: Annotated[str, typer.Option("--actual", help="Current behavior.")] = "",
    target: Annotated[str, typer.Option("--target", help="Target behavior.")] = "",
    name: Annotated[str | None, typer.Option("--name", help="Gap name.")] = None,
    priority: Annotated[str, typer.Option("--priority", help="low, medium, or high.")] = "low",
    duplicate_decision: Annotated[
        str | None,
        typer.Option("--duplicate-decision", help="duplicate, original, or move_original_to_backlog."),
    ] = None,
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    cfg, runner_call = _backend_runner_for_cli(ctx, port)
    body = {
        "reporter": reporter,
        "actual": actual,
        "target": target,
        "priority": priority,
    }
    if name is not None:
        body["name"] = name
    if duplicate_decision is not None:
        body["duplicate_decision"] = duplicate_decision
    try:
        status_code, payload = import_ops.create_gap(runner_call, body)
    except Exception as e:
        raise click.ClickException(str(e)) from e
    _sync_gap_cli_mutation(cfg, payload, status_code, "refine: create gap")
    _print_json(payload)
    return 0 if status_code < 400 else 1


@gaps_app.command("update", help="Update Gap metadata or notes.")
def gaps_update_command(
    ctx: typer.Context,
    gap_id: Annotated[str, typer.Argument(help="Gap ID.")],
    name: Annotated[str | None, typer.Option("--name", help="Gap name.")] = None,
    priority: Annotated[str | None, typer.Option("--priority", help="low, medium, or high.")] = None,
    status: Annotated[str | None, typer.Option("--status", help="Workflow status.")] = None,
    notes: Annotated[str | None, typer.Option("--notes", help="Notes JSON, file path, or '-' for stdin.")] = None,
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    cfg, runner_call = _backend_runner_for_cli(ctx, port)
    body: dict[str, object] = {}
    if name is not None:
        body["name"] = name
    if priority is not None:
        body["priority"] = priority
    if status is not None:
        body["status"] = status
    if notes is not None:
        body["notes"] = _load_json_value(notes)
    conn = db.connect(cfg.sqlite_path)
    try:
        try:
            status_code, payload = gap_ops.update_gap(
                conn,
                runner_call,
                gap_id.upper(),
                body,
                background_processes_stopped=(
                    process_ops._truthy(db.get_setting(conn, "paused"))
                    or process_ops._truthy(db.get_setting(conn, "agents_paused"))
                ),
            )
        except Exception as e:
            raise click.ClickException(str(e)) from e
    finally:
        conn.close()
    _sync_gap_cli_mutation(cfg, payload, status_code, "refine: update gap")
    _print_json(payload)
    return 0 if status_code < 400 else 1


@gaps_app.command("delete", help="Delete a Gap.")
def gaps_delete_command(
    ctx: typer.Context,
    gap_id: Annotated[str, typer.Argument(help="Gap ID.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    return _run_gap_conn_mutation(
        ctx,
        port,
        "refine: delete gap",
        lambda conn, runner_call: gap_ops.delete_gap(conn, runner_call, gap_id.upper()),
    )


@gaps_app.command("append-round", help="Append a review round.")
def gaps_append_round_command(
    ctx: typer.Context,
    gap_id: Annotated[str, typer.Argument(help="Gap ID.")],
    reporter: Annotated[str, typer.Option("--reporter", help="Reporter name.")],
    actual: Annotated[str, typer.Option("--actual", help="Current behavior.")] = "",
    target: Annotated[str, typer.Option("--target", help="Target behavior.")] = "",
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    return _run_gap_conn_mutation(
        ctx,
        port,
        "refine: append gap round",
        lambda conn, runner_call: gap_ops.append_round(
            conn,
            runner_call,
            gap_id.upper(),
            {"reporter": reporter, "actual": actual, "target": target},
        ),
    )


@gaps_app.command("edit-round", help="Edit the latest unaddressed Gap round.")
def gaps_edit_round_command(
    ctx: typer.Context,
    gap_id: Annotated[str, typer.Argument(help="Gap ID.")],
    reporter: Annotated[str | None, typer.Option("--reporter", help="Reporter name.")] = None,
    actual: Annotated[str | None, typer.Option("--actual", help="Current behavior.")] = None,
    target: Annotated[str | None, typer.Option("--target", help="Target behavior.")] = None,
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    return _run_gap_conn_mutation(
        ctx,
        port,
        "refine: edit gap round",
        lambda conn, runner_call: gap_ops.edit_latest_round(
            conn,
            runner_call,
            gap_id.upper(),
            {"reporter": reporter, "actual": actual, "target": target},
        ),
    )


@gaps_app.command("verify", help="Approve a Gap in review.")
def gaps_verify_command(
    ctx: typer.Context,
    gap_id: Annotated[str, typer.Argument(help="Gap ID.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    return _run_gap_conn_mutation(
        ctx,
        port,
        "refine: verify gap",
        lambda conn, runner_call: gap_ops.verify(conn, runner_call, gap_id.upper()),
    )


@gaps_app.command("retry", help="Reopen a terminal Gap.")
def gaps_retry_command(
    ctx: typer.Context,
    gap_id: Annotated[str, typer.Argument(help="Gap ID.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    return _run_gap_conn_mutation(
        ctx,
        port,
        "refine: retry gap",
        lambda conn, runner_call: gap_ops.retry(conn, runner_call, gap_id.upper()),
    )


@gaps_app.command("retry-merge", help="Retry a failed merge.")
def gaps_retry_merge_command(
    ctx: typer.Context,
    gap_id: Annotated[str, typer.Argument(help="Gap ID.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    return _run_gap_conn_mutation(
        ctx,
        port,
        "refine: retry gap merge",
        lambda conn, runner_call: gap_ops.retry_merge(conn, runner_call, gap_id.upper()),
    )


@gaps_app.command("retry-quality", help="Retry a failed Quality run.")
def gaps_retry_quality_command(
    ctx: typer.Context,
    gap_id: Annotated[str, typer.Argument(help="Gap ID.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    return _run_gap_conn_mutation(
        ctx,
        port,
        "refine: retry gap quality",
        lambda conn, runner_call: gap_ops.retry_qa(conn, runner_call, gap_id.upper()),
    )


@gaps_app.command("cancel", help="Cancel a Gap.")
def gaps_cancel_command(
    ctx: typer.Context,
    gap_id: Annotated[str, typer.Argument(help="Gap ID.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    return _run_gap_conn_mutation(
        ctx,
        port,
        "refine: cancel gap",
        lambda conn, runner_call: gap_ops.cancel(conn, runner_call, gap_id.upper()),
    )


@gaps_app.command("bulk-update", help="Bulk update Gap priority, status, or reporter.")
def gaps_bulk_update_command(
    ctx: typer.Context,
    priority: Annotated[str | None, typer.Option("--priority", help="Set priority to low, medium, or high.")] = None,
    status_update: Annotated[str | None, typer.Option("--status-update", help="Set workflow status.")] = None,
    set_reporter: Annotated[str | None, typer.Option("--set-reporter", help="Set reporter name.")] = None,
    filter_json: Annotated[str | None, typer.Option("--filter", help="Filter JSON, file path, or '-' for stdin.")] = None,
    selected_ids: Annotated[str | None, typer.Option("--selected-ids", help="Selected Gap ids as JSON array or comma list.")] = None,
    exclude_ids: Annotated[str | None, typer.Option("--exclude-ids", help="Excluded Gap ids as JSON array or comma list.")] = None,
    status: Annotated[str | None, typer.Option("--status", help="Filter by workflow status.")] = None,
    q: Annotated[str | None, typer.Option("--q", help="Search query.")] = None,
    severity: Annotated[str | None, typer.Option("--severity", help="Filter by activity severity.")] = None,
    category: Annotated[str | None, typer.Option("--category", help="Filter by activity category.")] = None,
    actor: Annotated[str | None, typer.Option("--actor", help="Filter by activity actor.")] = None,
    reporter: Annotated[str | None, typer.Option("--reporter", help="Filter by reporter.")] = None,
    node: Annotated[str | None, typer.Option("--node", help="Node id, current, unknown, or all.")] = None,
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    update = {
        key: value
        for key, value in {
            "priority": priority,
            "status": status_update,
            "reporter": set_reporter,
        }.items()
        if value is not None
    }
    body = _bulk_gap_body(
        update=update,
        filter_json=filter_json,
        selected_ids=selected_ids,
        exclude_ids=exclude_ids,
        status=status,
        q=q,
        severity=severity,
        category=category,
        actor=actor,
        reporter=reporter,
        node=node,
    )
    return _run_gap_conn_mutation(
        ctx,
        port,
        "refine: bulk update gaps",
        lambda conn, runner_call: gap_ops.bulk_update_gaps(conn, runner_call, body),
    )


@gaps_app.command("bulk-delete", help="Bulk delete Gaps.")
def gaps_bulk_delete_command(
    ctx: typer.Context,
    filter_json: Annotated[str | None, typer.Option("--filter", help="Filter JSON, file path, or '-' for stdin.")] = None,
    selected_ids: Annotated[str | None, typer.Option("--selected-ids", help="Selected Gap ids as JSON array or comma list.")] = None,
    exclude_ids: Annotated[str | None, typer.Option("--exclude-ids", help="Excluded Gap ids as JSON array or comma list.")] = None,
    status: Annotated[str | None, typer.Option("--status", help="Filter by workflow status.")] = None,
    q: Annotated[str | None, typer.Option("--q", help="Search query.")] = None,
    severity: Annotated[str | None, typer.Option("--severity", help="Filter by activity severity.")] = None,
    category: Annotated[str | None, typer.Option("--category", help="Filter by activity category.")] = None,
    actor: Annotated[str | None, typer.Option("--actor", help="Filter by activity actor.")] = None,
    reporter: Annotated[str | None, typer.Option("--reporter", help="Filter by reporter.")] = None,
    node: Annotated[str | None, typer.Option("--node", help="Node id, current, unknown, or all.")] = None,
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    body = _bulk_gap_body(
        update={},
        filter_json=filter_json,
        selected_ids=selected_ids,
        exclude_ids=exclude_ids,
        status=status,
        q=q,
        severity=severity,
        category=category,
        actor=actor,
        reporter=reporter,
        node=node,
    )
    body.pop("update", None)
    return _run_gap_conn_mutation(
        ctx,
        port,
        "refine: bulk delete gaps",
        lambda conn, runner_call: gap_ops.bulk_delete_gaps(conn, runner_call, body),
    )


def _bulk_gap_body(
    *,
    update: dict[str, str],
    filter_json: str | None,
    selected_ids: str | None,
    exclude_ids: str | None,
    status: str | None,
    q: str | None,
    severity: str | None,
    category: str | None,
    actor: str | None,
    reporter: str | None,
    node: str | None,
) -> dict[str, object]:
    filt: dict[str, object] = {}
    if filter_json is not None:
        raw_filter = _load_json_value(filter_json)
        if not isinstance(raw_filter, dict):
            raise click.ClickException("filter must be a JSON object")
        filt.update(raw_filter)
    for key, value in {
        "status": status,
        "q": q,
        "severity": severity,
        "category": category,
        "actor": actor,
        "reporter": reporter,
        "node": node,
    }.items():
        if value is not None:
            filt[key] = value
    body: dict[str, object] = {"filter": filt, "update": update}
    if selected_ids is not None:
        body["selected_ids"] = _load_id_list(selected_ids)
    if exclude_ids is not None:
        body["exclude_ids"] = _load_id_list(exclude_ids)
    return body


def _load_id_list(value: str) -> list[str]:
    source = str(value or "").strip()
    if not source:
        return []
    try:
        raw = _load_json_value(source)
    except click.ClickException:
        raw = [part.strip() for part in source.split(",")]
    if not isinstance(raw, list):
        raise click.ClickException("Gap id list must be a JSON array or comma list")
    return [str(item or "").strip().upper() for item in raw if str(item or "").strip()]


def _run_gap_conn_mutation(
    ctx: typer.Context,
    port: int | None,
    message: str,
    operation: Callable[[object, object], tuple[int, dict]],
) -> int:
    cfg, runner_call = _backend_runner_for_cli(ctx, port)
    conn = db.connect(cfg.sqlite_path)
    try:
        try:
            status_code, payload = operation(conn, runner_call)
        except Exception as e:
            raise click.ClickException(str(e)) from e
    finally:
        conn.close()
    _sync_gap_cli_mutation(cfg, payload, status_code, message)
    _print_json(payload)
    return 0 if status_code < 400 else 1


def _sync_gap_cli_mutation(
    cfg: config.Config,
    payload: dict,
    status_code: int,
    message: str,
) -> None:
    if status_code >= 400:
        return
    if payload.get("created") is False:
        return
    payload["sync"] = _sync_cli_refine_state(cfg, message=message)


@changes_app.command("list", help="List merged Refine changes.")
def changes_list_command(
    ctx: typer.Context,
    limit: Annotated[int, typer.Option("--limit", help="Maximum rows.")] = 50,
    offset: Annotated[int, typer.Option("--offset", help="Rows to skip.")] = 0,
    q: Annotated[str | None, typer.Option("--q", help="Search query.")] = None,
    status: Annotated[str | None, typer.Option("--status", help="Filter by Gap status.")] = None,
    priority: Annotated[str | None, typer.Option("--priority", help="Filter by Gap priority.")] = None,
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    _cfg, runner_call = _backend_runner_for_cli(ctx, port)
    try:
        status_code, payload = gap_ops.list_changes(
            runner_call,
            limit=limit,
            offset=offset,
            q=q,
            status=status,
            priority=priority,
        )
    except Exception as e:
        raise click.ClickException(str(e)) from e
    _print_json(payload)
    return 0 if status_code < 400 else 1


@changes_app.command("undo", help="Undo a merged Refine change.")
def changes_undo_command(
    ctx: typer.Context,
    commit: Annotated[str, typer.Argument(help="Commit SHA to undo.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    return _run_gap_conn_mutation(
        ctx,
        port,
        "refine: undo change",
        lambda conn, runner_call: gap_ops.undo_change(
            conn,
            runner_call,
            {"commit": commit},
        ),
    )


@import_app.command("extract", help="Extract Gap drafts from raw text.")
def import_extract_command(
    ctx: typer.Context,
    source: Annotated[str, typer.Argument(help="Text, file path, or '-' for stdin.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    _cfg, runner_call = _backend_runner_for_cli(ctx, port)
    try:
        status, payload = import_ops.extract(
            runner_call,
            {"text": _load_text_value(source)},
        )
    except Exception as e:
        raise click.ClickException(str(e)) from e
    _print_json(payload)
    return 0 if status < 400 else 1


@import_app.command("parse-csv", help="Parse CSV into Gap import drafts.")
def import_parse_csv_command(
    ctx: typer.Context,
    source: Annotated[str, typer.Argument(help="CSV text, file path, or '-' for stdin.")],
    dedup: Annotated[bool, typer.Option("--dedup", help="Annotate possible duplicates.")] = False,
    distribute: Annotated[bool, typer.Option("--distribute", help="Assign drafts across enabled nodes.")] = False,
) -> int:
    _cli_project_config(ctx)
    status, payload = import_ops.parse_csv({
        "text": _load_text_value(source),
        "dedup": dedup,
        "distribute": distribute,
    })
    _print_json(payload)
    return 0 if status < 400 else 1


@import_app.command("dedup", help="Check Gap import drafts for duplicates.")
def import_dedup_command(
    ctx: typer.Context,
    source: Annotated[str, typer.Argument(help="Draft JSON, file path, or '-' for stdin.")],
) -> int:
    _cli_project_config(ctx)
    value = _load_json_value(source)
    body = {"drafts": value} if isinstance(value, list) else value
    if not isinstance(body, dict):
        raise click.ClickException("dedup input must be a JSON object or draft list")
    status, payload = import_ops.dedup(body)
    _print_json(payload)
    return 0 if status < 400 else 1


@import_app.command("persist", help="Persist reviewed Gap import drafts.")
def import_persist_command(
    ctx: typer.Context,
    source: Annotated[str, typer.Argument(help="Import JSON, file path, or '-' for stdin.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    cfg, runner_call = _backend_runner_for_cli(ctx, port)
    value = _load_json_value(source)
    body = {"drafts": value} if isinstance(value, list) else value
    if not isinstance(body, dict):
        raise click.ClickException("persist input must be a JSON object or draft list")
    try:
        status, payload = import_ops.persist(runner_call, body)
    except Exception as e:
        raise click.ClickException(str(e)) from e
    if status < 400 and _import_persist_changed(payload):
        payload["sync"] = _sync_cli_refine_state(cfg, message="refine: import gaps")
    _print_json(payload)
    return 0 if status < 400 else 1


def _import_persist_changed(payload: dict[str, object]) -> bool:
    if int(payload.get("count") or 0) > 0:
        return True
    actions = payload.get("duplicate_actions")
    if not isinstance(actions, dict):
        return False
    return any(
        int(actions.get(key) or 0) > 0
        for key in ("moved_to_backlog", "updated_original")
    )


@chat_app.command("start", help="Start a chat session.")
def chat_start_command(
    ctx: typer.Context,
    gap_id: Annotated[str | None, typer.Option("--gap-id", help="Gap ID to discuss.")] = None,
    purpose: Annotated[str | None, typer.Option("--purpose", help="Chat purpose.")] = None,
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    _cfg, runner_call = _backend_runner_for_cli(ctx, port)
    try:
        status, payload = chat_ops.start(
            runner_call,
            {"gap_id": gap_id, "purpose": purpose},
        )
    except Exception as e:
        raise click.ClickException(str(e)) from e
    _print_json(payload)
    return 0 if status < 400 and payload.get("ok", True) is not False else 1


@chat_app.command("input", help="Send input to a chat session.")
def chat_input_command(
    ctx: typer.Context,
    session_id: Annotated[str, typer.Argument(help="Chat session ID.")],
    text: Annotated[str, typer.Argument(help="Text to send.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    _cfg, runner_call = _backend_runner_for_cli(ctx, port)
    try:
        status, payload = chat_ops.input(runner_call, session_id, {"text": text})
    except Exception as e:
        raise click.ClickException(str(e)) from e
    _print_json(payload)
    return 0 if status < 400 and payload.get("ok", True) is not False else 1


@chat_app.command("read", help="Read a chat session snapshot.")
def chat_read_command(
    ctx: typer.Context,
    session_id: Annotated[str, typer.Argument(help="Chat session ID.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    _cfg, runner_call = _backend_runner_for_cli(ctx, port)
    try:
        status, payload = chat_ops.read(runner_call, session_id)
    except Exception as e:
        raise click.ClickException(str(e)) from e
    _print_json(payload)
    return 0 if status < 400 and payload.get("ok", True) is not False else 1


@chat_app.command("stop", help="Stop a chat session.")
def chat_stop_command(
    ctx: typer.Context,
    session_id: Annotated[str, typer.Argument(help="Chat session ID.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    _cfg, runner_call = _backend_runner_for_cli(ctx, port)
    try:
        status, payload = chat_ops.stop(runner_call, session_id)
    except Exception as e:
        raise click.ClickException(str(e)) from e
    _print_json(payload)
    return 0 if status < 400 and payload.get("ok", True) is not False else 1


@apps_app.command("status", help="Show the active target app for a port.")
def app_status_command(
    ctx: typer.Context,
    port: Annotated[
        int | None,
        typer.Option("--port", help="Refine port. Defaults to 8080."),
    ] = None,
) -> int:
    clone = Path.cwd().resolve()
    payload = project_apps.status(
        clone,
        port=_effective_port(_Args(port=port), None),
        include_nodes=True,
    )
    typer.echo(json.dumps(payload, indent=2))
    return 0


@apps_app.command("list", help="List known target apps for a port.")
def app_list_command(
    ctx: typer.Context,
    port: Annotated[
        int | None,
        typer.Option("--port", help="Refine port. Defaults to 8080."),
    ] = None,
) -> int:
    clone = Path.cwd().resolve()
    payload = project_apps.list_apps(
        clone,
        port=_effective_port(_Args(port=port), None),
    )
    typer.echo(json.dumps(payload, indent=2))
    return 0


@apps_app.command("templates", help="List app scaffold templates.")
def app_templates_command(
    _ctx: typer.Context,
) -> int:
    _print_json(project_apps.list_project_templates())
    return 0


@apps_app.command("sync", help="Sync the active target app through the backend runner.")
def app_sync_command(
    ctx: typer.Context,
    port: Annotated[int | None, typer.Option("--port", help="Refine port.")] = None,
) -> int:
    _cfg, runner_call = _backend_runner_for_cli(ctx, port)
    try:
        payload = runner_call(M_PROJECT_SYNC, {}, 120.0)
    except Exception as e:
        raise click.ClickException(str(e)) from e
    _print_json(payload)
    return 0 if payload.get("ok") else 1


@apps_app.command("scaffold", help="Create a high-priority scaffold Gap from an app template.")
def app_scaffold_command(
    ctx: typer.Context,
    template: Annotated[str, typer.Argument(help="Template id.")],
    reporter: Annotated[str, typer.Option("--reporter", help="Reporter name.")] = "Refine",
) -> int:
    cfg = _cli_project_config(ctx)
    status, payload = project_apps.create_project_scaffold_gap(
        lambda: db.connect(cfg.sqlite_path),
        {"template": template, "reporter": reporter},
    )
    if status < 400:
        payload["sync"] = _sync_cli_refine_state(
            cfg,
            message="refine: create scaffold gap",
        )
    _print_json(payload)
    return 0 if status < 400 else 1


@apps_app.command("attach", help="Create or attach a target app path and make it active.")
def app_attach_command(
    ctx: typer.Context,
    path: Annotated[str, typer.Argument(help="Path or Git remote for the target app.")],
    port: Annotated[int | None, typer.Option("--port", help="Refine port. Defaults to 8080.")] = None,
    migrate: Annotated[bool, typer.Option("--migrate", help="Run a pending Refine state migration.")] = False,
) -> int:
    clone_dir = Path.cwd().resolve()
    effective_port = _effective_port(_Args(port=port), None)
    status, payload = _cli_attach_project_payload(
        {
            "path": path,
            "migrate": migrate,
            "start_poller": False,
            "start_runner": False,
        },
        clone_dir=clone_dir,
        port=effective_port,
    )
    _print_json(payload)
    return 0 if status < 400 else 1


@apps_app.command("switch", help="Attach an existing target app and make it active.")
def app_switch_command(
    ctx: typer.Context,
    path: Annotated[str, typer.Argument(help="Path to the target app repo.")],
    port: Annotated[
        int | None,
        typer.Option("--port", help="Refine port. Defaults to 8080."),
    ] = None,
    force: Annotated[
        bool,
        typer.Option("--force", help="Overwrite an existing refine.toml."),
    ] = False,
) -> int:
    return _run_command(cmd_target, ctx, path=path, port=port, force=force)


@apps_app.command("remove", help="Remove a target app from the known-apps list.")
def app_remove_command(
    ctx: typer.Context,
    path: Annotated[str, typer.Argument(help="Path to remove.")],
    port: Annotated[
        int | None,
        typer.Option("--port", help="Refine port. Defaults to 8080."),
    ] = None,
) -> int:
    clone = Path.cwd().resolve()
    effective_port = _effective_port(_Args(port=port), None)
    status, payload = project_apps.remove_project(
        {"path": path, "start_runner": False, "start_poller": False},
        clone_dir=clone,
        port=effective_port,
        attach_next=lambda body: _cli_attach_project_payload(
            {**body, "start_runner": False, "start_poller": False},
            clone_dir=clone,
            port=effective_port,
        ),
        detach_current=_cli_detach_current_project,
        project_status=lambda: (
            200,
            project_apps.status(clone, port=effective_port, include_nodes=True),
        ),
    )
    _print_json(payload)
    return 0 if status < 400 else 1


@apps_app.command("start", help="Run the configured target-app start command.")
def app_start_command(
    ctx: typer.Context,
    port: Annotated[int | None, typer.Option("--port", help="Refine port. Defaults to 8080.")] = None,
) -> int:
    return _run_target_app_cli_operation(ctx, port, "start")


@apps_app.command("stop", help="Run the configured target-app stop command.")
def app_stop_command(
    ctx: typer.Context,
    port: Annotated[int | None, typer.Option("--port", help="Refine port. Defaults to 8080.")] = None,
) -> int:
    return _run_target_app_cli_operation(ctx, port, "stop")


@apps_app.command("rebuild", help="Queue the target-app rebuild worker.")
def app_rebuild_command(
    ctx: typer.Context,
    port: Annotated[int | None, typer.Option("--port", help="Refine port. Defaults to 8080.")] = None,
) -> int:
    cfg, effective_port = _target_app_cli_config(ctx, port)
    runner_call = _target_app_cli_runner_call(cfg, effective_port)
    try:
        status, payload = target_app_ops.queue_rebuild(runner_call)
    except Exception as e:
        raise click.ClickException(str(e)) from e
    typer.echo(json.dumps(payload, indent=2))
    return 0 if status < 400 else 1


@apps_app.command("check", help="Run an immediate target-app status check.")
def app_check_command(
    ctx: typer.Context,
    port: Annotated[int | None, typer.Option("--port", help="Refine port. Defaults to 8080.")] = None,
    quiet: Annotated[bool, typer.Option("--quiet", help="Do not persist status transitions.")] = False,
) -> int:
    cfg, effective_port = _target_app_cli_config(ctx, port)
    runner_call = _target_app_cli_runner_call(cfg, effective_port)
    try:
        status, payload = target_app_ops.check(
            lambda: db.connect(cfg.sqlite_path),
            runner_call,
            quiet=quiet,
        )
    except Exception as e:
        raise click.ClickException(str(e)) from e
    typer.echo(json.dumps(payload, indent=2))
    return 0 if status < 400 and payload.get("ok", True) is not False else 1


@apps_app.command("hard-reset", help="Hard reset the target app worktree through the runner.")
def app_hard_reset_command(
    ctx: typer.Context,
    port: Annotated[int | None, typer.Option("--port", help="Refine port. Defaults to 8080.")] = None,
) -> int:
    cfg, effective_port = _target_app_cli_config(ctx, port)
    runner_call = _target_app_cli_runner_call(cfg, effective_port)
    try:
        status, payload = target_app_ops.hard_reset(runner_call)
    except Exception as e:
        raise click.ClickException(str(e)) from e
    typer.echo(json.dumps(payload, indent=2))
    return 0 if status < 400 and payload.get("ok", True) is not False else 1


@apps_app.command("generate", help="Generate target-app command configuration.")
def app_generate_command(
    ctx: typer.Context,
    kind: Annotated[
        str,
        typer.Option("--kind", help="One of all, start, stop, rebuild, status."),
    ] = "all",
    port: Annotated[int | None, typer.Option("--port", help="Refine port. Defaults to 8080.")] = None,
) -> int:
    cfg, effective_port = _target_app_cli_config(ctx, port)
    runner_call = _target_app_cli_runner_call(cfg, effective_port)
    try:
        status, payload = target_app_ops.generate(runner_call, {"kind": kind})
    except Exception as e:
        raise click.ClickException(str(e)) from e
    typer.echo(json.dumps(payload, indent=2))
    return 0 if status < 400 and payload.get("ok", True) is not False else 1


def _run_target_app_cli_operation(
    ctx: typer.Context,
    port: int | None,
    kind: str,
) -> int:
    cfg, effective_port = _target_app_cli_config(ctx, port)
    runner_call = _target_app_cli_runner_call(cfg, effective_port)
    try:
        status, payload = target_app_ops.run(
            lambda: db.connect(cfg.sqlite_path),
            runner_call,
            kind,
        )
    except Exception as e:
        raise click.ClickException(str(e)) from e
    typer.echo(json.dumps(payload, indent=2))
    return 0 if status < 400 and payload.get("ok", True) is not False else 1


def _target_app_cli_config(
    ctx: typer.Context,
    port: int | None,
) -> tuple[config.Config, int]:
    clone = Path.cwd().resolve()
    if not project_apps.is_refine_source_dir(clone):
        raise click.ClickException(
            "target-app lifecycle commands must run from the Refine source checkout."
        )
    effective_port = _effective_port(_Args(port=port), None)
    cfg = _require_config_for_port(
        _Args(config=_ctx_config(ctx)),
        clone,
        effective_port,
        "app",
    )
    if cfg is None:
        raise click.ClickException(f"no app is attached on port {effective_port}")
    _ensure_config_initialized(cfg)
    return cfg, effective_port


def _ensure_config_initialized(cfg: config.Config) -> None:
    db.init_db(cfg.sqlite_path)
    conn = db.connect(cfg.sqlite_path)
    try:
        status = project_state.ensure_initialized(conn, migrate=True, root=cfg.volume_root)
        if not status.get("compatible"):
            raise click.ClickException(project_state.migration_block_details(status))
        project_state.rebuild_sqlite_cache(conn)
    finally:
        conn.close()


def _target_app_cli_runner_call(
    cfg: config.Config,
    port: int,
) -> target_app_ops.RunnerCall:
    from refine_runtime import ipc
    from refine_runtime.supervisor_protocol import (
        M_ENSURE_WORKER,
        WORKER_STARTUP_TIMEOUT_SECONDS,
    )

    def call(method: str, params: dict[str, object], timeout: float) -> dict:
        supervisor_socket = ipc.supervisor_socket_path(port, start=Path.cwd())
        worker = ipc.request(
            supervisor_socket,
            M_ENSURE_WORKER,
            {"config_path": str(cfg.config_path)},
            timeout=WORKER_STARTUP_TIMEOUT_SECONDS + 15.0,
        )
        worker_socket = str(
            worker.get("worker_socket")
            or worker.get("socket_path")
            or ipc.runner_socket_path(port=port, config_path=cfg.config_path),
        )
        return ipc.request(worker_socket, method, params, timeout=timeout)

    return call


@app.command(
    "target",
    help="Attach a target app and make it active.",
    epilog=(
        "Attaches a target app: creates or updates <app>/.refine/refine.toml "
        "+ gaps/, records the app in the port's known-apps list, and prepares "
        "the checkout for `refine start` or "
        "`refine install`."
    ),
)
def target_command(
    ctx: typer.Context,
    path: Annotated[
        str | None,
        typer.Argument(
            help="Path to the target app repo. Defaults to cwd.",
        ),
    ] = None,
    port: Annotated[
        int | None,
        typer.Option("--port", help="Refine port to attach this app to. Defaults to 8080."),
    ] = None,
    force: Annotated[
        bool,
        typer.Option(
            "--force",
            help="Overwrite an existing refine.toml or systemd unit.",
        ),
    ] = False,
) -> int:
    return _run_command(cmd_target, ctx, path=path, port=port, force=force)

@app.command(
    "install",
    help="Install and start a persistent refine service.",
    epilog=(
        "Writes, enables, and starts a system-level systemd unit for this "
        "checkout. The service runs as the installing user, restarts on "
        "failure, survives terminal close, and starts at boot. Pass a port "
        "to run multiple Refine nodes on one host."
    ),
)
def install_command(
    ctx: typer.Context,
    port: Annotated[
        int | None,
        typer.Argument(help="Web server port. Defaults to the configured port."),
    ] = None,
) -> int:
    return _run_command(cmd_install, ctx, port=port)


@app.command("uninstall", help="Stop and remove a persistent refine service.")
def uninstall_command(
    ctx: typer.Context,
    port: Annotated[
        int | None,
        typer.Argument(help="Web server port. Defaults to the configured port."),
    ] = None,
) -> int:
    return _run_command(cmd_uninstall, ctx, port=port)


@app.command(
    "reset",
    help="Remove local port state so this checkout can attach a different app.",
    epilog=(
        "Removes run/<port>/ state or all local run state, disables + removes "
        "persistent systemd units, and (with --purge) wipes the active app's "
        ".refine/ directory. The app source tree is never touched."
    ),
)
def reset_command(
    ctx: typer.Context,
    port: Annotated[
        int | None,
        typer.Argument(help="Optional port to reset. Omit to reset all local port state."),
    ] = None,
    purge: Annotated[
        bool,
        typer.Option(
            "--purge",
            help=(
                "Also delete the active target app's .refine/ directory "
                "(gap.json files, sqlite index, .gitignore). DATA LOSS."
            ),
        ),
    ] = False,
    yes: Annotated[
        bool,
        typer.Option("-y", "--yes", help="Skip the confirmation prompt for --purge."),
    ] = False,
) -> int:
    return _run_command(cmd_reset, ctx, port=port, purge=purge, yes=yes)


@app.command(
    "start",
    help="Start refine.",
    epilog=(
        "Starts host-native refine, then prints a status block. If "
        "this checkout has an installed systemd service, the command starts "
        "that service; otherwise it starts a detached background supervisor. "
        "The supervisor keeps the UI/control process separate from the work "
        "runner. Pass a port to run multiple Refine nodes on one host."
    ),
)
def start_command(
    ctx: typer.Context,
    port: Annotated[
        int | None,
        typer.Argument(help="Web server port. Defaults to the configured port."),
    ] = None,
) -> int:
    return _run_command(cmd_start, ctx, port=port)


@app.command(
    "restart",
    help="Stop refine, then start it again.",
    epilog="Equivalent to `refine stop && refine start`.",
)
def restart_command(
    ctx: typer.Context,
    port: Annotated[
        int | None,
        typer.Argument(help="Web server port. Defaults to the configured port."),
    ] = None,
) -> int:
    return _run_command(cmd_restart, ctx, port=port)


@app.command("stop", help="Stop refine.")
def stop_command(
    ctx: typer.Context,
    port: Annotated[
        int | None,
        typer.Argument(help="Web server port. Defaults to the configured port."),
    ] = None,
) -> int:
    return _run_command(cmd_stop, ctx, port=port)


@app.command("status", help="Show what's running (read-only).")
def status_command(
    ctx: typer.Context,
    port: Annotated[
        int | None,
        typer.Argument(help="Web server port. Defaults to the configured port."),
    ] = None,
) -> int:
    return _run_command(cmd_status, ctx, port=port)


@app.command(
    "update",
    help="Update Refine to the latest version.",
    epilog=(
        "Runs the Refine installer in update mode. The installer handles "
        "fresh setup, repair, and release updates."
    ),
)
def update_command(ctx: typer.Context) -> int:
    return _run_command(cmd_update, ctx)


@app.command(
    "ps",
    help="Show CPU and memory usage for refine processes.",
    epilog=(
        "Samples host process stats for the Refine UI/supervisor process "
        "and its children, including agent CLI subprocesses."
    ),
)
def ps_command(
    ctx: typer.Context,
    port: Annotated[
        int | None,
        typer.Argument(help="Web server port. Defaults to the configured port."),
    ] = None,
    sample: Annotated[
        float,
        typer.Option(
            "--sample",
            help="Seconds to sample CPU usage before printing. Default: 0.5.",
        ),
    ] = 0.5,
    watch: Annotated[
        float | None,
        typer.Option(
            "--watch",
            help="Repeat every N seconds. Default when supplied without N: 2.",
        ),
    ] = None,
    once: Annotated[
        bool,
        typer.Option("--once", help="Print one snapshot. This is the default."),
    ] = False,
    limit: Annotated[
        int,
        typer.Option(
            "--limit",
            help="Maximum process rows to print per port. Default: 30.",
        ),
    ] = 30,
) -> int:
    if watch is not None and once:
        typer.echo("refine ps: --watch and --once are mutually exclusive", err=True)
        return 2
    return _run_command(
        cmd_ps,
        ctx,
        port=port,
        sample=sample,
        watch=watch,
        once=once,
        limit=limit,
    )


@app.command(
    "test",
    help="Run the full test suite.",
    epilog=(
        "Runs every top-level tests/*_test.py script sequentially with the "
        "current Python interpreter. Returns non-zero if any test script fails."
    ),
)
def test_command(ctx: typer.Context) -> int:
    return _run_command(cmd_test, ctx)


@app.command("server", help="Run the server component in the foreground for debugging.")
def server_command(ctx: typer.Context) -> int:
    return _run_command(cmd_server_foreground, ctx)


@app.command("runner", hidden=True)
def runner_command(ctx: typer.Context) -> int:
    return server_command(ctx)


@app.command("ui", help="Start the UI in the foreground.")
def ui_command(ctx: typer.Context) -> int:
    return _run_command(cmd_ui, ctx)


@app.command("web", hidden=True)
def web_command(ctx: typer.Context) -> int:
    return ui_command(ctx)


@app.command("supervisor", hidden=True)
def supervisor_command(ctx: typer.Context) -> int:
    return _run_command(cmd_supervisor, ctx)


@app.command("doctor", help="Print a diagnostic snapshot.")
def doctor_command(ctx: typer.Context) -> int:
    return _run_command(cmd_doctor, ctx)


# ----- target -----------------------------------------------------------------

def cmd_target(args: _Args) -> int:
    cwd = Path.cwd().resolve()
    client_repo = Path(args.path).expanduser().resolve() if args.path else cwd
    port = _effective_port(args, None)

    try:
        result = bootstrap_client_repo(
            client_repo,
            clone_dir=cwd,
            port=port,
            force=args.force,
            create=False,
            init_git=False,
            reuse_existing_config=not args.force,
            install_unit=False,
        )
    except (config.ConfigError, _InitError) as e:
        print(f"refine target: {e}", file=sys.stderr)
        return 1

    cfg_path = result["config_path"]
    target = result["volume_root"]
    print(f"Wrote {cfg_path}")
    print(f"Created directories: {target}/gaps")
    if result.get("registry_path"):
        print(f"Set active target app → {client_repo}")
        print(f"Wrote {result['registry_path']}")
    print()
    print("Next steps:")
    if result.get("registry_path"):
        print(f"  uv run refine start {port}    # background refine supervisor")
        print(f"  uv run refine install {port}  # persistent service, auto-restarts")
        print(f"  uv run refine status {port}   # check it's healthy")
        print(f"  uv run refine stop {port}     # tear it all down")
    else:
        print(f"  cd {client_repo}")
        print(f"  refine doctor                 # sanity check the config")
        print()
        print("Note: full refine start/stop/status/install orchestration requires running")
        print("`refine target` from inside a refine source dir so the systemd")
        print("service can be wired up.")
    return 0


def _cli_load_project_attach_configured(
    config_path: Path,
    _start_poller: bool,
    _start_runner: bool,
    _migrate: bool,
    port: int,
) -> config.Config:
    cfg = config.get(path=str(config_path), reload=True, port=port)
    db.init_db(cfg.sqlite_path)
    conn = db.connect(cfg.sqlite_path)
    try:
        status = project_state.ensure_initialized(conn, migrate=True)
        if not status.get("compatible"):
            raise config.ConfigError(project_state.migration_block_details(status))
        project_state.rebuild_sqlite_cache(conn)
    finally:
        conn.close()
    return cfg


def _cli_attach_project_payload(
    body: dict[str, object],
    *,
    clone_dir: Path,
    port: int,
) -> tuple[int, dict[str, object]]:
    return project_apps.attach_project(
        body,
        clone_dir=clone_dir,
        port=port,
        load_configured=_cli_load_project_attach_configured,
        current_client_repo=lambda: _cli_current_client_repo(port),
        loaded_client_repo=lambda: None,
        prepare_current_project_for_switch=_cli_prepare_current_project_for_switch,
        commit_refine_state=_cli_commit_refine_state,
        node_summary=_cli_node_summary,
    )


def _cli_detach_current_project(clone_dir: Path, _target: Path, port: int | None) -> None:
    project_registry.detach_port(clone_dir, port=port)


def _cli_current_client_repo(port: int) -> Path | None:
    try:
        return config.get(reload=True, port=port).client_repo
    except config.ConfigError:
        return None


def _cli_prepare_current_project_for_switch(current_repo: Path | None) -> dict[str, object]:
    warnings: list[str] = []
    if current_repo is None:
        return {"warnings": warnings}
    _cli_commit_refine_state(current_repo)
    dirty = _git_stdout_for_cli(current_repo, ["status", "--porcelain"])
    if dirty.strip():
        raise project_apps.SwitchBlocked(
            "Current app has uncommitted changes.",
            (
                "Commit, stash, or discard changes in the current app before switching:\n"
                + dirty.strip()
            ),
        )
    return {"warnings": warnings}


def _cli_commit_refine_state(repo: Path) -> None:
    config.ensure_refine_gitignore(repo / ".refine")
    dirty_refine = _git_stdout_for_cli(repo, ["status", "--porcelain", "--", ".refine"])
    if not dirty_refine.strip():
        return
    repo_cfg = config.Config.load(repo / ".refine" / config.CONFIG_FILENAME)
    db.init_db(repo_cfg.sqlite_path)
    conn = db.connect(repo_cfg.sqlite_path)
    try:
        result = project_sync.commit_and_push_refine_state(
            conn,
            actor="cli",
            cwd=repo,
            state_message="refine: sync project state before switch",
        )
    finally:
        conn.close()
    if result.get("ok"):
        return
    raise project_apps.SwitchBlocked(
        "Could not commit current app Refine state.",
        str(result.get("details") or result.get("message") or "git commit failed").strip(),
    )


def _git_stdout_for_cli(repo: Path, args: list[str]) -> str:
    out = subprocess.run(
        ["git", *args],
        cwd=str(repo),
        capture_output=True,
        text=True,
        timeout=30,
    )
    if out.returncode != 0:
        raise project_apps.SwitchBlocked(
            "Could not inspect current app git state.",
            (out.stderr or out.stdout or f"git {' '.join(args)} failed").strip(),
        )
    return out.stdout


def _cli_node_summary() -> dict[str, object]:
    try:
        return node_ops.summary()
    except Exception:
        return {"nodes": [], "active_node_id": ""}


def cmd_test(_args: _Args) -> int:
    root = Path(__file__).resolve().parents[1]
    tests_dir = root / "tests"
    tests = sorted(tests_dir.glob("*_test.py"))
    if not tests:
        print(f"refine test: no tests found in {tests_dir}", file=sys.stderr)
        return 1

    failures: list[Path] = []
    env = os.environ.copy()
    root_path = str(root)
    env["PYTHONPATH"] = (
        root_path if not env.get("PYTHONPATH")
        else f"{root_path}{os.pathsep}{env['PYTHONPATH']}"
    )
    print(f"Running {len(tests)} test scripts")
    for test in tests:
        rel = test.relative_to(root)
        print(f"\n=== {rel} ===", flush=True)
        result = subprocess.run([sys.executable, str(test)], cwd=str(root), env=env)
        if result.returncode != 0:
            failures.append(rel)

    if failures:
        print("\nFAILED:")
        for failed in failures:
            print(f"  {failed}")
        return 1
    print("\nALL TESTS OK")
    return 0


def cmd_update(_args: _Args) -> int:
    print(f"Running: {README_INSTALL_COMMAND}", flush=True)
    try:
        result = subprocess.run(["bash", "-lc", README_INSTALL_COMMAND])
    except OSError as e:
        print(f"refine update: could not launch installer: {e}", file=sys.stderr)
        return 1
    return int(result.returncode)


def _is_refine_source_dir(p: Path) -> bool:
    return project_apps.is_refine_source_dir(p)


_InitError = project_apps.InitError


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
) -> dict[str, Path | bool | None]:
    """Create or attach a target app using the shared project-app operation."""
    return project_apps.bootstrap_client_repo(
        client_repo,
        clone_dir=clone_dir,
        port=port,
        force=force,
        create=create,
        init_git=init_git,
        reuse_existing_config=reuse_existing_config,
        install_unit=install_unit,
        prepare_clone=lambda clone: _remove_legacy_docker_artifacts(clone),
        install_ui_unit=(
            lambda clone, _client: _write_and_enable_ui_unit(
                clone,
                None,
                force=force,
                port=port or config.runtime_port(),
            )
        ),
    )


def _write_and_enable_ui_unit(
    clone_dir: Path,
    client_repo: Path | None,
    *,
    force: bool,
    runner_unit_name: str | None = None,
    host: str = "0.0.0.0",
    port: int = 8080,
) -> Path:
    """Write the refine systemd system unit, daemon-reload, and enable it.

    Refuses if a unit by the same name already points at a different checkout,
    unless --force is given (in which case it's overwritten).
    """
    runner_unit = runner_unit_name or config.unit_name_for(clone_dir)
    ui_unit = _ui_unit_name(runner_unit, port)
    ui_unit_path = SYSTEMD_SYSTEM_DIR / f"{ui_unit}.service"
    _remove_legacy_runtime_units(runner_unit)
    _remove_legacy_user_ui_unit(ui_unit)

    if not force:
        existing = _read_unit_text(ui_unit_path)
        if existing is not None:
            existing_wd = _grep_first(existing, "WorkingDirectory=")
            if existing_wd and existing_wd != str(clone_dir):
                raise _InitError(
                    f"systemd unit {ui_unit} already exists for a different checkout:\n"
                    f"  existing WorkingDirectory: {existing_wd}\n"
                    f"  this checkout:             {clone_dir}\n"
                    f"Use --force to overwrite, or rename one of the checkouts."
                )

    uv = _find_host_command("uv")
    if uv is None:
        raise _InitError(
            "could not find `uv` on PATH or the login-shell PATH; install it "
            "before running `refine target` (the systemd unit needs an absolute "
            "path to invoke it)."
        )
    captured_env = dict(os.environ)
    if "PATH" not in captured_env:
        login_path = _user_login_path()
        if login_path:
            captured_env["PATH"] = login_path
    service_user = _service_user()

    ui_body = (
        "# Auto-generated by `refine install` / `refine target`. Do not edit by hand — re-run\n"
        "# the setup command with --force to regenerate.\n"
        "[Unit]\n"
        f"Description=refine — {clone_dir} — port {port}\n"
        "After=network-online.target\n"
        "Wants=network-online.target\n"
        "\n"
        "[Service]\n"
        "Type=simple\n"
        f"User={service_user}\n"
        f"WorkingDirectory={clone_dir}\n"
        f"{_systemd_environment_lines(captured_env)}"
        f"Environment=REFINE_UI_HOST={host}\n"
        f"Environment=REFINE_UI_PORT={port}\n"
        f"Environment=REFINE_UI_SCOPE={port}\n"
        f"Environment={config.ENV_RUN_DIR}={config.local_run_dir(clone_dir, port=port)}\n"
        f"ExecStart={uv} run refine supervisor\n"
        "Restart=on-failure\n"
        "RestartSec=2s\n"
        "TimeoutStopSec=30s\n"
        "\n"
        "[Install]\n"
        "WantedBy=multi-user.target\n"
    )
    _write_system_unit(ui_unit_path, ui_body)

    rc, out = _systemctl("daemon-reload")
    if rc != 0:
        raise _InitError(f"systemctl daemon-reload failed: {out.strip()}")
    rc, out = _systemctl("enable", ui_unit)
    if rc != 0:
        raise _InitError(f"systemctl enable {ui_unit} failed: {out.strip()}")

    return ui_unit_path


def _grep_first(text: str, prefix: str) -> str | None:
    for line in text.splitlines():
        if line.startswith(prefix):
            return line[len(prefix):].strip()
    return None


# ----- start / stop / status -------------------------------------------------

def cmd_install(args: _Args) -> int:
    clone, unit = _resolve_clone_and_unit_or_exit()
    port = _effective_port(args, None)
    cfg = _config_for_port(args, clone, port)
    host = cfg.web_host if cfg is not None else SETUP_UI_HOST
    try:
        ui_unit_path = _write_and_enable_ui_unit(
            clone,
            None,
            force=True,
            runner_unit_name=unit,
            host=host,
            port=port,
        )
    except _InitError as e:
        print(f"refine install: {e}", file=sys.stderr)
        return 1
    ui_unit = _ui_unit_name(unit, port)
    print(f"Installed refine unit: {ui_unit_path}")
    print(f"Starting persistent refine (systemctl start {ui_unit})...")
    rc, out = _systemctl("start", ui_unit)
    if rc != 0:
        print(f"refine install: {out.strip()}", file=sys.stderr)
        return 1
    if not _wait_for_port(host, port, timeout=20.0):
        print(
            f"refine install: refine did not start listening on "
            f"{host}:{port} within 20s. "
            f"Check `journalctl -u {ui_unit} -n 200`.",
            file=sys.stderr,
        )
        return 1
    if cfg is not None:
        _print_status_block(clone, unit, cfg, port=port)
    else:
        _print_setup_status_block(clone, port=port, unit=unit)
    return 0


def cmd_uninstall(args: _Args) -> int:
    clone, unit = _resolve_clone_and_unit_or_exit()
    port = _runtime_action_port(args, clone, None, unit)
    ui_unit = _ui_unit_name(unit, port)
    print(f"Stopping persistent refine (systemctl stop {ui_unit})...")
    rc, out = _systemctl("stop", ui_unit)
    if rc != 0:
        print(f"  (systemctl: {out.strip()})", file=sys.stderr)
    rc, out = _systemctl("disable", ui_unit)
    if rc != 0:
        print(f"  (systemctl disable {ui_unit}: {out.strip()})", file=sys.stderr)
    unit_path = SYSTEMD_SYSTEM_DIR / f"{ui_unit}.service"
    if _unit_file_exists(unit_path):
        if _remove_system_unit(unit_path):
            print(f"Removed {unit_path}")
            _systemctl("daemon-reload")
        else:
            print(f"Could not remove {unit_path}", file=sys.stderr)
    else:
        print(f"No unit file found at {unit_path}")
    legacy_path = SYSTEMD_USER_DIR / f"{ui_unit}.service"
    if _remove_legacy_user_ui_unit(ui_unit):
        print(f"Removed legacy user unit {legacy_path}")
    _remove_legacy_runtime_units(unit)
    return 0


def cmd_start(args: _Args) -> int:
    clone, unit = _resolve_clone_and_unit_or_exit()
    port = _runtime_action_port(args, clone, None, unit)
    cfg = _config_for_port(args, clone, port)
    if cfg is None:
        print("No refine project is attached yet.")
        print(
            "Starting refine at "
            f"http://{_displayable_host(SETUP_UI_HOST)}:{port}"
        )
        print("Use the browser to create or attach a target app path.")
        if _installed_ui_unit(unit, port) is not None:
            return _start_setup_systemd_ui(clone, unit, port)
        if _port_open(SETUP_UI_HOST, port):
            print(f"Refine is already reachable on port {port}.")
            _print_setup_status_block(clone, port=port, unit=unit)
            _print_upgrade_notice(clone)
            return 0
        try:
            pid = _start_background_ui(clone, None, host=SETUP_UI_HOST, port=port)
        except _InitError as e:
            print(f"refine start: {e}", file=sys.stderr)
            return 1
        if not _wait_for_port(SETUP_UI_HOST, port, timeout=20.0):
            print(
                f"refine start: refine did not start listening on "
                f"{SETUP_UI_HOST}:{port} within 20s.",
                file=sys.stderr,
            )
            return 1
        print(f"Started refine on port {port} (pid {pid}).")
        _print_setup_status_block(clone, port=port, unit=unit)
        _print_upgrade_notice(clone)
        return 0

    try:
        _ensure_sqlite_schema(cfg)
    except _InitError as e:
        print(f"refine start: {e}", file=sys.stderr)
        return 1
    ui_unit = _installed_ui_unit(unit, port)
    if ui_unit is not None:
        return _start_systemd_ui(clone, unit, cfg, port)
    if _port_open(cfg.web_host, port):
        print(f"Refine is already reachable on port {port}.")
        _print_status_block(clone, unit, cfg, port=port)
        _print_upgrade_notice(clone)
        return 0
    try:
        pid = _start_background_ui(clone, cfg, host=cfg.web_host, port=port)
    except _InitError as e:
        print(f"refine start: {e}", file=sys.stderr)
        return 1

    print(f"Starting refine on port {port} (pid {pid})...")
    if not _wait_for_port(cfg.web_host, port, timeout=20.0):
        log_path = _runtime_log_path(clone, cfg, port)
        print(
            f"refine start: refine did not start listening on "
            f"{cfg.web_host}:{port} within 20s. "
            f"Check `{log_path}`.",
            file=sys.stderr,
        )
        return 1

    _print_status_block(clone, unit, cfg, port=port)
    _print_upgrade_notice(clone)
    return 0


def cmd_stop(args: _Args) -> int:
    clone, unit = _resolve_clone_and_unit_or_exit()
    port = _runtime_action_port(args, clone, None, unit)
    cfg = _config_for_port(args, clone, port)
    if cfg is None:
        if _installed_ui_unit(unit, port) is not None:
            return _stop_setup_systemd_ui(clone, unit, port)
        stopped = _stop_background_ui(clone, None, port)
        if stopped:
            print(f"Stopped refine on port {port}.")
        else:
            print(f"No refine process was running on port {port}.")
        return 0

    ui_unit = _installed_ui_unit(unit, port)
    if ui_unit is not None:
        _pause_agents_for_clean_shutdown(cfg, port)
        return _stop_systemd_ui(clone, unit, cfg, port)
    _pause_agents_for_clean_shutdown(cfg, port)
    stopped = _stop_background_ui(clone, cfg, port)
    if stopped:
        print(f"Stopped refine on port {port}.")
    else:
        print(f"No refine process was running on port {port}.")
    return 0


def cmd_restart(args: _Args) -> int:
    """`refine stop && refine start` — picks up source changes the
    running host processes haven't loaded yet without forcing the operator
    to run two commands."""
    clone, unit = _resolve_clone_and_unit_or_exit()
    port = _runtime_action_port(args, clone, None, unit)
    cfg = _config_for_port(args, clone, port)
    if cfg is None:
        if _installed_ui_unit(unit, port) is not None:
            return _restart_setup_systemd_ui(clone, unit, port)
        restart_args = _Args(**vars(args))
        restart_args.port = port
        rc = cmd_stop(restart_args)
        if rc != 0:
            return rc
        print()
        return cmd_start(restart_args)

    try:
        _ensure_sqlite_schema(cfg)
    except _InitError as e:
        print(f"refine restart: {e}", file=sys.stderr)
        return 1
    ui_unit = _installed_ui_unit(unit, port)
    if ui_unit is not None:
        _pause_agents_for_clean_shutdown(cfg, port)
        return _restart_systemd_ui(clone, unit, cfg, port)
    restart_args = _Args(**vars(args))
    restart_args.port = port
    rc = cmd_stop(restart_args)
    if rc != 0:
        return rc
    print()
    return cmd_start(restart_args)


def cmd_reset(args: _Args) -> int:
    """Remove this checkout's local port state."""
    cwd = Path.cwd().resolve()
    if not _is_refine_source_dir(cwd):
        print("refine reset: run this from a Refine source checkout.", file=sys.stderr)
        return 1

    unit = config.unit_name_for(cwd)
    port = getattr(args, "port", None)
    client_repo = project_registry.active_app(cwd, port=port) if port is not None else None
    client_refine_dir = (client_repo / ".refine") if client_repo else None

    # Purge confirmation up front (refuse to silently delete data).
    if args.purge:
        if not client_refine_dir or not client_refine_dir.is_dir():
            print("refine reset: --purge: no client .refine/ directory to delete.")
        elif not args.yes:
            print(f"--purge will DELETE {client_refine_dir}")
            print("This removes ALL gap data, the sqlite index, and run state.")
            try:
                answer = input("Type 'yes' to confirm: ").strip().lower()
            except EOFError:
                answer = ""
            if answer != "yes":
                print("Aborted.")
                return 1

    # 1. Stop + remove persistent units (best-effort — keep going if already down).
    print("Stopping persistent refine units...")
    removed_units = False
    unit_names = [_ui_unit_name(unit, int(port))] if port is not None else _unit_names_for_reset(unit)
    for unit_name in unit_names:
        rc, out = _systemctl("stop", unit_name)
        if rc != 0:
            print(f"  (systemctl stop {unit_name}: {out.strip()})", file=sys.stderr)
        rc, out = _systemctl("disable", unit_name)
        if rc != 0:
            # If it wasn't enabled or doesn't exist, that's fine.
            print(f"  (systemctl disable {unit_name}: {out.strip()})", file=sys.stderr)
        unit_path = SYSTEMD_SYSTEM_DIR / f"{unit_name}.service"
        if _unit_file_exists(unit_path):
            if _remove_system_unit(unit_path):
                removed_units = True
                print(f"Removed {unit_path}")
        legacy_unit_path = SYSTEMD_USER_DIR / f"{unit_name}.service"
        if legacy_unit_path.exists():
            _systemctl_user("stop", unit_name)
            _systemctl_user("disable", unit_name)
            _remove_user_unit_file(legacy_unit_path)
            removed_units = True
            print(f"Removed {legacy_unit_path}")
    if removed_units:
        _systemctl("daemon-reload")
        _systemctl_user("daemon-reload")

    # 3. Remove port-local run state and legacy checkout state.
    if port is not None:
        run_dir = config.local_run_dir(cwd, port=port)
        if run_dir.exists():
            shutil.rmtree(run_dir)
            print(f"Removed {run_dir}")
    else:
        run_root = config.local_run_root(cwd)
        if run_root.exists():
            shutil.rmtree(run_root)
            print(f"Removed {run_root}")
    legacy_binding = cwd / config.BINDING_FILENAME
    if legacy_binding.exists():
        legacy_binding.unlink()
        print(f"Removed legacy {legacy_binding}")
    _remove_legacy_docker_artifacts(cwd, verbose=True)
    legacy_registry = cwd / project_registry.LEGACY_REGISTRY_FILENAME
    if legacy_registry.exists():
        legacy_registry.unlink()
        print(f"Removed legacy {legacy_registry}")

    # 4. Optional: purge the target app's .refine/ directory.
    if args.purge and client_refine_dir and client_refine_dir.is_dir():
        shutil.rmtree(client_refine_dir)
        print(f"Removed {client_refine_dir}")

    print()
    print("Reset complete. To attach a target app:")
    print(f"  cd {cwd}")
    suffix = f" --port {port}" if port is not None else ""
    print(f"  uv run refine target{suffix} <path/to/target-app>")
    if client_refine_dir and client_refine_dir.is_dir() and not args.purge:
        print()
        print(f"The previous app's refine data is preserved at:")
        print(f"  {client_refine_dir}")
        print("Re-running `refine target` against that path will pick it up again.")
    return 0


def cmd_status(args: _Args) -> int:
    clone, unit = _resolve_clone_and_unit_or_exit()
    for port in _status_ports(args, clone, None, unit):
        cfg = _config_for_port(args, clone, port)
        if cfg is None:
            _print_setup_status_block(clone, port=port, unit=unit)
        else:
            _print_status_block(clone, unit, cfg, port=port)
    return 0


def cmd_ps(args: _Args) -> int:
    if args.sample < 0:
        print("refine ps: --sample must be 0 or greater", file=sys.stderr)
        return 1
    watch_interval = args.watch
    if watch_interval is not None and watch_interval <= 0:
        print("refine ps: --watch interval must be greater than 0", file=sys.stderr)
        return 1
    if args.limit <= 0:
        print("refine ps: --limit must be greater than 0", file=sys.stderr)
        return 1

    if watch_interval is None:
        try:
            if sys.stdout.isatty():
                rc, frame = _render_performance_snapshot_frame(args)
                _write_truncated_frame(frame)
                return rc
            return _print_performance_snapshot(args)
        except KeyboardInterrupt:
            print()
            return 130

    live = sys.stdout.isatty()
    rendered_lines = 0
    try:
        while True:
            rc, frame = _render_performance_watch_frame(args, is_tty=False)
            if live:
                rendered_lines = _write_in_place_frame(frame, rendered_lines)
            else:
                print()
                sys.stdout.write(frame)
                sys.stdout.flush()
            time.sleep(watch_interval)
    except KeyboardInterrupt:
        print()
        return 130


def _print_performance_snapshot(args: _Args) -> int:
    clone, unit = _resolve_clone_and_unit_or_exit()
    for port in _status_ports(args, clone, None, unit):
        cfg = _config_for_port(args, clone, port)
        _print_performance_block(
            clone, cfg, unit, port=port,
            sample_seconds=args.sample, limit=args.limit,
        )
    return 0


class _PerformanceCapture(StringIO):
    def __init__(self, *, is_tty: bool) -> None:
        super().__init__()
        self._is_tty = is_tty

    def isatty(self) -> bool:
        return self._is_tty


def _render_performance_watch_frame(args: _Args, *, is_tty: bool) -> tuple[int, str]:
    buf = _PerformanceCapture(is_tty=is_tty)
    with redirect_stdout(buf):
        print(f"refine ps sampled at {time.strftime('%Y-%m-%d %H:%M:%S')}")
        rc = _print_performance_snapshot(args)
    return rc, buf.getvalue()


def _render_performance_snapshot_frame(args: _Args) -> tuple[int, str]:
    buf = _PerformanceCapture(is_tty=False)
    with redirect_stdout(buf):
        rc = _print_performance_snapshot(args)
    return rc, buf.getvalue()


def _write_truncated_frame(frame: str) -> None:
    lines = _terminal_frame_lines(frame)
    for line in lines:
        print(line)


def _write_in_place_frame(frame: str, previous_lines: int) -> int:
    if previous_lines > 0:
        sys.stdout.write(f"\033[{previous_lines}A\r")
    lines = _terminal_frame_lines(frame)
    printed_lines = max(previous_lines, len(lines))
    for line in lines:
        sys.stdout.write(f"\033[2K{line}\n")
    for _ in range(printed_lines - len(lines)):
        sys.stdout.write("\033[2K\n")
    sys.stdout.flush()
    return printed_lines


def _terminal_frame_lines(frame: str) -> list[str]:
    width = max(20, shutil.get_terminal_size((120, 24)).columns)
    limit = max(1, width - 1)
    return [_truncate(line, limit) for line in frame.splitlines()]


def _print_status_block(clone: Path, unit: str, cfg: "config.Config", *,
                        port: int | None = None) -> None:
    effective_port = port or cfg.web_port
    ui_unit = _ui_unit_name(unit, effective_port)
    web_up = _port_open(cfg.web_host, effective_port)
    process_pid = _running_pid(clone, cfg, effective_port)
    supervisor_status = _supervisor_status(clone, cfg, effective_port)
    display_cfg = _running_config(process_pid) or cfg
    service_active = _systemctl_is_active(ui_unit)
    ui = supervisor_status.get("ui") if supervisor_status else {}
    worker = supervisor_status.get("worker") if supervisor_status else {}
    ui_pid = ui.get("pid") if isinstance(ui, dict) else None
    worker_pid = worker.get("pid") if isinstance(worker, dict) else None

    print()
    print(_bold(f"refine ({effective_port})"))
    print(f"  checkout: {clone}")
    print(f"  app:      {display_cfg.client_repo}")
    print(f"  ui:       {_dot((process_pid is not None or service_active) and web_up)} "
          f"http {'reachable' if web_up else 'unreachable'} at "
          f"http://{_displayable_host(cfg.web_host)}:{effective_port}")
    print(f"  supervisor: {_dot(process_pid is not None)} "
          f"{'pid ' + str(process_pid) if process_pid is not None else 'not running'}")
    print(f"  ui pid:   {ui_pid if ui_pid is not None else 'unknown'}")
    print(f"  worker:   {worker_pid if worker_pid is not None else 'not running'}")
    print(f"  service:  {_dot(service_active)} systemd unit `{ui_unit}` "
          f"({'active' if service_active else 'inactive'})")
    print(f"  server:   {_dot((process_pid is not None or service_active) and web_up)} "
          "supervisor-managed UI + runner worker")
    print(f"  logs:     {_runtime_log_path(clone, display_cfg, effective_port)}")
    print(f"  journal:  journalctl -u {ui_unit} -f")
    print(f"  stop:     uv run refine stop {effective_port}")
    print()


def _print_setup_status_block(clone: Path, *, port: int, unit: str | None = None) -> None:
    ui_unit = _ui_unit_name(unit, port) if unit is not None else ""
    web_up = _port_open(SETUP_UI_HOST, port)
    process_pid = _running_pid(clone, None, port)
    supervisor_status = _supervisor_status(clone, None, port)
    service_active = _systemctl_is_active(ui_unit) if ui_unit else False
    ui = supervisor_status.get("ui") if supervisor_status else {}
    ui_pid = ui.get("pid") if isinstance(ui, dict) else None
    print()
    print(_bold(f"refine ({port})"))
    print(f"  checkout: {clone}")
    print("  app:      not attached")
    print(f"  ui:       {_dot((process_pid is not None or service_active) and web_up)} "
          f"http {'reachable' if web_up else 'unreachable'} at "
          f"http://{_displayable_host(SETUP_UI_HOST)}:{port}")
    print(f"  supervisor: {_dot(process_pid is not None)} "
          f"{'pid ' + str(process_pid) if process_pid is not None else 'not running'}")
    print(f"  ui pid:   {ui_pid if ui_pid is not None else 'unknown'}")
    if ui_unit:
        print(f"  service:  {_dot(service_active)} systemd unit `{ui_unit}` "
              f"({'active' if service_active else 'inactive'})")
    print(f"  logs:     {_runtime_log_path(clone, None, port)}")
    if ui_unit:
        print(f"  journal:  journalctl -u {ui_unit} -f")
    print(f"  stop:     uv run refine stop {port}")
    print()


def _print_upgrade_notice(clone: Path) -> None:
    info = upgrade.status(clone)
    if not info.upgrade_available:
        return
    current = info.current_version or "unknown"
    print(_section("Upgrade available"))
    print(f"Refine {info.latest_version} is available (current {current}).")
    print("Upgrade with:")
    print(f"  {info.command}")
    print()


def _print_performance_block(
    clone: Path,
    cfg: "config.Config | None",
    unit: str | None,
    *,
    port: int,
    sample_seconds: float,
    limit: int,
) -> None:
    display_cfg = cfg
    root_pids = _refine_performance_roots(clone, cfg, unit, port)
    if display_cfg is None and root_pids:
        display_cfg = _running_config(root_pids[0])
    pids = _process_tree_pids(root_pids)
    rows = _sample_process_rows(pids, sample_seconds=sample_seconds)
    total_cpu = sum(row["cpu_percent"] for row in rows)
    total_rss = sum(row["rss_kb"] for row in rows)
    total_vms = sum(row["vms_kb"] for row in rows)

    print()
    print(_bold("refine ps"))
    print(f"  checkout: {clone}")
    print(f"  app:      {display_cfg.client_repo if display_cfg is not None else 'not attached'}")
    print(f"  port:     {port}")
    print(f"  roots:    {_format_pid_list(root_pids) if root_pids else 'none'}")
    print(
        f"  totals:   {len(rows)} process(es), "
        f"CPU {total_cpu:.1f}%, RSS {_format_mib(total_rss)}, VMS {_format_mib(total_vms)}"
    )
    if not rows:
        print("  No refine UI/supervisor processes found for this port.")
        print()
        return

    print()
    print(
        "  "
        f"{'PID':>7} {'PPID':>7} {'PGID':>7} {'S':<2} "
        f"{'CPU%':>6} {'MEM%':>6} {'RSS':>9} {'VMS':>9} "
        f"{'ELAPSED':>10} {'ROLE':<10} COMMAND"
    )
    shown = rows[:limit]
    for row in shown:
        print(
            "  "
            f"{row['pid']:>7} {row['ppid']:>7} {row['pgid']:>7} {row['state']:<2} "
            f"{row['cpu_percent']:>6.1f} {row['mem_percent']:>6.1f} "
            f"{_format_mib(row['rss_kb']):>9} {_format_mib(row['vms_kb']):>9} "
            f"{_format_elapsed(row['elapsed_seconds']):>10} "
            f"{_process_role(row['pid'], root_pids, row['command']):<10} "
            f"{_truncate(row['command'], 100)}"
        )
    if len(rows) > len(shown):
        print(f"  ... {len(rows) - len(shown)} more process(es); rerun with --limit {len(rows)}")
    print()


def _refine_performance_roots(
    clone: Path,
    cfg: "config.Config | None",
    unit: str | None,
    port: int,
) -> list[int]:
    roots: list[int] = []
    running = _running_pid(clone, cfg, port)
    if running is not None:
        roots.append(running)
    if unit is not None:
        service_pid = _systemd_main_pid(_ui_unit_name(unit, port))
        if service_pid is not None:
            roots.append(service_pid)
    return _dedupe_ints(pid for pid in roots if _pid_alive(pid))


def _systemd_main_pid(unit: str) -> int | None:
    try:
        out = subprocess.run(
            ["systemctl", "show", unit, "-p", "MainPID", "--value"],
            check=False,
            capture_output=True,
            text=True,
            timeout=2,
        ).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return None
    if not out.isdigit():
        return None
    pid = int(out)
    return pid if pid > 0 else None


def _process_tree_pids(root_pids: list[int]) -> list[int]:
    if not root_pids:
        return []
    children: dict[int, list[int]] = {}
    for pid in _proc_pids():
        stat = _read_proc_stat(pid)
        if stat is None:
            continue
        children.setdefault(int(stat["ppid"]), []).append(pid)

    found: list[int] = []
    seen: set[int] = set()
    stack = list(root_pids)
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        found.append(pid)
        stack.extend(children.get(pid, []))
    return sorted(found)


def _sample_process_rows(pids: list[int], *, sample_seconds: float) -> list[dict[str, object]]:
    if not pids:
        return []
    first = {pid: sample for pid in pids if (sample := _read_proc_sample(pid)) is not None}
    if sample_seconds > 0:
        time.sleep(sample_seconds)
    second = {pid: sample for pid in pids if (sample := _read_proc_sample(pid)) is not None}
    mem_total_kb = _mem_total_kb()
    rows: list[dict[str, object]] = []
    for pid in pids:
        sample = second.get(pid) or first.get(pid)
        if sample is None:
            continue
        prev = first.get(pid)
        if prev is not None and sample is second.get(pid) and sample_seconds > 0:
            cpu_percent = max(
                0.0,
                (sample["ticks"] - prev["ticks"]) / _clk_tck() / sample_seconds * 100.0,
            )
        else:
            elapsed = max(float(sample["elapsed_seconds"]), 0.001)
            cpu_percent = max(0.0, sample["ticks"] / _clk_tck() / elapsed * 100.0)
        rss_kb = int(sample["rss_kb"])
        row = {
            **sample,
            "cpu_percent": cpu_percent,
            "mem_percent": (rss_kb / mem_total_kb * 100.0) if mem_total_kb else 0.0,
        }
        rows.append(row)
    rows.sort(key=lambda row: (float(row["cpu_percent"]), int(row["rss_kb"])), reverse=True)
    return rows


def _read_proc_sample(pid: int) -> dict[str, object] | None:
    stat = _read_proc_stat(pid)
    if stat is None:
        return None
    rss_kb, vms_kb = _read_proc_memory_kb(pid)
    return {
        **stat,
        "rss_kb": rss_kb,
        "vms_kb": vms_kb,
        "command": _pid_cmdline(pid) or stat["comm"],
    }


def _read_proc_stat(pid: int) -> dict[str, object] | None:
    try:
        text = (Path("/proc") / str(pid) / "stat").read_text(encoding="utf-8")
    except OSError:
        return None
    close = text.rfind(")")
    open_ = text.find("(")
    if open_ < 0 or close < open_:
        return None
    comm = text[open_ + 1:close]
    fields = text[close + 2:].split()
    if len(fields) < 20:
        return None
    try:
        ticks = int(fields[11]) + int(fields[12])
        start_ticks = int(fields[19])
    except ValueError:
        return None
    elapsed = max(0.0, _proc_uptime_seconds() - (start_ticks / _clk_tck()))
    return {
        "pid": pid,
        "ppid": int(fields[1]),
        "pgid": int(fields[2]),
        "state": fields[0],
        "comm": comm,
        "ticks": ticks,
        "elapsed_seconds": elapsed,
    }


def _read_proc_memory_kb(pid: int) -> tuple[int, int]:
    try:
        parts = (Path("/proc") / str(pid) / "statm").read_text(encoding="utf-8").split()
    except OSError:
        return 0, 0
    if len(parts) < 2:
        return 0, 0
    page_kb = _page_size_kb()
    try:
        vms_kb = int(parts[0]) * page_kb
        rss_kb = int(parts[1]) * page_kb
    except ValueError:
        return 0, 0
    return rss_kb, vms_kb


def _proc_pids() -> list[int]:
    try:
        entries = list(Path("/proc").iterdir())
    except OSError:
        return []
    return [int(path.name) for path in entries if path.name.isdigit()]


def _proc_uptime_seconds() -> float:
    try:
        return float(Path("/proc/uptime").read_text(encoding="utf-8").split()[0])
    except (OSError, ValueError, IndexError):
        return time.monotonic()


def _mem_total_kb() -> int:
    try:
        for line in Path("/proc/meminfo").read_text(encoding="utf-8").splitlines():
            if line.startswith("MemTotal:"):
                return int(line.split()[1])
    except (OSError, ValueError, IndexError):
        return 0
    return 0


def _clk_tck() -> int:
    try:
        return int(os.sysconf("SC_CLK_TCK"))
    except (OSError, ValueError):
        return 100


def _page_size_kb() -> int:
    try:
        return max(1, int(os.sysconf("SC_PAGE_SIZE")) // 1024)
    except (OSError, ValueError):
        return 4


def _process_role(pid: int, root_pids: list[int], command: str) -> str:
    lowered = command.lower()
    if pid in root_pids:
        return "root"
    if any(token in lowered for token in ("codex", "claude", "gemini", "copilot")):
        return "agent"
    return "child"


def _format_pid_list(pids: list[int]) -> str:
    return ", ".join(str(pid) for pid in pids)


def _format_mib(kb: int) -> str:
    return f"{kb / 1024.0:.1f}M"


def _format_elapsed(seconds: float) -> str:
    seconds = max(0, int(seconds))
    days, rem = divmod(seconds, 86400)
    hours, rem = divmod(rem, 3600)
    minutes, secs = divmod(rem, 60)
    if days:
        return f"{days}d{hours:02d}h"
    if hours:
        return f"{hours:02d}:{minutes:02d}:{secs:02d}"
    return f"{minutes:02d}:{secs:02d}"


def _truncate(value: str, limit: int) -> str:
    if len(value) <= limit:
        return value
    return value[: max(0, limit - 3)] + "..."


def _dedupe_ints(values) -> list[int]:  # noqa: ANN001
    out: list[int] = []
    for value in values:
        if value not in out:
            out.append(value)
    return out


def _running_config(pid: int | None) -> "config.Config | None":
    if pid is None:
        return None
    cfg_path = _pid_env_value(pid, config.ENV_CONFIG_PATH)
    if not cfg_path:
        return None
    try:
        return config.Config.load(cfg_path)
    except config.ConfigError:
        return None


# ----- server / ui ------------------------------------------------------------

def cmd_server_foreground(args: _Args) -> int:
    """Run the server component in the foreground for debugging.

    The production path is `refine supervisor`, which keeps the UI/control
    process separate from the work runner.
    """
    _load_config_or_exit(args)
    from refine_server.__main__ import main as server_main
    return server_main()


def cmd_ui(args: _Args) -> int:
    from refine_ui.__main__ import main as ui_main
    return ui_main()


# ----- doctor -----------------------------------------------------------------

def cmd_doctor(args: _Args) -> int:
    cfg_path = args.config
    try:
        cfg = config.get(path=cfg_path) if cfg_path else config.get()
    except config.ConfigError as e:
        print(_red("No refine configuration found."))
        print(f"  {e}")
        print()
        print("Run `refine target <target-app>` from the refine checkout to attach one.")
        return 1

    print(_section("Configuration"))
    _kv("config file",   cfg.config_path)
    _kv("volume root",   cfg.volume_root)
    _kv("target app",    cfg.client_repo)
    _kv("web host:port", f"{cfg.web_host}:{cfg.web_port}")

    print(_section("Volume root"))
    sqlite_present = cfg.sqlite_path.is_file()
    _kv("index.sqlite",  f"{cfg.sqlite_path} ({'present' if sqlite_present else 'missing'})")
    gap_count = _count_gap_files(cfg.gaps_dir)
    _kv("gaps/ files",   f"{gap_count} gap.json file(s)")

    print(_section("Agent CLI"))
    agent_cli = _configured_agent_cli(cfg.sqlite_path)
    cli_path = _agent_cli_path(agent_cli)
    _kv("provider", agent_cli)
    _kv(f"{agent_cli} path", cli_path)
    ok, msg = _cli_version(cli_path, agent_cli)
    _kv(f"{agent_cli} --version", _bool(ok))
    if not ok:
        _kv("error", msg or "")

    print(_section("Client repo / git"))
    repo = cfg.client_repo
    if not (repo / ".git").exists():
        _kv("git", _red("not a git repository"))
    else:
        branch = _git(repo, "symbolic-ref", "--quiet", "--short", "HEAD")
        upstream = _git(repo, "rev-parse", "--abbrev-ref", f"{branch.strip()}@{{upstream}}") if branch.strip() else None
        dirty = _git(repo, "status", "--porcelain")
        _kv("branch", branch.strip() or _red("detached HEAD"))
        _kv("upstream", (upstream or "").strip() or _red("no upstream"))
        _kv("clean", _bool(not (dirty or "").strip()))

    print()
    ok_all = reachable and ok and (repo / ".git").exists()
    print(_green("doctor: ok") if ok_all else _red("doctor: issues to address (see above)"))
    binding = config.find_binding()
    if binding is not None:
        _sync_bound_project_registry(binding.parent.resolve(), cfg)
    return 0 if ok_all else 1


# ----- helpers ----------------------------------------------------------------

def _sync_bound_project_registry(clone: Path, cfg: "config.Config") -> None:
    """Migrate an old single-app binding into the port-local app registry."""
    if not _is_refine_source_dir(clone):
        return
    try:
        project_registry.upsert_app(clone, cfg.client_repo, make_current=True)
    except (OSError, config.ConfigError):
        # Registry migration is best-effort; startup/status should not fail
        # because the source checkout is unexpectedly read-only.
        pass


def _config_for_port(args: _Args, clone: Path, port: int) -> config.Config | None:
    try:
        cfg_path = getattr(args, "config", None)
        if cfg_path:
            return config.Config.load(cfg_path)
        return config.get(reload=True, port=port)
    except config.ConfigError:
        return None


def _require_config_for_port(args: _Args, clone: Path, port: int, command: str) -> config.Config | None:
    cfg = _config_for_port(args, clone, port)
    if cfg is None:
        print(
            f"refine {command}: no app is attached on port {port}. "
            f"Run `refine target --port {port} <target-app>` first.",
            file=sys.stderr,
        )
    return cfg


def _effective_port(args: _Args, cfg: "config.Config | None") -> int:
    default_port = cfg.web_port if cfg is not None else config.runtime_port()
    raw_port = getattr(args, "port", None)
    port = int(raw_port if raw_port is not None else default_port)
    if port <= 0 or port > 65535:
        raise SystemExit(f"refine: invalid port {port}")
    return port


def _start_background_ui(
    clone: Path,
    cfg: "config.Config | None",
    *,
    host: str,
    port: int,
) -> int:
    live = _running_pid(clone, cfg, port)
    if live is not None:
        return live
    pid_path = _runtime_pid_path(clone, cfg, port)
    log_path = _runtime_log_path(clone, cfg, port)
    pid_path.parent.mkdir(parents=True, exist_ok=True)
    log_path.parent.mkdir(parents=True, exist_ok=True)

    uv = _find_host_command("uv")
    if uv is None:
        raise _InitError(
            "could not find `uv` on PATH or the login-shell PATH; install it "
            "before running `refine start`."
        )

    env = os.environ.copy()
    env["REFINE_UI_HOST"] = host
    env["REFINE_UI_PORT"] = str(port)
    env["REFINE_UI_SCOPE"] = str(port)
    env[config.ENV_RUN_DIR] = str(config.local_run_dir(clone, port=port))
    env["REFINE_SUPERVISOR_SOCKET"] = str(_supervisor_socket_path(clone, cfg, port))
    if cfg is not None:
        env["REFINE_CONFIG_PATH"] = str(cfg.config_path)
    env.setdefault("PYTHONUNBUFFERED", "1")
    command = [uv, "run", "refine", "supervisor"]
    with log_path.open("ab") as log:
        proc = subprocess.Popen(
            command,
            cwd=str(clone),
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            env=env,
            start_new_session=True,
        )
    pid_path.write_text(f"{proc.pid}\n", encoding="utf-8")
    return proc.pid


def cmd_supervisor(args: _Args) -> int:  # noqa: ARG001
    from refine_runtime.supervisor import main as supervisor_main

    return supervisor_main()


def _stop_background_ui(clone: Path, cfg: "config.Config | None", port: int) -> bool:
    pid_path = _runtime_pid_path(clone, cfg, port)
    pid = _running_pid(clone, cfg, port)
    if pid is None:
        return False
    if _request_supervisor_shutdown(clone, cfg, port):
        deadline = time.time() + 10
        while time.time() < deadline:
            if not _pid_alive(pid):
                _unlink_quietly(pid_path)
                return True
            time.sleep(0.2)
    try:
        pgid = os.getpgid(pid)
    except ProcessLookupError:
        _unlink_quietly(pid_path)
        return False
    try:
        os.killpg(pgid, signal.SIGTERM)
    except ProcessLookupError:
        _unlink_quietly(pid_path)
        return False
    except OSError:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            _unlink_quietly(pid_path)
            return False
    deadline = time.time() + 10
    while time.time() < deadline:
        if not _pid_alive(pid):
            _unlink_quietly(pid_path)
            return True
        time.sleep(0.2)
    try:
        os.killpg(pgid, signal.SIGKILL)
    except OSError:
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass
    _unlink_quietly(pid_path)
    return True


def _start_setup_systemd_ui(clone: Path, unit: str, port: int) -> int:
    ui_unit = _ui_unit_name(unit, port)
    print(f"Starting persistent refine (systemctl start {ui_unit})...")
    rc, out = _systemctl("enable", ui_unit)
    if rc != 0:
        print(f"refine start: systemctl enable {ui_unit} failed: {out.strip()}",
              file=sys.stderr)
        return 1
    rc, out = _systemctl("start", ui_unit)
    if rc != 0:
        print(f"refine start: systemctl start {ui_unit} failed: {out.strip()}",
              file=sys.stderr)
        return 1
    _unlink_quietly(_runtime_pid_path(clone, None, port))
    if not _wait_for_port(SETUP_UI_HOST, port, timeout=20.0):
        print(
            f"refine start: systemd unit {ui_unit} did not start listening on "
            f"{SETUP_UI_HOST}:{port} within 20s.",
            file=sys.stderr,
        )
        return 1
    _print_setup_status_block(clone, port=port, unit=unit)
    _print_upgrade_notice(clone)
    return 0


def _stop_setup_systemd_ui(clone: Path, unit: str, port: int) -> int:
    ui_unit = _ui_unit_name(unit, port)
    print(f"Stopping persistent refine (systemctl disable + stop {ui_unit})...")
    rc, out = _systemctl("disable", ui_unit)
    if rc != 0:
        print(f"refine stop: systemctl disable {ui_unit} failed: {out.strip()}",
              file=sys.stderr)
        return 1
    rc, out = _systemctl("stop", ui_unit)
    if rc != 0:
        print(f"refine stop: systemctl stop {ui_unit} failed: {out.strip()}",
              file=sys.stderr)
        return 1
    _unlink_quietly(_runtime_pid_path(clone, None, port))
    print(f"Stopped refine on port {port}.")
    return 0


def _restart_setup_systemd_ui(clone: Path, unit: str, port: int) -> int:
    ui_unit = _ui_unit_name(unit, port)
    print(f"Restarting persistent refine (systemctl restart {ui_unit})...")
    rc, out = _systemctl("enable", ui_unit)
    if rc != 0:
        print(
            f"refine restart: systemctl enable {ui_unit} failed: {out.strip()}",
            file=sys.stderr,
        )
        return 1
    rc, out = _systemctl("restart", ui_unit)
    if rc != 0:
        print(
            f"refine restart: systemctl restart {ui_unit} failed: {out.strip()}",
            file=sys.stderr,
        )
        return 1
    _unlink_quietly(_runtime_pid_path(clone, None, port))
    if not _wait_for_port(SETUP_UI_HOST, port, timeout=20.0):
        print(
            f"refine restart: systemd unit {ui_unit} did not start listening on "
            f"{SETUP_UI_HOST}:{port} within 20s.",
            file=sys.stderr,
        )
        return 1
    _print_setup_status_block(clone, port=port, unit=unit)
    _print_upgrade_notice(clone)
    return 0


def _pause_agents_for_clean_shutdown(cfg: "config.Config", port: int) -> bool:
    """Stop background work through the UI API before tearing down the backend."""
    host = "127.0.0.1" if cfg.web_host in ("0.0.0.0", "::") else cfg.web_host
    url = f"http://{host}:{port}/api/processes/background"
    body = json.dumps({"stopped": True}).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30.0):
            return True
    except urllib.error.HTTPError as e:
        message = _shutdown_cleanup_http_error_message(e)
        print(
            f"refine: shutdown cleanup failed; continuing: {message}",
            file=sys.stderr,
        )
        return False
    except (OSError, urllib.error.URLError):
        return True


def _shutdown_cleanup_http_error_message(e: urllib.error.HTTPError) -> str:
    try:
        raw = e.read().decode("utf-8", errors="replace")
    except Exception:
        raw = ""
    if raw:
        try:
            data = json.loads(raw)
            error = data.get("error") if isinstance(data, dict) else None
            if isinstance(error, dict):
                message = str(error.get("message") or "").strip()
                details = str(error.get("details") or "").strip()
                if message and details:
                    return f"{message}: {details}"
                if message:
                    return message
        except json.JSONDecodeError:
            text = raw.strip()
            if text:
                return text
    return str(e)


def _refresh_installed_ui_unit_if_stale(
    clone: Path,
    unit: str,
    cfg: "config.Config",
    port: int,
) -> int:
    ui_unit = _ui_unit_name(unit, port)
    unit_path = SYSTEMD_SYSTEM_DIR / f"{ui_unit}.service"
    text = _read_unit_text(unit_path)
    if text is None:
        return 0
    stale = (
        f"Environment={config.ENV_CONFIG_PATH}=" in text
        or f"Environment=\"{config.ENV_CONFIG_PATH}=" in text
        or _grep_first(text, "WorkingDirectory=") != str(clone)
    )
    if not stale:
        return 0
    try:
        _write_and_enable_ui_unit(
            clone,
            cfg.client_repo,
            force=True,
            runner_unit_name=unit,
            host=cfg.web_host,
            port=port,
        )
    except _InitError as e:
        print(
            f"refine start: could not refresh installed systemd unit {ui_unit}: {e}",
            file=sys.stderr,
        )
        return 1
    return 0


def _start_systemd_ui(clone: Path, unit: str, cfg: "config.Config", port: int) -> int:
    refresh = _refresh_installed_ui_unit_if_stale(clone, unit, cfg, port)
    if refresh != 0:
        return refresh
    ui_unit = _ui_unit_name(unit, port)
    print(f"Starting persistent refine (systemctl start {ui_unit})...")
    rc, out = _systemctl("enable", ui_unit)
    if rc != 0:
        print(f"refine start: systemctl enable {ui_unit} failed: {out.strip()}",
              file=sys.stderr)
        return 1
    rc, out = _systemctl("start", ui_unit)
    if rc != 0:
        print(f"refine start: systemctl start {ui_unit} failed: {out.strip()}",
              file=sys.stderr)
        return 1
    _unlink_quietly(_runtime_pid_path(clone, cfg, port))
    if not _wait_for_port(cfg.web_host, port, timeout=20.0):
        print(
            f"refine start: systemd unit {ui_unit} did not start listening on "
            f"{cfg.web_host}:{port} within 20s.",
            file=sys.stderr,
        )
        return 1
    _print_status_block(clone, unit, cfg, port=port)
    _print_upgrade_notice(clone)
    return 0


def _stop_systemd_ui(clone: Path, unit: str, cfg: "config.Config", port: int) -> int:
    ui_unit = _ui_unit_name(unit, port)
    print(f"Stopping persistent refine (systemctl disable + stop {ui_unit})...")
    rc, out = _systemctl("disable", ui_unit)
    if rc != 0:
        print(f"refine stop: systemctl disable {ui_unit} failed: {out.strip()}",
              file=sys.stderr)
        return 1
    rc, out = _systemctl("stop", ui_unit)
    if rc != 0:
        print(f"refine stop: systemctl stop {ui_unit} failed: {out.strip()}",
              file=sys.stderr)
        return 1
    _unlink_quietly(_runtime_pid_path(clone, cfg, port))
    print(f"Stopped refine on port {port}.")
    return 0


def _restart_systemd_ui(clone: Path, unit: str, cfg: "config.Config", port: int) -> int:
    refresh = _refresh_installed_ui_unit_if_stale(clone, unit, cfg, port)
    if refresh != 0:
        return refresh
    ui_unit = _ui_unit_name(unit, port)
    print(f"Restarting persistent refine (systemctl restart {ui_unit})...")
    rc, out = _systemctl("enable", ui_unit)
    if rc != 0:
        print(
            f"refine restart: systemctl enable {ui_unit} failed: {out.strip()}",
            file=sys.stderr,
        )
        return 1
    rc, out = _systemctl("restart", ui_unit)
    if rc != 0:
        print(
            f"refine restart: systemctl restart {ui_unit} failed: {out.strip()}",
            file=sys.stderr,
        )
        return 1
    _unlink_quietly(_runtime_pid_path(clone, cfg, port))
    if not _wait_for_port(cfg.web_host, port, timeout=20.0):
        print(
            f"refine restart: systemd unit {ui_unit} did not start listening on "
            f"{cfg.web_host}:{port} within 20s.",
            file=sys.stderr,
        )
        return 1
    _print_status_block(clone, unit, cfg, port=port)
    _print_upgrade_notice(clone)
    return 0


def _running_pid(clone: Path, cfg: "config.Config | None", port: int) -> int | None:
    pid_path = _runtime_pid_path(clone, cfg, port)
    pid = _read_pid(pid_path)
    if pid is not None and _pid_alive(pid):
        return pid
    if pid is not None:
        _unlink_quietly(pid_path)
    legacy_pid_path = config.local_run_root(clone) / f"ui-{port}.pid"
    legacy_pid = _read_pid(legacy_pid_path)
    if legacy_pid is not None and _pid_alive(legacy_pid):
        return legacy_pid
    if legacy_pid is not None:
        _unlink_quietly(legacy_pid_path)

    host = cfg.web_host if cfg is not None else SETUP_UI_HOST
    listener = _refine_ui_listener_pid(clone, host, port)
    if listener is not None and _pid_alive(listener):
        return listener
    return None


def _runtime_pid_path(clone: Path, cfg: "config.Config | None", port: int) -> Path:
    return _runtime_dir(clone, cfg, port) / "supervisor.pid"


def _runtime_log_path(clone: Path, cfg: "config.Config | None", port: int) -> Path:
    return _runtime_dir(clone, cfg, port) / "supervisor.log"


def _supervisor_socket_path(clone: Path, cfg: "config.Config | None", port: int) -> Path:
    from refine_runtime import ipc

    return ipc.supervisor_socket_path(port, start=clone)


def _supervisor_status(
    clone: Path,
    cfg: "config.Config | None",
    port: int,
) -> dict | None:
    from refine_runtime import ipc
    from refine_runtime.supervisor_protocol import M_STATUS

    try:
        return ipc.request(
            _supervisor_socket_path(clone, cfg, port),
            M_STATUS,
            {},
            timeout=2.0,
        )
    except Exception:
        return None


def _request_supervisor_shutdown(
    clone: Path,
    cfg: "config.Config | None",
    port: int,
) -> bool:
    from refine_runtime import ipc
    from refine_runtime.supervisor_protocol import M_SHUTDOWN

    try:
        ipc.request(
            _supervisor_socket_path(clone, cfg, port),
            M_SHUTDOWN,
            {},
            timeout=2.0,
        )
        return True
    except Exception:
        return False


def _runtime_dir(clone: Path, cfg: "config.Config | None", port: int) -> Path:
    return config.local_run_dir(clone, port=port)


def _read_pid(path: Path) -> int | None:
    try:
        return int(path.read_text(encoding="utf-8").strip())
    except (OSError, ValueError):
        return None


def _pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def _refine_ui_listener_pid(clone: Path, host: str, port: int) -> int | None:
    for pid in _listener_pids(port):
        if _pid_matches_refine_ui(pid, clone):
            return pid
    return None


def _listener_pids(port: int) -> list[int]:
    pids: list[int] = []
    lsof = shutil.which("lsof")
    if lsof:
        try:
            out = subprocess.run(
                [lsof, "-nP", f"-iTCP:{port}", "-sTCP:LISTEN", "-Fp"],
                check=False,
                capture_output=True,
                text=True,
                timeout=2,
            ).stdout
            for line in out.splitlines():
                if line.startswith("p") and line[1:].isdigit():
                    pids.append(int(line[1:]))
        except (OSError, subprocess.TimeoutExpired):
            pass
    if pids:
        return pids

    ss = shutil.which("ss")
    if not ss:
        return []
    try:
        out = subprocess.run(
            [ss, "-ltnp", f"sport = :{port}"],
            check=False,
            capture_output=True,
            text=True,
            timeout=2,
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return []
    return [int(m.group(1)) for m in re.finditer(r"pid=(\d+)", out)]


def _pid_matches_refine_ui(pid: int, clone: Path) -> bool:
    cmdline = _pid_cmdline(pid)
    if not cmdline:
        return False
    if (
        "refine" not in cmdline
        or re.search(r"(?:^|\s)(?:ui|supervisor)(?:\s|$)", cmdline) is None
    ):
        return False
    cwd = _pid_cwd(pid)
    return cwd is not None and cwd == clone.resolve()


def _owned_refine_ui_ports(clone: Path) -> list[int]:
    ports: set[int] = set()
    for pid, listen_port in _listener_port_pids():
        if not _pid_matches_refine_ui(pid, clone):
            continue
        env_port = _pid_env_value(pid, "REFINE_UI_PORT")
        if env_port and env_port.isdigit():
            ports.add(int(env_port))
        else:
            ports.add(listen_port)
    return sorted(ports)


def _status_ports(args: _Args, clone: Path,
                  cfg: "config.Config | None",
                  unit: str | None = None) -> list[int]:
    if getattr(args, "port", None) is not None:
        return [_effective_port(args, cfg)]
    ports = set(_runtime_pid_ports(clone, cfg))
    ports.update(_runtime_app_ports(clone))
    ports.update(_owned_refine_ui_ports(clone))
    if unit is not None:
        ports.update(_installed_ui_unit_ports(unit))
    if not ports:
        ports.add(_effective_port(args, cfg))
    return sorted(ports)


def _runtime_pid_ports(clone: Path, cfg: "config.Config | None") -> list[int]:
    run_root = config.local_run_root(clone)
    ports: set[int] = set()
    try:
        entries = list(run_root.glob("*/supervisor.pid"))
    except OSError:
        entries = []
    for path in entries:
        if path.parent.name.isdigit():
            port = int(path.parent.name)
            if 0 < port <= 65535:
                ports.add(port)
    try:
        legacy_entries = list(run_root.glob("ui-*.pid"))
    except OSError:
        legacy_entries = []
    for path in legacy_entries:
        m = re.fullmatch(r"ui-(\d+)\.pid", path.name)
        if not m:
            continue
        port = int(m.group(1))
        if 0 < port <= 65535:
            ports.add(port)
    return sorted(ports)


def _runtime_app_ports(clone: Path) -> list[int]:
    ports: set[int] = set()
    try:
        entries = list(config.local_run_root(clone).glob("*/apps.json"))
    except OSError:
        return []
    for path in entries:
        if path.parent.name.isdigit():
            port = int(path.parent.name)
            if 0 < port <= 65535:
                ports.add(port)
    return sorted(ports)


def _listener_port_pids() -> list[tuple[int, int]]:
    pairs: list[tuple[int, int]] = []
    lsof = shutil.which("lsof")
    if lsof:
        try:
            out = subprocess.run(
                [lsof, "-nP", "-iTCP", "-sTCP:LISTEN", "-Fp", "-Fn"],
                check=False,
                capture_output=True,
                text=True,
                timeout=2,
            ).stdout
        except (OSError, subprocess.TimeoutExpired):
            out = ""
        pid: int | None = None
        for line in out.splitlines():
            if line.startswith("p") and line[1:].isdigit():
                pid = int(line[1:])
                continue
            if pid is None or not line.startswith("n"):
                continue
            m = re.search(r":(\d+)(?:\s|$)", line)
            if m:
                pairs.append((pid, int(m.group(1))))
        if pairs:
            return pairs

    ss = shutil.which("ss")
    if not ss:
        return []
    try:
        out = subprocess.run(
            [ss, "-ltnp"],
            check=False,
            capture_output=True,
            text=True,
            timeout=2,
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return []
    for line in out.splitlines():
        pid_match = re.search(r"pid=(\d+)", line)
        port_match = re.search(r":(\d+)\s+", line)
        if pid_match and port_match:
            pairs.append((int(pid_match.group(1)), int(port_match.group(1))))
    return pairs


def _runtime_action_port(args: _Args, clone: Path,
                         cfg: "config.Config | None",
                         unit: str | None = None) -> int:
    configured = _effective_port(args, cfg)
    if getattr(args, "port", None) is not None:
        return configured
    if _running_pid(clone, cfg, configured) is not None:
        return configured
    if unit is not None and _installed_ui_unit(unit, configured) is not None:
        return configured
    live_ports = [p for p in _owned_refine_ui_ports(clone) if p != configured]
    if len(live_ports) == 1:
        return live_ports[0]
    app_ports = [p for p in _runtime_app_ports(clone) if p != configured]
    if len(app_ports) == 1:
        return app_ports[0]
    if unit is not None:
        installed_ports = [p for p in _installed_ui_unit_ports(unit) if p != configured]
        if len(installed_ports) == 1:
            return installed_ports[0]
    return configured


def _pid_cmdline(pid: int) -> str:
    proc_path = Path("/proc") / str(pid) / "cmdline"
    try:
        raw = proc_path.read_bytes()
    except OSError:
        raw = b""
    if raw:
        return raw.replace(b"\0", b" ").decode("utf-8", "replace").strip()
    try:
        out = subprocess.run(
            ["ps", "-p", str(pid), "-o", "command="],
            check=False,
            capture_output=True,
            text=True,
            timeout=2,
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return out.strip()


def _pid_cwd(pid: int) -> Path | None:
    try:
        return Path(f"/proc/{pid}/cwd").resolve(strict=True)
    except OSError:
        return None


def _pid_env_value(pid: int, key: str) -> str | None:
    try:
        raw = (Path("/proc") / str(pid) / "environ").read_bytes()
    except OSError:
        return None
    prefix = f"{key}=".encode("utf-8")
    for item in raw.split(b"\0"):
        if item.startswith(prefix):
            return item[len(prefix):].decode("utf-8", "replace")
    return None


def _setup_source_dir() -> Path | None:
    cwd = Path.cwd().resolve()
    if _is_refine_source_dir(cwd) and config.find_config(cwd) is None:
        return cwd
    return None


def _unlink_quietly(path: Path) -> None:
    try:
        path.unlink()
    except OSError:
        pass


def _unit_names_for_reset(unit: str) -> list[str]:
    names = [unit, _legacy_pre_ui_unit_name(unit), f"{unit}-ui"]
    try:
        names.extend(path.stem for path in SYSTEMD_SYSTEM_DIR.glob(f"{unit}-*-ui.service"))
        names.extend(path.stem for path in SYSTEMD_USER_DIR.glob(f"{unit}-*-ui.service"))
    except OSError:
        pass
    out: list[str] = []
    for name in names:
        if name not in out:
            out.append(name)
    return out


def _installed_ui_unit(unit: str, port: int) -> str | None:
    ui_unit = _ui_unit_name(unit, port)
    if _unit_file_exists(SYSTEMD_SYSTEM_DIR / f"{ui_unit}.service"):
        return ui_unit
    return None


def _installed_ui_unit_ports(unit: str) -> list[int]:
    ports: set[int] = set()
    pattern = re.compile(rf"^{re.escape(unit)}-(\d+)-ui\.service$")
    try:
        paths = list(SYSTEMD_SYSTEM_DIR.glob(f"{unit}-*-ui.service"))
    except OSError:
        return []
    for path in paths:
        m = pattern.fullmatch(path.name)
        if m:
            ports.add(int(m.group(1)))
    return sorted(ports)


def _ui_unit_name(runner_unit: str, port: int) -> str:
    return f"{runner_unit}-{port}-ui"


def _legacy_pre_ui_unit_name(runner_unit: str) -> str:
    return f"{runner_unit}-web"


def _find_host_command(name: str) -> str | None:
    """Resolve a host command using the current PATH, then the user's login PATH."""
    direct = shutil.which(name)
    if direct:
        return direct
    login_path = _user_login_path()
    if login_path:
        return shutil.which(name, path=login_path)
    return None


def _systemd_environment_lines(env: dict[str, str]) -> str:
    return "".join(
        _systemd_environment_line(name, value)
        for name, value in sorted(env.items())
        if _valid_environment_name(name) and name not in _SYSTEMD_ENV_BLOCKLIST
    )


_SYSTEMD_ENV_BLOCKLIST = {
    config.ENV_CONFIG_PATH,
    "REFINE_NO_INPROCESS_RUNNER",
    "REFINE_RUNNER_SOCKET",
    config.ENV_RUN_DIR,
    "REFINE_SUPERVISOR_PID",
    "REFINE_UI_HOST",
    "REFINE_UI_PORT",
    "REFINE_UI_SCOPE",
}


def _valid_environment_name(name: str) -> bool:
    return bool(re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", name or ""))


def _systemd_environment_line(name: str, value: str | None) -> str:
    if value is None:
        return ""
    escaped = (
        value
        .replace("\\", "\\\\")
        .replace("\n", "\\n")
        .replace("\r", "\\r")
        .replace('"', '\\"')
        .replace("%", "%%")
    )
    return f'Environment="{name}={escaped}"\n'


def _service_user() -> str:
    """User account the system service should run as.

    `refine install` may be invoked directly by an unprivileged user, in which
    case the CLI uses sudo only for writing/enabling the system unit. If the
    operator invokes the whole command through sudo, preserve the original
    account via SUDO_USER so the service still owns files as the operator.
    """
    sudo_user = (os.environ.get("SUDO_USER") or "").strip()
    if sudo_user and sudo_user != "root":
        return sudo_user
    return getpass.getuser()


def _read_unit_text(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return None
    except PermissionError:
        try:
            out = subprocess.run(
                _sudo_cmd(["cat", str(path)]),
                capture_output=True, text=True, timeout=5,
            )
        except (OSError, subprocess.TimeoutExpired):
            return None
        return out.stdout if out.returncode == 0 else None
    except OSError:
        return None


def _unit_file_exists(path: Path) -> bool:
    try:
        return path.exists()
    except OSError:
        return False


def _write_system_unit(path: Path, text: str) -> None:
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return
    except PermissionError:
        pass
    except OSError as e:
        if path.parent != SYSTEMD_SYSTEM_DIR:
            raise _InitError(f"could not write {path}: {e}") from e
    tmp = _runtime_temp_unit_file(text)
    try:
        proc = subprocess.run(
            _sudo_cmd(["install", "-m", "0644", str(tmp), str(path)]),
            capture_output=True, text=True, timeout=15,
        )
    finally:
        _unlink_quietly(tmp)
    if proc.returncode != 0:
        raise _InitError(
            f"could not install systemd unit {path}: "
            f"{(proc.stderr or proc.stdout).strip()}"
        )


def _remove_system_unit(path: Path) -> bool:
    try:
        path.unlink()
        return True
    except FileNotFoundError:
        return True
    except PermissionError:
        pass
    except OSError:
        if path.parent != SYSTEMD_SYSTEM_DIR:
            return False
    try:
        proc = subprocess.run(
            _sudo_cmd(["rm", "-f", str(path)]),
            capture_output=True, text=True, timeout=15,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return proc.returncode == 0


def _remove_user_unit_file(path: Path) -> None:
    try:
        path.unlink()
    except OSError:
        pass


def _runtime_temp_unit_file(text: str) -> Path:
    import tempfile

    fd, tmp = tempfile.mkstemp(prefix="refine-unit-", suffix=".service")
    path = Path(tmp)
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write(text)
    return path


def _sudo_cmd(args: list[str]) -> list[str]:
    if os.geteuid() == 0:
        return args
    sudo = shutil.which("sudo")
    if sudo is None:
        return args
    return [sudo, *args]


def _user_login_path() -> str | None:
    """Return the PATH an interactive login shell sees.

    systemd services run with a minimal PATH that may miss uv installs in
    ~/.local/bin, ~/.cargo/bin, asdf/mise shims, Homebrew, etc. Project setup
    may run inside the host-native refine service, so resolving uv must match the
    operator's terminal rather than systemd's stripped env.
    """
    global _LOGIN_PATH_CACHE, _LOGIN_PATH_RESOLVED
    if _LOGIN_PATH_RESOLVED:
        return _LOGIN_PATH_CACHE
    _LOGIN_PATH_RESOLVED = True
    shell = os.environ.get("SHELL") or "/bin/bash"
    for flag in ("-lic", "-lc"):
        try:
            out = subprocess.run(
                [shell, flag, 'printf %s "$PATH"'],
                capture_output=True, text=True, timeout=5,
            )
        except Exception:
            continue
        path = (out.stdout or "").strip()
        if out.returncode == 0 and path:
            _LOGIN_PATH_CACHE = path
            break
    return _LOGIN_PATH_CACHE


def _ensure_host_unit_installed(clone: Path, cfg: "config.Config", runner_unit: str) -> None:
    ui_unit = _ui_unit_name(runner_unit, cfg.web_port)
    ui_path = SYSTEMD_SYSTEM_DIR / f"{ui_unit}.service"
    _remove_legacy_runtime_units(runner_unit)
    if _unit_file_exists(ui_path):
        return
    _write_and_enable_ui_unit(
        clone, cfg.client_repo, force=True, runner_unit_name=runner_unit,
        host=cfg.web_host, port=cfg.web_port,
    )


def _remove_legacy_runtime_units(runner_unit: str) -> None:
    for unit_name in (runner_unit, _legacy_pre_ui_unit_name(runner_unit), f"{runner_unit}-ui"):
        _remove_unit(unit_name)


def _remove_unit(unit_name: str) -> None:
    _systemctl("stop", unit_name)
    _systemctl("disable", unit_name)
    unit_path = SYSTEMD_SYSTEM_DIR / f"{unit_name}.service"
    removed = False
    if _unit_file_exists(unit_path):
        removed = _remove_system_unit(unit_path)
    legacy_path = SYSTEMD_USER_DIR / f"{unit_name}.service"
    if legacy_path.exists():
        _systemctl_user("stop", unit_name)
        _systemctl_user("disable", unit_name)
        _remove_user_unit_file(legacy_path)
        removed = True
    if not removed:
        return
    _systemctl("daemon-reload")
    _systemctl_user("daemon-reload")


def _remove_legacy_user_ui_unit(unit_name: str) -> bool:
    legacy_path = SYSTEMD_USER_DIR / f"{unit_name}.service"
    if not legacy_path.exists():
        return False
    _systemctl_user("stop", unit_name)
    _systemctl_user("disable", unit_name)
    _remove_user_unit_file(legacy_path)
    _systemctl_user("daemon-reload")
    return True


def _remove_legacy_docker_artifacts(clone: Path, *, verbose: bool = False) -> None:
    env_path = clone / ".env"
    try:
        text = env_path.read_text(encoding="utf-8")
    except OSError:
        text = ""
    if text and "REFINE_CLIENT_REFINE_DIR=" in text:
        remaining = [
            line for line in text.splitlines()
            if not line.strip().startswith("REFINE_CLIENT_REFINE_DIR=")
        ]
        if any(line.strip() for line in remaining):
            env_path.write_text("\n".join(remaining).rstrip() + "\n", encoding="utf-8")
            if verbose:
                print(f"Removed legacy REFINE_CLIENT_REFINE_DIR from {env_path}")
        else:
            env_path.unlink()
            if verbose:
                print(f"Removed legacy {env_path}")
    current_link = clone / ".refine-current"
    if current_link.is_symlink() or current_link.exists():
        current_link.unlink()
        if verbose:
            print(f"Removed legacy {current_link}")

def _load_config_or_exit(args: _Args) -> None:
    try:
        if args.config:
            config.get(path=args.config)
        else:
            config.get()
    except config.ConfigError as e:
        print(f"refine: {e}", file=sys.stderr)
        sys.exit(1)


def _ensure_sqlite_schema(cfg: "config.Config") -> None:
    """Apply blocking project-state and SQLite migrations before runtime handoff.

    The supervisor initializes SQLite too, but start/restart can delegate to a
    systemd unit that was installed from a different checkout. Running schema
    setup from the invoking CLI makes project-state/cache migrations complete
    before the service process starts serving requests.
    """
    from refine_server import db, project_state

    try:
        db.init_db(cfg.sqlite_path)
        conn = db.connect(cfg.sqlite_path)
        try:
            status = project_state.ensure_initialized(
                conn,
                migrate=True,
                root=cfg.volume_root,
            )
            if not status.get("compatible"):
                raise _InitError(project_state.migration_block_details(status))
            project_state.rebuild_sqlite_cache(conn, force=True)
        finally:
            conn.close()
    except _InitError:
        raise
    except Exception as e:
        raise _InitError(f"project migration failed before startup: {e}") from e


def _resolve_clone_and_unit_or_exit() -> tuple[Path, str]:
    """Find the Refine checkout for runtime commands."""
    cwd = Path.cwd().resolve()
    if _is_refine_source_dir(cwd):
        return cwd, config.unit_name_for(cwd)
    binding = config.find_binding()
    if binding is not None:
        clone = binding.parent.resolve()
        unit = config.read_binding_unit(binding) or config.unit_name_for(clone)
        return clone, unit
    print(
        "refine: run this command from a Refine source checkout.",
        file=sys.stderr,
    )
    sys.exit(1)


def _systemctl(*args: str) -> tuple[int, str]:
    # systemd's TimeoutStopSec defaults to 90s and our unit template caps
    # it to 30s. The wrapper has to give systemd at least its full stop
    # window or else we report a false-positive "timed out" — which is
    # what `refine stop` used to do on units the agent had spawned child
    # processes for.
    cmd = args[0] if args else ""
    timeout = 60 if cmd in ("stop", "start", "restart") else 15
    base = ["systemctl", *args]
    if cmd in {"daemon-reload", "enable", "disable", "start", "stop", "restart"}:
        base = _sudo_cmd(base)
    try:
        out = subprocess.run(
            base,
            capture_output=True, text=True, timeout=timeout,
        )
    except FileNotFoundError:
        return 127, "systemctl not found (systemd required)"
    except subprocess.TimeoutExpired:
        return 124, "systemctl timed out"
    return out.returncode, (out.stderr or out.stdout)


def _systemctl_is_active(unit: str) -> bool:
    rc, _ = _systemctl("is-active", "--quiet", unit)
    return rc == 0


def _systemctl_user(*args: str) -> tuple[int, str]:
    cmd = args[0] if args else ""
    timeout = 60 if cmd in ("stop", "start", "restart") else 15
    try:
        out = subprocess.run(
            ["systemctl", "--user", *args],
            capture_output=True, text=True, timeout=timeout,
        )
    except FileNotFoundError:
        return 127, "systemctl not found (systemd --user required)"
    except subprocess.TimeoutExpired:
        return 124, "systemctl --user timed out"
    return out.returncode, (out.stderr or out.stdout)


def _wait_for_port(host: str, port: int, *, timeout: float) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if _port_open(host, port):
            return True
        time.sleep(0.2)
    return False


def _port_open(host: str, port: int) -> bool:
    target = "127.0.0.1" if host in ("0.0.0.0", "::") else host
    try:
        with urllib.request.urlopen(f"http://{target}:{port}/api/project/status", timeout=1.0):
            return True
    except (OSError, urllib.error.URLError):
        return False


def _configured_agent_cli(sqlite_path: Path) -> str:
    if not sqlite_path.is_file():
        return "claude"
    try:
        import sqlite3
        conn = sqlite3.connect(str(sqlite_path))
        row = conn.execute(
            "SELECT value FROM settings WHERE key = 'agent_cli'",
        ).fetchone()
        conn.close()
        value = (row[0] if row else "claude") or "claude"
        value = str(value).strip().lower()
        valid = ("claude", "codex", "gemini", "copilot", "smoke-ai")
        return value if value in valid else "claude"
    except Exception:
        return "claude"


def _agent_cli_path(agent_cli: str) -> str:
    if agent_cli == "smoke-ai":
        smoke_ai_path = (os.environ.get("REFINE_SMOKE_AI_PATH") or "").strip()
        if smoke_ai_path:
            return smoke_ai_path
    return shutil.which(agent_cli) or "(not on PATH)"


def _cli_version(cli_path: str, binary: str) -> tuple[bool, str | None]:
    try:
        out = subprocess.run(
            [cli_path, "--version"],
            capture_output=True, text=True, timeout=10,
        )
        if out.returncode == 0:
            return True, out.stdout.strip()
        return False, (out.stderr.strip() or out.stdout.strip()
                       or f"exit code {out.returncode}")
    except FileNotFoundError as e:
        return False, repr(e)
    except subprocess.TimeoutExpired:
        return False, f"{binary} --version timed out (10s)"
    except Exception as e:
        return False, repr(e)


def _git(cwd: Path, *args: str) -> str:
    try:
        out = subprocess.run(
            ["git", *args], cwd=str(cwd),
            capture_output=True, text=True, timeout=10,
        )
        return out.stdout
    except Exception:
        return ""


def _count_gap_files(gaps_dir: Path) -> int:
    if not gaps_dir.is_dir():
        return 0
    return sum(1 for _ in gaps_dir.rglob("gap.json"))


def _displayable_host(h: str) -> str:
    return "localhost" if h in ("0.0.0.0", "::") else h


def _section(title: str) -> str:
    return f"\n{_bold(title)}\n" + "-" * len(title)


def _kv(label: str, value) -> None:  # noqa: ANN001
    print(f"  {label:<22} {value}")


def _bool(b: bool) -> str:
    return _green("yes") if b else _red("no")


def _dot(b: bool) -> str:
    return _green("●") if b else _red("○")


def _bold(s: str) -> str:
    return f"\033[1m{s}\033[0m" if sys.stdout.isatty() else s


def _red(s: str) -> str:
    return f"\033[31m{s}\033[0m" if sys.stdout.isatty() else s


def _green(s: str) -> str:
    return f"\033[32m{s}\033[0m" if sys.stdout.isatty() else s
