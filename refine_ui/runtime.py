"""Web process runtime state shared by the entry point and project API."""
from __future__ import annotations

from pathlib import Path

from refine_shared import config, db

from .poller import SqlitePoller

_poller: SqlitePoller | None = None
_runner = None


def load_configured(
    path: Path | str | None = None,
    *,
    start_poller: bool = True,
    start_runner: bool = True,
) -> config.Config:
    """Load config, initialize SQLite, and ensure background services run."""
    cfg = config.get(path=path, reload=True)
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
