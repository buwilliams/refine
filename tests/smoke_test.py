"""End-to-end smoke test (no real agent CLI / git remote required).

Validates:
- `refine init`-equivalent config bootstrap
- DB init + schema is creatable
- ULID generation
- Reporter add/list
- Gap creation through gap_writer + SQLite index
- Round append + log append
- Friendly classification of various outcomes
- Activity feed read/write
- Runner instantiation against the configured paths
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="refine-smoke-"))
    print(f"using tmp dir: {tmp}")
    # Make a fake "client repo" and `refine init` a volume root inside it.
    client = tmp / "client"
    client.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=client, check=True)
    subprocess.run(["git", "-c", "user.email=t@x", "-c", "user.name=t",
                    "commit", "--allow-empty", "-m", "init"], cwd=client, check=True)

    # Drop into the client repo so config discovery works.
    os.chdir(client)

    # Re-import in this clean environment.
    for mod in list(sys.modules):
        if mod.startswith("refine"):
            del sys.modules[mod]

    from refine_server import config

    # Equivalent of `refine init`
    cfg_path = config.write_defaults(client / ".refine")
    print(f"wrote config: {cfg_path}")
    cfg = config.get()
    print(f"volume root:  {cfg.volume_root}")
    print(f"client repo:  {cfg.client_repo}")

    from refine_server import db, reporters, activity, gaps as shared_gaps
    from refine_server.ulid import new_ulid, is_ulid
    from refine_server.friendly import classify_subprocess_failure, classify_git_failure
    from refine_server import agent_cli, gap_writer

    # --- DB ------------------------------------------------------------------
    db.init_db()
    conn = db.connect()
    settings = db.list_settings(conn)
    assert settings["parallel_run_cap"] == "3"
    assert settings["agent_idle_timeout_seconds"] == "900"
    assert settings["agent_hard_cap_seconds"] == "86400"
    assert settings["agent_limit_pause_seconds"] == "60"
    print("[ok] DB init + defaults seeded")

    # --- Agent CLI abstraction ---------------------------------------------
    codex = agent_cli.get_spec("codex")
    cargs = codex.agent_args("/bin/codex", "do it", cwd=client)
    assert cargs[:2] == ["/bin/codex", "exec"]
    assert "--dangerously-bypass-approvals-and-sandbox" in cargs
    assert "--ask-for-approval" not in cargs
    assert "--sandbox" not in cargs
    assert "--json" in cargs and "-C" in cargs
    assert "--full-auto" not in cargs
    assert codex.chat_args("/bin/codex", "hi", session_id="abc")[:3] == [
        "/bin/codex", "exec", "resume",
    ]
    from refine_server.llm import _extract_final_text
    from refine_server.subprocess_mgr import _summarize_codex_event
    codex_jsonl = (
        '{"type":"item.completed","item":{"type":"agent_message",'
        '"text":"[{\\\"name\\\":\\\"N\\\",\\\"actual\\\":\\\"A\\\",'
        '\\"target\\\":\\\"T\\\"}]"}}\n'
    )
    assert _extract_final_text(codex_jsonl).startswith("[")
    assert _summarize_codex_event({
        "type": "item.completed",
        "item": {"type": "agent_message", "text": "done"},
    }) == ["done"]
    from refine_server.chat_mgr import _chat_env
    old_openai_key = os.environ.get("OPENAI_API_KEY")
    old_openai_base = os.environ.get("OPENAI_BASE_URL")
    old_codex_ci = os.environ.get("CODEX_CI")
    old_codex_thread = os.environ.get("CODEX_THREAD_ID")
    try:
        os.environ["OPENAI_API_KEY"] = "sk-test-should-not-leak"
        os.environ["OPENAI_BASE_URL"] = "https://example.invalid/v1"
        os.environ["CODEX_CI"] = "1"
        os.environ["CODEX_THREAD_ID"] = "test-thread"
        chat_env = _chat_env()
        assert "OPENAI_API_KEY" not in chat_env
        assert "OPENAI_BASE_URL" not in chat_env
        assert "CODEX_CI" not in chat_env
        assert "CODEX_THREAD_ID" not in chat_env
    finally:
        if old_openai_key is None:
            os.environ.pop("OPENAI_API_KEY", None)
        else:
            os.environ["OPENAI_API_KEY"] = old_openai_key
        if old_openai_base is None:
            os.environ.pop("OPENAI_BASE_URL", None)
        else:
            os.environ["OPENAI_BASE_URL"] = old_openai_base
        if old_codex_ci is None:
            os.environ.pop("CODEX_CI", None)
        else:
            os.environ["CODEX_CI"] = old_codex_ci
        if old_codex_thread is None:
            os.environ.pop("CODEX_THREAD_ID", None)
        else:
            os.environ["CODEX_THREAD_ID"] = old_codex_thread
    print("[ok] codex CLI args + JSONL parsing")

    # --- Target-app command/check runtime -----------------------------------
    from refine_server import target_app
    tcfg = {
        "start_command": "true",
        "stop_command": "true",
        "rebuild_command": "true",
        "status_command": "true",
        "cwd": "",
        "env": {},
        "start_timeout_seconds": 5,
        "stop_timeout_seconds": 5,
        "rebuild_timeout_seconds": 5,
        "status_timeout_seconds": 5,
    }
    tres = target_app.run_operation("start", tcfg)
    assert tres["ok"] and tres["state"] == "running", tres
    sres = target_app.run_operation("status", tcfg)
    assert sres["ok"] and sres["state"] == "running", sres
    rres = target_app.run_operation("rebuild", tcfg)
    assert rres["ok"] and rres["state"] == "running", rres
    stop_cfg = {**tcfg, "status_command": "false"}
    xres = target_app.run_operation("stop", stop_cfg)
    assert xres["ok"] and xres["state"] == "stopped", xres
    gen = target_app.normalize_generated_config({
        "start_command": "npm run dev\n",
        "stop_command": "pkill -f dev || true",
        "rebuild_command": "npm run build",
        "status_command": "pgrep -f dev",
        "env": {"PORT": 3000},
    })
    assert gen["start_command"] == "npm run dev"
    assert gen["rebuild_command"] == "npm run build"
    assert gen["env"]["PORT"] == "3000"
    ready_file = client / ".refine" / "ready"
    delayed_cfg = {
        "start_command": (
            "mkdir -p .refine; "
            "sh -c 'sleep 1; touch .refine/ready' >/dev/null 2>&1 &"
        ),
        "stop_command": "rm -f .refine/ready",
        "status_command": "test -f .refine/ready",
        "cwd": "",
        "env": {},
        "start_timeout_seconds": 5,
        "stop_timeout_seconds": 5,
        "status_timeout_seconds": 1,
    }
    t0 = time.monotonic()
    delayed = target_app.run_operation("start", delayed_cfg)
    assert delayed["ok"] and delayed["state"] == "running", delayed
    assert time.monotonic() - t0 >= 1.0
    assert ready_file.exists()
    stopped = target_app.run_operation("stop", delayed_cfg)
    assert stopped["ok"] and stopped["state"] == "stopped", stopped
    print("[ok] target-app command runtime + config normalization")

    # --- UI project bootstrap helper ----------------------------------------
    from refine_cli import cli as refine_cli
    from refine_cli.cli import bootstrap_client_repo, _sync_bound_project_registry
    clone = tmp / "refine-clone"
    (clone / "refine_cli").mkdir(parents=True)
    (clone / "pyproject.toml").write_text("[project]\nname = \"refine\"\n", encoding="utf-8")
    (clone / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
    (clone / ".env").write_text("REFINE_CLIENT_REFINE_DIR=/old/path\n", encoding="utf-8")
    (clone / ".refine-current").symlink_to(tmp / "old-refine-data", target_is_directory=True)
    ui_client = tmp / "ui-created-client"
    boot = bootstrap_client_repo(
        ui_client,
        clone_dir=clone,
        force=True,
        create=True,
        init_git=True,
        reuse_existing_config=True,
        install_unit=False,
    )
    assert ui_client.is_dir()
    assert (ui_client / ".git").exists()
    assert (ui_client / ".refine" / "refine.toml").is_file()
    assert not (ui_client / ".refine" / "run").exists()
    assert (ui_client / ".refine" / "gaps").is_dir()
    assert (clone / ".refine-binding").read_text(encoding="utf-8").strip().endswith(str(ui_client))
    assert str(ui_client) in (clone / ".refine-apps.json").read_text(encoding="utf-8")
    assert "/run/" in (Path(__file__).resolve().parents[1] / ".gitignore").read_text(encoding="utf-8")
    assert "run/" not in (ui_client / ".refine" / ".gitignore").read_text(encoding="utf-8").splitlines()
    assert not (clone / ".env").exists()
    assert not (clone / ".refine-current").exists()
    assert boot["git_initialized"] is True
    assert boot["config_created"] is True
    print("[ok] UI project bootstrap creates git repo + host-native refine binding")

    unit_clone = tmp / "refine-unit-clone"
    (unit_clone / "refine_cli").mkdir(parents=True)
    (unit_clone / "pyproject.toml").write_text("[project]\nname = \"refine\"\n", encoding="utf-8")
    (unit_clone / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
    unit_client = tmp / "unit-client"
    fake_uv_bin = tmp / "login-bin"
    fake_uv_bin.mkdir()
    fake_uv = fake_uv_bin / "uv"
    fake_uv.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    fake_uv.chmod(0o755)
    old_systemd_dir = refine_cli.SYSTEMD_USER_DIR
    old_systemctl = refine_cli._systemctl
    old_which = refine_cli.shutil.which
    old_login_path = refine_cli._user_login_path
    systemctl_calls: list[tuple[str, ...]] = []

    def fake_systemctl(*args: str) -> tuple[int, str]:
        systemctl_calls.append(args)
        return 0, ""

    def fake_which(name: str, path: str | None = None) -> str | None:
        if name == "uv" and path == str(fake_uv_bin):
            return str(fake_uv)
        if name == "uv":
            return None
        return old_which(name, path=path)

    try:
        refine_cli.SYSTEMD_USER_DIR = tmp / "systemd-user"
        refine_cli._systemctl = fake_systemctl
        refine_cli.shutil.which = fake_which
        refine_cli._user_login_path = lambda: str(fake_uv_bin)
        unit_boot = bootstrap_client_repo(
            unit_client,
            clone_dir=unit_clone,
            force=True,
            create=True,
            init_git=True,
            reuse_existing_config=True,
            install_unit=True,
        )
    finally:
        refine_cli.SYSTEMD_USER_DIR = old_systemd_dir
        refine_cli._systemctl = old_systemctl
        refine_cli.shutil.which = old_which
        refine_cli._user_login_path = old_login_path
    ui_unit = Path(unit_boot["ui_unit_path"])
    assert ui_unit.is_file()
    assert unit_boot.get("unit_path") is None
    unit_text = ui_unit.read_text(encoding="utf-8")
    assert ui_unit.name == "refine-unit-clone-8080-ui.service"
    assert "Environment=REFINE_UI_PORT=8080" in unit_text
    assert f"ExecStart={fake_uv} run refine ui" in unit_text
    assert "Restart=on-failure" in unit_text
    assert "docker" not in unit_text.lower()
    assert ("enable", "refine-unit-clone-8080-ui") in systemctl_calls
    print("[ok] refine install writes per-port host-native UI backend systemd unit")

    unit_name = "refine-unit-clone"
    unit_cfg_path = unit_client / ".refine" / "refine.toml"
    unit_cfg = config.Config.load(unit_cfg_path)
    nondefault_unit = ui_unit.with_name("refine-unit-clone-18124-ui.service")
    ui_unit.rename(nondefault_unit)
    nondefault_unit.write_text(unit_text.replace("8080", "18124"), encoding="utf-8")
    old_systemd_dir = refine_cli.SYSTEMD_USER_DIR
    old_systemctl = refine_cli._systemctl
    old_wait_for_port = refine_cli._wait_for_port
    old_print_status_block = refine_cli._print_status_block
    old_start_background = refine_cli._start_background_ui
    old_stop_background = refine_cli._stop_background_ui
    old_cwd = Path.cwd()
    lifecycle_calls: list[tuple[str, ...]] = []

    def fail_start_background(*_args, **_kwargs):  # noqa: ANN002, ANN003
        raise AssertionError("installed systemd unit should handle refine start")

    def fail_stop_background(*_args, **_kwargs):  # noqa: ANN002, ANN003
        raise AssertionError("installed systemd unit should handle refine stop")

    try:
        os.chdir(unit_clone)
        refine_cli.SYSTEMD_USER_DIR = tmp / "systemd-user"
        refine_cli._systemctl = fake_systemctl
        refine_cli._wait_for_port = lambda host, port, timeout: True
        refine_cli._print_status_block = lambda clone_arg, unit_arg, cfg_arg, *, port: (
            lifecycle_calls.append(("status", unit_arg, str(port)))
        )
        refine_cli._start_background_ui = fail_start_background
        refine_cli._stop_background_ui = fail_stop_background
        no_port_args = type("Args", (), {"port": None, "config": str(unit_cfg_path)})()

        assert refine_cli._runtime_action_port(no_port_args, unit_clone, unit_cfg, unit_name) == 18124
        assert refine_cli.cmd_start(no_port_args) == 0
        assert refine_cli.cmd_stop(no_port_args) == 0
        assert refine_cli.cmd_restart(no_port_args) == 0
    finally:
        os.chdir(old_cwd)
        refine_cli.SYSTEMD_USER_DIR = old_systemd_dir
        refine_cli._systemctl = old_systemctl
        refine_cli._wait_for_port = old_wait_for_port
        refine_cli._print_status_block = old_print_status_block
        refine_cli._start_background_ui = old_start_background
        refine_cli._stop_background_ui = old_stop_background
    assert ("start", "refine-unit-clone-18124-ui") in systemctl_calls
    assert ("stop", "refine-unit-clone-18124-ui") in systemctl_calls
    assert ("restart", "refine-unit-clone-18124-ui") in systemctl_calls
    assert ("status", "refine-unit-clone", "18124") in lifecycle_calls
    print("[ok] refine start/stop/restart route installed UI units through systemd")

    bg_cfg = config.Config.load(ui_client / ".refine" / "refine.toml")
    old_find_host_command = refine_cli._find_host_command
    old_popen = refine_cli.subprocess.Popen
    old_listener_pids = refine_cli._listener_pids
    popen_calls: list[dict] = []

    class FakePopen:
        pid = 43210

        def __init__(self, cmd, **kwargs):  # noqa: ANN001
            popen_calls.append({"cmd": cmd, **kwargs})

    try:
        refine_cli._find_host_command = lambda name: str(fake_uv) if name == "uv" else None
        refine_cli._listener_pids = lambda port: []
        refine_cli.subprocess.Popen = FakePopen
        pid = refine_cli._start_background_ui(clone, bg_cfg, host=bg_cfg.web_host, port=18111)
    finally:
        refine_cli._find_host_command = old_find_host_command
        refine_cli._listener_pids = old_listener_pids
        refine_cli.subprocess.Popen = old_popen
    assert pid == 43210
    assert (clone / "run" / "ui-18111.pid").read_text(encoding="utf-8").strip() == "43210"
    assert not (bg_cfg.volume_root / "run" / "ui-18111.pid").exists()
    assert popen_calls[0]["cmd"] == [str(fake_uv), "run", "refine", "ui"]
    assert popen_calls[0]["cwd"] == str(clone)
    assert popen_calls[0]["env"]["REFINE_UI_PORT"] == "18111"
    try:
        refine_cli._effective_port(type("Args", (), {"port": 0})(), bg_cfg)
        raise AssertionError("port 0 should be rejected")
    except SystemExit as e:
        assert "invalid port 0" in str(e)
    print("[ok] refine start launches a detached per-port UI backend process")

    old_listener_pids = refine_cli._listener_pids
    old_listener_port_pids = refine_cli._listener_port_pids
    old_pid_cmdline = refine_cli._pid_cmdline
    old_pid_cwd = refine_cli._pid_cwd
    old_pid_env_value = refine_cli._pid_env_value
    old_pid_alive = refine_cli._pid_alive
    old_getpgid = refine_cli.os.getpgid
    old_killpg = refine_cli.os.killpg
    listener_alive = {"value": True}
    killed: list[tuple[int, int]] = []
    try:
        refine_cli._listener_pids = lambda port: [24680] if port == 18112 else []
        refine_cli._listener_port_pids = lambda: [(24680, 18112)]
        refine_cli._pid_cmdline = lambda pid: "/tmp/refine2/.venv/bin/refine ui" if pid == 24680 else ""
        refine_cli._pid_cwd = lambda pid: clone if pid == 24680 else None
        refine_cli._pid_env_value = lambda pid, key: "18112" if pid == 24680 and key == "REFINE_UI_PORT" else None
        refine_cli._pid_alive = lambda pid: listener_alive["value"] if pid == 24680 else False
        refine_cli.os.getpgid = lambda pid: 24000 if pid == 24680 else pid

        def fake_killpg(pgid: int, sig: int) -> None:
            killed.append((pgid, sig))
            listener_alive["value"] = False

        refine_cli.os.killpg = fake_killpg
        assert refine_cli._running_pid(clone, bg_cfg, 18112) == 24680
        other_clone = tmp / "other-refine-clone"
        other_clone.mkdir()
        assert refine_cli._running_pid(other_clone, bg_cfg, 18112) is None
        no_port_args = type("Args", (), {"port": None})()
        assert refine_cli._runtime_action_port(no_port_args, clone, bg_cfg) == 18112
        assert refine_cli._stop_background_ui(clone, bg_cfg, 18112) is True
    finally:
        refine_cli._listener_pids = old_listener_pids
        refine_cli._listener_port_pids = old_listener_port_pids
        refine_cli._pid_cmdline = old_pid_cmdline
        refine_cli._pid_cwd = old_pid_cwd
        refine_cli._pid_env_value = old_pid_env_value
        refine_cli._pid_alive = old_pid_alive
        refine_cli.os.getpgid = old_getpgid
        refine_cli.os.killpg = old_killpg
    assert killed == [(24000, refine_cli.signal.SIGTERM)]
    print("[ok] refine stop recovers a missing pid file from the listening UI backend")

    (clone / "run" / "ui-18120.pid").write_text("111\n", encoding="utf-8")
    (clone / "run" / "ui-18121.pid").write_text("222\n", encoding="utf-8")
    old_owned_ports = refine_cli._owned_refine_ui_ports
    try:
        refine_cli._owned_refine_ui_ports = lambda clone_arg: [18122] if clone_arg == clone else []
        assert refine_cli._status_ports(type("Args", (), {"port": None})(), clone, bg_cfg) == [
            18111, 18120, 18121, 18122,
        ]
        assert refine_cli._status_ports(type("Args", (), {"port": 18123})(), clone, bg_cfg) == [18123]
    finally:
        refine_cli._owned_refine_ui_ports = old_owned_ports
        for p in (clone / "run").glob("ui-1812*.pid"):
            p.unlink(missing_ok=True)
    print("[ok] refine status lists every checkout-local UI backend")

    setup_clone = tmp / "setup-refine-clone"
    (setup_clone / "refine_cli").mkdir(parents=True)
    (setup_clone / "pyproject.toml").write_text("[project]\nname = \"refine\"\n", encoding="utf-8")
    (setup_clone / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
    old_cwd = Path.cwd()
    old_start_background = refine_cli._start_background_ui
    old_stop_background = refine_cli._stop_background_ui
    old_print_setup_status = refine_cli._print_setup_status_block
    old_port_open = refine_cli._port_open
    old_wait_for_port = refine_cli._wait_for_port
    setup_calls: list[tuple] = []
    try:
        os.chdir(setup_clone)
        refine_cli._start_background_ui = lambda clone_arg, cfg_arg, *, host, port: (
            setup_calls.append(("start", clone_arg, cfg_arg, host, port)) or 555
        )
        refine_cli._stop_background_ui = lambda clone_arg, cfg_arg, port_arg: (
            setup_calls.append(("stop", clone_arg, cfg_arg, port_arg)) or True
        )
        refine_cli._print_setup_status_block = lambda clone_arg, *, port: (
            setup_calls.append(("status", clone_arg, port))
        )
        refine_cli._port_open = lambda host, port: False
        refine_cli._wait_for_port = lambda host, port, timeout: (
            setup_calls.append(("wait", host, port)) or True
        )
        assert refine_cli.cmd_start(type("Args", (), {"port": 19000})()) == 0
        assert refine_cli.cmd_stop(type("Args", (), {"port": 19001})()) == 0
        assert refine_cli.cmd_status(type("Args", (), {"config": None, "port": 19002})()) == 0
        (setup_clone / "run").mkdir(exist_ok=True)
        (setup_clone / "run" / "ui-19003.pid").write_text("333\n", encoding="utf-8")
        (setup_clone / "run" / "ui-19004.pid").write_text("444\n", encoding="utf-8")
        assert refine_cli.cmd_status(type("Args", (), {"config": None, "port": None})()) == 0
    finally:
        os.chdir(old_cwd)
        refine_cli._start_background_ui = old_start_background
        refine_cli._stop_background_ui = old_stop_background
        refine_cli._print_setup_status_block = old_print_setup_status
        refine_cli._port_open = old_port_open
        refine_cli._wait_for_port = old_wait_for_port
    assert setup_calls == [
        ("start", setup_clone.resolve(), None, "0.0.0.0", 19000),
        ("wait", "0.0.0.0", 19000),
        ("stop", setup_clone.resolve(), None, 19001),
        ("status", setup_clone.resolve(), 19002),
        ("status", setup_clone.resolve(), 19003),
        ("status", setup_clone.resolve(), 19004),
    ]
    print("[ok] setup-mode start/stop/status honor supplied ports before project attach")

    old_clone = tmp / "old-refine-clone"
    (old_clone / "refine_cli").mkdir(parents=True)
    (old_clone / "pyproject.toml").write_text("[project]\nname = \"refine\"\n", encoding="utf-8")
    (old_clone / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
    old_client = tmp / "old-single-app-client"
    old_client.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=old_client, check=True)
    old_cfg_path = config.write_defaults(old_client / ".refine")
    config.write_binding(old_clone, old_client)
    assert not (old_clone / ".refine-apps.json").exists()
    _sync_bound_project_registry(old_clone, config.Config.load(old_cfg_path))
    assert str(old_client) in (old_clone / ".refine-apps.json").read_text(encoding="utf-8")
    print("[ok] old single-app binding migrates into known-app registry")

    # --- Reporters -----------------------------------------------------------
    jane = reporters.add(conn, "Jane Doe")
    again = reporters.add(conn, "Jane Doe")  # idempotent
    assert jane["name"] == "Jane Doe"
    assert again["id"] == jane["id"]
    print("[ok] reporters: add + dedupe")

    # --- ULID ----------------------------------------------------------------
    uid = new_ulid()
    assert is_ulid(uid) and len(uid) == 26
    print(f"[ok] ulid: {uid}")

    # --- Gap creation + JSON write ------------------------------------------
    round_obj = shared_gaps.new_round(
        reporter="Jane Doe",
        actual="Button is red.",
        target="Button should be blue.",
    )
    gap = gap_writer.create_gap(gap_id=uid, name="Recolor button", initial_round=round_obj)
    assert gap["id"] == uid
    assert len(gap["rounds"]) == 1
    assert gap["rounds"][0]["reporter"] == "Jane Doe"

    from refine_server.paths import relative_gap_path
    with db.transaction(conn):
        conn.execute(
            "INSERT INTO gaps_index (id, name, status, created, updated, json_path) "
            "VALUES (?, ?, 'todo', ?, ?, ?)",
            (uid, "Recolor button", gap["created"], gap["updated"],
             relative_gap_path(uid)),
        )

    g2 = shared_gaps.read_gap_json(uid)
    assert g2 is not None and g2["name"] == "Recolor button"
    print("[ok] gap created + persisted")

    # --- Append round + edit + log ------------------------------------------
    r2 = shared_gaps.new_round(reporter="Jane Doe", actual="It's purple now.",
                               target="Make it actually blue.")
    gap_writer.append_round(uid, r2)
    g3 = shared_gaps.read_gap_json(uid)
    assert len(g3["rounds"]) == 2

    gap_writer.edit_latest_round(uid, actual="It's purple, not blue.")
    g4 = shared_gaps.read_gap_json(uid)
    assert g4["rounds"][-1]["actual"] == "It's purple, not blue."

    gap_writer.append_round_log(
        gap_id=uid, round_idx=1,
        message="agent started",
        severity="info", category="cli",
    )
    g5 = shared_gaps.read_gap_json(uid)
    assert len(g5["rounds"][1]["logs"]) == 1
    print("[ok] round append + edit + log append")

    # --- Friendly summaries --------------------------------------------------
    idle = classify_subprocess_failure(killed_reason="idle")
    assert "stuck" in idle.message.lower() or "no output" in idle.message.lower()
    auth = classify_subprocess_failure(stderr="invalid api key 401")
    assert auth.category == "auth"
    nopush = classify_git_failure("non-fast-forward; updates were rejected", op="push")
    assert nopush.message.startswith("Push rejected")
    print("[ok] friendly summaries classify correctly")

    # --- Activity feed -------------------------------------------------------
    activity.append(conn, message="Gap created: Recolor button",
                    severity="info", category="state", gap_id=uid, actor="Jane Doe")
    activity.append(conn, message="Agent run started",
                    severity="info", category="cli", gap_id=uid, actor="runner")
    feed = activity.recent(conn, limit=10)
    assert len(feed) >= 2
    print(f"[ok] activity feed has {len(feed)} entries")

    # --- Friendly outcome ----------------------------------------------------
    from refine_server.friendly_outcome import classify_outcome
    assert classify_outcome(exit_code=0, killed_reason=None, no_new_commits=False).kind == "success"
    assert classify_outcome(exit_code=0, killed_reason=None, no_new_commits=True).kind == "failure"
    assert classify_outcome(exit_code=0, killed_reason="idle", no_new_commits=False).kind == "failure"
    assert classify_outcome(exit_code=0, killed_reason="hard_cap", no_new_commits=False).kind == "failure"
    assert classify_outcome(exit_code=2, killed_reason=None, no_new_commits=False).kind == "failure"
    rate_limited = classify_outcome(
        exit_code=1,
        killed_reason=None,
        no_new_commits=True,
        failure_text="provider returned 429 rate limit exceeded",
    )
    assert rate_limited.limit_kind == "rate_limit", rate_limited
    token_limited = classify_outcome(
        exit_code=1,
        killed_reason=None,
        no_new_commits=True,
        failure_text="prompt exceeds maximum context length",
    )
    assert token_limited.limit_kind == "token_limit", token_limited
    # No commits but the agent's `result` event reported success
    # ("target already met") — trust the agent over the no-commits heuristic.
    target_met = classify_outcome(exit_code=0, killed_reason=None,
                                   no_new_commits=True,
                                   agent_reported_success=True)
    assert target_met.kind == "success", target_met
    assert "target was already met" in target_met.message, target_met
    # Same applies on the `result_grace` exit path.
    assert classify_outcome(exit_code=0, killed_reason="result_grace",
                            no_new_commits=True,
                            agent_reported_success=True).kind == "success"
    # Without an explicit success signal, the no-commits heuristic still
    # demotes to `failed` (preserves prior behavior).
    assert classify_outcome(exit_code=0, killed_reason=None,
                            no_new_commits=True,
                            agent_reported_success=None).kind == "failure"
    print("[ok] subprocess outcome classification")

    # --- Pre-flight ---------------------------------------------------------
    from refine_server import preflight
    ok, msg = preflight.check(conn)
    print(f"[ok] preflight ran (ok={ok}, msg={'set' if msg else 'none'})")

    # --- Runner instantiation -----------------------------------------------
    from refine_server.runner import Runner
    r = Runner()
    assert r.sub_mgr is not None
    assert r.dispatcher is not None
    print("[ok] runner wires up")

    # cleanup
    conn.close()
    os.chdir(tempfile.gettempdir())  # release the cwd before rmtree
    shutil.rmtree(tmp, ignore_errors=True)
    print("\nALL OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
