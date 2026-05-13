"""Run the refine-runner daemon on the host.

Invoked as `uv run refine start` (the CLI dispatcher).
"""
from __future__ import annotations

import signal
import sys
import threading

from refine_shared import config

from .runner import Runner


def main() -> int:
    config.get()  # ensure refine.toml is found early; surfaces a clean error
    runner = Runner()
    stop_event = threading.Event()

    def _on_signal(signum, _frame):  # noqa: ARG001
        sys.stderr.write(f"\n[refine-runner] caught signal {signum}, shutting down\n")
        stop_event.set()

    signal.signal(signal.SIGINT, _on_signal)
    signal.signal(signal.SIGTERM, _on_signal)

    runner.start()
    try:
        while not stop_event.is_set():
            stop_event.wait(timeout=1.0)
    finally:
        runner.shutdown()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
