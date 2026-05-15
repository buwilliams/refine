"""Web process runtime state shared by the entry point and project API."""
from __future__ import annotations

from pathlib import Path

from refine_shared import config, db

from .poller import SqlitePoller

_poller: SqlitePoller | None = None


def load_configured(path: Path | str | None = None, *, start_poller: bool = True) -> config.Config:
    """Load config, initialize SQLite, and ensure the SSE poller is running."""
    cfg = config.get(path=path, reload=True)
    db.init_db()
    if start_poller:
        ensure_poller()
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
