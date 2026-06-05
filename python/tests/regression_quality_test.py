"""Managed Playwright regression storage and runner tests."""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-regressions-")
    conn = init_refine(client)
    try:
        from refine_server import config, db, regressions, target_app

        root = config.get().volume_root
        gitignore = config.ensure_refine_gitignore(root).read_text(encoding="utf-8")
        assert "regressions/runs/" in gitignore

        reg = regressions.create_regression(
            title="Dashboard smoke",
            description="Open dashboard",
            prompt="Go to the dashboard and capture the status cards.",
            root=root,
        )
        assert reg["id"] == "dashboard-smoke"
        assert (root / "regressions" / "manifest.json").is_file()
        assert (root / "regressions" / "specs" / "dashboard-smoke.js").is_file()
        listed = regressions.list_regressions(root)
        assert listed[0]["title"] == "Dashboard smoke"

        updated = regressions.update_regression(
            "dashboard-smoke",
            {"enabled": False, "viewport": {"width": 800, "height": 600}},
            root=root,
        )
        assert updated and updated["enabled"] is False
        assert updated["viewport"] == {"width": 800, "height": 600}
        regressions.update_regression("dashboard-smoke", {"enabled": True}, root=root)

        db.set_setting(conn, "target_app_url", "http://127.0.0.1:19180")
        db.set_setting(conn, "target_app_start_command", "start-app")
        db.set_setting(conn, "target_app_stop_command", "stop-app")

        calls: list[tuple[str, str]] = []
        old_run_operation = target_app.run_operation
        old_subprocess_run = regressions.subprocess.run
        old_which = regressions.shutil.which

        def fake_run_operation(kind: str, cfg: dict) -> dict:
            calls.append((kind, cfg.get("root") or ""))
            return {"ok": True, "state": "running", "message": f"{kind} ok"}

        def fake_subprocess_run(cmd, **kwargs):  # noqa: ANN001, ANN202
            env = kwargs["env"]
            Path(env["REFINE_REGRESSION_SCREENSHOT"]).write_bytes(b"png")
            return subprocess.CompletedProcess(cmd, 0, stdout="ok\n", stderr="")

        target_app.run_operation = fake_run_operation
        regressions.subprocess.run = fake_subprocess_run
        regressions.shutil.which = lambda _name: "/usr/bin/npx"
        try:
            target_root = client / ".git" / "refine-worktrees" / "example"
            target_root.mkdir(parents=True)
            result = regressions.run_all(conn, root=root, target_root=target_root)
        finally:
            target_app.run_operation = old_run_operation
            regressions.subprocess.run = old_subprocess_run
            regressions.shutil.which = old_which

        assert result["ok"] is True, result
        assert result["runs"][0]["screenshot_path"].endswith("screenshot.png")
        assert calls[0] == ("start", str(target_root))
        assert calls[-1] == ("stop", str(target_root))
        latest = regressions.latest_run("dashboard-smoke", root=root)
        assert latest and latest["ok"] is True
        assert latest["screenshot_data_url"].startswith("data:image/png;base64,")

        assert regressions.delete_regression("dashboard-smoke", root=root) is True
        assert regressions.list_regressions(root) == []
        print("regression quality tests OK")
        return 0
    finally:
        conn.close()
        cleanup_tmp(tmp)


if __name__ == "__main__":
    raise SystemExit(main())
