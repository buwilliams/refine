"""Web process runtime state shared by the entry point and project API."""
from __future__ import annotations

from pathlib import Path

from refine_server import config, db

from .poller import SqlitePoller

_poller: SqlitePoller | None = None
_runner = None
_loaded_config_path: Path | None = None


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
    _loaded_config_path = cfg.config_path
    db.init_db()
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


def runner_call(method: str, params: dict | None = None) -> dict:
    return ensure_runner().call(method, params or {})


def stop_all() -> None:
    stop_runner()
    stop_poller()
