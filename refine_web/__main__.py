"""Webapp entry point.

Initializes SQLite, starts the SSE poller, and serves HTTP.
"""
from __future__ import annotations

import os
import sys

from refine_shared import db

from .poller import SqlitePoller
from .server import run


def main() -> int:
    # Initialize DB if needed (idempotent).
    db.init_db()
    # Start polling SQLite for SSE events.
    poller = SqlitePoller(interval=1.0)
    poller.start()
    port = int(os.environ.get("REFINE_WEB_PORT", "8080"))
    host = os.environ.get("REFINE_WEB_HOST", "0.0.0.0")
    try:
        run(host=host, port=port)
    except KeyboardInterrupt:
        sys.stderr.write("\n[refine-web] shutting down\n")
        poller.stop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
