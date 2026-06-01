"""Runner worker process entrypoint."""
from __future__ import annotations

import os
import signal
import sys
import threading
from pathlib import Path

from refine_runtime.ipc import IpcServer
from refine_server import config
from refine_server.runner import Runner


def main() -> int:
    sock = os.environ.get("REFINE_RUNNER_SOCKET")
    if not sock:
        print("REFINE_RUNNER_SOCKET is required", file=sys.stderr)
        return 2
    config.get()  # fail early with a clear config error
    runner = Runner()
    stop_event = threading.Event()
    server = IpcServer(Path(sock), runner.call)

    def _on_signal(signum, _frame):  # noqa: ANN001
        sys.stderr.write(f"\n[refine-worker] caught signal {signum}, shutting down\n")
        stop_event.set()

    signal.signal(signal.SIGINT, _on_signal)
    signal.signal(signal.SIGTERM, _on_signal)

    runner.start()
    server.start()
    sys.stderr.write(f"[refine-worker] listening on {sock}\n")
    try:
        while not stop_event.is_set():
            stop_event.wait(1.0)
    finally:
        server.stop()
        runner.shutdown()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
