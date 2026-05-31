"""Runtime startup state normalization tests."""
from __future__ import annotations

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
        from refine_ui import runtime

        runtime.load_configured(
            client / ".refine" / "refine.toml",
            start_poller=False,
            start_runner=False,
        )
        db.set_setting(conn, "paused", "1")
        try:
            socket = Path(runtime.backend_info()["socket_path"])
            socket.parent.mkdir(parents=True, exist_ok=True)
            socket.touch()
            runner = runtime.ensure_runner()
            assert runner.socket_path == str(socket)
            assert runtime.backend_info()["in_process_runner_allowed"] is False
            assert project_state.list_settings()["paused"] == "1"
        finally:
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


def main() -> int:
    test_configured_app_start_resumes_agents()
    test_lazy_runner_client_preserves_operator_pause()
    print("runtime startup tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
