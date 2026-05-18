"""Project switch state consistency tests."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, git, init_refine, make_client_repo


def test_client_switch_path(root: Path) -> None:
    common_js = (root / "refine_ui/static/js/common.js").read_text(encoding="utf-8")
    settings_js = (root / "refine_ui/static/js/features/settings.js").read_text(encoding="utf-8")
    chat_js = (root / "refine_ui/static/js/features/chat.js").read_text(encoding="utf-8")

    assert "function openAddAppModal(options = {})" in common_js
    add_app_body = common_js.split("function openAddAppModal(options = {})", 1)[1]
    add_app_body = add_app_body.split("\n}", 1)[0]
    for expected in (
        'title: "Add app"',
        'okLabel: "Add and switch"',
        "reloadOnSuccess: false",
    ):
        assert expected in add_app_body, expected

    first_run_body = common_js.split("async function ensureProjectAttached()", 1)[1]
    first_run_body = first_run_body.split("\n}", 1)[0]
    assert "openAddAppModal(" in first_run_body
    assert "return !!result" in first_run_body

    assert "async function applyProjectAttachResult(result)" in common_js
    switch_body = common_js.split("async function applyProjectAttachResult(result)", 1)[1]
    switch_body = switch_body.split("\n}", 1)[0]
    for expected in (
        "state.project = result",
        "resetChatForProjectSwitch()",
        "initSSE()",
        "await refreshFeatures()",
        "await refreshReporters({ selectFallback: true })",
        "await refreshTargetAppToggle()",
        'location.hash = "#/system/project"',
    ):
        assert expected in switch_body, expected

    assert "function reconcileLastReporter" in common_js
    assert "localStorage.removeItem(\"refine_last_reporter\")" in common_js
    assert "function resetChatForProjectSwitch()" in chat_js
    assert "await openAddAppModal()" in settings_js
    assert "await applyProjectAttachResult(result)" in settings_js
    assert "window.location.reload()" not in settings_js


def test_runtime_switch_resets_services() -> None:
    tmp, client1 = make_client_repo("refine-project-switch-")
    conn = init_refine(client1)
    conn.close()
    try:
        from refine_cli.cli import bootstrap_client_repo
        from refine_ui import runtime

        client2 = tmp / "client-two"
        client2.mkdir()
        git(client2, "init", "-q")
        git(client2, "config", "user.email", "t@x")
        git(client2, "config", "user.name", "t")
        (client2 / "app.txt").write_text("base\n", encoding="utf-8")
        git(client2, "add", "app.txt")
        git(client2, "commit", "-m", "init")
        boot = bootstrap_client_repo(
            client2,
            clone_dir=Path.cwd(),
            force=True,
            create=False,
            init_git=False,
            reuse_existing_config=True,
            install_unit=False,
        )

        runtime.load_configured(
            client1 / ".refine" / "refine.toml",
            start_poller=False,
            start_runner=False,
        )

        class FakePoller:
            stopped = False

            def stop(self) -> None:
                self.stopped = True

        class FakeRunner:
            stopped = False

            def shutdown(self) -> None:
                self.stopped = True

        fake_poller = FakePoller()
        fake_runner = FakeRunner()
        runtime._poller = fake_poller  # type: ignore[attr-defined]
        runtime._runner = fake_runner  # type: ignore[attr-defined]

        runtime.load_configured(
            boot["config_path"],
            start_poller=False,
            start_runner=False,
        )

        assert fake_poller.stopped is True
        assert fake_runner.stopped is True
        assert runtime._poller is None  # type: ignore[attr-defined]
        assert runtime._runner is None  # type: ignore[attr-defined]
    finally:
        try:
            runtime.stop_all()  # type: ignore[name-defined]
        except Exception:
            pass
        cleanup_tmp(tmp)


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    test_client_switch_path(root)
    test_runtime_switch_resets_services()
    print("project switch state tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
