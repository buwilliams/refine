"""Runtime startup state normalization tests."""
from __future__ import annotations

import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


_RUNTIME_ENV_KEYS = (
    "REFINE_CONFIG_PATH",
    "REFINE_UI_PORT",
    "REFINE_UI_SCOPE",
    "REFINE_RUN_DIR",
    "REFINE_LOCAL_NODE_ID",
    "REFINE_RUNNER_SOCKET",
    "REFINE_SUPERVISOR_SOCKET",
    "REFINE_NO_INPROCESS_RUNNER",
)


def _save_runtime_env() -> dict[str, str | None]:
    return {key: os.environ.get(key) for key in _RUNTIME_ENV_KEYS}


def _clear_runtime_env() -> None:
    for key in _RUNTIME_ENV_KEYS:
        os.environ.pop(key, None)


def _restore_runtime_env(saved: dict[str, str | None]) -> None:
    for key, value in saved.items():
        if value is None:
            os.environ.pop(key, None)
        else:
            os.environ[key] = value


def test_configured_app_start_resumes_agents() -> None:
    saved_env = _save_runtime_env()
    _clear_runtime_env()
    tmp, client = make_client_repo("refine-runtime-startup-")
    conn = init_refine(client)
    try:
        from refine_server import db, project_state
        from refine_ui import runtime

        db.set_setting(conn, "paused", "1")
        db.set_setting(conn, "agents_paused", "1")
        runtime.load_configured(
            client / ".refine" / "refine.toml",
            start_poller=False,
            start_runner=False,
        )
        assert db.get_setting(conn, "paused") == "0"
        assert db.get_setting(conn, "agents_paused") == "0"
        assert project_state.list_settings()["paused"] == "0"
        assert project_state.list_settings()["agents_paused"] == "0"
    finally:
        try:
            from refine_ui import runtime

            runtime.stop_all()
        except Exception:
            pass
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)
        _restore_runtime_env(saved_env)


def test_lazy_runner_client_preserves_operator_pause() -> None:
    saved_env = _save_runtime_env()
    _clear_runtime_env()
    tmp, client = make_client_repo("refine-runtime-lazy-pause-")
    conn = init_refine(client)
    try:
        from refine_server import db, project_state
        from refine_ui import runtime

        original_supervisor_request = runtime._supervisor_request  # type: ignore[attr-defined]
        calls: list[tuple[str, dict]] = []

        runtime.load_configured(
            client / ".refine" / "refine.toml",
            start_poller=False,
            start_runner=False,
        )
        db.set_setting(conn, "paused", "1")
        db.set_setting(conn, "agents_paused", "1")
        try:
            socket = Path(runtime.backend_info()["socket_path"])

            def fake_supervisor_request(method, params=None, *, timeout=30.0):  # noqa: ANN001, ANN202
                calls.append((method, params or {}))
                return {
                    "worker_socket": str(socket),
                    "worker_pid": 43210,
                    "started": True,
                }

            runtime._supervisor_request = fake_supervisor_request  # type: ignore[attr-defined]
            runner = runtime.ensure_runner()
            assert runner.socket_path == str(socket)
            assert calls and calls[0][0] == "ensure_worker", calls
            assert runtime.backend_info()["in_process_runner_allowed"] is False
            assert project_state.list_settings()["paused"] == "1"
            assert project_state.list_settings()["agents_paused"] == "1"
        finally:
            runtime._supervisor_request = original_supervisor_request  # type: ignore[attr-defined]
            runtime.stop_runner()
    finally:
        try:
            from refine_ui import runtime

            runtime.stop_all()
        except Exception:
            pass
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)
        _restore_runtime_env(saved_env)


def test_runner_socket_comes_from_supervisor() -> None:
    saved_env = _save_runtime_env()
    _clear_runtime_env()
    tmp, client = make_client_repo("refine-runtime-supervisor-runner-")
    conn = init_refine(client)
    try:
        from refine_ui import runtime

        original_supervisor_request = runtime._supervisor_request  # type: ignore[attr-defined]
        calls: list[tuple[str, dict]] = []

        try:
            runtime.load_configured(
                client / ".refine" / "refine.toml",
                start_poller=False,
                start_runner=False,
            )
            socket = Path(runtime.backend_info()["socket_path"])

            def fake_supervisor_request(method, params=None, *, timeout=30.0):  # noqa: ANN001, ANN202
                calls.append((method, params or {}))
                return {"worker_socket": str(socket), "worker_pid": 98765}

            runtime._supervisor_request = fake_supervisor_request  # type: ignore[attr-defined]
            runner = runtime.ensure_runner()
            assert runner.socket_path == str(socket)
            assert calls == [("ensure_worker", {"config_path": str(client / ".refine" / "refine.toml")})]
        finally:
            runtime._supervisor_request = original_supervisor_request  # type: ignore[attr-defined]
            runtime.stop_all()
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)
        _restore_runtime_env(saved_env)


def test_runtime_local_node_is_stable_after_active_switch() -> None:
    saved_env = _save_runtime_env()
    _clear_runtime_env()
    tmp, client = make_client_repo("refine-runtime-local-node-")
    conn = init_refine(client)
    try:
        from refine_server import project_state
        from refine_ui import runtime

        runtime.load_configured(
            client / ".refine" / "refine.toml",
            start_poller=False,
            start_runner=False,
        )
        initial = runtime.backend_info()["local_node_id"]
        other = project_state.create_node("Other Node")
        project_state.set_active_node(other["id"])
        assert project_state.active_node_id() == other["id"]
        assert runtime.backend_info()["local_node_id"] == initial
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)
        _restore_runtime_env(saved_env)


def main() -> int:
    test_configured_app_start_resumes_agents()
    test_lazy_runner_client_preserves_operator_pause()
    test_runner_socket_comes_from_supervisor()
    test_runtime_local_node_is_stable_after_active_switch()
    worker_source = (
        Path(__file__).resolve().parents[1] / "refine_runtime" / "worker.py"
    ).read_text(encoding="utf-8")
    server_source = (
        Path(__file__).resolve().parents[1] / "refine_server" / "__main__.py"
    ).read_text(encoding="utf-8")
    runtime_source = (
        Path(__file__).resolve().parents[1] / "refine_ui" / "runtime.py"
    ).read_text(encoding="utf-8")
    assert "resume_agents_for_startup" not in worker_source
    assert "resume_agents_for_startup" not in server_source
    assert "subprocess.Popen" not in runtime_source
    assert "_start_external_runner" not in runtime_source
    assert "_terminate_workers_for_socket" not in runtime_source
    assert "os.kill" not in runtime_source
    assert 'M_ENSURE_WORKER' in runtime_source
    print("runtime startup tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
