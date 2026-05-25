"""Validate first-run project setup without a pre-attached target."""
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
    tmp = Path(tempfile.mkdtemp(prefix="refine-setup-"))
    print(f"using tmp dir: {tmp}")
    clone = tmp / "refine-clone"
    (clone / "refine_cli").mkdir(parents=True)
    (clone / "pyproject.toml").write_text("[project]\nname = \"refine\"\n", encoding="utf-8")
    (clone / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
    os.chdir(clone)

    for mod in list(sys.modules):
        if mod.startswith("refine"):
            del sys.modules[mod]

    from refine_ui import server as web_server

    host = "127.0.0.1"
    port = 18123
    web_thread = threading.Thread(
        target=lambda: web_server.run(host=host, port=port),
        daemon=True,
    )
    web_thread.start()

    def get_json(path: str) -> tuple[int, dict]:
        req = urllib.request.Request(
            f"http://{host}:{port}{path}",
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
            f"http://{host}:{port}{path}",
            data=data,
            method="POST",
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=5) as r:
                return r.status, json.loads(r.read())
        except urllib.error.HTTPError as e:
            return e.code, json.loads(e.read() or b"{}")

    def delete_json(path: str, body: dict) -> tuple[int, dict]:
        data = json.dumps(body).encode("utf-8")
        req = urllib.request.Request(
            f"http://{host}:{port}{path}",
            data=data,
            method="DELETE",
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=5) as r:
                return r.status, json.loads(r.read())
        except urllib.error.HTTPError as e:
            return e.code, json.loads(e.read() or b"{}")

    try:
        for _ in range(30):
            try:
                status, snap = get_json("/api/project/status")
                break
            except OSError:
                time.sleep(0.1)
        else:
            raise AssertionError("web server did not start")

        assert status == 200, snap
        assert snap["attached"] is False, snap
        print("[ok] uninitialized web reports no attached project")

        client = tmp / "new-client"
        status, attached = post_json("/api/project/attach", {
            "path": str(client),
            "install_unit": False,
            "start_runner": False,
            "start_poller": False,
        })
        assert status == 200, attached
        assert attached["attached"] is True
        assert attached["client_repo"] == str(client)
        assert attached["git_initialized"] is True
        assert attached["config_created"] is True
        assert len(attached["apps"]) == 1
        assert (client / ".git").exists()
        assert (client / ".refine" / "refine.toml").is_file()
        assert (clone / ".refine-binding").is_file()
        assert not (clone / ".refine-current").exists()
        print("[ok] project attach creates repo + refine target artifacts")

        status, snap = get_json("/api/project/status")
        assert status == 200, snap
        assert snap["attached"] is True
        assert snap["client_repo"] == str(client)
        assert snap["apps"][0]["path"] == str(client)
        print("[ok] attached project is visible to the UI status check")

        from refine_server import config

        existing = tmp / "existing-client"
        existing.mkdir()
        cfg_path = config.write_defaults(existing / ".refine")
        cfg_path.write_text(
            cfg_path.read_text(encoding="utf-8") + "\n# sentinel: keep me\n",
            encoding="utf-8",
        )
        status, switched = post_json("/api/project/attach", {
            "path": str(existing),
            "install_unit": False,
            "start_runner": False,
            "start_poller": False,
        })
        assert status == 200, switched
        assert switched["client_repo"] == str(existing)
        assert switched["git_initialized"] is True
        assert switched["config_created"] is False
        assert not (clone / ".refine-current").exists()
        assert "# sentinel: keep me" in cfg_path.read_text(encoding="utf-8")
        assert len(switched["apps"]) == 2
        dirty = subprocess.run(
            ["git", "-C", str(client), "status", "--porcelain"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        assert dirty == "", dirty
        print("[ok] switching preserves existing .refine and cleans previous app")

        status, removed = delete_json("/api/projects", {"path": str(client)})
        assert status == 200, removed
        assert [app["path"] for app in removed["apps"]] == [str(existing)]
        print("[ok] app registry remove works")

        status, rejected = delete_json("/api/projects", {"path": str(existing)})
        assert status == 409, rejected
        print("[ok] current app cannot be removed from registry")
    finally:
        os.chdir(tempfile.gettempdir())
        shutil.rmtree(tmp, ignore_errors=True)

    print("\nALL OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
