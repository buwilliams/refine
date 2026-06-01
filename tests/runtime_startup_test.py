"""Runtime startup state normalization tests."""
from __future__ import annotations

import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def test_configured_app_start_resumes_agents() -> None:
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


def test_lazy_runner_client_preserves_operator_pause() -> None:
    tmp, client = make_client_repo("refine-runtime-lazy-pause-")
    conn = init_refine(client)
    try:
        from refine_server import db, project_state
        from refine_runtime import ipc
        from refine_ui import runtime

        original_start_external_runner = runtime._start_external_runner  # type: ignore[attr-defined]
        original_request = ipc.request
        socket: Path | None = None
        starts: list[Path] = []

        class FakeProc:
            pid = 43210
            exited = False

            def poll(self):  # noqa: ANN202
                return 0 if self.exited else None

            def terminate(self) -> None:
                self.exited = True

            def kill(self) -> None:
                self.exited = True

        def stale_request(*_args, **_kwargs):  # noqa: ANN002, ANN003, ANN202
            raise TimeoutError("stale socket")

        def fake_start(_cfg, socket: Path):  # noqa: ANN001, ANN202
            starts.append(socket)
            assert not socket.exists()
            socket.touch()
            return FakeProc()

        runtime.load_configured(
            client / ".refine" / "refine.toml",
            start_poller=False,
            start_runner=False,
        )
        db.set_setting(conn, "paused", "1")
        db.set_setting(conn, "agents_paused", "1")
        try:
            socket = Path(runtime.backend_info()["socket_path"])
            socket.parent.mkdir(parents=True, exist_ok=True)
            socket.touch()
            ipc.request = stale_request  # type: ignore[assignment]
            runtime._start_external_runner = fake_start  # type: ignore[attr-defined]
            runner = runtime.ensure_runner()
            assert runner.socket_path == str(socket)
            assert starts == [socket]
            assert runtime.backend_info()["in_process_runner_allowed"] is False
            assert project_state.list_settings()["paused"] == "1"
            assert project_state.list_settings()["agents_paused"] == "1"
        finally:
            ipc.request = original_request  # type: ignore[assignment]
            runtime._start_external_runner = original_start_external_runner  # type: ignore[attr-defined]
            runtime.stop_runner()
            try:
                socket.unlink()
            except FileNotFoundError:
                pass
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


def test_matching_runner_socket_is_adopted() -> None:
    tmp, client = make_client_repo("refine-runtime-adopt-runner-")
    conn = init_refine(client)
    try:
        from refine_runtime import identity, ipc
        from refine_ui import runtime

        original_start_external_runner = runtime._start_external_runner  # type: ignore[attr-defined]
        original_request = ipc.request
        socket: Path | None = None

        def matching_request(_path, method, _params=None, *, timeout=30.0):  # noqa: ANN001, ANN202
            assert method == "ping"
            return {
                "pong": True,
                "pid": 98765,
                "parent_pid": os.getpid(),
                "expected_parent_pid": os.getpid(),
                "refine_version": identity.REFINE_VERSION,
                "source_fingerprint": identity.SOURCE_FINGERPRINT,
            }

        def fail_start(*_args, **_kwargs):  # noqa: ANN002, ANN003, ANN202
            raise AssertionError("matching runner socket should be adopted")

        try:
            runtime.load_configured(
                client / ".refine" / "refine.toml",
                start_poller=False,
                start_runner=False,
            )
            socket = Path(runtime.backend_info()["socket_path"])
            socket.parent.mkdir(parents=True, exist_ok=True)
            socket.touch()
            ipc.request = matching_request  # type: ignore[assignment]
            runtime._start_external_runner = fail_start  # type: ignore[attr-defined]
            runner = runtime.ensure_runner()
            assert runner.socket_path == str(socket)
        finally:
            ipc.request = original_request  # type: ignore[assignment]
            runtime._start_external_runner = original_start_external_runner  # type: ignore[attr-defined]
            runtime.stop_all()
            if socket is not None:
                try:
                    socket.unlink()
                except Exception:
                    pass
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)


def main() -> int:
    test_configured_app_start_resumes_agents()
    test_lazy_runner_client_preserves_operator_pause()
    test_matching_runner_socket_is_adopted()
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
    assert 'env["REFINE_PARENT_PID"] = str(os.getpid())' in runtime_source
    assert "start_new_session=True" not in runtime_source
    print("runtime startup tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
