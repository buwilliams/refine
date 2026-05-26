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
        runtime.load_configured(
            client / ".refine" / "refine.toml",
            start_poller=False,
            start_runner=False,
        )
        assert db.get_setting(conn, "paused") == "0"
        assert project_state.list_settings()["paused"] == "0"
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


def test_lazy_runner_start_preserves_operator_pause() -> None:
    tmp, client = make_client_repo("refine-runtime-lazy-pause-")
    conn = init_refine(client)
    try:
        from refine_server import db, project_state
        from refine_server import runner as runner_mod
        from refine_ui import runtime

        runtime.load_configured(
            client / ".refine" / "refine.toml",
            start_poller=False,
            start_runner=False,
        )
        db.set_setting(conn, "paused", "1")
        old_reconcile = runner_mod.recovery.reconcile_on_start
        old_preflight = runner_mod.preflight.check
        runner_mod.recovery.reconcile_on_start = lambda _conn: None
        runner_mod.preflight.check = lambda _conn: None
        old_runner_cls = runner_mod.Runner

        class QuietRunner(old_runner_cls):
            def __init__(self) -> None:
                super().__init__()
                for worker in (
                    self.governance_agent,
                    self.dispatcher,
                    self.merger,
                    self.target_app_rebuilder,
                    self.state_committer,
                ):
                    worker.start = lambda: None  # type: ignore[method-assign]
                    worker.stop = lambda: None  # type: ignore[method-assign]

        runner_mod.Runner = QuietRunner
        try:
            runner = runtime.ensure_runner()
            assert db.get_setting(runner._conn, "paused") == "1"  # noqa: SLF001
            assert project_state.list_settings()["paused"] == "1"
        finally:
            runtime.stop_runner()
            runner_mod.Runner = old_runner_cls
            runner_mod.recovery.reconcile_on_start = old_reconcile
            runner_mod.preflight.check = old_preflight
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
    test_lazy_runner_start_preserves_operator_pause()
    print("runtime startup tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
