"""Project switch state consistency tests."""
from __future__ import annotations

import json
import os
import sqlite3
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, git, init_refine, make_client_repo, reset_refine_imports


def _read_optional(path: Path) -> bytes | None:
    try:
        return path.read_bytes()
    except FileNotFoundError:
        return None


def _restore_optional(path: Path, data: bytes | None) -> None:
    if data is None:
        try:
            path.unlink()
        except FileNotFoundError:
            pass
        return
    path.write_bytes(data)


def test_client_switch_path(root: Path) -> None:
    index_html = (root / "refine_ui/static/index.html").read_text(encoding="utf-8")
    base_css = (root / "refine_ui/static/css/base.css").read_text(encoding="utf-8")
    common_js = (root / "refine_ui/static/js/common.js").read_text(encoding="utf-8")
    settings_js = (
        root / "refine_ui/static/js/features/settings.js"
    ).read_text(encoding="utf-8") + (
        root / "refine_ui/static/js/features/settings_application.js"
    ).read_text(encoding="utf-8") + (
        root / "refine_ui/static/js/features/settings_instances.js"
    ).read_text(encoding="utf-8")
    toolbar_js = (root / "refine_ui/static/js/features/toolbar.js").read_text(encoding="utf-8")
    settings_surface_js = (root / "refine_ui/static/js/features/settings.js").read_text(encoding="utf-8")
    dashboard_js = (root / "refine_ui/static/js/features/dashboard.js").read_text(encoding="utf-8")
    gaps_list_js = (root / "refine_ui/static/js/features/gaps-list.js").read_text(encoding="utf-8")
    changes_js = (root / "refine_ui/static/js/features/changes.js").read_text(encoding="utf-8")
    logs_js = (root / "refine_ui/static/js/features/logs.js").read_text(encoding="utf-8")
    init_js = (root / "refine_ui/static/js/init.js").read_text(encoding="utf-8")
    target_app_js = (root / "refine_ui/static/js/target-app.js").read_text(encoding="utf-8")
    guide_js = (root / "refine_ui/static/js/features/guide.js").read_text(encoding="utf-8")
    api_py = (root / "refine_ui/api.py").read_text(encoding="utf-8")

    assert 'id="active-instance-label"' in index_html
    assert ".brand-instance" in base_css
    assert "function updateActiveInstanceLabel()" in common_js
    assert "updateActiveInstanceLabel()" in common_js

    assert "function openAddAppModal(options = {})" in common_js
    assert "async function maybeOpenProjectTemplateModal(project)" in common_js
    assert "function rememberPendingProjectTemplateModal(project)" in common_js
    assert "async function maybeOpenPendingProjectTemplateModal(project = state.project)" in common_js
    assert "function openProjectTemplateModal(templates)" in common_js
    assert 'api("GET", "/api/project/templates")' in common_js
    assert 'api("POST", "/api/project/scaffold"' in common_js
    assert "rememberPendingProjectTemplateModal(result)" in common_js
    assert "sessionStorage.setItem(PENDING_SCAFFOLD_PROJECT_KEY" in common_js
    assert "sessionStorage.removeItem(PENDING_SCAFFOLD_PROJECT_KEY)" in common_js
    add_app_body = common_js.split("function openAddAppModal(options = {})", 1)[1]
    add_app_body = add_app_body.split("\n}", 1)[0]
    for expected in (
        'title: "Add app"',
        'okLabel: "Add and switch"',
        "reloadOnSuccess: false",
        "openGuideOnSuccess: true",
    ):
        assert expected in add_app_body, expected

    first_run_body = common_js.split("async function ensureProjectAttached()", 1)[1]
    first_run_body = first_run_body.split("\n}", 1)[0]
    assert "openAddAppModal(" not in first_run_body
    assert "enterNoProjectMode(snap)" in first_run_body
    assert "return false" in first_run_body
    assert "await syncProjectUpdates({ silent: true })" in common_js
    assert "function renderNoProjectEmptyState" in common_js
    assert "Open the Guide to configure Refine and attach an app." in common_js
    assert "function renderNoProjectIfDetached" in common_js
    assert "function renderNoProjectIfApiDetached" in common_js
    assert 'updateNavAppContextLabel("No app")' in common_js
    assert "sseSource.close()" in common_js
    assert "enterNoProjectMode(state.project, { openGuidePanel: true })" in init_js
    assert "maybeOpenPendingProjectTemplateModal();" in init_js
    assert "if (!attached) return" not in init_js
    for source, title in (
        (dashboard_js, "Dashboard"),
        (gaps_list_js, "Gaps"),
        (changes_js, "Changes"),
        (logs_js, "Logs"),
    ):
        assert f'renderNoProjectIfDetached("{title}")' in source
        assert "renderNoProjectIfApiDetached" in source
    assert "function applyNoTargetAppSnapshot()" in target_app_js
    assert 'if (project.attached === false) return "No app";' in target_app_js
    assert 'if (!hasAttachedProject()) {' in target_app_js
    assert 'guideState.context === "no-app"' in guide_js
    assert "options.openTarget !== false" in guide_js
    assert "function resetGuideState" in guide_js
    assert "localStorage.removeItem(GUIDE_CHECKLIST_KEY)" in guide_js
    assert "async function loadSettingsSurfaceData()" in settings_surface_js
    assert 'const project = await api("GET", "/api/project/status");' in settings_surface_js
    assert "if (project.attached === false)" in settings_surface_js
    assert "return detachedSettingsSurfaceData(project);" in settings_surface_js
    assert "function renderSettingsNoProjectTab" in settings_surface_js
    assert "Open the Guide to configure Refine and attach an app before using" in settings_surface_js
    assert "data-settings-open-guide" in settings_surface_js
    no_app_tab_body = settings_surface_js.split("if (data.noProject)", 1)[1]
    no_app_tab_body = no_app_tab_body.split("if (surface === SETTINGS_SURFACES.settings)", 1)[0]
    assert "surface === SETTINGS_SURFACES.project && slug === \"application\"" in no_app_tab_body
    assert "return renderSettingsApplicationTab({" in no_app_tab_body
    assert "return renderSettingsNoProjectTab(surface.title);" in no_app_tab_body
    assert "_project_attached()" in api_py
    assert '"attached": False' in api_py

    assert "async function applyProjectAttachResult(result, options = {})" in common_js
    switch_body = common_js.split("async function applyProjectAttachResult(result, options = {})", 1)[1]
    switch_body = switch_body.split("\n}", 1)[0]
    for expected in (
        'if (typeof resetGuideState === "function") resetGuideState({ redraw: false });',
        "state.project = result",
        "resetChatForProjectSwitch()",
        "initSSE()",
        "await syncProjectUpdates({ silent: true })",
        "await refreshInstanceScopedState({ selectReporterFallback: true })",
        "await refreshTargetAppToggle()",
        'location.hash = "#/project/application"',
        "openGuide({",
    ):
        assert expected in switch_body, expected

    assert "function reconcileLastReporter" in common_js
    assert "async function refreshInstanceScopedState" in common_js
    instance_state_body = common_js.split("async function refreshInstanceScopedState", 1)[1]
    instance_state_body = instance_state_body.split("\n}", 1)[0]
    assert "resetChatForProjectSwitch()" in instance_state_body
    assert "localStorage.removeItem(\"refine_last_reporter\")" in common_js
    assert "Migrate and open" in common_js
    assert 'api("POST", "/api/project/attach", {' in common_js
    assert "migrate: true" in common_js
    assert "function resetChatForProjectSwitch()" in toolbar_js
    assert "await openAddAppModal()" in settings_js
    assert "await applyProjectAttachResult(result)" in settings_js
    remove_body = settings_js.split('api("DELETE", "/api/projects", { path })', 1)[1]
    remove_body = remove_body.split("await refreshSettingsTab", 1)[0]
    assert 'resetGuideState({ redraw: false })' in remove_body
    assert "await refreshInstanceScopedState()" in settings_js
    assert "active_instance_id: result.active_instance_id" in settings_js
    assert "updateActiveInstanceLabel()" in settings_js
    assert "window.location.reload()" not in settings_js
    assert "restart_pending" in api_py
    assert '"scaffold_required": scaffold_required' in api_py
    assert '"scaffold_required": _project_needs_scaffold_template(cfg.client_repo)' in api_py
    assert "PROJECT_TEMPLATE_DIR" in api_py
    assert "def list_project_templates()" in api_py
    assert "def create_project_scaffold_gap" in api_py
    assert "Refine is restarting for the selected app" in common_js


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


def test_blocked_switch_does_not_stop_current_app(root: Path) -> None:
    tmp, client1 = make_client_repo("refine-blocked-switch-")
    original_cwd = Path.cwd()
    binding = root / ".refine-binding"
    prior_binding = binding.read_text(encoding="utf-8") if binding.exists() else None
    registry = root / ".refine-apps.json"
    prior_registry = _read_optional(registry)
    old_cfg = os.environ.get("REFINE_CONFIG_PATH")
    try:
        conn = init_refine(client1)
        conn.close()
        os.chdir(root)
        from refine_server import config
        from refine_ui import api, runtime

        config.write_binding(root, client1)
        config.get(reload=True)
        runtime.load_configured(
            client1 / ".refine" / "refine.toml",
            start_runner=False,
            start_poller=False,
        )

        client2 = tmp / "legacy-client"
        client2.mkdir()
        git(client2, "init", "-q")
        git(client2, "config", "user.email", "t@x")
        git(client2, "config", "user.name", "t")
        config.write_defaults(client2 / ".refine")
        (client2 / ".refine" / "index.sqlite").write_text("legacy", encoding="utf-8")

        class FakeRunner:
            stopped = False

            def shutdown(self) -> None:
                self.stopped = True

        class FakePoller:
            stopped = False

            def stop(self) -> None:
                self.stopped = True

        fake_runner = FakeRunner()
        fake_poller = FakePoller()
        runtime._runner = fake_runner  # type: ignore[attr-defined]
        runtime._poller = fake_poller  # type: ignore[attr-defined]

        status, body = api.project_attach({
            "path": str(client2),
            "install_unit": False,
            "start_runner": False,
            "start_poller": False,
        })
        assert status == 409, body
        assert "migration required" in body["error"]["message"].lower()
        assert fake_runner.stopped is False
        assert fake_poller.stopped is False
        assert config.get(reload=True).client_repo == client1

        os.environ.pop("REFINE_CONFIG_PATH", None)
        config.write_binding(root, client2)
        config.get(reload=True)
        status, body = api.dashboard_summary()
        assert status == 409, body
        assert "migration required" in body["error"]["message"].lower()
        status, body = api.list_gaps()
        assert status == 409, body
        status, body = api.list_instances()
        assert status == 409, body
        config.write_binding(root, client1)
        config.get(reload=True)

        try:
            runtime.load_configured(
                client2 / ".refine" / "refine.toml",
                start_runner=False,
                start_poller=False,
            )
            raise AssertionError("legacy target should require migration")
        except config.ConfigError as e:
            assert "migration required" in str(e).lower()
        assert fake_runner.stopped is False
        assert fake_poller.stopped is False

        newer = tmp / "newer-client"
        newer.mkdir()
        git(newer, "init", "-q")
        git(newer, "config", "user.email", "t@x")
        git(newer, "config", "user.name", "t")
        config.write_defaults(newer / ".refine")
        (newer / ".refine" / "config.json").write_text(
            json.dumps({"schema_version": 999}),
            encoding="utf-8",
        )
        status, body = api.project_attach({
            "path": str(newer),
            "install_unit": False,
            "start_runner": False,
            "start_poller": False,
        })
        assert status == 409, body
        assert "not supported" in body["error"]["message"].lower()
        assert fake_runner.stopped is False
        assert fake_poller.stopped is False
        assert config.get(reload=True).client_repo == client1
    finally:
        try:
            from refine_ui import runtime
            runtime._runner = None  # type: ignore[attr-defined]
            runtime._poller = None  # type: ignore[attr-defined]
        except Exception:
            pass
        if prior_binding is None:
            try:
                binding.unlink()
            except FileNotFoundError:
                pass
        else:
            binding.write_text(prior_binding, encoding="utf-8")
        _restore_optional(registry, prior_registry)
        if old_cfg is None:
            os.environ.pop("REFINE_CONFIG_PATH", None)
        else:
            os.environ["REFINE_CONFIG_PATH"] = old_cfg
        os.chdir(original_cwd)
        cleanup_tmp(tmp)


def test_supervised_switch_schedules_restart_without_hot_loading(root: Path) -> None:
    tmp, client1 = make_client_repo("refine-supervised-switch-")
    conn = init_refine(client1)
    conn.close()
    original_cwd = Path.cwd()
    binding = root / ".refine-binding"
    prior_binding = binding.read_text(encoding="utf-8") if binding.exists() else None
    registry = root / ".refine-apps.json"
    prior_registry = _read_optional(registry)
    old_cfg_env = os.environ.get("REFINE_CONFIG_PATH")
    old_port = os.environ.get("REFINE_UI_PORT")
    try:
        os.chdir(root)
        from refine_server import config
        from refine_ui import api, runtime

        config.write_binding(root, client1)
        config.get(reload=True)
        runtime.load_configured(
            client1 / ".refine" / "refine.toml",
            start_runner=False,
            start_poller=False,
        )

        client2 = tmp / "client-two"
        client2.mkdir()
        git(client2, "init", "-q")
        git(client2, "config", "user.email", "t@x")
        git(client2, "config", "user.name", "t")
        (client2 / "app.txt").write_text("base\n", encoding="utf-8")
        git(client2, "add", "app.txt")
        git(client2, "commit", "-m", "init")

        old_backend_info = runtime.backend_info
        old_schedule_restart = api._schedule_supervisor_restart  # type: ignore[attr-defined]
        old_commit_refine_state = api._commit_refine_state  # type: ignore[attr-defined]
        old_git_stdout = api._git_stdout  # type: ignore[attr-defined]
        restarts: list[tuple[Path, Path]] = []
        try:
            runtime.backend_info = lambda: {  # type: ignore[assignment]
                "process_model": "supervisor",
                "ui_controls_runner_lifecycle": False,
            }
            api._commit_refine_state = lambda _repo: None  # type: ignore[assignment]
            api._git_stdout = lambda _repo, _args: ""  # type: ignore[assignment]
            api._schedule_supervisor_restart = (  # type: ignore[assignment]
                lambda clone_arg, cfg_arg: restarts.append(
                    (clone_arg, cfg_arg.config_path)
                ) or {"scheduled": True, "port": 18181, "log_path": "restart.log"}
            )
            os.environ["REFINE_UI_PORT"] = "18181"

            status, body = api.project_attach({
                "path": str(client2),
                "install_unit": False,
                "start_runner": False,
                "start_poller": False,
            })
        finally:
            runtime.backend_info = old_backend_info  # type: ignore[assignment]
            api._schedule_supervisor_restart = old_schedule_restart  # type: ignore[assignment]
            api._commit_refine_state = old_commit_refine_state  # type: ignore[assignment]
            api._git_stdout = old_git_stdout  # type: ignore[assignment]

        assert status == 200, body
        assert body["restart_pending"] is True
        assert body["client_repo"] == str(client2.resolve())
        assert restarts == [
            (root.resolve(), client2.resolve() / ".refine" / "refine.toml")
        ]
        assert config.read_binding(binding) == client2.resolve()
        assert runtime._loaded_config_path == client1 / ".refine" / "refine.toml"  # type: ignore[attr-defined]
    finally:
        try:
            from refine_ui import runtime
            runtime.stop_all()
        except Exception:
            pass
        if prior_binding is None:
            try:
                binding.unlink()
            except FileNotFoundError:
                pass
        else:
            binding.write_text(prior_binding, encoding="utf-8")
        _restore_optional(registry, prior_registry)
        if old_cfg_env is None:
            os.environ.pop("REFINE_CONFIG_PATH", None)
        else:
            os.environ["REFINE_CONFIG_PATH"] = old_cfg_env
        if old_port is None:
            os.environ.pop("REFINE_UI_PORT", None)
        else:
            os.environ["REFINE_UI_PORT"] = old_port
        os.chdir(original_cwd)
        cleanup_tmp(tmp)


def test_supervised_initial_attach_schedules_restart(root: Path) -> None:
    tmp, client = make_client_repo("refine-supervised-initial-attach-")
    clone = tmp / "refine-source"
    (clone / "refine_cli").mkdir(parents=True)
    (clone / "pyproject.toml").write_text(
        "[project]\nname = \"refine\"\n",
        encoding="utf-8",
    )
    (clone / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
    original_cwd = Path.cwd()
    old_cfg_env = os.environ.get("REFINE_CONFIG_PATH")
    old_port = os.environ.get("REFINE_UI_PORT")
    try:
        os.chdir(clone)
        os.environ.pop("REFINE_CONFIG_PATH", None)
        os.environ["REFINE_UI_PORT"] = "18182"
        reset_refine_imports()
        from refine_ui import api, runtime

        old_backend_info = runtime.backend_info
        old_schedule_restart = api._schedule_supervisor_restart  # type: ignore[attr-defined]
        old_load_configured = runtime.load_configured
        restarts: list[tuple[Path, Path]] = []
        try:
            runtime.backend_info = lambda: {  # type: ignore[assignment]
                "process_model": "supervisor",
                "ui_controls_runner_lifecycle": False,
            }
            runtime.load_configured = (  # type: ignore[assignment]
                lambda *args, **kwargs: (_ for _ in ()).throw(
                    AssertionError("initial supervised attach must restart")
                )
            )
            api._schedule_supervisor_restart = (  # type: ignore[assignment]
                lambda clone_arg, cfg_arg: restarts.append(
                    (clone_arg, cfg_arg.config_path)
                ) or {"scheduled": True, "port": 18182, "log_path": "restart.log"}
            )

            status, body = api.project_attach({
                "path": str(client),
                "install_unit": False,
                "start_runner": False,
                "start_poller": False,
            })
        finally:
            runtime.backend_info = old_backend_info  # type: ignore[assignment]
            runtime.load_configured = old_load_configured  # type: ignore[assignment]
            api._schedule_supervisor_restart = old_schedule_restart  # type: ignore[assignment]

        assert status == 200, body
        assert body["restart_pending"] is True
        assert body["client_repo"] == str(client.resolve())
        assert restarts == [
            (clone.resolve(), client.resolve() / ".refine" / "refine.toml")
        ]
        assert (clone / ".refine-binding").is_file()
    finally:
        os.chdir(original_cwd)
        if old_cfg_env is None:
            os.environ.pop("REFINE_CONFIG_PATH", None)
        else:
            os.environ["REFINE_CONFIG_PATH"] = old_cfg_env
        if old_port is None:
            os.environ.pop("REFINE_UI_PORT", None)
        else:
            os.environ["REFINE_UI_PORT"] = old_port
        cleanup_tmp(tmp)


def test_supervised_switch_migrates_target_before_restart(root: Path) -> None:
    tmp, client1 = make_client_repo("refine-supervised-migrate-")
    conn = init_refine(client1)
    conn.close()
    original_cwd = Path.cwd()
    binding = root / ".refine-binding"
    prior_binding = binding.read_text(encoding="utf-8") if binding.exists() else None
    registry = root / ".refine-apps.json"
    prior_registry = _read_optional(registry)
    old_cfg_env = os.environ.get("REFINE_CONFIG_PATH")
    old_port = os.environ.get("REFINE_UI_PORT")
    try:
        os.chdir(root)
        from refine_server import config, db, project_state
        from refine_ui import api, runtime

        config.write_binding(root, client1)
        config.get(reload=True)
        runtime.load_configured(
            client1 / ".refine" / "refine.toml",
            start_runner=False,
            start_poller=False,
        )

        legacy = tmp / "legacy-client"
        legacy.mkdir()
        git(legacy, "init", "-q")
        git(legacy, "config", "user.email", "t@x")
        git(legacy, "config", "user.name", "t")
        (legacy / "app.txt").write_text("base\n", encoding="utf-8")
        git(legacy, "add", "app.txt")
        git(legacy, "commit", "-m", "init")
        config.write_defaults(legacy / ".refine")
        legacy_db = legacy / ".refine" / "index.sqlite"
        db.init_db(legacy_db)
        legacy_conn = sqlite3.connect(str(legacy_db))
        try:
            legacy_conn.execute(
                "INSERT OR REPLACE INTO settings(key, value) VALUES (?, ?)",
                ("governance_product", "Legacy app"),
            )
            legacy_conn.commit()
        finally:
            legacy_conn.close()
        assert project_state.schema_status(legacy / ".refine")["migration_required"] is True

        old_backend_info = runtime.backend_info
        old_schedule_restart = api._schedule_supervisor_restart  # type: ignore[attr-defined]
        old_git_stdout = api._git_stdout  # type: ignore[attr-defined]
        restarts: list[tuple[Path, Path]] = []
        try:
            runtime.backend_info = lambda: {  # type: ignore[assignment]
                "process_model": "supervisor",
                "ui_controls_runner_lifecycle": False,
            }
            api._git_stdout = (  # type: ignore[assignment]
                lambda repo, args: ""
                if repo.resolve() == client1.resolve()
                else old_git_stdout(repo, args)
            )
            api._schedule_supervisor_restart = (  # type: ignore[assignment]
                lambda clone_arg, cfg_arg: restarts.append(
                    (clone_arg, cfg_arg.config_path)
                ) or {"scheduled": True, "port": 18182, "log_path": "restart.log"}
            )
            os.environ["REFINE_UI_PORT"] = "18182"

            status, body = api.project_attach({
                "path": str(legacy),
                "migrate": True,
                "install_unit": False,
                "start_runner": False,
                "start_poller": False,
            })
        finally:
            runtime.backend_info = old_backend_info  # type: ignore[assignment]
            api._schedule_supervisor_restart = old_schedule_restart  # type: ignore[assignment]
            api._git_stdout = old_git_stdout  # type: ignore[assignment]

        assert status == 200, body
        assert body["restart_pending"] is True
        assert body["schema"]["compatible"] is True
        assert (legacy / ".refine" / "config.json").is_file()
        migrated = json.loads((legacy / ".refine" / "config.json").read_text(encoding="utf-8"))
        assert migrated["settings"]["governance_product"] == "Legacy app"
        assert git(legacy, "status", "--porcelain").stdout.strip() == ""
        assert restarts == [
            (root.resolve(), legacy.resolve() / ".refine" / "refine.toml")
        ]
        assert config.read_binding(binding) == legacy.resolve()
        assert runtime._loaded_config_path == client1 / ".refine" / "refine.toml"  # type: ignore[attr-defined]
    finally:
        try:
            from refine_ui import runtime
            runtime.stop_all()
        except Exception:
            pass
        if prior_binding is None:
            try:
                binding.unlink()
            except FileNotFoundError:
                pass
        else:
            binding.write_text(prior_binding, encoding="utf-8")
        _restore_optional(registry, prior_registry)
        if old_cfg_env is None:
            os.environ.pop("REFINE_CONFIG_PATH", None)
        else:
            os.environ["REFINE_CONFIG_PATH"] = old_cfg_env
        if old_port is None:
            os.environ.pop("REFINE_UI_PORT", None)
        else:
            os.environ["REFINE_UI_PORT"] = old_port
        os.chdir(original_cwd)
        cleanup_tmp(tmp)


def test_empty_project_attach_creates_scaffold_gap(root: Path) -> None:
    tmp = Path(tempfile.mkdtemp(prefix="refine-scaffold-template-"))
    clone = tmp / "refine-source"
    (clone / "refine_cli").mkdir(parents=True)
    (clone / "pyproject.toml").write_text(
        "[project]\nname = \"refine\"\n",
        encoding="utf-8",
    )
    (clone / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
    origin = tmp / "empty-origin.git"
    original_cwd = Path.cwd()
    old_cfg_env = os.environ.get("REFINE_CONFIG_PATH")
    try:
        git(tmp, "init", "--bare", str(origin))
        os.chdir(clone)
        os.environ.pop("REFINE_CONFIG_PATH", None)
        reset_refine_imports()
        from refine_server import db, gaps as shared_gaps
        from refine_ui import api

        status, templates = api.list_project_templates()
        assert status == 200, templates
        template_ids = {t["id"] for t in templates["templates"]}
        assert {"nodejs-webapp", "python-webapp", "blank"} <= template_ids

        status, attached = api.project_attach({
            "path": origin.as_uri(),
            "install_unit": False,
            "start_runner": False,
            "start_poller": False,
        })
        assert status == 200, attached
        assert attached["scaffold_required"] is True
        assert {t["id"] for t in attached["scaffold_templates"]} == template_ids
        cloned = tmp / "empty-origin"
        assert attached["client_repo"] == str(cloned.resolve())
        assert (cloned / ".git").exists()

        status, snap = api.project_status()
        assert status == 200, snap
        assert snap["scaffold_required"] is True

        status, created = api.create_project_scaffold_gap({"template": "nodejs-webapp"})
        assert status == 201, created
        gap = created["gap"]
        assert gap["priority"] == "high"
        assert gap["status"] == "backlog"
        assert gap["name"] == "Scaffold Node.js WebApp"
        latest = gap["rounds"][-1]
        assert latest["reporter"] == "Refine"
        assert "Node.js 24 LTS" in latest["target"]
        assert "Vite" in latest["target"]

        reread = shared_gaps.read_gap_json(gap["id"])
        assert reread["priority"] == "high"
        conn = db.connect()
        try:
            row = conn.execute(
                "SELECT priority, reporter FROM gaps_index WHERE id = ?",
                (gap["id"],),
            ).fetchone()
            assert dict(row) == {"priority": "high", "reporter": "Refine"}
        finally:
            conn.close()

        app = tmp / "existing-app"
        app.mkdir()
        (app / "package.json").write_text('{"scripts":{"dev":"vite"}}\n', encoding="utf-8")
        assert api._project_needs_scaffold_template(app) is False  # type: ignore[attr-defined]
    finally:
        try:
            from refine_ui import runtime
            runtime.stop_all()
        except Exception:
            pass
        os.chdir(original_cwd)
        if old_cfg_env is None:
            os.environ.pop("REFINE_CONFIG_PATH", None)
        else:
            os.environ["REFINE_CONFIG_PATH"] = old_cfg_env
        cleanup_tmp(tmp)


def test_active_instance_is_per_application() -> None:
    tmp, client1 = make_client_repo("refine-active-instance-")
    conn = init_refine(client1)
    conn.close()
    try:
        from refine_server import project_state as ps1

        laptop = ps1.create_instance("Laptop")
        ps1.set_active_instance(laptop["id"])

        client2 = tmp / "client-two"
        client2.mkdir()
        git(client2, "init", "-q")
        git(client2, "config", "user.email", "t@x")
        git(client2, "config", "user.name", "t")
        (client2 / "app.txt").write_text("base\n", encoding="utf-8")
        git(client2, "add", "app.txt")
        git(client2, "commit", "-m", "init")
        conn2 = init_refine(client2)
        conn2.close()
        from refine_server import project_state as ps2
        from refine_ui import runtime

        desktop = ps2.create_instance("Desktop")
        ps2.set_active_instance(desktop["id"])

        runtime.load_configured(
            client1 / ".refine" / "refine.toml",
            start_poller=False,
            start_runner=False,
        )
        from refine_server import project_state
        assert project_state.active_instance_id() == laptop["id"]

        runtime.load_configured(
            client2 / ".refine" / "refine.toml",
            start_poller=False,
            start_runner=False,
        )
        assert project_state.active_instance_id() == desktop["id"]

        runtime.load_configured(
            client1 / ".refine" / "refine.toml",
            start_poller=False,
            start_runner=False,
        )
        assert project_state.active_instance_id() == laptop["id"]
    finally:
        try:
            runtime.stop_all()  # type: ignore[name-defined]
        except Exception:
            pass
        cleanup_tmp(tmp)


def test_active_instance_is_checkout_local_for_same_application() -> None:
    tmp, client = make_client_repo("refine-active-instance-local-")
    conn = init_refine(client)
    conn.close()
    original_cwd = Path.cwd()
    try:
        from refine_server import config, project_state

        laptop = project_state.create_instance("Laptop")
        desktop = project_state.create_instance("Desktop")
        clone1 = tmp / "refine-one"
        clone2 = tmp / "refine-two"
        clone1.mkdir()
        clone2.mkdir()
        config.write_binding(clone1, client)
        config.write_binding(clone2, client)
        legacy_active = client / ".refine" / "run" / "active-instance.json"
        legacy_active.parent.mkdir(parents=True, exist_ok=True)
        legacy_active.write_text(
            json.dumps({"active_instance_id": laptop["id"]}),
            encoding="utf-8",
        )

        os.chdir(clone1)
        config.get(reload=True)
        assert project_state.active_instance_id() == laptop["id"]
        assert not legacy_active.exists()

        os.chdir(clone2)
        config.get(reload=True)
        project_state.set_active_instance(desktop["id"])
        assert project_state.active_instance_id() == desktop["id"]

        os.chdir(clone1)
        config.get(reload=True)
        assert project_state.active_instance_id() == laptop["id"]

        os.chdir(clone2)
        config.get(reload=True)
        assert project_state.active_instance_id() == desktop["id"]

        assert (clone1 / "run" / "active-instances.json").is_file()
        assert (clone2 / "run" / "active-instances.json").is_file()
        assert not (client / ".refine" / "run" / "active-instance.json").exists()
    finally:
        os.chdir(original_cwd)
        cleanup_tmp(tmp)


def test_active_instance_is_port_scoped_for_same_checkout() -> None:
    tmp, client = make_client_repo("refine-active-instance-port-")
    conn = init_refine(client)
    conn.close()
    original_cwd = Path.cwd()
    old_scope = os.environ.get("REFINE_UI_SCOPE")
    old_port = os.environ.get("REFINE_UI_PORT")
    old_cfg = os.environ.get("REFINE_CONFIG_PATH")
    try:
        from refine_server import config, project_state

        laptop = project_state.create_instance("Laptop")
        desktop = project_state.create_instance("Desktop")
        clone = tmp / "refine-one"
        clone.mkdir()
        config.write_binding(clone, client)

        os.environ.pop("REFINE_CONFIG_PATH", None)
        os.environ.pop("REFINE_UI_PORT", None)
        os.chdir(clone)

        os.environ["REFINE_UI_SCOPE"] = "8080"
        cfg8080 = config.get(reload=True)
        project_state.set_active_instance(laptop["id"])
        assert project_state.active_instance_id() == laptop["id"]
        sqlite8080 = cfg8080.sqlite_path

        os.environ["REFINE_UI_SCOPE"] = "8081"
        cfg8081 = config.get(reload=True)
        project_state.set_active_instance(desktop["id"])
        assert project_state.active_instance_id() == desktop["id"]
        sqlite8081 = cfg8081.sqlite_path

        os.environ["REFINE_UI_SCOPE"] = "8080"
        config.get(reload=True)
        assert project_state.active_instance_id() == laptop["id"]

        os.environ["REFINE_UI_SCOPE"] = "8081"
        config.get(reload=True)
        assert project_state.active_instance_id() == desktop["id"]

        assert sqlite8080 != sqlite8081
        assert sqlite8080.parent == clone / "run" / "cache"
        assert sqlite8081.parent == clone / "run" / "cache"
    finally:
        if old_scope is None:
            os.environ.pop("REFINE_UI_SCOPE", None)
        else:
            os.environ["REFINE_UI_SCOPE"] = old_scope
        if old_port is None:
            os.environ.pop("REFINE_UI_PORT", None)
        else:
            os.environ["REFINE_UI_PORT"] = old_port
        if old_cfg is None:
            os.environ.pop("REFINE_CONFIG_PATH", None)
        else:
            os.environ["REFINE_CONFIG_PATH"] = old_cfg
        os.chdir(original_cwd)
        cleanup_tmp(tmp)


def test_process_config_path_is_not_shared_through_binding() -> None:
    tmp, client1 = make_client_repo("refine-config-scope-")
    conn = init_refine(client1)
    conn.close()
    original_cwd = Path.cwd()
    old_cfg = os.environ.get("REFINE_CONFIG_PATH")
    try:
        from refine_server import config

        client2 = tmp / "client-two"
        client2.mkdir()
        git(client2, "init", "-q")
        git(client2, "config", "user.email", "t@x")
        git(client2, "config", "user.name", "t")
        (client2 / "app.txt").write_text("base\n", encoding="utf-8")
        git(client2, "add", "app.txt")
        git(client2, "commit", "-m", "init")
        conn2 = init_refine(client2)
        conn2.close()

        clone = tmp / "refine-one"
        clone.mkdir()
        config.write_binding(clone, client2)
        os.chdir(clone)

        os.environ["REFINE_CONFIG_PATH"] = str(client1 / ".refine" / "refine.toml")
        assert config.get(reload=True).client_repo == client1
        assert config.find_config() == client1 / ".refine" / "refine.toml"

        os.environ["REFINE_CONFIG_PATH"] = str(client2 / ".refine" / "refine.toml")
        assert config.get(reload=True).client_repo == client2
    finally:
        if old_cfg is None:
            os.environ.pop("REFINE_CONFIG_PATH", None)
        else:
            os.environ["REFINE_CONFIG_PATH"] = old_cfg
        os.chdir(original_cwd)
        cleanup_tmp(tmp)


def test_instance_switch_refreshes_reporter_cache() -> None:
    tmp, client = make_client_repo("refine-instance-reporters-")
    conn = init_refine(client)
    try:
        from refine_server import project_state, reporters
        from refine_ui import api

        reporters.add(conn, "Alice")
        other = project_state.create_instance("refine2")

        # Simulate another Refine process changing the checkout-local active
        # instance marker without touching this process's SQLite connection.
        project_state.set_active_instance(other["id"])
        status, body = api.list_reporters()
        assert status == 200, body
        assert body["reporters"] == []

        status, body = api.create_reporter({"name": "Bob"})
        assert status == 201, body
        assert body["reporter"]["name"] == "Bob"

        project_state.set_active_instance(project_state.DEFAULT_INSTANCE_ID)
        status, body = api.list_reporters()
        assert status == 200, body
        names = [r["name"] for r in body["reporters"]]
        assert names == ["Alice"]

        status, body = api.list_settings()
        assert status == 200, body
        assert project_state.CACHE_ACTIVE_INSTANCE_KEY not in body["settings"]
    finally:
        conn.close()
        cleanup_tmp(tmp)


def test_settings_are_scoped_to_active_instance_files() -> None:
    tmp, client = make_client_repo("refine-instance-settings-")
    conn = init_refine(client)
    try:
        from refine_server import db, project_state
        from refine_ui import api

        default = project_state.active_instance_id()
        other = project_state.create_instance("refine2")
        db.set_setting(conn, "governance_product", "Shared product")

        status, body = api.update_settings({
            "agent_subpath": "frontend",
            "project_update_pulse_interval_seconds": "300",
            "agent_limit_pause_seconds": "3600",
            "file_browser_ignore_patterns": "node_modules, .git, .refine",
            "target_app_auto_rebuild": "hourly",
        })
        assert status == 200, body

        project_state.set_active_instance(other["id"])
        status, body = api.list_settings()
        assert status == 200, body
        settings = body["settings"]
        assert settings["governance_product"] == "Shared product"
        assert settings["agent_subpath"] != "frontend"
        assert settings["project_update_pulse_interval_seconds"] != "300"
        assert settings["agent_limit_pause_seconds"] != "3600"
        assert settings["file_browser_ignore_patterns"] == "node_modules, .git, .refine"
        assert settings["target_app_auto_rebuild"] != "hourly"

        status, body = api.update_settings({
            "agent_subpath": "backend",
            "project_update_pulse_interval_seconds": "900",
            "agent_limit_pause_seconds": "10800",
            "file_browser_ignore_patterns": "vendor_cache",
            "target_app_auto_rebuild": "nightly",
            "target_app_url": "http://localhost:3001",
            "target_app_start_command": "npm run dev",
            "target_app_stop_command": "pkill -f 'npm run dev' || true",
            "target_app_rebuild_command": "npm run build",
            "target_app_status_command": "pgrep -f 'npm run dev'",
            "target_app_cwd": "apps/web",
            "target_app_env_json": '{"PORT":"3001"}',
            "target_app_process_check_command": "pgrep -f node",
        })
        assert status == 200, body
        status, body = api.list_settings()
        assert status == 200, body
        settings = body["settings"]
        assert settings["target_app_start_command"] == "npm run dev"
        assert settings["target_app_stop_command"] == "pkill -f 'npm run dev' || true"
        assert settings["target_app_rebuild_command"] == "npm run build"
        assert settings["target_app_status_command"] == "pgrep -f 'npm run dev'"
        assert settings["target_app_cwd"] == "apps/web"
        assert settings["target_app_env_json"] == '{"PORT": "3001"}'
        assert settings["target_app_process_check_command"] == "pgrep -f node"

        project_state.set_active_instance(default)
        status, body = api.list_settings()
        assert status == 200, body
        settings = body["settings"]
        assert settings["governance_product"] == "Shared product"
        assert settings["agent_subpath"] == "frontend"
        assert settings["project_update_pulse_interval_seconds"] == "300"
        assert settings["agent_limit_pause_seconds"] == "3600"
        assert settings["file_browser_ignore_patterns"] == "node_modules, .git, .refine"
        assert settings["target_app_auto_rebuild"] == "hourly"
        assert settings["target_app_url"] != "http://localhost:3001"
        assert settings["target_app_start_command"] != "npm run dev"
        assert settings["target_app_rebuild_command"] != "npm run build"

        root = client / ".refine"
        project_config = json.loads((root / "config.json").read_text(encoding="utf-8"))
        default_app = json.loads(
            (root / "instances" / default / "application.json").read_text(encoding="utf-8")
        )
        default_runtime = json.loads(
            (root / "instances" / default / "runtime.json").read_text(encoding="utf-8")
        )
        default_target = json.loads(
            (root / "instances" / default / "target-app.json").read_text(encoding="utf-8")
        )
        other_app = json.loads(
            (root / "instances" / other["id"] / "application.json").read_text(encoding="utf-8")
        )
        other_runtime = json.loads(
            (root / "instances" / other["id"] / "runtime.json").read_text(encoding="utf-8")
        )
        other_target = json.loads(
            (root / "instances" / other["id"] / "target-app.json").read_text(encoding="utf-8")
        )

        assert project_config["settings"]["governance_product"] == "Shared product"
        assert default_app["agent_subpath"] == "frontend"
        assert default_runtime["project_update_pulse_interval_seconds"] == "300"
        assert default_runtime["agent_limit_pause_seconds"] == "3600"
        assert default_runtime["file_browser_ignore_patterns"] == "node_modules, .git, .refine"
        assert default_target["target_app_auto_rebuild"] == "hourly"
        assert other_app["agent_subpath"] == "backend"
        assert other_runtime["project_update_pulse_interval_seconds"] == "900"
        assert other_runtime["agent_limit_pause_seconds"] == "10800"
        assert other_runtime["file_browser_ignore_patterns"] == "vendor_cache"
        assert other_target["target_app_auto_rebuild"] == "nightly"
        assert other_target["target_app_url"] == "http://localhost:3001"
        assert other_target["target_app_start_command"] == "npm run dev"
        assert other_target["target_app_stop_command"] == "pkill -f 'npm run dev' || true"
        assert other_target["target_app_rebuild_command"] == "npm run build"
        assert other_target["target_app_status_command"] == "pgrep -f 'npm run dev'"
        assert other_target["target_app_cwd"] == "apps/web"
        assert other_target["target_app_env_json"] == '{"PORT": "3001"}'
        assert other_target["target_app_process_check_command"] == "pgrep -f node"
    finally:
        conn.close()
        cleanup_tmp(tmp)


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    test_client_switch_path(root)
    test_runtime_switch_resets_services()
    test_blocked_switch_does_not_stop_current_app(root)
    test_supervised_switch_schedules_restart_without_hot_loading(root)
    test_supervised_initial_attach_schedules_restart(root)
    test_supervised_switch_migrates_target_before_restart(root)
    test_empty_project_attach_creates_scaffold_gap(root)
    test_active_instance_is_per_application()
    test_active_instance_is_checkout_local_for_same_application()
    test_active_instance_is_port_scoped_for_same_checkout()
    test_process_config_path_is_not_shared_through_binding()
    test_instance_switch_refreshes_reporter_cache()
    test_settings_are_scoped_to_active_instance_files()
    print("project switch state tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
