"""Spin up the UI backend on temp paths and validate the wiring.

This is a "does the whole thing boot and respond to pings" test. It does NOT
exercise a real agent CLI or push to a remote — those need a configured
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
import urllib.error
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
    os.chdir(client)

    # Clear any cached refine modules from prior test runs.
    for mod in list(sys.modules):
        if mod.startswith("refine"):
            del sys.modules[mod]

    from refine_server import config

    # Write the config (equivalent to `refine target`). Override the web port
    # so we don't collide with any local 8080.
    cfg_path = config.write_defaults(client / ".refine")
    cfg_path.write_text(
        cfg_path.read_text().replace("port = 8080", "port = 18099")
        .replace("0.0.0.0", "127.0.0.1"),
        encoding="utf-8",
    )
    cfg = config.get(reload=True)

    from refine_server.backend_protocol import M_PING
    from refine_ui import runtime
    from refine_ui import server as web_server

    runtime.load_configured(cfg_path)
    print("[ok] backend runner started in UI process")
    try:
        resp = runtime.runner_call(M_PING, {})
        assert resp.get("pong") is True
        print("[ok] direct backend ping → pong")

        web_thread = threading.Thread(
            target=lambda: web_server.run(host=cfg.web_host, port=cfg.web_port),
            daemon=True,
        )
        web_thread.start()
        time.sleep(0.6)  # give server time to bind

        def get_json(path: str) -> tuple[int, dict]:
            req = urllib.request.Request(
                f"http://{cfg.web_host}:{cfg.web_port}{path}",
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
                f"http://{cfg.web_host}:{cfg.web_port}{path}",
                data=data, method="POST",
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
        print("[ok] /api/dashboard → 200, backend runner reachable")

        status, rep = post_json("/api/reporters", {"name": "Jane Doe"})
        assert status == 201
        print(f"[ok] reporter created: {rep['reporter']['name']}")

        status, created = post_json("/api/gaps", {
            "reporter": "Jane Doe",
            "actual": "Login button is red.",
            "target": "Login button should be blue.",
            "name": "Recolor login button",
        })
        assert status == 201, created
        gap_id = created["gap"]["id"]
        print(f"[ok] gap created: {gap_id}")

        status, fetched = get_json(f"/api/gaps/{gap_id}")
        assert status == 200
        assert fetched["gap"]["name"] == "Recolor login button"
        assert fetched["gap"]["status"] == "backlog"
        assert fetched["gap"]["rounds"][0]["reporter"] == "Jane Doe"
        print("[ok] gap fetched + status=backlog + reporter on round")

        # Appending to a `backlog` gap should be rejected per spec — only
        # `review` (a previously-worked Gap with the user adding a follow-up
        # round) accepts new rounds via this endpoint.
        status, append_result = post_json(f"/api/gaps/{gap_id}/rounds", {
            "reporter": "Jane Doe", "actual": "still red", "target": "blue",
        })
        assert status == 409, append_result
        print("[ok] appending to a `todo` gap is correctly rejected (409)")

        # Cancel from todo → cancelled.
        status, _ = post_json(f"/api/gaps/{gap_id}/cancel", {})
        assert status == 200, _
        print("[ok] cancel from todo → 200")

        status, act = get_json("/api/activity?limit=20")
        assert status == 200
        assert isinstance(act["activity"], list) and len(act["activity"]) >= 1
        print(f"[ok] activity feed: {len(act['activity'])} entries")

        status, s = get_json("/api/settings")
        assert status == 200
        assert "parallel_run_cap" in s["settings"]
        print("[ok] /api/settings")

        status, project = get_json("/api/project/status")
        assert status == 200
        assert project["attached"] is True
        assert project["client_repo"] == str(client)
        assert [app["path"] for app in project["apps"]] == [str(client)]
        assert project["registry_enabled"] is False
        print("[ok] /api/project/status includes current app without registry")

        status, d = get_json("/api/diagnostics")
        assert status == 200
        assert d.get("reachable") is True
        print("[ok] /api/diagnostics")

    finally:
        runtime.stop_all()
        time.sleep(0.2)
        os.chdir(tempfile.gettempdir())
        shutil.rmtree(tmp, ignore_errors=True)

    print("\nALL OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
