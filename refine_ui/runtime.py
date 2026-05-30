"""Web process runtime state shared by the entry point and project API."""
from __future__ import annotations

import os
from pathlib import Path

from refine_server import config, db
from refine_server.backend_protocol import M_RUNNING

from .poller import SqlitePoller

_poller: SqlitePoller | None = None
_runner = None
_loaded_config_path: Path | None = None


class _SocketRunnerClient:
    def __init__(self, socket_path: str) -> None:
        self.socket_path = socket_path

    def call(
        self,
        method: str,
        params: dict | None = None,
        *,
        timeout: float = 30.0,
    ) -> dict:
        from refine_runtime import ipc

        return ipc.request(self.socket_path, method, params or {}, timeout=timeout)

    def status_snapshot(self) -> dict:
        return self.call(M_RUNNING, {}, timeout=5.0)

    def shutdown(self) -> None:
        return None


def load_configured(
    path: Path | str | None = None,
    *,
    start_poller: bool = True,
    start_runner: bool = True,
    migrate: bool = False,
) -> config.Config:
    """Load config, initialize SQLite, and ensure background services run."""
    global _loaded_config_path
    requested_path = Path(path).resolve() if path is not None else config.find_config()
    cfg_preview = config.Config.load(path)
    from refine_server import project_state

    schema = project_state.schema_status(cfg_preview.volume_root)
    if (
        not schema.get("compatible")
        and schema.get("migration_required")
        and not migrate
        and not project_state.empty_refine_state(cfg_preview.volume_root)
    ):
        raise config.ConfigError(
            "Project schema migration required. Open the app from Settings "
            "and choose migrate to upgrade .refine state."
        )
    if not schema.get("compatible") and not schema.get("migration_required"):
        raise config.ConfigError(
            "Project schema is not supported by this Refine version."
        )
    if (_loaded_config_path is not None and requested_path is not None
            and requested_path.resolve() != _loaded_config_path):
        stop_all()
    cfg = config.get(path=path, reload=True)
    os.environ[config.ENV_CONFIG_PATH] = str(cfg.config_path)
    _loaded_config_path = cfg.config_path
    db.init_db()
    project_state.resume_agents_for_startup()
    if start_poller:
        ensure_poller()
    if start_runner:
        ensure_runner()
    return cfg


def ensure_poller() -> None:
    global _poller
    if _poller is not None:
        return
    _poller = SqlitePoller(interval=1.0)
    _poller.start()


def stop_poller() -> None:
    global _poller
    if _poller is None:
        return
    _poller.stop()
    _poller = None


def ensure_runner():
    global _runner
    if _runner is not None:
        return _runner
    socket_path = os.environ.get("REFINE_RUNNER_SOCKET")
    if socket_path:
        _runner = _SocketRunnerClient(socket_path)
        return _runner
    if os.environ.get("REFINE_NO_INPROCESS_RUNNER") == "1":
        raise config.ConfigError("Backend runner socket is not configured.")
    from refine_server.runner import Runner

    _runner = Runner()
    _runner.start()
    return _runner


def stop_runner() -> None:
    global _runner
    if _runner is None:
        return
    _runner.shutdown()
    _runner = None


def runner_call(
    method: str,
    params: dict | None = None,
    *,
    timeout: float = 30.0,
) -> dict:
    runner = ensure_runner()
    if isinstance(runner, _SocketRunnerClient):
        return runner.call(method, params or {}, timeout=timeout)
    return runner.call(method, params or {})


def backend_info() -> dict:
    socket_path = os.environ.get("REFINE_RUNNER_SOCKET") or ""
    external = bool(socket_path)
    no_inprocess = os.environ.get("REFINE_NO_INPROCESS_RUNNER") == "1"
    return {
        "process_model": "supervisor" if external else "single_process",
        "transport": "unix_socket" if external else "direct_call",
        "socket_path": socket_path,
        "in_process_runner_allowed": not no_inprocess,
        "runner_client_loaded": _runner is not None,
        "ui_controls_runner_lifecycle": not external,
    }


def runner_status_snapshot() -> dict:
    """Best-effort live runner state for read-only UI summaries.

    Unlike `runner_call()`, this does not start the runner and does not route
    through the backend dispatcher/cache check. The dashboard can still render
    cached SQLite data quickly when the runner is busy or unavailable.
    """
    runner = _runner
    if runner is None:
        return {
            "runner_reachable": False,
            "backend": backend_info(),
            "pid": None,
            "running": [],
            "chat": [],
            "merger": None,
            "governance": None,
            "target_app_rebuild": None,
        }
    try:
        snap = runner.status_snapshot()
    except Exception:
        return {
            "runner_reachable": False,
            "backend": backend_info(),
            "pid": None,
            "running": [],
            "chat": [],
            "merger": None,
            "governance": None,
            "target_app_rebuild": None,
        }
    return {"runner_reachable": True, "backend": backend_info(), **snap}


def stop_all() -> None:
    stop_runner()
    stop_poller()


def detach_configured() -> None:
    """Stop project-scoped services and return this process to setup mode."""
    global _loaded_config_path
    stop_all()
    os.environ.pop(config.ENV_CONFIG_PATH, None)
    config.clear_cache()
    _loaded_config_path = None
