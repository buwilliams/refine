"""Supervisor control-plane unit tests."""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


def main() -> int:
    from refine_runtime.supervisor import Supervisor
    import refine_runtime.supervisor as supervisor_mod
    from refine_runtime.supervisor_protocol import (
        M_BACKEND_CALL,
        M_ENSURE_WORKER,
        M_PROCESS_LAUNCH,
        M_PROCESS_READ,
        M_PROCESS_WAIT,
        M_SHUTDOWN,
        M_STATUS,
        M_SWITCH_APP,
    )
    from refine_server import config, project_state
    from tests.helpers import cleanup_tmp, init_refine, make_client_repo

    tmp, client = make_client_repo("refine-supervisor-control-")
    conn = init_refine(client)
    conn.close()
    old_socket = os.environ.get("REFINE_SUPERVISOR_SOCKET")
    old_runner_socket = os.environ.get("REFINE_RUNNER_SOCKET")
    old_run_dir = os.environ.get(config.ENV_RUN_DIR)
    old_config = os.environ.get(config.ENV_CONFIG_PATH)
    try:
        cfg_path = client / ".refine" / "refine.toml"
        os.environ[config.ENV_CONFIG_PATH] = str(cfg_path)
        supervisor = Supervisor(host="127.0.0.1", port=19876, cfg_path=str(cfg_path))
        supervisor.resources = FakeResourceManager()
        supervisor._can_ping_worker = lambda _path: True  # type: ignore[method-assign]
        supervisor.start()
        try:
            pid_path = Path(__file__).resolve().parents[1] / "run" / "19876" / "supervisor.pid"
            assert pid_path.read_text(encoding="utf-8").strip() == str(os.getpid())
            status = supervisor.dispatch(M_STATUS, {})
            assert status["supervisor_pid"] == os.getpid()
            assert status["port"] == 19876
            assert status["run_dir"].endswith("/run/19876"), status
            assert status["supervisor_socket"].endswith("/run/19876/s.sock"), status
            assert status["ui"]["pid"] == 1000
            assert status["worker"]["pid"] is None
            assert supervisor.resources.procs[0].env["REFINE_RUN_DIR"].endswith("/run/19876")
            assert supervisor.resources.procs[0].env["REFINE_UI_PORT"] == "19876"
            assert "REFINE_RUNNER_SOCKET" not in supervisor.resources.procs[0].env

            worker = supervisor.dispatch(M_ENSURE_WORKER, {"config_path": str(cfg_path)})
            assert worker["worker_pid"] == 1001, worker
            assert supervisor.dispatch(M_STATUS, {})["worker"]["pid"] == 1001
            assert supervisor.resources.procs[1].env["REFINE_RUN_DIR"].endswith("/run/19876")
            assert supervisor.resources.procs[1].env["REFINE_UI_PORT"] == "19876"

            backend_requests: list[tuple[str, str, dict, float]] = []
            old_ipc_request = supervisor_mod.ipc.request

            def fake_ipc_request(path, method, params=None, *, timeout=30.0):  # noqa: ANN001, ANN202
                backend_requests.append((str(path), method, params or {}, timeout))
                if method == "running":
                    return {"runner_reachable": True}
                return {"proxied": True, "method": method, "params": params or {}}

            try:
                supervisor_mod.ipc.request = fake_ipc_request  # type: ignore[assignment]
                proxied = supervisor.dispatch(M_BACKEND_CALL, {
                    "config_path": str(cfg_path),
                    "method": "example_method",
                    "params": {"x": 1},
                    "timeout": 12.0,
                })
            finally:
                supervisor_mod.ipc.request = old_ipc_request  # type: ignore[assignment]
            assert proxied == {
                "proxied": True,
                "method": "example_method",
                "params": {"x": 1},
            }
            assert backend_requests[-1][1:] == ("example_method", {"x": 1}, 12.0)

            switched = supervisor.dispatch(M_SWITCH_APP, {"config_path": str(cfg_path)})
            assert switched["worker_pid"] == 1001, switched

            client2 = tmp / "client-two"
            client2.mkdir()
            cfg2_path = config.write_defaults(client2 / ".refine")
            project_state.ensure_initialized(root=client2 / ".refine")
            wait_started = threading.Event()

            def slow_wait(_socket_path, _proc):  # noqa: ANN001, ANN202
                Path(_socket_path).unlink(missing_ok=True)
                wait_started.set()
                time.sleep(1.0)

            original_wait = supervisor._wait_for_worker_socket
            supervisor._wait_for_worker_socket = slow_wait  # type: ignore[method-assign]
            result_holder: dict[str, object] = {}

            def ensure_worker() -> None:
                try:
                    result_holder["result"] = supervisor.dispatch(
                        M_ENSURE_WORKER,
                        {"config_path": str(cfg2_path)},
                    )
                except Exception as e:  # noqa: BLE001
                    result_holder["error"] = e

            thread = threading.Thread(target=ensure_worker)
            thread.start()
            try:
                assert wait_started.wait(1.0)
                t0 = time.monotonic()
                status_during_start = supervisor.dispatch(M_STATUS, {})
                assert time.monotonic() - t0 < 0.5
                assert status_during_start["worker"]["pid"] == 1002
                supervisor._repair_runtime_namespace()  # noqa: SLF001
                worker_procs = [p for p in supervisor.resources.procs if p.kind == "worker"]
                assert [p.pid for p in worker_procs] == [1001, 1002]
                thread.join(timeout=2.0)
            finally:
                supervisor._wait_for_worker_socket = original_wait  # type: ignore[method-assign]
            assert not thread.is_alive()
            assert "error" not in result_holder, result_holder
            assert result_holder["result"]["worker_pid"] == 1002

            proc = supervisor.dispatch(M_PROCESS_LAUNCH, {
                "args": ["fake", "command"],
                "cwd": str(client),
                "env": {},
                "kind": "agent",
            })
            assert proc["pid"] == 1003, proc
            assert supervisor.resources.procs[3].env["REFINE_RUN_DIR"].endswith("/run/19876")
            assert supervisor.resources.procs[3].env["REFINE_UI_PORT"] == "19876"
            read = supervisor.dispatch(M_PROCESS_READ, {
                "process_id": proc["process_id"],
                "cursor": 0,
                "timeout": 1,
            })
            assert "fake output" in read["data"], read
            waited = supervisor.dispatch(M_PROCESS_WAIT, {
                "process_id": proc["process_id"],
                "timeout": 0,
            })
            assert waited["exited"] is True
            assert waited["returncode"] == 0

            supervisor.dispatch(M_SHUTDOWN, {})
        finally:
            supervisor.shutdown()
        assert all(p.terminated or p.returncode is not None for p in supervisor.resources.procs)
        assert not pid_path.exists()
    finally:
        if old_socket is None:
            os.environ.pop("REFINE_SUPERVISOR_SOCKET", None)
        else:
            os.environ["REFINE_SUPERVISOR_SOCKET"] = old_socket
        if old_runner_socket is None:
            os.environ.pop("REFINE_RUNNER_SOCKET", None)
        else:
            os.environ["REFINE_RUNNER_SOCKET"] = old_runner_socket
        if old_run_dir is None:
            os.environ.pop(config.ENV_RUN_DIR, None)
        else:
            os.environ[config.ENV_RUN_DIR] = old_run_dir
        if old_config is None:
            os.environ.pop(config.ENV_CONFIG_PATH, None)
        else:
            os.environ[config.ENV_CONFIG_PATH] = old_config
        shutil.rmtree(Path(__file__).resolve().parents[1] / "run" / "19876", ignore_errors=True)
        cleanup_tmp(tmp)
    exercise_ipc_server_does_not_unlink_replacement_socket()
    exercise_ipc_server_rebinds_deleted_socket()
    root = Path(__file__).resolve().parents[1]
    runtime_source = (root / "refine_ui" / "runtime.py").read_text(encoding="utf-8")
    api_source = (root / "refine_ui" / "api.py").read_text(encoding="utf-8")
    assert "subprocess.Popen" not in runtime_source
    assert "_start_external_runner" not in runtime_source
    assert "_terminate_workers_for_socket" not in runtime_source
    assert "os.kill(" not in runtime_source
    assert "subprocess.Popen" not in api_source
    print("supervisor control tests OK")
    return 0


def exercise_ipc_server_does_not_unlink_replacement_socket() -> None:
    from refine_runtime import ipc

    path = Path(__file__).resolve().parents[1] / "run" / "19877" / "s.sock"
    path.parent.mkdir(parents=True, exist_ok=True)

    def dispatcher(_method, _params):  # noqa: ANN001, ANN202
        return {"ok": True}

    first = ipc.IpcServer(path, dispatcher)
    second = ipc.IpcServer(path, dispatcher)
    first.start()
    try:
        first_stat = path.stat()
        second.start()
        try:
            second_stat = path.stat()
            assert (first_stat.st_dev, first_stat.st_ino) != (
                second_stat.st_dev,
                second_stat.st_ino,
            )
            first.stop()
            assert path.exists(), "old IPC server removed replacement socket"
        finally:
            second.stop()
    finally:
        first.stop()
        shutil.rmtree(path.parent, ignore_errors=True)


def exercise_ipc_server_rebinds_deleted_socket() -> None:
    from refine_runtime import ipc

    path = Path(__file__).resolve().parents[1] / "run" / "19878" / "s.sock"
    path.parent.mkdir(parents=True, exist_ok=True)

    def dispatcher(method, _params):  # noqa: ANN001, ANN202
        return {"method": method}

    server = ipc.IpcServer(path, dispatcher)
    server.start()
    try:
        assert ipc.request(path, "first") == {"method": "first"}
        path.unlink()
        try:
            ipc.request(path, "missing", timeout=0.1)
            raise AssertionError("request should fail while socket pathname is missing")
        except OSError:
            pass
        assert server.ensure_available() is True
        assert ipc.request(path, "second") == {"method": "second"}
    finally:
        server.stop()
        shutil.rmtree(path.parent, ignore_errors=True)


class FakeResourceManager:
    def __init__(self) -> None:
        self.procs: list[FakeProc] = []
        self._next_pid = 1000

    def capabilities(self):
        return type(
            "Capabilities",
            (),
            {
                "name": "fake",
                "isolation": "best_effort",
                "enforced": False,
                "details": "fake resource backend",
            },
        )()

    def popen(self, args, *, cwd, env, kind, stdin, stdout, stderr, text=True, bufsize=1):  # noqa: ANN001
        proc = FakeProc(self._next_pid, kind=kind, cwd=cwd, env=env)
        self._next_pid += 1
        self.procs.append(proc)
        if kind == "worker":
            Path(env["REFINE_RUNNER_SOCKET"]).parent.mkdir(parents=True, exist_ok=True)
            Path(env["REFINE_RUNNER_SOCKET"]).touch()
        return proc


class FakeStdout:
    def __init__(self, chunks: list[str]) -> None:
        self._chunks = chunks

    def read(self, _size: int = -1) -> str:
        if not self._chunks:
            return ""
        return self._chunks.pop(0)


class FakeProc:
    def __init__(self, pid: int, *, kind: str, cwd, env) -> None:  # noqa: ANN001
        self.pid = pid
        self.kind = kind
        self.cwd = Path(cwd)
        self.env = dict(env)
        self.returncode = None if kind in {"ui", "worker"} else 0
        self.stdout = None if kind in {"ui", "worker"} else FakeStdout(["fake output\n"])
        self.stderr = None
        self.stdin = None
        self.terminated = False

    def poll(self):
        return self.returncode

    def wait(self, timeout=None):  # noqa: ANN001
        if self.returncode is None and timeout == 0:
            raise subprocess.TimeoutExpired(str(self.pid), timeout)
        if self.returncode is None:
            self.returncode = -15
        return self.returncode

    def send_signal(self, sig: int) -> None:
        self.terminated = True
        self.returncode = -int(sig)

    def terminate(self) -> None:
        self.send_signal(15)

    def kill(self) -> None:
        self.send_signal(9)


if __name__ == "__main__":
    raise SystemExit(main())
