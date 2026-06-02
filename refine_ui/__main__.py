"""Webapp entry point.

Loads refine.toml when available, initializes SQLite, starts the SSE poller,
and serves HTTP. Without config, it serves the host-native no-app UI so the
user can create or attach a project from the browser.
Invoked as `uv run refine ui` (the CLI dispatcher).
"""
from __future__ import annotations

import os
import signal
import sys

from refine_server import config

from . import runtime
from .server import run


def main() -> int:
    config.load_dotenv()

    def _shutdown(signum, _frame):  # noqa: ANN001
        sys.stderr.write(f"\n[refine-ui] caught signal {signum}, shutting down\n")
        runtime.stop_all()
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, _shutdown)
    try:
        cfg = runtime.load_configured()
        host = os.environ.get("REFINE_UI_HOST", cfg.web_host)
        port = int(os.environ.get("REFINE_UI_PORT", str(cfg.web_port)))
    except config.ConfigError as e:
        try:
            cfg = config.get(reload=True)
            host = os.environ.get("REFINE_UI_HOST", cfg.web_host)
            port = int(os.environ.get("REFINE_UI_PORT", str(cfg.web_port)))
        except config.ConfigError:
            host = os.environ.get("REFINE_UI_HOST", "0.0.0.0")
            port = int(os.environ.get("REFINE_UI_PORT", "8080"))
        sys.stderr.write(f"[refine-ui] no app attached: {e}\n")
    try:
        run(host=host, port=port)
    except KeyboardInterrupt:
        sys.stderr.write("\n[refine-ui] shutting down\n")
    finally:
        runtime.stop_all()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
