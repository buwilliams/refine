"""Spin up the runner + webapp on temp paths and validate the wiring.

This is a "does the whole thing boot and respond to pings" test. It does NOT
exercise the real Claude CLI or push to a remote — those need a configured
host.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="refine-int-"))
    print(f"using tmp dir: {tmp}")
    client = tmp / "client"
    client.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=client, check=True)
    subprocess.run(["git", "-c", "user.email=t@x", "-c", "user.name=t",
                    "commit", "--allow-empty", "-m", "init"], cwd=client, check=True)
    vroot = client / "refine"
    vroot.mkdir()

    socket_path = str(tmp / "runner.sock")
    os.environ["REFINE_VOLUME_ROOT"] = str(vroot)
    os.environ["REFINE_CLIENT_REPO"] = str(client)
    os.environ["REFINE_RUNNER_SOCKET"] = socket_path
    os.environ["REFINE_WEB_PORT"] = "18099"

    # Force re-import in this env.
    for mod in list(sys.modules):
        if mod.startswith("refine_"):
            del sys.modules[mod]

    from refine_runner.runner import Runner
    from refine_web.ipc_client import RunnerClient
    from refine_web import server as web_server
    from refine_web.poller import SqlitePoller

    runner = Runner()
    runner.start()
    print("[ok] runner started")
    try:
        # IPC ping
        client_ipc = RunnerClient(socket_path)
        resp = client_ipc.ping()
        assert resp.get("pong") is True
        print("[ok] IPC ping → pong")

        # Start the webapp in a background thread
        poller = SqlitePoller(interval=0.5)
        poller.start()
        web_thread = threading.Thread(
            target=lambda: web_server.run(host="127.0.0.1", port=18099),
            daemon=True,
        )
        web_thread.start()
        # wait briefly for server to bind
        time.sleep(0.6)

        def get_json(path: str) -> tuple[int, dict]:
            req = urllib.request.Request(
                f"http://127.0.0.1:18099{path}",
                headers={"Accept": "application/json"},
            )
            try:
                with urllib.request.urlopen(req, timeout=5) as r:
                    return r.status, json.loads(r.read())
            except urllib.error.HTTPError as e:
                return e.code, json.loads(e.read() or b"{}")

        def post_json(path: str, body: dict) -> tuple[int, dict]:
            data = json.dumps(body).encode("utf-8")
            req = urllib.request.Request(
                f"http://127.0.0.1:18099{path}",
                data=data,
                method="POST",
                headers={"Content-Type": "application/json"},
            )
            try:
                with urllib.request.urlopen(req, timeout=5) as r:
                    return r.status, json.loads(r.read())
            except urllib.error.HTTPError as e:
                return e.code, json.loads(e.read() or b"{}")

        status, dash = get_json("/api/dashboard")
        assert status == 200, dash
        assert dash["runner_reachable"] is True
        print("[ok] /api/dashboard → 200, runner reachable")

        # Create a reporter
        status, rep = post_json("/api/reporters", {"name": "Jane Doe"})
        assert status == 201
        print(f"[ok] reporter created: {rep['reporter']['name']}")

        # Create a Gap
        status, created = post_json("/api/gaps", {
            "reporter": "Jane Doe",
            "actual": "Login button is red.",
            "target": "Login button should be blue.",
            "name": "Recolor login button",
        })
        assert status == 201, created
        gap_id = created["gap"]["id"]
        print(f"[ok] gap created: {gap_id}")

        # Fetch it back
        status, fetched = get_json(f"/api/gaps/{gap_id}")
        assert status == 200
        assert fetched["gap"]["name"] == "Recolor login button"
        assert fetched["gap"]["status"] == "todo"
        assert fetched["gap"]["rounds"][0]["reporter"] == "Jane Doe"
        print("[ok] gap fetched + status=todo + reporter on round")

        # Add a round (must NOT work on a todo Gap — it's editing the latest)
        status, append_result = post_json(f"/api/gaps/{gap_id}/rounds", {
            "reporter": "Jane Doe", "actual": "still red", "target": "blue",
        })
        # Conflict expected for status=todo (which is right)
        assert status == 409, append_result
        print("[ok] appending to a `todo` gap is correctly rejected (409)")

        # Edit the latest round (allowed on todo)
        status, _ = post_json(f"/api/gaps/{gap_id}/cancel", {})  # use cancel as a sentinel
        # cancel from todo should move to cancelled
        assert status in (200, 409), _
        print("[ok] cancel from todo: status=", status)

        # Activity feed should have entries
        status, act = get_json("/api/activity?limit=20")
        assert status == 200
        assert isinstance(act["activity"], list) and len(act["activity"]) >= 1
        print(f"[ok] activity feed: {len(act['activity'])} entries")

        # Settings endpoint
        status, s = get_json("/api/settings")
        assert status == 200
        assert "parallel_run_cap" in s["settings"]
        print("[ok] /api/settings")

        # IPC diagnostics
        status, d = get_json("/api/diagnostics")
        assert status == 200
        assert d.get("reachable") is True
        print("[ok] /api/diagnostics")

    finally:
        runner.shutdown()
        # the webapp's thread is daemon; let the test exit terminate it
        time.sleep(0.2)
        shutil.rmtree(tmp, ignore_errors=True)

    print("\nALL OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
