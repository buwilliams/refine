"""Webapp entry point.

Loads refine.toml when available, initializes SQLite, starts the SSE poller,
and serves HTTP. Without config, it serves a host-native setup UI so the user
can create or attach a project from the browser.
Invoked as `uv run refine web` (the CLI dispatcher).
"""
from __future__ import annotations

import os
import signal
import sys

from refine_shared import config

from . import runtime
from .server import run


def main() -> int:
    def _shutdown(signum, _frame):  # noqa: ANN001
        sys.stderr.write(f"\n[refine-web] caught signal {signum}, shutting down\n")
        runtime.stop_all()
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, _shutdown)
    try:
        cfg = runtime.load_configured()
        host = cfg.web_host
        port = cfg.web_port
    except config.ConfigError as e:
        host = os.environ.get("REFINE_WEB_HOST", "127.0.0.1")
        port = int(os.environ.get("REFINE_WEB_PORT", "8080"))
        sys.stderr.write(f"[refine-web] setup mode: {e}\n")
    try:
        run(host=host, port=port)
    except KeyboardInterrupt:
        sys.stderr.write("\n[refine-web] shutting down\n")
        runtime.stop_all()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
