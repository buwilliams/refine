"""Webapp entry point.

Loads refine.toml, initializes SQLite, starts the SSE poller, and serves HTTP.

Use `python -m refine web` (the CLI dispatcher) for normal operation. This
module's main() is kept for backwards compatibility with `python -m refine_web`.
"""
from __future__ import annotations

import sys

from refine_shared import config, db

from .poller import SqlitePoller
from .server import run


def main() -> int:
    cfg = config.get()
    db.init_db()
    poller = SqlitePoller(interval=1.0)
    poller.start()
    try:
        run(host=cfg.web_host, port=cfg.web_port)
    except KeyboardInterrupt:
        sys.stderr.write("\n[refine-web] shutting down\n")
        poller.stop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
