"""Web process runtime state shared by the entry point and project API."""
from __future__ import annotations

import os
import threading
import time
from pathlib import Path

from refine_server import config, db, project_state
from refine_runtime import identity
from refine_runtime.supervisor_protocol import (
    M_ATTACH_APP,
    M_BACKEND_CALL,
    M_DETACH_APP,
    M_ENSURE_WORKER,
    M_STATUS,
    M_STOP_WORKER,
    WORKER_STARTUP_TIMEOUT_SECONDS,
)
from refine_server.backend_protocol import M_RUNNING

from .poller import SqlitePoller

_poller: SqlitePoller | None = None
_runner = None
_runner_lock = threading.Lock()
_loaded_config_path: Path | None = None
_worker_pid: int | None = None


class _SupervisorRunnerClient:
    def __init__(self, worker_pid: int | None = None) -> None:
        self.worker_pid = worker_pid

    def call(
        self,
        method: str,
        params: dict | None = None,
        *,
        timeout: float = 30.0,
    ) -> dict:
        return runner_call(method, params or {}, timeout=timeout)

    def status_snapshot(self) -> dict:
        return _supervisor_worker_snapshot()

    def shutdown(self) -> None:
        return None


class _InProcessRunnerClient:
    def __init__(self) -> None:
        from refine_server.runner import Runner

        self.runner = Runner()

    def call(
        self,
        method: str,
        params: dict | None = None,
        *,
        timeout: float = 30.0,  # noqa: ARG002
    ) -> dict:
        return self.runner.call(method, params or {})

    def status_snapshot(self) -> dict:
        return self.runner.call(M_RUNNING, {})

    def shutdown(self) -> None:
        self.runner.shutdown()


def load_configured(
    path: Path | str | None = None,
    *,
    port: int | str | None = None,
    start_poller: bool = True,
    start_runner: bool = True,
    migrate: bool = False,
) -> config.Config:
    """Load config, initialize SQLite, and ensure background services run."""
    global _loaded_config_path
    requested_path = Path(path).resolve() if path is not None else config.find_config(port=port)
    cfg_preview = config.Config.load(path, port=port)
    os.environ.setdefault(config.ENV_UI_PORT, str(cfg_preview.web_port))
    os.environ.setdefault(config.ENV_UI_SCOPE, str(cfg_preview.web_port))
    os.environ[config.ENV_RUN_DIR] = str(config.local_run_dir(port=cfg_preview.web_port))
    from refine_server import project_state

    schema = project_state.schema_status(cfg_preview.volume_root)
    if (
        not schema.get("compatible")
        and schema.get("migration_required")
        and (
            project_state.migration_requires_manual(schema)
            or not project_state.empty_refine_state(cfg_preview.volume_root)
        )
    ):
        if not migrate or project_state.migration_requires_manual(schema):
            raise config.ConfigError(
                f"{project_state.migration_block_message(schema)} "
                f"{project_state.migration_block_details(schema)}"
            )
    if not schema.get("compatible") and not schema.get("migration_required"):
        raise config.ConfigError(
            "Project schema is not supported by this Refine version."
        )
    if schema.get("migration_required") and migrate:
        project_state.ensure_initialized(migrate=True, root=cfg_preview.volume_root)
    if (_loaded_config_path is not None and requested_path is not None
            and requested_path.resolve() != _loaded_config_path):
        stop_all()
    cfg = config.get(path=path, reload=True, port=port)
    os.environ[config.ENV_CONFIG_PATH] = str(cfg.config_path)
    os.environ.pop("REFINE_RUNNER_SOCKET", None)
    os.environ["REFINE_NO_INPROCESS_RUNNER"] = "1"
    _loaded_config_path = cfg.config_path
    db.init_db()
    conn = db.connect(cfg.sqlite_path)
    try:
        project_state.ensure_initialized(conn, migrate=migrate, root=cfg.volume_root)
    finally:
        conn.close()
    os.environ[config.ENV_LOCAL_NODE_ID] = project_state.local_node_id(
        root=cfg.volume_root,
    )
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
    global _runner, _worker_pid
    with _runner_lock:
        if _runner is not None:
            return _runner

        cfg = config.get(reload=True)
        if os.environ.get("REFINE_TEST_INPROCESS_BACKEND") == "1":
            _runner = _InProcessRunnerClient()
            _worker_pid = os.getpid()
            return _runner
        supervisor_socket = _supervisor_socket_path(cfg)
        try:
            result = _supervisor_request(
                M_ENSURE_WORKER,
                {
                    "config_path": str(cfg.config_path),
                    "local_node_id": _local_node_id_or_empty(),
                },
                timeout=WORKER_STARTUP_TIMEOUT_SECONDS + 15.0,
            )
        except config.ConfigError:
            raise
        os.environ["REFINE_SUPERVISOR_SOCKET"] = str(supervisor_socket)
        os.environ["REFINE_NO_INPROCESS_RUNNER"] = "1"
        _worker_pid = _int_or_none(result.get("worker_pid"))
        _runner = _SupervisorRunnerClient(_worker_pid)
        return _runner


def stop_runner() -> None:
    global _runner, _worker_pid
    runner = _runner
    _runner = None
    worker_pid = _worker_pid
    if runner is not None:
        try:
            runner.shutdown()
        except Exception:
            pass
    if os.environ.get("REFINE_TEST_INPROCESS_BACKEND") == "1":
        _worker_pid = None
        return
    if runner is not None or worker_pid is not None:
        try:
            _supervisor_request(M_STOP_WORKER, {}, timeout=10.0)
        except Exception:
            pass
    _worker_pid = None


def runner_call(
    method: str,
    params: dict | None = None,
    *,
    timeout: float = 30.0,
) -> dict:
    global _worker_pid
    if os.environ.get("REFINE_TEST_INPROCESS_BACKEND") == "1":
        runner = ensure_runner()
        return runner.call(method, params or {})
    cfg = config.get(reload=True)
    result = _supervisor_request(
        M_BACKEND_CALL,
        {
            "config_path": str(cfg.config_path),
            "local_node_id": _local_node_id_or_empty(),
            "method": method,
            "params": params or {},
            "timeout": timeout,
        },
        timeout=timeout + WORKER_STARTUP_TIMEOUT_SECONDS + 15.0,
    )
    _worker_pid = _int_or_none(result.get("worker_pid")) or _worker_pid
    return result


def backend_info() -> dict:
    try:
        cfg = config.get(reload=False)
    except config.ConfigError:
        cfg = None
    supervisor_socket = str(_supervisor_socket_path(cfg))
    worker_pid = _worker_pid
    socket_path = ""
    try:
        status = _supervisor_request(M_STATUS, {}, timeout=2.0)
        worker = status.get("worker") if isinstance(status.get("worker"), dict) else {}
        worker_pid = _int_or_none(worker.get("pid")) or worker_pid
        socket_path = str(worker.get("socket_path") or socket_path)
    except Exception:
        pass
    return {
        "process_model": "supervisor",
        "transport": "unix_socket",
        "socket_path": socket_path,
        "worker_socket_path": socket_path,
        "supervisor_socket_path": supervisor_socket,
        "worker_pid": worker_pid,
        "source_fingerprint": identity.SOURCE_FINGERPRINT,
        "refine_version": identity.REFINE_VERSION,
        "local_node_id": _local_node_id_or_empty(),
        "in_process_runner_allowed": False,
        "runner_client_loaded": _runner is not None,
        "ui_controls_runner_lifecycle": False,
    }


def _supervisor_socket_path(cfg: config.Config | None) -> Path:
    from refine_runtime import ipc

    try:
        port = int(os.environ.get("REFINE_UI_PORT") or (cfg.web_port if cfg else 8080))
    except ValueError:
        port = cfg.web_port if cfg else 8080
    raw = os.environ.get("REFINE_SUPERVISOR_SOCKET")
    return Path(raw) if raw else ipc.supervisor_socket_path(port)


def _supervisor_request(
    method: str,
    params: dict | None = None,
    *,
    timeout: float = 30.0,
) -> dict:
    from refine_runtime import ipc

    try:
        cfg = config.get(reload=True)
    except config.ConfigError:
        cfg = None
    socket_path = _supervisor_socket_path(cfg)
    os.environ["REFINE_SUPERVISOR_SOCKET"] = str(socket_path)
    try:
        return ipc.request(socket_path, method, params or {}, timeout=timeout)
    except ipc.IpcError as e:
        message = f"Refine supervisor reported {e.code} at {socket_path}: {e.message}"
        if e.details:
            message = f"{message} ({e.details})"
        raise config.ConfigError(message) from e
    except Exception as e:
        raise config.ConfigError(
            f"Refine supervisor is not reachable at {socket_path}: {e}"
        ) from e


def _int_or_none(value: object) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _local_node_id_or_empty() -> str:
    try:
        return project_state.local_node_id()
    except Exception:
        return ""


def refresh_local_node(node_id: str | None = None) -> str:
    cfg = config.get(reload=True)
    local_node_id = str(
        node_id or project_state.local_node_id(root=cfg.volume_root),
    ).strip()
    if not local_node_id:
        local_node_id = project_state.active_node_id(root=cfg.volume_root)
    os.environ[config.ENV_LOCAL_NODE_ID] = local_node_id
    return local_node_id


def runner_status_snapshot() -> dict:
    """Best-effort live runner state for read-only UI summaries.

    Unlike `runner_call()`, this does not start the runner and does not route
    through the backend dispatcher/cache check. The dashboard can still render
    cached SQLite data quickly when the runner is busy or unavailable.
    """
    if os.environ.get("REFINE_TEST_INPROCESS_BACKEND") == "1" and _runner is not None:
        try:
            snap = _runner.status_snapshot()
        except Exception:
            snap = {}
        return {"runner_reachable": bool(snap), "backend": backend_info(), **snap}
    snap = _supervisor_worker_snapshot()
    if not snap.get("runner_reachable"):
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
    global _runner, _worker_pid
    if isinstance(_runner, _InProcessRunnerClient):
        try:
            _runner.shutdown()
        except Exception:
            pass
    _runner = None
    _worker_pid = None
    stop_poller()


def _supervisor_worker_snapshot() -> dict:
    try:
        status = _supervisor_request(M_STATUS, {}, timeout=2.0)
    except Exception:
        return {"runner_reachable": False}
    worker = status.get("worker") if isinstance(status.get("worker"), dict) else {}
    snapshot = status.get("worker_snapshot") if isinstance(status.get("worker_snapshot"), dict) else {}
    pid = _int_or_none(worker.get("pid"))
    if pid is not None:
        global _worker_pid
        _worker_pid = pid
    return {"pid": pid, **snapshot}


def attach_project_via_supervisor(body: dict, *, clone_dir: Path, port: int) -> tuple[int, dict]:
    result = _supervisor_request(
        M_ATTACH_APP,
        {"body": body, "clone_dir": str(clone_dir)},
        timeout=WORKER_STARTUP_TIMEOUT_SECONDS + 120.0,
    )
    code = int(result.get("http_status") or 500)
    payload = result.get("body") if isinstance(result.get("body"), dict) else {}
    if code == 200 and payload.get("config_path"):
        adopt_supervisor_config(
            payload["config_path"],
            port=port,
            start_poller=body.get("start_poller") is not False,
        )
    return code, payload


def adopt_supervisor_config(
    path: Path | str,
    *,
    port: int | str | None = None,
    start_poller: bool = True,
) -> config.Config:
    global _loaded_config_path
    cfg = config.get(path=str(path), reload=True, port=port)
    os.environ[config.ENV_CONFIG_PATH] = str(cfg.config_path)
    os.environ[config.ENV_UI_PORT] = str(cfg.web_port)
    os.environ[config.ENV_UI_SCOPE] = str(cfg.web_port)
    os.environ[config.ENV_RUN_DIR] = str(config.local_run_dir(port=cfg.web_port))
    os.environ.pop("REFINE_RUNNER_SOCKET", None)
    os.environ["REFINE_NO_INPROCESS_RUNNER"] = "1"
    os.environ[config.ENV_LOCAL_NODE_ID] = project_state.local_node_id(
        root=cfg.volume_root,
    )
    _loaded_config_path = cfg.config_path
    db.init_db(cfg.sqlite_path)
    if start_poller:
        stop_poller()
        ensure_poller()
    return cfg


def detach_configured() -> None:
    """Stop project-scoped services and return this process to no-app state."""
    global _loaded_config_path
    try:
        _supervisor_request(M_DETACH_APP, {}, timeout=10.0)
    except Exception:
        stop_all()
    else:
        stop_poller()
    os.environ.pop(config.ENV_CONFIG_PATH, None)
    config.clear_cache()
    _loaded_config_path = None
